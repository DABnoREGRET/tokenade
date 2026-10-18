//! **A thousand objects of the same shape are ONE shape and a thousand rows.**
//!
//! An API response is often a homogeneous array: describing the schema once
//! then rendering the values as a block says the same thing for a fraction of the
//! price.
//!
//! Two bounds keep it honest. There must be enough elements for the
//! schema to pay for itself, and one shape must DOMINATE — a heterogeneous
//! array folded onto its majority schema would lose the minority items,
//! which are precisely the ones we were looking for.

use super::*;

/// Minimum array length to consider schema-folding. Below this the
/// fold annotation overhead (`[×N items fit schema]…`) costs more
/// than rendering each item.
pub const FOLD_MIN_ITEMS: usize = 5;

/// Below this many bytes of items, the summary writes every item instead of
/// three. A CLI's `show --json` carries twelve writing principles,
/// 4 KB together, each a distinct sentence with its evidence: three were
/// shown and nine "elided · use jq", for ~750 tokens saved — then paid back
/// by the re-read the agent needed to follow principles it could not see.
/// The fold earns its keep on hundreds of records, not on a dozen.
pub const SMALL_ARRAY_BYTES: usize = 6_000;
/// Required share of items whose structural signature matches the
/// dominant signature, in percent. 80 % keeps folds robust to a
/// handful of anomalous rows (e.g. a `{"_meta":...}` envelope mixed
/// into a list of users).
pub const FOLD_MIN_DOMINANT_PCT: usize = 80;

/// L65 + L76 — Schema-fold a homogeneous JSON array. Returns
/// `Some(rendered)` when a foldable array is found at:
///   * the root (top-level array), OR
///   * one level deep under a known payload key (`data`, `users`,
///     `items`, `results`, `nodes`, `edges`, `records`, `entries`,
///     `posts`, `rows`), OR
///   * two levels deep along the same key set (e.g. GraphQL's
///     `data.users`, `data.viewer.posts`).
///
/// When the foldable array is NESTED, the wrapping object's other
/// fields are preserved in the output — so a REST payload like
/// `{"users":[...500 users...], "page":1, "total":1200}` keeps the
/// `page` / `total` metadata AND describes the array shape, instead
/// of losing the array entirely (pre-L76 bug).
///
/// Rendered shape (top-level array):
///
/// ```text
/// [×N items fit schema] {id:<int>, user:<str>, score:<int>}
///   distinct: id=N, user=K, score=L
///   e.g. {"id":1,"user":"alice","score":95}
///   …
/// ```
///
/// Rendered shape (nested array):
///
/// ```text
/// {
///   "page": 1,
///   "total": 1200,
///   "users": "<see-fold-block-below>"
/// }
///
/// [fold @ users] ×500 items fit schema: {id:<int>, ...}
///   distinct: id=500, …
///   e.g. {...}
/// ```
pub fn schema_fold(json: &str) -> Option<String> {
    let v: Value = serde_json::from_str(json).ok()?;
    // 1. Root-level array — original behaviour.
    if let Some(arr) = v.as_array() {
        if arr.len() >= FOLD_MIN_ITEMS {
            return fold_array_with_source(arr, None, Some(json)).or_else(|| records_of(arr, json));
        }
        return None;
    }
    // 2. Nested array under a known payload key. Walk up to 2 levels
    // deep. Pick the LARGEST matching array (most info to compress).
    let (path, arr) = find_largest_payload_array(&v, 2)?;
    let shown = display_path(&path);
    let block = fold_array_with_source(&arr, Some(&shown), Some(json))
        .or_else(|| records_of(&arr, json))?;
    // Reconstruct the wrapping object with the array replaced by a
    // pointer to the fold block.
    let mut wrapper = v.clone();
    if let Some(target) = wrapper.pointer_mut(&pointer_path(&path)) {
        *target = Value::String(format!("<see [fold @ {shown}] block below>"));
    }
    // #292 — MINIFIED, and it was pretty-printed. The fold replaces one array
    // with a pointer and re-emits everything around it; on a large schema that
    // "everything around it" is the bulk of the file, so every space and
    // newline of the original survived AND the fold block was added on top.
    // Measured over 1 180 real `.json` files delivered to the agent unchanged:
    // the fold cost 15 458 230 tokens against 15 007 774 for the input itself,
    // which is why `auto`'s anti-inflation guard threw it away and handed back
    // the original — uncompacted. Minifying the wrapper is lossless: it drops
    // whitespace, not data.
    let wrapper_json = serde_json::to_string(&wrapper).unwrap_or_default();
    Some(format!("{wrapper_json}\n\n[fold @ {shown}] {block}"))
}

