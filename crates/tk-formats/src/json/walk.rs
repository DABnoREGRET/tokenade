//! **The descent**, and what it drops along the way.
//!
//! A key is noise or signal depending on the SHAPE of the response: `url` is
//! decoration on GitHub and the data itself on npm. So the descent consults
//! the trunk's tables, it decides nothing on its own.
//!
//! The depth is bounded: a hostile JSON is an arbitrarily deep
//! tree, and overflowing the stack is a crash, not a refusal.

use super::*;

/// Produce a stable structural signature for a JSON value — used to
/// group "shaped the same" items in a homogeneous array.
pub(crate) fn structural_signature(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(_) => "bool".into(),
        Value::Number(_) => "num".into(),
        Value::String(_) => "str".into(),
        Value::Array(a) => {
            // Just record the elt signature (assumed homogeneous for
            // typical arrays). For mixed inner arrays the signature
            // collapses; that's fine — outer fold may still match.
            let inner = a.first().map(structural_signature).unwrap_or_default();
            format!("arr[{inner}]")
        }
        Value::Object(o) => {
            let mut parts: Vec<String> = o
                .iter()
                .map(|(k, vv)| format!("{k}:{}", structural_signature(vv)))
                .collect();
            parts.sort();
            format!("{{{}}}", parts.join(","))
        }
    }
}

pub(crate) fn value_kind_token(v: &Value) -> &'static str {
    match v {
        Value::Null => "<null>",
        Value::Bool(_) => "<bool>",
        Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                "<int>"
            } else {
                "<num>"
            }
        }
        Value::String(_) => "<str>",
        Value::Array(_) => "<arr>",
        Value::Object(_) => "<obj>",
    }
}

pub(crate) fn json_quote_key(k: &str) -> String {
    // Match jq-readable formatting for the schema line: quote when the
    // key contains chars outside `[A-Za-z0-9_]`, else raw.
    if k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        k.to_string()
    } else {
        format!("\"{k}\"")
    }
}

pub fn compact_with(json: &str, opts: &CompactOptions) -> String {
    let v: Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(_) => return json.to_string(),
    };
    if is_a_wrapped_document(&v, json.len(), opts.str_cap) {
        return json.to_string();
    }
    let out = walk(v, opts, 0);
    serde_json::to_string(&out).unwrap_or_else(|_| json.to_string())
}

/// **A JSON that IS one document is not a record with a long field.**
///
/// The walk cuts every string past `str_cap`, which is right for a record —
/// an API reply whose `description` runs on — and wrong for a file whose whole
/// point is one text: `{"context": "<15 KB of markdown>"}`. Measured
/// 2026-10-02 on a real `Read` of such a file (a tool's context export):
/// 4 399 tokens served as 77, the first 200 characters and `…[+15068 chars]`,
/// i.e. 98 % of the very text the agent opened the file for.
///
/// The text need not be ONE leaf. A CLI's `show --json` (4.5 KB)
/// carries its payload in three: `instructions`, `owner_instructions` and the
/// idea's `text`. Each was cut to 200 characters and the agent's next call was
/// `expand-ref` — the instructions WERE the answer. So the test is the share of
/// the bytes held by text leaves the walk would cut, summed: when they carry
/// half the document or more (and 2 000 bytes at least), the compactor declines
/// and the caller's ordinary route decides.
///
/// Two limits keep the record case cut. A leaf reads as text only with spaces
/// (one character in twenty) — a base64, PEM or minified blob has none to
/// speak of, and cutting THAT stays right. And a leaf reached through an array
/// does not count: in a list of records (`gh issue list --json body`) the cut
/// is what makes the list an overview. A list of one or two is not one:
/// A CLI's `plan --json` (7.5 KB) carries the ONE
/// target article to answer in `targets[0].article`, beside `guidance` and
/// `disallowed_claims` — all three cut, and the agent went to `expand-ref`
/// three times in a week.
fn is_a_wrapped_document(v: &Value, total: usize, cap: usize) -> bool {
    fn text_bytes(v: &Value, cap: usize) -> usize {
        match v {
            Value::String(s) => {
                let n = s.chars().count();
                let spaces = s.chars().filter(|c| c.is_whitespace()).count();
                if n > cap + cap / 4 && spaces * 20 >= n {
                    s.len()
                } else {
                    0
                }
            }
            Value::Object(o) => o.values().map(|x| text_bytes(x, cap)).sum(),
            Value::Array(a) if a.len() <= 2 => a.iter().map(|x| text_bytes(x, cap)).sum(),
            _ => 0,
        }
    }
    let text = text_bytes(v, cap);
    text >= 2_000 && text * 2 >= total
}

