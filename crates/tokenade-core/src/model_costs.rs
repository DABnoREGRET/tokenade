//! Real-dollar token prices per model.
//!
//! The dashboard turns token *savings* into *dollars saved* for the model the
//! agent actually runs. Prices are per 1M tokens, fetched on demand, cached for
//! a day under `~/.tokenade/`, and **fail-open**: a network error falls back to
//! the (possibly stale) cache, and an unknown model yields no dollar figure
//! rather than a wrong one.
//!
//! ## Two sources, in order (rewritten 2026-08-01)
//!
//! 1. **LiteLLM** `model_prices_and_context_window.json` — a flat map keyed by
//!    the exact model id, carrying `litellm_provider` and explicit cache prices.
//!    Looked up by EXACT KEY only: no prefix, no substring, so it cannot
//!    mis-attribute one model's price to another.
//! 2. **Helicone** `llm-costs` — the previous sole source. Kept as a fallback
//!    because it covers ids LiteLLM misses, but its entries carry an `operator`
//!    (`equals` / `startsWith` / `includes`) and matching on those is where the
//!    old bugs lived. Both are fixed below.
//!
//! ## Why this was rewritten — two live defects, both measured
//!
//! Against the real cached Helicone table (1129 entries) on 2026-08-01:
//!
//! * **Every current model priced at $0.** `claude-opus-5`, `claude-sonnet-5`,
//!   `claude-haiku-4-5` and `claude-fable-5` matched NOTHING, so every session
//!   on the current lineup contributed nothing to the dashboard.
//! * **A newer model inheriting an older, dearer one's rate.**
//!   `claude-opus-4-8` matched `claude-opus-4` by `includes` at **$15/M** where
//!   Opus 4.8 is **$5/M** — a 3× overstatement. This is the structural danger
//!   of substring fallback: the newer a model is, the likelier its id CONTAINS
//!   an older and pricier one.
//! * **258 duplicate `(model, operator)` keys, 32 with diverging prices**
//!   (`gpt-4o-mini` at AZURE $0.15 / AZURE $0.20 / OPENAI $0.15). `provider` was
//!   never consulted, so the winner was whichever came first in the array —
//!   non-deterministic with respect to upstream ordering.
//!
//! The first defect is the one that recurs at **every model launch**, which is
//! why the primary source is now a table that carries new ids on release day
//! and why [`SPOT_CHECK_MODELS`] exists.
//!
//! ## Not a source: the Anthropic Models API
//!
//! `GET /v1/models/{id}` returns `max_input_tokens`, `max_tokens` and a
//! capability tree — **no pricing field**. Checked 2026-08-01; do not re-derive
//! this. There is no first-party price endpoint to switch to.

mod matching;
pub(crate) use matching::*;

use serde_json::Value;
use std::path::PathBuf;

const LITELLM_URL: &str = "https://raw.githubusercontent.com/BerriAI/litellm/main/\
                           model_prices_and_context_window.json";
const HELICONE_URL: &str = "https://www.helicone.ai/api/llm-costs";
const CACHE_TTL_SECS: u64 = 24 * 3600;
const MAX_BYTES: usize = 16 * 1024 * 1024;

/// Providers that publish a model themselves. A reseller (Bedrock, Vertex,
/// Azure, OpenRouter…) prices the same id differently, so when a table lists
/// both, the first-party row is the one an agent hitting the vendor's own API
/// actually pays.
const FIRST_PARTY: &[&str] = &[
    "anthropic",
    "openai",
    "google",
    "gemini",
    "mistral",
    "cohere",
    "x",
    "xai",
    "deepseek",
    "meta",
    "groq",
];

/// The lineup a spot-check must be able to price. Not used at runtime — read by
/// the live `#[ignore]` test that catches a source going stale after a launch.
pub const SPOT_CHECK_MODELS: &[&str] = &[
    "claude-opus-5",
    "claude-sonnet-5",
    "claude-haiku-4-5",
    "claude-opus-4-8",
    "claude-fable-5",
];

/// Per-1M-token prices for one model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelCost {
    pub input_per_1m: f64,
    pub output_per_1m: f64,
    pub cache_write_per_1m: f64,
    pub cache_read_per_1m: f64,
}

impl ModelCost {
    /// Is this "priced" at nothing? A row that exists but carries no numbers
    /// yields all-zero rates through the `unwrap_or(0.0)` defaults, and a $0
    /// price is indistinguishable on the dashboard from a model we simply do
    /// not know — except that it is worse, because it looks answered. Treated
    /// as a MISS everywhere, which is what makes it trigger the recovery
    /// refresh instead of quietly reporting zero.
    pub fn is_unpriced(&self) -> bool {
        self.input_per_1m <= 0.0 && self.output_per_1m <= 0.0
    }

