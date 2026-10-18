//! **Count what was redacted, and make it reviewable.**
//!
//! A silent redaction is indistinguishable from a redaction that never
//! happened. The ledger says which rule fired, when, and how many times — which
//! is how one finds out that a rule fires ten thousand times a day on
//! build identifiers.

use super::*;

/// True when a KEY names a credential, whatever its value looks like.
///
/// Structured output is where this matters. `{"aws_secret_access_key":"wJal…"}`
/// carries a 40-character blob that no pattern can safely match on its own —
/// matching it would redact every base64 payload in every response. What makes
/// it a secret is the key next to it, and in JSON that key is a sibling field
/// rather than adjacent text, so the flat-text rules never see the pair.
///
/// Callers that walk a parsed document (MCP responses, config trees) should ask
/// this about each key and mask the value outright; callers that see only text
/// keep using [`redact`].
pub fn key_names_a_secret(key: &str) -> bool {
    let k = key.to_ascii_lowercase().replace(['-', '.', ' '], "_");
    // A key that merely MENTIONS a credential is not one: `password_policy`,
    // `token_count`, `secret_name` and `has_api_key` are metadata, and masking
    // them costs the agent a fact while protecting nothing.
    const NOT_SECRETS: &[&str] = &[
        "_policy",
        "_count",
        "_name",
        "_id",
        "_type",
        "_length",
        "_expiry",
        "_expires",
        "_created",
        "_updated",
        "_rotation",
        "_url",
        "_uri",
        "_path",
        "_file",
        "_enabled",
        "_required",
        "_hint",
        "_prefix",
        "_suffix",
        "_algorithm",
        "_version",
    ];
    if NOT_SECRETS.iter().any(|s| k.ends_with(s)) {
        return false;
    }
    const NAMES: &[&str] = &[
        "password",
        "passwd",
        "pwd",
        "secret",
        "client_secret",
        "api_key",
        "apikey",
        "access_token",
        "auth_token",
        "refresh_token",
        "id_token",
        "session_token",
        "bearer_token",
        "private_key",
        "privatekey",
        "secret_key",
        "secretkey",
        "aws_secret_access_key",
        "aws_session_token",
        "authorization",
        "credentials",
        "credential",
        "passphrase",
        "signing_key",
        "encryption_key",
        "webhook_secret",
    ];
    NAMES.contains(&k.as_str()) || NAMES.iter().any(|n| k.ends_with(&format!("_{n}")))
}

/// Model-bound variant: redact, and meter every masked value into the
/// local redaction ledger (best-effort, never blocks or fails the fold).
pub fn redact_for_model(s: &str) -> String {
    let (out, n, by_rule) = redact_counting_tallied(s);
    if n > 0 {
        record_redactions(n as u64, &by_rule);
    }
    out
}

/// Honours `TOKENADE_REDACT_LEDGER`, for the reason
/// `platform::every_store_path_honours_an_env_override` states: a path with no
/// escape hatch cannot be isolated, so its tests write the developer's REAL
/// file. Measured 2026-08-04 — running the core test binary under a scratch
/// HOME showed `web_pipeline` appending to the real `~/.tokenade/redact.jsonl`.
/// That is the same ledger where torn records were found this morning and
/// blamed on concurrent hooks; the test suite was another writer on it.
///
/// This was the LAST store without an override. `semantic_db` was the previous
/// one, and it had accumulated 81 test rows in the real database before it got
/// one.
pub(super) fn redact_ledger_path() -> std::path::PathBuf {
    if let Some(p) = std::env::var_os("TOKENADE_REDACT_LEDGER") {
        if !p.is_empty() {
            return std::path::PathBuf::from(p);
        }
    }
    crate::platform::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".tokenade")
        .join("redact.jsonl")
}