pub fn compact_with_savings(json: &str) -> (String, crate::Savings) {
    let before = tokenizer::estimate(json);
    let out = compact(json);
    let after = tokenizer::estimate(&out);
    (out, crate::Savings::new(before, after))
}

pub(crate) fn key_dropped(k: &str) -> bool {
    DROP_KEYS.contains(&k)
        || (!KEEP_URL_KEYS.contains(&k) && DROP_SUFFIXES.iter().any(|s| k.ends_with(s)))
}

/// #294 — a telemetry key is chrome when it HOLDS an id and payload when it
/// DESCRIBES one.
///
/// Measured over 44 008 real `.json` files: `requestId` alone accounted for
/// 1 009 771 bytes across 324 files, a median of 405 bytes per occurrence —
/// which is no id at all. They are property definitions inside API schemas
/// (`{"description":"Optional. An optional request ID to identify requests…",
/// "type":"string"}`), so dropping them deletes a documented parameter from the
/// description of an endpoint. 1 318 of the 2 444 occurrences are over 200
/// bytes.
///
/// A scalar value is the discriminator, and it is the shape these keys have
/// wherever they really are chrome.
pub(crate) fn telemetry_key_holding_a_value(k: &str, v: &Value) -> bool {
    const TELEMETRY_KEYS: &[&str] = &[
        "trace_id",
        "traceId",
        "span_id",
        "spanId",
        "request_id",
        "requestId",
        "correlation_id",
        "correlationId",
        "debug_info",
        "debugInfo",
    ];
    TELEMETRY_KEYS.contains(&k) && !matches!(v, Value::Object(_) | Value::Array(_))
}

/// Shape-specific extra drops applied to a key BEFORE generic drop rules
/// see it. Returning `true` means "skip this key entirely".
fn schema_drops(shape: Shape, k: &str) -> bool {
    match shape {
        Shape::GitHub => GH_PR_NESTED_DROPS.contains(&k),
        Shape::NpmRegistry => NPM_REGISTRY_DROPS.contains(&k),
        Shape::Kubernetes | Shape::Generic => false,
    }
}

/// Specialised walk for a Kubernetes object's `metadata` field. The plain
/// walk loses too much (we drop nothing) or too little (we'd drop labels);
/// this one drops the server-side noise + the kubectl applied-config blob.
pub(crate) fn walk_k8s_metadata(v: Value, opts: &CompactOptions, depth: usize) -> Value {
    let Value::Object(map) = v else {
        return walk(v, opts, depth);
    };
    let mut out = serde_json::Map::new();
    for (k, child) in map {
        if K8S_METADATA_DROPS.contains(&k.as_str()) {
            continue;
        }
        // annotations: drop just the kubectl applied-config / revision spam
        let walked = if k == "annotations" {
            if let Value::Object(ann) = child {
                let mut kept = serde_json::Map::new();
                for (ak, av) in ann {
                    if K8S_ANNOTATION_DROPS.contains(&ak.as_str()) {
                        continue;
                    }
                    kept.insert(ak, walk(av, opts, depth + 1));
                }
                Value::Object(kept)
            } else {
                walk(child, opts, depth + 1)
            }
        } else {
            walk(child, opts, depth + 1)
        };
        if opts.drop_nulls && walked.is_null() {
            continue;
        }
        out.insert(k, walked);
    }
    Value::Object(out)
}