    /// Dollar cost of a given token mix.
    ///
    /// **`input` must be FRESH tokens — cache excluded.** The four arguments
    /// are priced independently and summed, so passing a provider's raw prompt
    /// total bills the cached history twice: once at the full input rate, once
    /// at the cache rate.
    ///
    /// This is a live trap, not a hypothetical, because providers disagree on
    /// what their own field means. Anthropic's `input_tokens` is already
    /// fresh-only, while OpenAI-style `prompt_tokens` INCLUDES the cached
    /// portion — the same split every agent parser in `agent_sqlite_usage` /
    /// `agent_jsonl_usage` has to undo before it stores anything (zcode, mux,
    /// forge, kimi all report the inclusive form). Decompose at the reader,
    /// never here: this function cannot tell which convention its caller used.
    pub fn usd(&self, input: u64, cache_write: u64, cache_read: u64, output: u64) -> f64 {
        let m = 1_000_000.0;
        input as f64 / m * self.input_per_1m
            + cache_write as f64 / m * self.cache_write_per_1m
            + cache_read as f64 / m * self.cache_read_per_1m
            + output as f64 / m * self.output_per_1m
    }
}

fn cache_path(file: &str) -> Option<PathBuf> {
    Some(crate::platform::home_dir()?.join(".tokenade").join(file))
}

/// Raw JSON for one source — cache if fresh, else revalidated.
///
/// ## Daily refresh without re-downloading the file
///
/// The LiteLLM table is ~1.7 MB and changes a few times a week, so an
/// unconditional daily GET would cost every client ~600 MB a year to learn
/// nothing. The refresh is a **conditional** GET: the `ETag` /
/// `Last-Modified` from the previous fetch is stored in a `<file>.etag`
/// sidecar and sent back as `If-None-Match` / `If-Modified-Since`. GitHub raw
/// answers `304 Not Modified` in a few hundred bytes when the file is
/// unchanged, and we simply reset the cache's freshness clock.
///
/// Fail-open at every step: a network error, a truncated body or a corrupt
/// sidecar all fall back to the existing cache, however stale. A price table
/// from last month is worth far more than none.
fn load_json(url: &str, file: &str, force: bool) -> Option<String> {
    let path = cache_path(file);
    let etag_path = cache_path(&format!("{file}.etag"));
    let cached = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok());
    let fresh = !force
        && path
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .map(|e| e.as_secs() < CACHE_TTL_SECS)
            .unwrap_or(false);
    if fresh {
        if let Some(s) = cached {
            return Some(s);
        }
    }

    let (etag, last_mod) = etag_path
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| parse_validators(&s))
        .unwrap_or_default();
    // Only send a validator we actually have AND whose body we still hold —
    // a 304 with no cache to serve would leave us with nothing.
    let have_body = cached.is_some();
    let resp = crate::net::http_get_conditional(
        url,
        60,
        MAX_BYTES,
        (have_body && !etag.is_empty()).then_some(etag.as_str()),
        (have_body && !last_mod.is_empty()).then_some(last_mod.as_str()),
    );

    let Some(resp) = resp else {
        return cached;
    };
    if resp.not_modified() {
        // Unchanged upstream: reset the freshness clock so we do not revalidate
        // again until tomorrow, and keep serving what we have.
        //
        // The clock IS the data file's mtime, so it is reset by rewriting the
        // same bytes. An append of zero bytes does not move mtime on Linux,
        // which would have left every later call revalidating again — the one
        // thing this whole conditional-GET path exists to avoid.
        if let (Some(p), Some(body)) = (&path, &cached) {
            let _ = std::fs::write(p, body);
        }
        return cached;
    }
    if resp.status < 200 || resp.status >= 300 || resp.body.is_empty() {
        return cached;
    }
    let s = String::from_utf8_lossy(&resp.body).into_owned();
    // Refuse to overwrite a good cache with something that does not parse —
    // a captive-portal HTML page answering 200 is a real failure mode.
    if serde_json::from_str::<Value>(&s).is_err() {
        return cached;
    }
    if let Some(p) = &path {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, &s);
    }
    if let Some(p) = &etag_path {
        let _ = std::fs::write(
            p,
            format!(
                "{}\n{}\n",
                resp.etag.unwrap_or_default(),
                resp.last_modified.unwrap_or_default()
            ),
        );
    }
    Some(s)
}

/// Split the `<file>.etag` sidecar into `(etag, last_modified)`. Two lines, in
/// that order, either possibly empty. Tolerates a truncated or empty file.
fn parse_validators(s: &str) -> (String, String) {
    let mut it = s.lines();
    (
        it.next().unwrap_or("").trim().to_string(),
        it.next().unwrap_or("").trim().to_string(),
    )
}

// ---------------------------------------------------------------- LiteLLM