/// An array of objects the schema fold refused — keys missing on some items,
/// a type that differs — as a record table: what never changes once, a row
/// per item of what does. Without it the refusal sent the whole document to
/// the generic path: a 246-video playlist came back minified and cut after
/// its FIRST record.
fn records_of(arr: &[Value], source: &str) -> Option<String> {
    let objs: Vec<serde_json::Map<String, Value>> = arr
        .iter()
        .map(|v| v.as_object().cloned())
        .collect::<Option<_>>()?;
    super::records_table(&objs, source, "records")
}

/// Known payload keys, in priority order. When multiple foldable
/// arrays exist, the first match wins (so `users` beats `meta`).
pub(crate) const PAYLOAD_KEYS: &[&str] = &[
    "data", "users", "items", "results", "nodes", "edges", "records", "entries", "posts", "rows",
    "list", "values",
];

/// Walk up to `max_depth` levels into `v` (an object), looking for
/// the largest array that meets the fold criteria. Returns
/// `Some((dotted_path, array_clone))` on hit. The path uses `.` as a
/// separator (`data.users`, `viewer.posts`); call [`pointer_path`]
/// to convert to a JSON-pointer.
pub(crate) fn find_largest_payload_array(
    v: &Value,
    max_depth: usize,
) -> Option<(String, Vec<Value>)> {
    fn walk(
        v: &Value,
        path: &str,
        depth_left: usize,
        best: &mut Option<(String, usize, Vec<Value>)>,
    ) {
        let Some(obj) = v.as_object() else { return };
        // Prefer the priority payload keys at this level.
        let ordered: Vec<_> = {
            let mut keys: Vec<_> = obj.keys().collect();
            keys.sort_by_key(|k| {
                PAYLOAD_KEYS
                    .iter()
                    .position(|pk| *pk == k.as_str())
                    .unwrap_or(usize::MAX)
            });
            keys
        };
        for k in ordered {
            let child = &obj[k];
            let sub_path = if path.is_empty() {
                k.clone()
            } else {
                format!("{path}.{k}")
            };
            if let Some(arr) = child.as_array() {
                if arr.len() >= FOLD_MIN_ITEMS {
                    let len = arr.len();
                    let take = match best {
                        Some((_, n, _)) => len > *n,
                        None => true,
                    };
                    if take {
                        *best = Some((sub_path.clone(), len, arr.clone()));
                    }
                } else if depth_left > 0 {
                    // An array too SHORT to fold is not a dead end: the payload
                    // is often one level inside its elements. `aws ec2
                    // describe-instances` returns `Reservations[0].Instances`,
                    // and the walk descended through OBJECTS only — so the
                    // single most common AWS response shape fell back to
                    // "first 3 items + …N more items…", losing the one
                    // instance that differed. Measured 2026-08-28 on 121
                    // instances, one terminated with a `StateTransitionReason`:
                    // `{"Instances": […]}` and `{"Reservations": {"Instances":
                    // […]}}` both fold and surface the odd item;
                    // `{"Reservations": [{"Instances": […]}]}` did not.
                    //
                    // Only the first few elements, and only objects: a short
                    // list of scalars has nothing to walk into, and a list long
                    // enough to matter would have been foldable right here.
                    for (i, el) in arr.iter().take(ARRAY_HOP_ELEMENTS).enumerate() {
                        if el.is_object() {
                            walk(el, &format!("{sub_path}.{i}"), depth_left - 1, best);
                        }
                    }
                }
            } else if child.is_object() && depth_left > 0 {
                walk(child, &sub_path, depth_left - 1, best);
            }
        }
    }
    let mut best: Option<(String, usize, Vec<Value>)> = None;
    walk(v, "", max_depth, &mut best);
    best.map(|(p, _, a)| (p, a))
}

/// How many elements of a non-foldable array the payload walk looks inside.
/// A wrapper list (`Reservations`) is almost always one element; scanning more
/// buys nothing and costs a clone per candidate.
pub(crate) const ARRAY_HOP_ELEMENTS: usize = 4;

/// A dotted path with numeric segments, written the way a reader indexes:
/// `Reservations.0.Instances` → `Reservations[0].Instances`.
pub(crate) fn display_path(dotted: &str) -> String {
    let mut out = String::with_capacity(dotted.len() + 4);
    for (i, seg) in dotted.split('.').enumerate() {
        if seg.chars().all(|c| c.is_ascii_digit()) && !seg.is_empty() && i > 0 {
            out.push('[');
            out.push_str(seg);
            out.push(']');
        } else {
            if i > 0 {
                out.push('.');
            }
            out.push_str(seg);
        }
    }
    out
}

