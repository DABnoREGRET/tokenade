//! **This model is not in the table. Which one does it resemble?**
//!
//! A model name is a family, a version and a date suffix. The nearest
//! neighbour by edit distance would work — and would give `claude-3-opus` for
//! `claude-4-opus`, i.e. a price from an earlier generation on the most
//! expensive one.
//!
//! Hence the VERSION boundary, which forbids the jump, and the ranking by
//! provider. An approximate price must be announced as approximate: getting
//! it wrong here does not cost a byte, it publishes a dollar figure that is
//! wrong.

use super::*;

/// Per-token prices → per-1M. LiteLLM omits a cache field when the model has no
/// cache tier; the Anthropic-shaped fallbacks (write 1.25×, read 0.1×) then
/// apply, same as the Helicone path.
pub(crate) fn litellm_to_cost(e: &Value) -> Option<ModelCost> {
    let f = |k: &str| e.get(k).and_then(|v| v.as_f64());
    // A row with no input price is a stub (context-window-only entry); refusing
    // it lets the Helicone fallback have a go instead of booking $0 as a price.
    let input = f("input_cost_per_token")? * 1e6;
    let output = f("output_cost_per_token").unwrap_or(0.0) * 1e6;
    if input <= 0.0 && output <= 0.0 {
        return None;
    }
    Some(ModelCost {
        input_per_1m: input,
        output_per_1m: output,
        cache_write_per_1m: f("cache_creation_input_token_cost")
            .map(|v| v * 1e6)
            .unwrap_or(input * 1.25),
        cache_read_per_1m: f("cache_read_input_token_cost")
            .map(|v| v * 1e6)
            .unwrap_or(input * 0.1),
    })
}

/// EXACT-key lookup over the LiteLLM map. No prefix or substring matching: an
/// id we don't know must fall through to the next source (or to `None`), never
/// borrow a neighbour's price.
pub(crate) fn litellm_lookup(json: &str, candidates: &[String]) -> Option<ModelCost> {
    let root: Value = serde_json::from_str(json).ok()?;
    // Keys matched lowercased, like the candidates (`candidates` lowercases):
    // upstream spells some ids in mixed case (`minimax/MiniMax-M3`).
    let map: std::collections::HashMap<String, &Value> = root
        .as_object()?
        .iter()
        .map(|(k, v)| (k.to_ascii_lowercase(), v))
        .collect();
    for c in candidates {
        if c == "sample_spec" {
            continue;
        }
        if let Some(cost) = map
            .get(&c.to_ascii_lowercase())
            .and_then(|v| litellm_to_cost(v))
        {
            return Some(cost);
        }
    }
    None
}

// ------------------------------------------------- nearest-neighbour pricing

/// Split an id into its non-numeric stem and its trailing version numbers.
///
/// `claude-opus-5-1` → `("claude-opus", [5, 1])`
/// `claude-opus-4-8` → `("claude-opus", [4, 8])`
/// `gpt-4o-mini`     → `("gpt-4o-mini", [])`   (no trailing numbers)
///
/// Only TRAILING numeric segments are the version: `gpt-4o-mini` keeps its `4o`
/// in the stem because the `o` makes that segment non-numeric, which is what we
/// want — the stem is the thing that must match for two ids to be comparable.
pub(crate) fn stem_and_version(id: &str) -> (String, Vec<u64>) {
    let parts: Vec<&str> = id.split(['-', '.', '_']).collect();
    let mut cut = parts.len();
    while cut > 0
        && !parts[cut - 1].is_empty()
        && parts[cut - 1].bytes().all(|b| b.is_ascii_digit())
    {
        cut -= 1;
    }
    let stem = parts[..cut].join("-");
    let ver = parts[cut..].iter().filter_map(|s| s.parse().ok()).collect();
    (stem, ver)
}