/// Ids to try, most specific first: the model as recorded, then with a routing
/// prefix peeled off (`anthropic/claude-…`, `custom:claude-…`, `bedrock/…`),
/// then with a `-latest`/`:latest` alias suffix dropped. Agents record all of
/// these shapes; the price is the same underlying model.
fn candidates(model: &str) -> Vec<String> {
    fn push(out: &mut Vec<String>, s: &str) {
        if !s.is_empty() && !out.iter().any(|x| x == s) {
            out.push(s.to_string());
        }
    }

    // Region / vendor segments that sit in front of a Bedrock id. Only these
    // are stripped: a dot is legal inside a model id (`claude-3.5-sonnet` on
    // some gateways), so removing every dotted prefix would eat the model.
    const ROUTING_PREFIXES: &[&str] = &[
        "us",
        "eu",
        "apac",
        "global",
        "anthropic",
        "bedrock",
        "aws",
        "azure",
        "vertex",
        "vertex_ai",
        "openrouter",
        "gcp",
    ];

    // Case carries no meaning in a model id (`GPT-5.6-terra` and `gpt-5.6-terra`
    // are one model), and upstream keys are matched lowercased too.
    let mut out = vec![model.trim().to_ascii_lowercase()];
    // Reductions applied to every candidate until nothing new appears. One id
    // routinely carries several at once — `us.anthropic.claude-opus-5-v1:0` has
    // a routing prefix, a revision and an inference-profile suffix — so a
    // single pass is not enough.
    let mut i = 0;
    while i < out.len() && out.len() < 24 {
        let b = out[i].clone();
        i += 1;

        if let Some((_, rest)) = b.rsplit_once('/') {
            push(&mut out, rest);
        }
        // `provider:model` (a gateway) and `model:profile` (Bedrock) are the
        // same character meaning opposite things. Take the tail always; take
        // the HEAD only when the tail is a bare number, which is what an
        // inference profile looks like and what a model name never does —
        // pushing `anthropic` as a candidate would let an `includes` rule match
        // an unrelated row.
        if let Some((head, rest)) = b.split_once(':') {
            push(&mut out, rest);
            if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
                push(&mut out, head);
            }
        }
        for suf in ["-latest", ":latest"] {
            if let Some(x) = b.strip_suffix(suf) {
                push(&mut out, x);
            }
        }
        // Context-window variant: `claude-opus-5[1m]`. Nothing upstream lists a
        // bracketed id, and this is how a 1M-context session names itself — so
        // without this the session's whole cost column reads $0.
        if let Some(open) = b.find('[') {
            if b.ends_with(']') {
                push(&mut out, b[..open].trim_end());
            }
        }
        // Bedrock routing prefix: `us.anthropic.claude-opus-5-v1`.
        if let Some((head, rest)) = b.split_once('.') {
            if ROUTING_PREFIXES.contains(&head.to_ascii_lowercase().as_str()) {
                push(&mut out, rest);
            }
        }
        // Bedrock revision suffix: `claude-opus-5-v1` is `claude-opus-5`.
        if let Some((head, tail)) = b.rsplit_once("-v") {
            if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) {
                push(&mut out, head);
            }
        }
    }
    out
}

/// One pass over both sources. `force` bypasses the cache freshness so a miss
/// can be retried against a just-revalidated table.
fn lookup(cands: &[String], force: bool) -> Option<ModelCost> {
    if let Some(json) = load_json(LITELLM_URL, "litellm-costs.json", force) {
        if let Some(c) = litellm_lookup(&json, cands) {
            if !c.is_unpriced() {
                return Some(c);
            }
        }
    }
    if let Some(json) = load_json(HELICONE_URL, "llm-costs.json", force) {
        let es = helicone_entries(&json);
        for c in cands {
            if let Some(e) = best_match(&es, c) {
                let cost = helicone_to_cost(e);
                if !cost.is_unpriced() {
                    return Some(cost);
                }
            }
        }
    }
    None
}

/// Cooldown between two miss-driven refreshes. Without it, an id no upstream
/// will ever carry — a local Ollama tag, a bespoke deployment name — would
/// force a revalidation on EVERY pricing call.
const MISS_REFRESH_COOLDOWN_SECS: u64 = 3600;

/// Once per process, on top of the on-disk cooldown: a command that prices a
/// hundred rows must not attempt a hundred recoveries.
static MISS_REFRESH_TRIED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Age of a cache file, or `None` when it does not exist / has no usable mtime.
fn age_secs(file: &str) -> Option<u64> {
    let p = cache_path(file)?;
    std::fs::metadata(&p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .map(|e| e.as_secs())
}

/// May we spend a revalidation trying to recover from a miss right now?
///
/// Three brakes, cheapest first:
///
///  1. **Once per process** — a command pricing a hundred rows must not attempt
///     a hundred recoveries.
///  2. **The DATA is younger than the cooldown** — if the table was revalidated
///     within the hour, asking again cannot teach us anything: either it was
///     refetched (we have the newest) or it 304'd (upstream unchanged). Note a
///     304 rewrites the file, so this one rule covers "just downloaded" and
///     "just checked" alike.
///  3. **The last ATTEMPT is younger than the cooldown** — the case (2) misses,
///     when the fetch failed outright and left no mtime behind.
fn miss_refresh_allowed() -> bool {
    if MISS_REFRESH_TRIED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return false;
    }
    let recent = |file: &str| {
        age_secs(file)
            .map(|a| a < MISS_REFRESH_COOLDOWN_SECS)
            .unwrap_or(false)
    };
    !recent("litellm-costs.json") && !recent("litellm-costs.miss")
}