/// Convert a dotted path (`data.users`) to a JSON pointer (`/data/users`).
pub(crate) fn pointer_path(dotted: &str) -> String {
    if dotted.is_empty() {
        return String::new();
    }
    format!("/{}", dotted.replace('.', "/"))
}

/// Extract the fold block computation from the rest of `schema_fold`
/// so the nested-array path can reuse it. `path` is `None` for
/// root-level arrays (cosmetic — affects only the header text).
#[cfg(test)]
pub(crate) fn fold_array_to_block(arr: &[Value], path: Option<&str>) -> Option<String> {
    fold_array_with_source(arr, path, None)
}

/// [`fold_array_to_block`], told the source text: before the LOSSY summary
/// (`[×N items fit schema]`, one example and distinct counts), the record
/// table is tried — a uniform list of issues kept ONE title of twenty there.
pub(crate) fn fold_array_with_source(
    arr: &[Value],
    path: Option<&str>,
    source: Option<&str>,
) -> Option<String> {
    use std::collections::BTreeMap;
    use std::collections::BTreeSet;
    if arr.len() < FOLD_MIN_ITEMS {
        return None;
    }
    let signatures: Vec<String> = arr.iter().map(structural_signature).collect();
    // Dominant signature = the most common one across the array.
    let mut sig_counts: BTreeMap<&String, usize> = BTreeMap::new();
    for s in &signatures {
        *sig_counts.entry(s).or_insert(0) += 1;
    }
    let (dominant_sig, dominant_count) = sig_counts
        .iter()
        .max_by_key(|(_, n)| **n)
        .map(|(s, n)| ((*s).clone(), *n))?;
    let pct = dominant_count * 100 / arr.len();
    if pct < FOLD_MIN_DOMINANT_PCT {
        return None;
    }
    // Collect matching items (those whose signature matches) and
    // anomalous ones (everything else).
    let matching: Vec<&Value> = arr
        .iter()
        .zip(&signatures)
        .filter_map(|(v, s)| (s == &dominant_sig).then_some(v))
        .collect();
    let anomalous: Vec<&Value> = arr
        .iter()
        .zip(&signatures)
        .filter_map(|(v, s)| (s != &dominant_sig).then_some(v))
        .collect();
    let anomalous_count = anomalous.len();

    // Need at least one matching object to template. Bail on
    // non-object dominant shapes (`["a","b","c","d","e"]` ⇒
    // unproductive fold — already compact).
    let template_obj = matching.first().and_then(|v| v.as_object())?;
    let keys: Vec<String> = template_obj.keys().cloned().collect();
    if keys.is_empty() {
        return None;
    }

    // L?? — TOON lossless re-encode, tried BEFORE the lossy schema summary.
    // When the array is a CLEAN table — every item matches the dominant
    // shape (no anomalies), every value is a scalar, and the row count is
    // within a budget where a full dump still beats a shape summary — we
    // render it as TOON (Token-Oriented Object Notation): the shared keys
    // are written ONCE in a header, then one comma-separated row per object.
    // This sheds the per-row key repetition (the bulk of the redundancy in
    // an array-of-objects) WITHOUT dropping a single value, so the agent
    // keeps all the data. The genuinely-summarisable cases — anomalous rows,
    // nested array/object values, or arrays too large to dump in full — fall
    // through to the lossy `[×N items fit schema]` block below.
    const TOON_MAX_ROWS: usize = 500;
    if anomalous_count == 0 && matching.len() <= TOON_MAX_ROWS {
        if let Some(toon) = fold_array_to_toon(&matching, &keys) {
            // Inflation guard: only prefer TOON when it is actually shorter
            // than the raw array it would replace (it always is for a real
            // table, but never trust that blindly).
            let raw_len: usize = matching
                .iter()
                .map(|v| serde_json::to_string(v).map(|s| s.len()).unwrap_or(0) + 1)
                .sum();
            if toon.len() < raw_len {
                return Some(toon);
            }
        }
    }

    if let Some(table) = source.and_then(|src| records_of(arr, src)) {
        return Some(table);
    }

    // Per-key distinct value sets (capped — we don't need to know exactly that
    // user=487 distinct, just `>1 → varying`).
    //
    // The cap is right and the way it was REPORTED was not: the set stopped at
    // 200 and the line printed `id=200` whatever the truth. Measured
    // 2026-08-28 on arrays of 100/200/300/400 uniform items, the reported
    // distinct count read 100, 200, **200, 200** — so above the cap the number
    // is not an approximation, it is wrong, and an agent reading "400 items,
    // 200 distinct ids" concludes each id appears twice. The set is allowed one
    // element past the cap so saturation can be TOLD apart from a dataset that
    // genuinely has exactly 200, and the line then says `200+`.
    let mut distinct_per_key: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for k in &keys {
        distinct_per_key.insert(k.clone(), BTreeSet::new());
    }
    for item in &matching {
        if let Some(o) = item.as_object() {
            for k in &keys {
                let v_str = o
                    .get(k)
                    .map(|v| serde_json::to_string(v).unwrap_or_default())
                    .unwrap_or_default();
                let set = distinct_per_key.get_mut(k).expect("key inserted above");
                if set.len() <= DISTINCT_CAP {
                    set.insert(v_str);
                }
            }
        }
    }

    // Build the schema template line: {key:<TYPE>, ...} where
    // <TYPE> is the value-kind in the dominant signature.
    let schema_line = {
        let mut parts: Vec<String> = Vec::with_capacity(keys.len());
        for k in &keys {
            let kind_token = value_kind_token(template_obj.get(k).unwrap_or(&Value::Null));
            parts.push(format!("{}:{}", json_quote_key(k), kind_token));
        }
        format!("{{{}}}", parts.join(", "))
    };
    let distinct_str: Vec<String> = keys
        .iter()
        .map(|k| {
            let n = distinct_per_key.get(k).map(|s| s.len()).unwrap_or(0);
            if n > DISTINCT_CAP {
                format!("{}={DISTINCT_CAP}+", json_quote_key(k))
            } else {
                format!("{}={}", json_quote_key(k), n)
            }
        })
        .collect();
    // 3 representative samples (compact serialise — single line each) — or
    // every item, when all of them weigh less than [`SMALL_ARRAY_BYTES`].
    let all_lines: Vec<String> = matching
        .iter()
        .map(|v| serde_json::to_string(v).unwrap_or_default())
        .collect();
    let keep = if all_lines.iter().map(String::len).sum::<usize>() <= SMALL_ARRAY_BYTES {
        all_lines.len()
    } else {
        3
    };
    let sample_lines: Vec<String> = all_lines.into_iter().take(keep).collect();
    let elided = matching.len().saturating_sub(keep);
    // The distinct counts are computed over the items that FIT the schema, and
    // saying so matters only when some do not: on the 800-job list whose one
    // failure was off-schema, a bare `state=1` reads as "every job has the same
    // state", which is not true of the data. Qualified only when there are
    // anomalies, so the common fold pays nothing for it.
    let dist_label = if anomalous_count > 0 {
        "distinct (among those that fit)"
    } else {
        "distinct"
    };
    let mut out = format!(
        "[×{n} items fit schema] {schema_line}\n  {dist_label}: {dist}\n  e.g. {first}",
        n = matching.len(),
        dist = distinct_str.join(", "),
        first = sample_lines.first().cloned().unwrap_or_default(),
    );
    for s in sample_lines.iter().skip(1) {
        out.push_str("\n       ");
        out.push_str(s);
    }
    if elided > 0 {
        let jq_target = match path {
            Some(p) => format!(".{p}[]"),
            None => ".[]".to_string(),
        };
        let _ = write!(
            out,
            "\n  ({elided} more items elided · use jq to query specifics, e.g. `jq '{jq_target} | select(...)'`)"
        );
    }
    if anomalous_count > 0 {
        // SHOW them, do not just count them.
        //
        // An item that does not fit the shape the other 799 share is, by
        // construction, the interesting one — the failed job among the
        // succeeded ones, the record with the extra `error` field. Announcing
        // "there is one, fetch raw for it" hands the reader a summary of the
        // boring part and hides the part they were looking for, with nothing
        // in it to say what they would find. MEASURED: an 800-item job list
        // whose single failure carried `"message":"disk full on shard 7"` was
        // folded to a schema that mentioned neither the failure nor the disk.
        //
        // Bounded, and each one clipped: an array of 400 distinct shapes must
        // not reprint itself here.
        let shown: Vec<String> = anomalous
            .iter()
            .take(ANOMALY_SHOW_MAX)
            .map(|v| {
                let mut line = serde_json::to_string(v).unwrap_or_default();
                if line.chars().count() > ANOMALY_MAX_CHARS {
                    line = line.chars().take(ANOMALY_MAX_CHARS).collect::<String>() + "…";
                }
                line
            })
            .collect();
        let more = anomalous_count.saturating_sub(shown.len());
        let tail = if more > 0 {
            format!(" (+{more} more, fetch raw)")
        } else {
            String::new()
        };
        let _ = write!(
            out,
            "\n  [+{anomalous_count} item(s) with a DIFFERENT shape — the ones that do not fit \
             the schema above]{tail}"
        );
        for line in &shown {
            let _ = write!(out, "\n       {line}");
        }
    }
    Some(out)
}