/// Version as one comparable number, so `5.1` sits next to `5` and far from
/// `4.8`. Three components is enough for every naming scheme in the tables
/// (`5`, `4-8`, `3-5`), and a dated snapshot (`20241022`) still compares
/// consistently because it is only ever weighed against its own stem.
pub(crate) fn version_key(v: &[u64]) -> u64 {
    let g = |i: usize| v.get(i).copied().unwrap_or(0);
    g(0).saturating_mul(1_000_000) + g(1).saturating_mul(1_000) + g(2)
}

pub(crate) fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Beyond this share of the id's length, an edit-distance match is a
/// coincidence rather than a sibling. Only used when the stem match fails.
pub(crate) const MAX_REL_EDIT_DISTANCE: f64 = 0.25;

/// The closest priced id to `model`, for when no source lists it.
///
/// ## Why not plain edit distance
///
/// A version digit is ONE edit but can triple the price. For a hypothetical
/// `claude-opus-5-1`, `levenshtein` puts `claude-opus-4-1` at distance 1 and
/// `claude-opus-5` at distance 2 — so pure edit distance picks the **$15**
/// legacy row over the **$5** current one, recreating the exact 3×
/// overstatement this module was rewritten to remove.
///
/// So the ranking is:
///   1. same STEM (`claude-opus`), which is what makes two ids comparable;
///   2. nearest VERSION, `5.1` landing on `5` rather than `4.8`;
///   3. the NEWER of two equidistant versions — a fresh release is priced like
///      the current generation, not the one before it;
///   4. edit distance only as a final tie-break.
///
/// With no stem match at all, it falls back to edit distance over ids sharing
/// the first token, capped by [`MAX_REL_EDIT_DISTANCE`]. Past that we would be
/// inventing a number, and an invented number on a money column is worse than
/// an admitted gap.
pub(crate) fn nearest_priced<'a, I>(known: I, model: &str) -> Option<(String, ModelCost)>
where
    I: Iterator<Item = (&'a str, ModelCost)>,
{
    let (stem, ver) = stem_and_version(model);
    let want = version_key(&ver);
    let first_token = model.split(['-', '.', '_']).next().unwrap_or(model);

    let mut best_stem: Option<(u64, u64, usize, String, ModelCost)> = None;
    let mut best_edit: Option<(usize, String, ModelCost)> = None;

    for (id, cost) in known {
        if cost.is_unpriced() || id == model {
            continue;
        }
        let (s, v) = stem_and_version(id);
        if s == stem && !stem.is_empty() {
            let key = version_key(&v);
            // (version distance ASC, newer first, edit distance ASC)
            let cand = (
                want.abs_diff(key),
                u64::MAX - key,
                levenshtein(model, id),
                id.to_string(),
                cost,
            );
            if best_stem
                .as_ref()
                .map(|b| (cand.0, cand.1, cand.2) < (b.0, b.1, b.2))
                .unwrap_or(true)
            {
                best_stem = Some(cand);
            }
            continue;
        }
        if id.split(['-', '.', '_']).next() == Some(first_token) {
            let d = levenshtein(model, id);
            if (d as f64) <= model.chars().count() as f64 * MAX_REL_EDIT_DISTANCE
                && best_edit.as_ref().map(|b| d < b.0).unwrap_or(true)
            {
                best_edit = Some((d, id.to_string(), cost));
            }
        }
    }

    best_stem
        .map(|(_, _, _, id, c)| (id, c))
        .or_else(|| best_edit.map(|(_, id, c)| (id, c)))
}

/// Every priced id in the LiteLLM table, for the neighbour search.
pub(crate) fn litellm_priced(json: &str) -> Vec<(String, ModelCost)> {
    serde_json::from_str::<Value>(json)
        .ok()
        .and_then(|v| v.as_object().cloned())
        .map(|m| {
            m.iter()
                .filter(|(k, _)| k.as_str() != "sample_spec")
                .filter_map(|(k, v)| litellm_to_cost(v).map(|c| (k.to_ascii_lowercase(), c)))
                .collect()
        })
        .unwrap_or_default()
}

// --------------------------------------------------------------- Helicone

pub(crate) fn helicone_entries(json: &str) -> Vec<Value> {
    serde_json::from_str::<Value>(json)
        .ok()
        .and_then(|v| v.get("data").and_then(|d| d.as_array()).cloned())
        .unwrap_or_default()
}

/// Would treating `name` as a prefix of `model` cross a VERSION boundary?
///
/// This is the guard that kills the measured 3× bug: `claude-opus-4` is a
/// prefix of `claude-opus-4-8`, but they are different models at different
/// prices. A dated snapshot is not — `claude-3-5-sonnet-20241022` is the same
/// model as `claude-3-5-sonnet` and legitimately shares its row.
///
/// The discriminator is the shape of the first token after the prefix: a short
/// run of digits (`-8`, `-1`, `-45`) is a version bump; a long one (`-20241022`)
/// is a release date; anything non-numeric (`-latest`, `-thinking`) is a variant
/// of the same model. Only the version bump is rejected.
pub(crate) fn crosses_version_boundary(name: &str, model: &str) -> bool {
    let Some(rest) = model.strip_prefix(name) else {
        return false;
    };
    let rest = rest.strip_prefix(['-', '.', '_']).unwrap_or(rest);
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return false;
    }
    // 1-2 digits => version bump (4 -> 4-8). 6+ => a date snapshot.
    digits.len() <= 2
}