fn mark_miss_refresh() {
    if let Some(stamp) = cache_path("litellm-costs.miss") {
        if let Some(dir) = stamp.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&stamp, b"");
    }
}

/// Per-1M prices for `model`: LiteLLM by exact id, else Helicone by operator.
///
/// ## A miss triggers an immediate refresh
///
/// The daily revalidation is what keeps prices current; it cannot help the day
/// a model SHIPS, because the table cached this morning predates it and the
/// next scheduled check is up to 24 h away. So a total miss — no row, or a row
/// carrying no numbers ([`ModelCost::is_unpriced`]) — revalidates on the spot
/// and retries once.
///
/// This is cheap precisely because the refresh is conditional: when upstream
/// has not changed, GitHub answers `304` in a few hundred bytes and we learn
/// immediately that the model really is absent. Only a genuine upstream change
/// costs the 1.7 MB, and that is the case where the retry succeeds.
///
/// Bounded twice so an id no upstream will ever carry cannot turn into a fetch
/// per call: once per process, and once per hour on disk.
///
/// `None` when the retry does not help either — deliberately visible (a
/// `model_unpriced` debug-log line) rather than silently valued at zero.
pub fn cost_for_model(model: &str) -> Option<ModelCost> {
    if let Some(c) = exact_cost_for_model(model) {
        return Some(c);
    }
    // LAST RESORT: price it like its nearest priced sibling. A launch-day id
    // that upstream has not published yet is far better served by the current
    // generation's rate than by a $0 column that reads as "this cost nothing".
    if let Some(c) = nearest_cost_for_model(model) {
        return Some(c);
    }
    // Nothing even resembles it. Leave a trace so a $0 column is diagnosable
    // instead of being read as "this model cost nothing".
    crate::debug_log::log(
        "model-costs",
        "model_unpriced",
        serde_json::json!({ "model": model }),
    );
    None
}

/// The price upstream publishes **for this exact id**, and nothing else.
///
/// Split out of [`cost_for_model`] so a caller can interleave its own fallback
/// table between "upstream knows this id" and "borrow a sibling's price" — the
/// order the server's `ratesForModel` uses. Folding the two together is what
/// made [`crate::pricing::rates_for_model`] answer a different number from the
/// server for the same id.
pub fn exact_cost_for_model(model: &str) -> Option<ModelCost> {
    let cands = candidates(model);
    if let Some(c) = lookup(&cands, false) {
        return Some(c);
    }
    if miss_refresh_allowed() {
        mark_miss_refresh();
        crate::debug_log::log(
            "model-costs",
            "model_unpriced_refresh",
            serde_json::json!({ "model": model }),
        );
        if let Some(c) = lookup(&cands, true) {
            return Some(c);
        }
    }
    None
}

/// The price of the nearest priced sibling in the LiteLLM table — approximate
/// by construction, so it is logged with the id it borrowed from.
pub fn nearest_cost_for_model(model: &str) -> Option<ModelCost> {
    let json = load_json(LITELLM_URL, "litellm-costs.json", false)?;
    let priced = litellm_priced(&json);
    let lower = model.trim().to_ascii_lowercase();
    let (near, cost) = nearest_priced(priced.iter().map(|(k, c)| (k.as_str(), *c)), &lower)?;
    crate::debug_log::log(
        "model-costs",
        "model_priced_by_neighbour",
        serde_json::json!({
            "model": model,
            "borrowed_from": near,
            "input_per_1m": cost.input_per_1m,
            "output_per_1m": cost.output_per_1m,
        }),
    );
    Some(cost)
}

/// The cached LiteLLM table, parsed ONCE into id → price.
///
/// **Reads the on-disk cache only — never the network**, exactly like
/// [`windows_table`] and for the same reason: its callers are the statusline
/// and `gain`, which price a hundred rows inside a hook, where a conditional
/// GET would land a round-trip on the user's prompt. The cache is kept fresh
/// by the paths that do call [`cost_for_model`].
fn cached_price_table() -> &'static std::collections::HashMap<String, ModelCost> {
    static T: std::sync::OnceLock<std::collections::HashMap<String, ModelCost>> =
        std::sync::OnceLock::new();
    T.get_or_init(|| {
        cache_path("litellm-costs.json")
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|raw| litellm_priced(&raw).into_iter().collect())
            .unwrap_or_default()
    })
}

/// [`exact_cost_for_model`], off the cache alone. `None` when there is no
/// cache yet, or upstream does not carry the id.
pub fn cached_cost_for_model(model: &str) -> Option<ModelCost> {
    let t = cached_price_table();
    candidates(model)
        .iter()
        .find_map(|c| t.get(c).copied())
        .filter(|c| !c.is_unpriced())
}

/// [`nearest_cost_for_model`], off the cache alone.
pub fn cached_nearest_cost_for_model(model: &str) -> Option<ModelCost> {
    let t = cached_price_table();
    let lower = model.trim().to_ascii_lowercase();
    nearest_priced(t.iter().map(|(k, c)| (k.as_str(), *c)), &lower).map(|(_, c)| c)
}