/// A string is cut only when the cut removes more than a quarter of `cap`.
///
/// A CLI's `show --json` delivered a 201-character command
/// line as 200 characters and `…[+1 chars]` — thirteen characters of marker to
/// save one, and a command the agent could no longer copy. Every cut is paid
/// for by the re-read it may cause; one that saves a dozen tokens cannot be.
pub(crate) fn truncate_string(s: &str, cap: usize) -> String {
    let n = s.chars().count();
    if n <= cap + cap / 4 {
        return s.to_string();
    }
    // A multi-line value — a log, a stack trace held in one field — keeps its
    // last lines too: the summary or the raising frame is often there.
    if s.lines().nth(2).is_some() {
        return crate::head_tail::head_and_tail(s, cap);
    }
    let kept: String = s.chars().take(cap).collect();
    format!("{kept}…[+{} chars]", n - cap)
}

/// Recursive walk with a hard depth cap. We collapse anything past `max_depth`
/// into the placeholder `"…(deep)"` so we survive `adv:json:deeply-nested`.
pub(crate) fn walk(v: Value, opts: &CompactOptions, depth: usize) -> Value {
    if depth >= opts.max_depth {
        return Value::String("…(deep)".into());
    }
    match v {
        Value::Null => Value::Null,
        Value::Bool(_) | Value::Number(_) => v,
        Value::String(s) => Value::String(truncate_string(&s, opts.str_cap)),
        Value::Array(items) => {
            let total = items.len();
            // Adaptive cap: instead of always keeping the first `array_cap`
            // items, keep the smallest prefix that covers the array's variety
            // (bounded by array_cap). A redundant array trims harder; a
            // genuinely diverse one keeps up to array_cap. Never worse than the
            // fixed cap (keep ≤ array_cap).
            let keep = if total > opts.array_cap {
                let reprs: Vec<String> = items
                    .iter()
                    .take(opts.array_cap)
                    .map(|x| serde_json::to_string(x).unwrap_or_default())
                    .collect();
                let refs: Vec<&str> = reprs.iter().map(String::as_str).collect();
                crate::adaptive::adaptive_keep(
                    &refs,
                    ARRAY_KEEP_MIN.min(opts.array_cap),
                    opts.array_cap,
                )
            } else {
                total
            };
            let mut out: Vec<Value> = items
                .into_iter()
                .take(keep)
                .map(|x| walk(x, opts, depth + 1))
                .collect();
            if total > keep {
                out.push(Value::String(format!("…{} more items …", total - keep)));
            }
            // Drop empty arrays
            if opts.drop_empty && out.is_empty() {
                Value::Null
            } else {
                Value::Array(out)
            }
        }
        Value::Object(map) => {
            let shape = detect_shape(&map);
            let mut out = serde_json::Map::new();
            for (k, child) in map {
                if key_dropped(&k) || telemetry_key_holding_a_value(&k, &child) {
                    continue;
                }
                if schema_drops(shape, &k) {
                    continue;
                }
                let walked = if shape == Shape::Kubernetes && k == "metadata" {
                    walk_k8s_metadata(child, opts, depth + 1)
                } else {
                    walk(child, opts, depth + 1)
                };
                if opts.drop_nulls && walked.is_null() {
                    continue;
                }
                if opts.drop_empty {
                    if let Value::String(ref s) = walked {
                        if s.is_empty() {
                            continue;
                        }
                    }
                    if let Value::Array(ref a) = walked {
                        if a.is_empty() {
                            continue;
                        }
                    }
                    if let Value::Object(ref o) = walked {
                        if o.is_empty() {
                            continue;
                        }
                    }
                }
                out.insert(k, walked);
            }
            Value::Object(out)
        }
    }
}