/// Append `{ts, n}` to `~/.tokenade/redact.jsonl`. Compacts the ledger to
/// one row per UTC day when it grows past ~256 KB (secrets are rare; the
/// compaction is a safety valve, not a hot path).
pub fn record_redactions(n: u64, by_rule: &[(&'static str, usize)]) {
    use std::io::Write;
    let path = redact_ledger_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let ts = crate::platform::now_unix_secs();
    // Both the append and the compaction go through ONE exclusive lock on the
    // ledger's own inode.
    //
    // What was wrong before, measured on a dogfood box at 17 corrupt rows of
    // 984: the append was `writeln!(f, "{{\"ts\":{ts},\"n\":{n}}}")`, and a
    // format string is streamed as one `write()` per fragment. Hooks run as
    // concurrent PROCESSES, so their fragments interleaved mid-record —
    // `{{\"ts\":{{\"ts\":17857689501785768950,\"n\":,\"n\":11}}`. `read_redact_rows`
    // drops what it cannot parse, so the dashboard silently under-counted
    // secrets caught and nothing anywhere said so. `debug_log` had already been
    // given the single-write treatment for exactly this; this call site was
    // missed.
    //
    // The compaction needed the lock as well, and for a second reason: it was a
    // read-then-`fs::write`, which TRUNCATES. Two processes over the threshold
    // both rewrote, and any appender in between lost its row outright. It now
    // rewrites through the locked handle (`set_len` + write) so no other opener
    // can be inside the file while it happens.
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    // Same rule as the savings ledger: a redaction counter is telemetry, and
    // it must never make the fold that produced it wait.
    if !crate::platform::lock_briefly(&f) {
        return;
    }
    if f.metadata().map(|m| m.len() > 256 * 1024).unwrap_or(false) {
        let mut per_day: std::collections::BTreeMap<u64, (u64, RuleCounts)> = Default::default();
        for (day_ts, count, by) in read_redact_rows(&path) {
            let e = per_day.entry(day_ts - day_ts % 86_400).or_default();
            e.0 += count;
            // Merge the breakdown too. Flattening it here would mean the
            // ledger could only answer "which rule" for the last few days,
            // which is exactly when nobody is asking.
            for (rule, c) in by {
                *e.1.entry(rule).or_default() += c;
            }
        }
        let mut body = String::new();
        for (d, (c, by)) in per_day {
            body.push_str(&row_json(d, c, &by));
        }
        // O_APPEND makes every write land at the end, so truncating alone would
        // leave the old length; rewrite through a second handle held under the
        // same lock.
        if let Ok(mut w) = std::fs::OpenOptions::new().write(true).open(&path) {
            if w.set_len(0).is_ok() {
                let _ = w.write_all(body.as_bytes());
            }
        }
    }
    // ONE `write_all`, like the row above it: see the note at the top of this
    // function for what a format-string append did to this file.
    let _ = f.write_all(row_json(ts, n, &fold_rules(by_rule)).as_bytes());
}

/// rule name → how many values it masked.
pub(super) type RuleCounts = std::collections::BTreeMap<String, u64>;

/// One ledger row, serialised in one piece. `by` is omitted when empty so a
/// day with no breakdown reads exactly as it always did.
pub(super) fn row_json(ts: u64, n: u64, by: &RuleCounts) -> String {
    if by.is_empty() {
        return format!("{{\"ts\":{ts},\"n\":{n}}}\n");
    }
    let pairs: Vec<String> = by
        .iter()
        .map(|(k, v)| format!("\"{}\":{v}", k.escape_debug()))
        .collect();
    format!("{{\"ts\":{ts},\"n\":{n},\"by\":{{{}}}}}\n", pairs.join(","))
}

pub(super) fn fold_rules(by_rule: &[(&'static str, usize)]) -> RuleCounts {
    let mut m = RuleCounts::new();
    for (name, c) in by_rule {
        *m.entry((*name).to_string()).or_default() += *c as u64;
    }
    m
}

pub(super) fn read_redact_rows(path: &std::path::Path) -> Vec<(u64, u64, RuleCounts)> {
    let Ok(body) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    body.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l.trim()).ok())
        .map(|v| {
            (
                v.get("ts")
                    .and_then(|x| x.as_u64())
                    .filter(|ts| plausible_ledger_ts(*ts))
                    .unwrap_or(0),
                v.get("n").and_then(|x| x.as_u64()).unwrap_or(0),
                v.get("by")
                    .and_then(|x| x.as_object())
                    .map(|o| {
                        o.iter()
                            .filter_map(|(k, v)| v.as_u64().map(|n| (k.clone(), n)))
                            .collect()
                    })
                    .unwrap_or_default(),
            )
        })
        .collect()
}

/// Is this a timestamp a running tokenade could have written?
///
/// Concurrent hook processes used to interleave their `write()` fragments on
/// this ledger (fixed by [`crate::platform::append_line`]). Most torn rows are
/// unparseable and already dropped — but a tear can splice one record's digits
/// into another's and stay VALID JSON. A dogfood box carries two:
/// `{"ts":11784355200,"n":0}` — a digit prepended to a real ts, landing in the
/// year 2343.
///
/// That row is worse than a broken one. `redactions_since` keeps everything
/// with `ts >= cutoff`, so a far-future row is counted in EVERY window for as
/// long as the file lives, and `redactions_by_day` opens a `2343-06-08` bucket
/// in the dashboard. A reader that cannot tell a real ts from spliced digits
/// gives an answer that is silently wrong instead of visibly missing.
///
/// Out-of-range is treated as UNDATED (`0`), not as a dropped row: `n` is still
/// a real count of secrets masked, only its date is unusable — which is exactly
/// what the `""` bucket in `redactions_by_day` already means.
pub(super) fn plausible_ledger_ts(ts: u64) -> bool {
    // 2020-01-01. tokenade did not exist before this; anything earlier is a
    // truncated or spliced value, not history.
    const FLOOR: u64 = 1_577_836_800;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(u64::MAX);
    // A day of headroom absorbs clock skew between the writer and this reader
    // without admitting a spliced digit, which overshoots by centuries.
    ts >= FLOOR && ts <= now.saturating_add(86_400)
}

/// Total secrets redacted since `cutoff_secs` (unix; 0 = lifetime).
pub fn redactions_since(cutoff_secs: u64) -> u64 {
    read_redact_rows(&redact_ledger_path())
        .into_iter()
        .filter(|(ts, _, _)| *ts >= cutoff_secs)
        .map(|(_, n, _)| n)
        .sum()
}

/// Rule name → how many values it masked, since `cutoff_secs` (0 = lifetime).
///
/// The dashboard used to be able to say only HOW MANY secrets were caught. A
/// rule that starts firing wrongly is indistinguishable from a busy week at
/// that resolution — the entropy sweep spent weeks masking Rust test names and
/// the ledger's answer was a number. Rows written before the breakdown existed
/// carry none, so this under-reports history rather than inventing it.
pub fn redactions_by_rule(cutoff_secs: u64) -> std::collections::BTreeMap<String, u64> {
    let mut out: std::collections::BTreeMap<String, u64> = Default::default();
    for (ts, _, by) in read_redact_rows(&redact_ledger_path()) {
        if ts < cutoff_secs {
            continue;
        }
        for (rule, n) in by {
            *out.entry(rule).or_default() += n;
        }
    }
    out
}

/// Per-UTC-day totals (`YYYY-MM-DD` → count) for the HTML dashboard's
/// period slicing. Rows with an unusable ts land under `""` (undated).
pub fn redactions_by_day() -> std::collections::BTreeMap<String, u64> {
    let mut out: std::collections::BTreeMap<String, u64> = Default::default();
    for (ts, n, _) in read_redact_rows(&redact_ledger_path()) {
        let key = if ts == 0 {
            String::new()
        } else {
            crate::platform::utc_date_from_ts(ts)
        };
        *out.entry(key).or_default() += n;
    }
    out
}