/// Context window (max INPUT tokens) for a model id, read from the SAME daily
/// LiteLLM table the prices come from — `max_input_tokens`, present on 3271 of
/// its 3853 rows (measured 2026-09-09). This is the answer to a table of model
/// names written by hand: a hardcoded list is wrong the day a model ships, and
/// silently wrong forever after, whereas this one is refreshed conditionally
/// once a day and carries new ids on release day.
///
/// Looked up by EXACT KEY through [`candidates`], like the price: no prefix and
/// no substring, so a newer id cannot inherit an older one's window.
///
/// **Reads the on-disk cache ONLY — never the network.** The two callers make
/// that mandatory: the gain estimator asks from inside a per-turn hook, where a
/// conditional GET would put a round-trip on the user's prompt, and the
/// installer asks while writing a settings file. The cache is kept fresh by the
/// pricing path, which revalidates it once a day.
///
/// `None` when the table has no row for the id, the row carries no window, or
/// there is no cache yet. The caller picks the fail-safe direction for its own
/// use; see `autocompact`, where the two directions are opposite — and where a
/// `None` costs nothing, because both fallbacks were the whole behaviour until
/// this function existed.
pub fn context_window_for_model(model: &str) -> Option<u64> {
    let table = windows_table();
    candidates(model).iter().find_map(|c| table.get(c).copied())
}

/// The id → window map, parsed once per process. The file is 2.3 MB and the
/// estimator asks per turn; parsing it on every call would put ~15 ms of JSON
/// on each prompt for an answer that cannot change inside one process.
fn windows_table() -> &'static std::collections::HashMap<String, u64> {
    static T: std::sync::OnceLock<std::collections::HashMap<String, u64>> =
        std::sync::OnceLock::new();
    T.get_or_init(|| {
        let mut out = std::collections::HashMap::new();
        let Some(raw) =
            cache_path("litellm-costs.json").and_then(|p| std::fs::read_to_string(p).ok())
        else {
            return out;
        };
        if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&raw) {
            for (k, v) in map {
                if let Some(w) = v.get("max_input_tokens").and_then(|x| x.as_u64()) {
                    if w > 0 {
                        out.insert(k, w);
                    }
                }
            }
        }
        out
    })
}

/// Test-only: re-arm the once-per-process guard.
#[cfg(test)]
fn reset_miss_refresh_guard() {
    MISS_REFRESH_TRIED.store(false, std::sync::atomic::Ordering::Relaxed);
}

#[cfg(test)]
mod id_shape_tests {
    use super::candidates;