pub(crate) fn provider_rank(e: &Value) -> u8 {
    let p = e
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if FIRST_PARTY.contains(&p.as_str()) {
        0
    } else {
        1
    }
}

/// Best price match for `model`, honoring each entry's `operator`.
///
/// Ranks by, in order: match tier (exact > prefix > substring), name length
/// (more specific wins), first-party provider over reseller, then provider name
/// so a tie is resolved deterministically instead of by array position — the
/// old behaviour, which made 32 diverging duplicate groups a coin flip.
///
/// Non-exact matches that cross a version boundary are rejected outright; see
/// [`crosses_version_boundary`].
pub(crate) fn best_match<'a>(entries: &'a [Value], model: &str) -> Option<&'a Value> {
    let mut best: Option<(&Value, u8, usize, u8, &str)> = None;
    for e in entries {
        let name = e.get("model").and_then(|v| v.as_str()).unwrap_or("");
        if name.is_empty() {
            continue;
        }
        let op = e
            .get("operator")
            .and_then(|v| v.as_str())
            .unwrap_or("equals");
        let (hit, tier) = match op {
            "equals" => (model == name, 3u8),
            "startsWith" => (model.starts_with(name), 2),
            "includes" => (model.contains(name), 1),
            _ => (false, 0),
        };
        if !hit {
            continue;
        }
        if tier < 3 && crosses_version_boundary(name, model) {
            continue;
        }
        let prov = e.get("provider").and_then(|v| v.as_str()).unwrap_or("");
        // Higher is better for tier/specificity; LOWER is better for the
        // provider rank, so it is negated to keep one comparison direction.
        let key = (tier, name.len(), u8::MAX - provider_rank(e));
        if best
            .map(|(_, t, s, r, pn)| key > (t, s, r) || (key == (t, s, r) && prov < pn))
            .unwrap_or(true)
        {
            best = Some((e, key.0, key.1, key.2, prov));
        }
    }
    best.map(|(e, ..)| e)
}

pub(crate) fn helicone_to_cost(e: &Value) -> ModelCost {
    let f = |k: &str| e.get(k).and_then(|v| v.as_f64());
    let input = f("input_cost_per_1m").unwrap_or(0.0);
    ModelCost {
        input_per_1m: input,
        output_per_1m: f("output_cost_per_1m").unwrap_or(0.0),
        // Anthropic-style fallbacks when the entry omits cache prices.
        cache_write_per_1m: f("prompt_cache_write_per_1m").unwrap_or(input * 1.25),
        cache_read_per_1m: f("prompt_cache_read_per_1m").unwrap_or(input * 0.1),
    }
}