/// How many off-schema items the fold prints in full. Three is the same
/// budget the schema uses for representative samples, and past it the reader
/// is looking at a heterogeneous array, not an outlier.
pub(crate) const ANOMALY_SHOW_MAX: usize = 3;

/// Per-anomaly clip. An outlier carrying a 40 KB stack trace must not undo the
/// fold it appears in.
pub(crate) const ANOMALY_MAX_CHARS: usize = 400;

/// Render `matching` (objects that all share `keys`) as a lossless TOON
/// table: a single header naming the columns, then one comma-separated row
/// per object. Returns `None` — so the caller falls back to the lossy
/// schema summary — when any value under any key is NOT a scalar, since a
/// nested array/object can't live in a flat cell without re-introducing the
/// very redundancy TOON removes (and wouldn't round-trip cleanly).
///
/// Output shape:
/// ```text
/// [×5 rows · toon] id,user,score
///   1,"alice",95
///   2,"bob",82
///   …
/// ```
/// Keys and string values are JSON-quoted (so commas, newlines, and tokens
/// that would otherwise read as bare `null`/`true`/numbers stay unambiguous);
/// numbers, bools and null are bare. Nothing is elided — every row is kept.
pub(crate) fn fold_array_to_toon(matching: &[&Value], keys: &[String]) -> Option<String> {
    // Reject any non-scalar cell up front.
    for item in matching {
        let o = item.as_object()?;
        for k in keys {
            if matches!(o.get(k), Some(Value::Array(_)) | Some(Value::Object(_))) {
                return None;
            }
        }
    }
    // **A column that never changes is said once.** Measured 2026-10-02 on the
    // 378 payloads a CLI writes to `out/` (29 MB): 4 649 columns hold one
    // value in every row — `"github_followers":""`, `"seniority":""` — and
    // their cells are 2.3 % of the bytes, repeated on every row of the table.
    // Such a column moves to the header as `key=value`; nothing is lost, the
    // table only stops repeating it. (A table has `FOLD_MIN_ITEMS` rows at
    // least, so "constant" is never read off one or two rows.)
    let cell = |item: &Value, k: &String| toon_cell(item.get(k).unwrap_or(&Value::Null));
    let constant: Vec<bool> = keys
        .iter()
        .map(|k| {
            matching
                .iter()
                .all(|it| cell(it, k) == cell(matching[0], k))
        })
        .collect();
    // Every column constant = identical rows: keep the plain table rather
    // than a table with no columns.
    let constant: Vec<bool> = if constant.iter().all(|c| *c) {
        vec![false; keys.len()]
    } else {
        constant
    };
    let varying: Vec<&String> = keys
        .iter()
        .zip(&constant)
        .filter(|(_, c)| !**c)
        .map(|(k, _)| k)
        .collect();
    let header: Vec<String> = varying.iter().map(|k| json_quote_key(k)).collect();
    let mut out = format!("[×{} rows · toon] {}", matching.len(), header.join(","));
    let same: Vec<String> = keys
        .iter()
        .zip(&constant)
        .filter(|(_, c)| **c)
        .map(|(k, _)| format!("{}={}", json_quote_key(k), cell(matching[0], k)))
        .collect();
    if !same.is_empty() {
        out.push_str(&format!(" · same in every row: {}", same.join(", ")));
    }
    for item in matching {
        let cells: Vec<String> = varying.iter().map(|k| cell(item, k)).collect();
        out.push_str("\n  ");
        out.push_str(&cells.join(","));
    }
    Some(out)
}

/// One TOON cell. Strings are JSON-quoted (keeps embedded commas/newlines
/// unambiguous and prevents a string like `"12"` or `"null"` from reading
/// back as a bare token); scalars are rendered bare. Non-scalars never reach
/// here — [`fold_array_to_toon`] rejects them first.
pub(crate) fn toon_cell(v: &Value) -> String {
    match v {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        _ => serde_json::to_string(v).unwrap_or_default(),
    }
}