    /// The id a harness reports is not always the id a price table lists. Every
    /// shape below is one a real agent emits today, and each has to reduce to
    /// the plain model id or the row is missed and the column reads $0 — which
    /// is indistinguishable from "this model cost nothing".
    #[test]
    fn every_real_id_shape_reduces_to_the_plain_model() {
        let cases = [
            // Context-window variant: how a 1M-context session identifies
            // itself. Nothing upstream lists a bracketed id.
            ("claude-opus-5[1m]", "claude-opus-5"),
            ("claude-sonnet-5[1m]", "claude-sonnet-5"),
            // Bedrock, with a region prefix and an inference-profile suffix.
            ("us.anthropic.claude-opus-5-v1:0", "claude-opus-5"),
            ("eu.anthropic.claude-sonnet-5-v1:0", "claude-sonnet-5"),
            // Gateways.
            ("anthropic/claude-opus-5", "claude-opus-5"),
            ("openrouter/anthropic/claude-opus-5", "claude-opus-5"),
            ("claude-opus-5-latest", "claude-opus-5"),
        ];
        let mut missed = Vec::new();
        for (raw, plain) in cases {
            let cands = candidates(raw);
            if !cands.iter().any(|c| c == plain) {
                missed.push(format!("{raw} -> {cands:?} (wanted {plain})"));
            }
        }
        assert!(
            missed.is_empty(),
            "these ids never reach their price row:\n{}",
            missed.join("\n")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "data": [
        {"provider":"ANTHROPIC","model":"claude-3-5-sonnet","operator":"startsWith",
         "input_cost_per_1m":3.0,"output_cost_per_1m":15.0,
         "prompt_cache_write_per_1m":3.75,"prompt_cache_read_per_1m":0.3},
        {"provider":"ANTHROPIC","model":"claude-3-5-sonnet-20241022","operator":"equals",
         "input_cost_per_1m":3.0,"output_cost_per_1m":15.0},
        {"provider":"OPENAI","model":"gpt-4o","operator":"includes",
         "input_cost_per_1m":2.5,"output_cost_per_1m":10.0}
      ]
    }"#;

    #[test]
    fn exact_match_wins_over_prefix() {
        let es = helicone_entries(SAMPLE);
        let e = best_match(&es, "claude-3-5-sonnet-20241022").unwrap();
        assert_eq!(e.get("operator").unwrap(), "equals");
    }

    #[test]
    fn prefix_match_when_no_exact() {
        let es = helicone_entries(SAMPLE);
        let e = best_match(&es, "claude-3-5-sonnet-20240620").unwrap();
        assert_eq!(e.get("operator").unwrap(), "startsWith");
    }

    #[test]
    fn includes_operator_matches_substring() {
        let es = helicone_entries(SAMPLE);
        assert!(best_match(&es, "azure/gpt-4o-mini").is_some());
        assert!(best_match(&es, "llama-3").is_none());
    }

    #[test]
    fn a_newer_version_never_inherits_an_older_models_price() {
        // THE measured bug: `claude-opus-4-8` matched `claude-opus-4` by
        // `includes` and was priced at Opus 4's $15 instead of its own $5.
        const T: &str = r#"{"data":[
            {"provider":"AWS","model":"claude-opus-4","operator":"includes",
             "input_cost_per_1m":15.0,"output_cost_per_1m":75.0}]}"#;
        let es = helicone_entries(T);
        assert!(
            best_match(&es, "claude-opus-4-8").is_none(),
            "a version bump must not borrow the older model's row"
        );
        assert!(
            best_match(&es, "claude-opus-4-1").is_none(),
            "same for a point release"
        );
        // The model the row is actually for still matches.
        assert!(best_match(&es, "claude-opus-4").is_some());
    }

    #[test]
    fn a_dated_snapshot_still_shares_its_families_row() {
        // The other side of the guard: a release date is not a version bump, so
        // `claude-3-5-sonnet-20240620` must keep matching `claude-3-5-sonnet`.
        assert!(!crosses_version_boundary(
            "claude-3-5-sonnet",
            "claude-3-5-sonnet-20240620"
        ));
        assert!(crosses_version_boundary("claude-opus-4", "claude-opus-4-8"));
        assert!(!crosses_version_boundary(
            "claude-opus-4",
            "claude-opus-4-thinking"
        ));
    }

    #[test]
    fn a_duplicate_row_resolves_to_the_first_party_price_not_array_order() {
        // 32 groups in the live table diverge like this. Reseller row FIRST, so
        // an array-order tie-break would pick the wrong one.
        const T: &str = r#"{"data":[
            {"provider":"AZURE","model":"gpt-4o-mini","operator":"equals",
             "input_cost_per_1m":0.20,"output_cost_per_1m":0.80},
            {"provider":"OPENAI","model":"gpt-4o-mini","operator":"equals",
             "input_cost_per_1m":0.15,"output_cost_per_1m":0.60}]}"#;
        let es = helicone_entries(T);
        let e = best_match(&es, "gpt-4o-mini").unwrap();
        assert_eq!(e.get("provider").unwrap(), "OPENAI");
    }

    #[test]
    fn litellm_prices_by_exact_id_and_refuses_a_neighbour() {
        const T: &str = r#"{
          "sample_spec": {"input_cost_per_token": 0.0},
          "claude-opus-5": {"litellm_provider":"anthropic",
            "input_cost_per_token": 5e-6, "output_cost_per_token": 25e-6,
            "cache_creation_input_token_cost": 6.25e-6,
            "cache_read_input_token_cost": 0.5e-6}
        }"#;
        let c = litellm_lookup(T, &candidates("claude-opus-5")).unwrap();
        assert!((c.input_per_1m - 5.0).abs() < 1e-9);
        assert!((c.output_per_1m - 25.0).abs() < 1e-9);
        assert!((c.cache_read_per_1m - 0.5).abs() < 1e-9);
        // A neighbouring id is NOT a prefix hit — exact keys only.
        assert!(litellm_lookup(T, &candidates("claude-opus-5-mini")).is_none());
        // A routing prefix is peeled before lookup.
        assert!(litellm_lookup(T, &candidates("anthropic/claude-opus-5")).is_some());
        assert!(litellm_lookup(T, &candidates("custom:claude-opus-5")).is_some());
    }

    /// Case carries no meaning in a model id: `GPT-5.6-terra` fell to the
    /// built-in fallback (x8) while `gpt-5.6-terra` got its LiteLLM rate (x6).
    #[test]
    fn model_ids_match_whatever_their_case() {
        const T: &str = r#"{
          "gpt-5.6-terra": {"input_cost_per_token": 5e-6, "output_cost_per_token": 30e-6},
          "minimax/MiniMax-M3": {"input_cost_per_token": 1e-6, "output_cost_per_token": 4e-6}
        }"#;
        for id in ["GPT-5.6-terra", " gpt-5.6-TERRA ", "gpt-5.6-terra"] {
            let c = litellm_lookup(T, &candidates(id)).unwrap();
            assert!((c.output_per_1m - 30.0).abs() < 1e-9, "{id}");
        }
        // An upstream key in mixed case matches a lowercase id, and the other way.
        assert!(litellm_lookup(T, &candidates("minimax/minimax-m3")).is_some());
        assert!(litellm_lookup(T, &candidates("MINIMAX/MiniMax-M3")).is_some());
        assert!(litellm_priced(T)
            .iter()
            .all(|(k, _)| *k == k.to_ascii_lowercase()));
    }

    #[test]
    fn a_context_window_only_row_is_not_a_price() {
        // LiteLLM carries stub rows with no cost fields; booking those as $0
        // would be worse than falling through to Helicone.
        const T: &str = r#"{"some-model": {"max_input_tokens": 200000}}"#;
        assert!(litellm_lookup(T, &candidates("some-model")).is_none());
    }

    #[test]
    fn cost_computes_with_cache_fallbacks() {
        let es = helicone_entries(SAMPLE);
        // prefix entry carries explicit cache prices.
        let c = helicone_to_cost(best_match(&es, "claude-3-5-sonnet-20240620").unwrap());
        assert_eq!(c.cache_read_per_1m, 0.3);
        // The four buckets are priced independently, which is exactly why
        // `input` must arrive cache-free: handing `usd` a raw OpenAI-style
        // prompt total (fresh + cached, here 100 + 900) charges the cached
        // 900 twice — once at the input rate, once at the cache-read rate.
        let honest = c.usd(100, 0, 900, 0);
        let double_billed = c.usd(1000, 0, 900, 0);
        assert!(
            double_billed > honest * 2.0,
            "an inclusive prompt total must be visibly more expensive than the \
             decomposed one ({double_billed} vs {honest})"
        );
        // 1M output @ $15 = $15.
        assert!((c.usd(0, 0, 0, 1_000_000) - 15.0).abs() < 1e-9);
        // exact entry omits cache → Anthropic fallbacks (write 1.25×, read 0.1×).
        let c2 = helicone_to_cost(best_match(&es, "claude-3-5-sonnet-20241022").unwrap());
        assert!((c2.cache_write_per_1m - 3.75).abs() < 1e-9);
        assert!((c2.cache_read_per_1m - 0.30).abs() < 1e-9);
    }

    #[test]
    fn the_validator_sidecar_round_trips_and_tolerates_a_short_file() {
        assert_eq!(
            parse_validators("W/\"abc\"\nWed, 01 Jan 2026 00:00:00 GMT\n"),
            (
                "W/\"abc\"".to_string(),
                "Wed, 01 Jan 2026 00:00:00 GMT".to_string()
            )
        );
        // Only an ETag, no Last-Modified.
        assert_eq!(
            parse_validators("W/\"abc\"\n"),
            ("W/\"abc\"".to_string(), String::new())
        );
        // Empty / absent sidecar must not panic and must send no validator.
        assert_eq!(parse_validators(""), (String::new(), String::new()));
    }

    #[test]
    fn a_row_that_carries_no_numbers_counts_as_unpriced() {
        // Helicone's `unwrap_or(0.0)` means a row present but priceless yields
        // all-zero rates. Returning that as Some() would print $0 on the
        // dashboard and look ANSWERED, which is worse than admitting we do not
        // know — and it would never trigger the recovery refresh.
        const T: &str = r#"{"data":[
            {"provider":"ANTHROPIC","model":"claude-ghost-9","operator":"equals"}]}"#;
        let es = helicone_entries(T);
        let e = best_match(&es, "claude-ghost-9").expect("row matches by name");
        assert!(
            helicone_to_cost(e).is_unpriced(),
            "a row with no numbers is not a price"
        );
        // And a real price is not mistaken for one.
        assert!(!ModelCost {
            input_per_1m: 5.0,
            output_per_1m: 25.0,
            cache_write_per_1m: 6.25,
            cache_read_per_1m: 0.5,
        }
        .is_unpriced());
    }

    #[test]
    fn the_recovery_refresh_is_attempted_once_per_process() {
        // The retry costs a revalidation, so an id no upstream carries (a local
        // Ollama tag) must not force one per pricing call. First ask may pass,
        // every later ask in this process is refused.
        reset_miss_refresh_guard();
        let first = miss_refresh_allowed();
        let second = miss_refresh_allowed();
        let third = miss_refresh_allowed();
        assert!(!second && !third, "only the first attempt may proceed");
        // `first` depends on the on-disk hourly stamp, so it is not asserted —
        // what matters is that it cannot repeat.
        let _ = first;
    }

    #[test]
    fn a_new_point_release_borrows_the_current_generation_not_the_legacy_one() {
        // The trap plain Levenshtein walks into: for `claude-opus-5-1`,
        // edit distance to `claude-opus-4-1` is 1 (5→4) and to `claude-opus-5`
        // is 2 (delete "-1"), so pure edit distance picks the $15 legacy row
        // over the $5 current one — the exact 3× overstatement this module was
        // rewritten to remove. Stem + version proximity picks correctly.
        assert_eq!(levenshtein("claude-opus-5-1", "claude-opus-4-1"), 1);
        assert_eq!(levenshtein("claude-opus-5-1", "claude-opus-5"), 2);

        let five = ModelCost {
            input_per_1m: 5.0,
            output_per_1m: 25.0,
            cache_write_per_1m: 6.25,
            cache_read_per_1m: 0.5,
        };
        let legacy = ModelCost {
            input_per_1m: 15.0,
            output_per_1m: 75.0,
            cache_write_per_1m: 18.75,
            cache_read_per_1m: 1.5,
        };
        let known = [
            ("claude-opus-4-1", legacy),
            ("claude-opus-4-8", five),
            ("claude-opus-5", five),
            ("claude-haiku-4-5", five),
        ];
        let (borrowed, cost) =
            nearest_priced(known.iter().map(|(k, c)| (*k, *c)), "claude-opus-5-1")
                .expect("a sibling exists");
        assert_eq!(borrowed, "claude-opus-5", "must not borrow from Opus 4.1");
        assert_eq!(cost.input_per_1m, 5.0);
    }

    #[test]
    fn the_newer_of_two_equidistant_versions_wins() {
        let c = |i: f64| ModelCost {
            input_per_1m: i,
            output_per_1m: i * 5.0,
            cache_write_per_1m: i,
            cache_read_per_1m: i,
        };
        // A hypothetical `-5` sits exactly between `-4` and `-6`.
        let known = [("m-4", c(4.0)), ("m-6", c(6.0))];
        let (id, _) = nearest_priced(known.iter().map(|(k, v)| (*k, *v)), "m-5").unwrap();
        assert_eq!(
            id, "m-6",
            "a fresh release is priced like the newer sibling"
        );
    }

    #[test]
    fn stem_and_version_splits_only_trailing_numbers() {
        assert_eq!(
            stem_and_version("claude-opus-5-1"),
            ("claude-opus".to_string(), vec![5, 1])
        );
        assert_eq!(
            stem_and_version("claude-opus-4-8"),
            ("claude-opus".to_string(), vec![4, 8])
        );
        // `4o` is not numeric, so it stays in the stem — two ids are only
        // comparable when the non-numeric part matches exactly.
        assert_eq!(
            stem_and_version("gpt-4o-mini"),
            ("gpt-4o-mini".to_string(), vec![])
        );
    }

    #[test]
    fn an_unrelated_id_is_not_given_a_neighbours_price() {
        // "better a price than zero" has a limit: past the edit-distance cap we
        // would be inventing a number on a money column.
        let c = ModelCost {
            input_per_1m: 5.0,
            output_per_1m: 25.0,
            cache_write_per_1m: 5.0,
            cache_read_per_1m: 0.5,
        };
        let known = [("claude-opus-5", c)];
        assert!(
            nearest_priced(known.iter().map(|(k, v)| (*k, *v)), "ollama/llama-local").is_none(),
            "an unrelated local tag must not borrow Opus pricing"
        );
    }

    /// Live spot-check: the current lineup must price. This is the guard for
    /// "the problem comes back at every model launch" — run it after one.
    /// `cargo test -p tokenade-core model_costs -- --ignored --nocapture`
    #[test]
    #[ignore = "network"]
    fn the_current_lineup_is_priced_by_a_live_source() {
        let mut missing = Vec::new();
        for m in SPOT_CHECK_MODELS {
            match cost_for_model(m) {
                Some(c) => println!("  {m:20} ${:.2} / ${:.2}", c.input_per_1m, c.output_per_1m),
                None => missing.push(*m),
            }
        }
        assert!(missing.is_empty(), "unpriced models: {missing:?}");
    }

    /// The same models, named the way a real deployment names them. A price
    /// table lists the plain id; a session, a Bedrock profile and a gateway all
    /// report something else, and an id that misses its row is a $0 column —
    /// which reads as "this model cost nothing" rather than "we do not know".
    ///
    /// The variant that matters most is the one this very product runs under:
    /// a 1M-context session calls itself `claude-opus-5[1m]`, and nothing
    /// upstream has ever listed a bracketed id.
    #[test]
    #[ignore = "network"]
    fn the_lineup_is_priced_under_the_ids_a_deployment_actually_reports() {
        let plain = cost_for_model("claude-opus-5").expect("base id must price");
        let variants = [
            "claude-opus-5[1m]",
            "us.anthropic.claude-opus-5-v1:0",
            "eu.anthropic.claude-opus-5-v1:0",
            "anthropic/claude-opus-5",
            "openrouter/anthropic/claude-opus-5",
            "claude-opus-5-latest",
        ];
        let mut wrong = Vec::new();
        for v in variants {
            // A Bedrock id is not expected to equal the first-party price: AWS
            // publishes its own, higher, row and the exact id finds it. What
            // must hold is that every id finds SOME row, and that a reduction
            // never lands on a different generation.
            let bedrock = v.contains(".anthropic.");
            match cost_for_model(v) {
                None => wrong.push(format!("{v}: unpriced")),
                Some(c) if bedrock => {
                    if c.input_per_1m < plain.input_per_1m
                        || c.input_per_1m > plain.input_per_1m * 2.0
                    {
                        wrong.push(format!(
                            "{v}: ${:.2}, not a plausible markup on ${:.2}",
                            c.input_per_1m, plain.input_per_1m
                        ));
                    }
                }
                Some(c) if c.input_per_1m != plain.input_per_1m => wrong.push(format!(
                    "{v}: ${:.2} but the plain id is ${:.2}",
                    c.input_per_1m, plain.input_per_1m
                )),
                Some(_) => {}
            }
        }
        assert!(
            wrong.is_empty(),
            "deployment ids mispriced:\n{}",
            wrong.join("\n")
        );
    }
}
