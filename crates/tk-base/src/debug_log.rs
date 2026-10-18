//! Optional, opt-in debug log for production debugging.
//!
//! When the environment variable `TOKENADE_DEBUG_LOG` is set to a file path,
//! every interesting event in tokenade (MCP request/response, hook invocation,
//! sandbox execute, exec_compact run, install action, error path) is
//! appended as one JSON line to that file. When unset, every call here is
//! a single env var read + early return — no overhead worth measuring.
//!
//! Format: one self-contained JSON object per line with at minimum
//! `{ts, ts_iso, pid, component, event}` plus a free-form `details` object.
//!
//! Privacy: this is a tool the user opts INTO. Records can include
//! command lines, file paths, MCP arguments — i.e. potentially sensitive
//! data. We do redact obvious secrets (Bearer/AWS/etc.) via
//! [`crate::redact::redact`] on `details.command` and `details.script`
//! fields before writing. Everything else is written verbatim — by design,
//! since the user enabled this to debug their own workflow.
//!
//! File growth: the writer rotates if the file exceeds
//! [`ROTATION_THRESHOLD_BYTES`] (16 MiB by default — a long day of heavy
//! use). Rotation keeps the **last** 8 MiB worth of records and renames
//! the previous file to `<path>.1` (overwriting any prior `.1`). One
//! generation of backup is enough for "what just happened" debugging;
//! anything older is in `gain.jsonl` for the savings story.

use crate::redact;
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

/// Env var that turns the log on. Set to an absolute file path.
pub const ENV_VAR: &str = "TOKENADE_DEBUG_LOG";
/// Legacy env var name (pre-rename). Still honoured so existing setups
/// don't silently lose their debug stream — see `env_compat`.
pub const LEGACY_ENV_VAR: &str = "STS_DEBUG_LOG";

/// File-size threshold for rotation. 16 MiB ~= one day of heavy MCP
/// traffic at ~10 events/sec at ~200 bytes/event.
pub const ROTATION_THRESHOLD_BYTES: u64 = 16 * 1024 * 1024;

/// Largest write POSIX guarantees to append atomically. Above it a concurrent
/// writer can be spliced into the middle of a record, which is the corruption
/// the single-`write_all` note at the write site describes.
pub const PIPE_BUF: usize = 4096;

/// Cross-thread write lock. A process-local Mutex so multi-tool scenarios
/// (`tokenade mcp` + `tokenade proxy` in parallel) don't interleave. ACROSS
/// processes there is no lock and cannot be one here, so the write site bounds
/// each record to `PIPE_BUF` instead. This comment used to say the records
/// were "<200 bytes" and rest on that: measured 2026-09-03, 15 788 of 23 551
/// rows in a real log are over 200 bytes.
fn write_lock() -> &'static Mutex<()> {
    use std::sync::OnceLock;
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Cheap ISO-8601 UTC formatter — `2026-05-28T18:42:13Z`. Pure-Rust,
/// no chrono dep; precision is whole seconds which is plenty for debug.
fn iso_timestamp(secs: u64) -> String {
    // Days from 1970-01-01 to start of year y (Julian-style minus offset).
    // Algorithm is standard "civil_from_days" — see Howard Hinnant's
    // "date" library; reimplemented here in a few lines to avoid a dep.
    let z = secs / 86_400;
    let day_of_day = (secs % 86_400) as u32;
    let z = z as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    let h = day_of_day / 3600;
    let mi = (day_of_day % 3600) / 60;
    let s = day_of_day % 60;
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// True if `TOKENADE_DEBUG_LOG` (or legacy `STS_DEBUG_LOG`) is set to a
/// non-empty path.
pub fn enabled() -> bool {
    crate::env_compat::var(ENV_VAR, LEGACY_ENV_VAR).is_ok_and(|v| !v.is_empty())
}

/// Append one event. Cheap no-op when `TOKENADE_DEBUG_LOG` is unset.
///
/// `component` is the subsystem emitting the event (`mcp`, `hookwrap`,
/// `hookwrap-pretool`, `sandbox`, `install`, `cli`, …). `event` is the
/// event name within that subsystem (`request`, `response`, `error`,
/// `decision_deny`, …). `details` is a free-form object — keys we
/// suggest:
///
/// * `request_id` — JSON-RPC id where applicable
/// * `method` — JSON-RPC method
/// * `tool` — MCP tool name
/// * `lang` — sandbox lang
/// * `command` — bash command (auto-redacted)
/// * `script` — script content (auto-redacted)
/// * `path` — file path
/// * `outcome` — `ok` / `err` / `timeout` / `denied` / …
/// * `elapsed_ms` — duration the operation took
/// * `tokens_before` / `tokens_after` — savings, when applicable
/// * `error` — error message (auto-redacted)
pub fn log(component: &str, event: &str, mut details: Value) {
    let Ok(raw_path) = crate::env_compat::var(ENV_VAR, LEGACY_ENV_VAR) else {
        return;
    };
    if raw_path.is_empty() {
        return;
    }
    // Redact EVERY string in the details object, at every depth.
    //
    // This used to name five fields and redact only those, only at the top
    // level. An allowlist is the wrong shape here: `details` is documented as
    // free-form, there are 216 call sites emitting 241 distinct field names,
    // and a call site added tomorrow is unredacted by default. Measured
    // 2026-09-03, all three ways it failed at once: `gain.rs` logs a clamped
    // row's `label`, a full command line that is not on the list; a `reason`
    // is not either; and a `command` nested one level under another key was
    // skipped because the redactor only ever looked at `details[key]`.
    //
    // This log is opt-in (`TOKENADE_DEBUG_LOG`) but NOT private: `tokenade
    // report` collects it into a support bundle by name, so what lands here is
    // what a user hands to someone else.
    //
    // Numbers, booleans and keys are untouched; a secret is a string value.
    // Over-redacting a debug field costs legibility, under-redacting puts a
    // credential in a file built to be shared, so the default runs toward
    // redaction.
    redact_value_in_place(&mut details, false);

    let now = crate::platform::now_unix_secs();
    let record = json!({
        "ts": now,
        "ts_iso": iso_timestamp(now),
        "pid": std::process::id(),
        "component": component,
        "event": event,
        "details": details,
    });

    let path = PathBuf::from(&raw_path);
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }

    let _g = write_lock().lock().unwrap_or_else(|p| p.into_inner());
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        // ONE write_all of the full prebuilt line. `writeln!(f, "{record}")`
        // streamed serde's Display as dozens of tiny write() calls; the mutex
        // above only serializes THIS process, and hooks run as concurrent
        // PROCESSES sharing the file — their fragments interleaved mid-record
        // (measured: 1221 corrupted lines / 31k on a dogfood box). A single
        // O_APPEND write keeps each record contiguous.
        let mut line = record.to_string();
        line.push('\n');
        // A single O_APPEND write is atomic only BELOW `PIPE_BUF`; above it the
        // kernel is free to split, and a split is exactly the interleaving the
        // paragraph above describes. `details` is free-form, so nothing at the
        // call sites bounds this. Measured on a real 7.7 MB log 2026-09-03:
        // 23 551 rows, max 727 bytes, none over the limit — so this trims
        // nothing that happens today, and it is what makes the sentence above
        // true rather than lucky. The identifying head (ts, pid, component,
        // event) is kept whole; only `details` is dropped, and the row says so.
        if line.len() > PIPE_BUF {
            let mut trimmed = record;
            trimmed["details"] = Value::String(format!(
                "(dropped: {} bytes, over the {PIPE_BUF}-byte atomic-append limit)",
                line.len()
            ));
            line = trimmed.to_string();
            line.push('\n');
        }
        let _ = f.write_all(line.as_bytes());
    }
    if let Ok(meta) = fs::metadata(&path) {
        if meta.len() >= ROTATION_THRESHOLD_BYTES {
            let _ = rotate(&path);
        }
    }
}

/// Redact every string in `v`, recursively.
///
/// `in_command` is true inside the subtree of a key named `command`: those get
/// the argv-aware pass first, which catches paired secret flags the regex
/// layer cannot see, because it does not know which argument is the value of
/// which flag.
fn redact_value_in_place(v: &mut Value, in_command: bool) {
    match v {
        Value::String(s) => {
            let r = if in_command {
                let argv: Vec<String> = s.split_whitespace().map(String::from).collect();
                redact::redact(&redact::redact_argv(&argv).join(" "))
            } else {
                redact::redact(s)
            };
            if &r != s {
                *s = r;
            }
        }
        Value::Object(map) => {
            for (k, child) in map.iter_mut() {
                redact_value_in_place(child, k == "command" || k == "cmd");
            }
        }
        Value::Array(items) => {
            for child in items.iter_mut() {
                redact_value_in_place(child, in_command);
            }
        }
        _ => {}
    }
}

/// Move `path` to `path.1` (overwriting the previous `.1` if present) and
/// start fresh. One generation is enough for the "what just happened"
/// debugging this log is for; older history goes to `gain.jsonl`.
fn rotate(path: &std::path::Path) -> std::io::Result<()> {
    let one = {
        let mut p = path.as_os_str().to_owned();
        p.push(".1");
        PathBuf::from(p)
    };
    // Best-effort: ignore the prior `.1` being absent.
    let _ = fs::remove_file(&one);
    fs::rename(path, &one)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE lock, from `platform` — not a private one. `platform::ENV_LOCK`'s
    /// own doc names this module as one of the strays that made the release
    /// gate need `--test-threads=1`, and the stray was real: `crash_guard`'s
    /// test mutates this very variable (`TOKENADE_DEBUG_LOG`) and took no
    /// lock at all, so the two raced whatever this module did.
    use crate::platform::ENV_LOCK;

    /// Helper: write to a tmp file via the env var, then read back.
    /// Holds [`ENV_LOCK`] for the full lifetime of the env mutation +
    /// any `log()` calls so no parallel test sees a half-set state.
    fn run_with_log<F: FnOnce()>(f: F) -> (PathBuf, String) {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        // SAFETY: test sets process-wide env; restore after. The mutex
        // above prevents the parallel-test race.
        let prev = std::env::var(ENV_VAR).ok();
        // Use a nanosecond suffix so concurrent test BINARIES (unusual
        // but possible) can't collide on the file path either.
        let path = std::env::temp_dir().join(format!(
            "tokenade-debug-{}-{}-{}.log",
            std::process::id(),
            crate::platform::now_unix_secs(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0),
        ));
        let _ = fs::remove_file(&path);
        std::env::set_var(ENV_VAR, &path);
        f();
        let content = fs::read_to_string(&path).unwrap_or_default();
        std::env::remove_var(ENV_VAR);
        if let Some(v) = prev {
            std::env::set_var(ENV_VAR, v);
        }
        (path, content)
    }

    #[test]
    fn disabled_is_a_noop() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        // SAFETY: env scoped to this test; restore at the end.
        let prev = std::env::var(ENV_VAR).ok();
        std::env::remove_var(ENV_VAR);
        log("test", "noop", json!({}));
        // Restore.
        if let Some(v) = prev {
            std::env::set_var(ENV_VAR, v);
        }
        // No assertion — we just need to confirm it doesn't panic
        // and the env var stays unset (no path → no write).
    }

    #[test]
    fn enabled_writes_one_jsonl_line() {
        let (path, content) = run_with_log(|| log("test", "demo", json!({"k": "v", "n": 42})));
        assert!(!content.is_empty(), "expected a line");
        // Filter by our specific component/event — see comment in
        // `known_fields_get_redacted` for why we can't trust single-line.
        let v: Value = content
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
            .find(|v| {
                v["component"].as_str() == Some("test") && v["event"].as_str() == Some("demo")
            })
            .expect("test/demo record not found in log");
        assert_eq!(v["component"], "test");
        assert_eq!(v["event"], "demo");
        assert_eq!(v["details"]["k"], "v");
        assert_eq!(v["details"]["n"], 42);
        assert!(v["ts_iso"].as_str().unwrap().ends_with('Z'));
        assert!(v["pid"].as_u64().is_some());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn known_fields_get_redacted() {
        let (path, content) = run_with_log(|| {
            log(
                "test",
                "redact",
                json!({
                    "command": concat!("curl -H 'Authorization: Bearer sk_", "live_AAAAAAAAAAAAAAAAAAAAAAAA'"),
                    "harmless": concat!("Bearer sk_", "live_BBBBBBBBBBBBBBBBBBBBBBBB")
                }),
            )
        });
        // The shared debug.log file can pick up lines from other tests
        // running in parallel (filter compactors emit safety_net_*
        // events that hit the same env-var-bound path). Pick out OUR
        // record by component+event rather than assuming single-line.
        let v: Value = content
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
            .find(|v| {
                v["component"].as_str() == Some("test") && v["event"].as_str() == Some("redact")
            })
            .expect("test/redact record not found in log");
        let cmd = v["details"]["command"].as_str().unwrap();
        assert!(
            !cmd.contains(concat!("sk_", "live_AAAAAAAAAAAAAAAAAAAAAAAA")),
            "command not redacted: {cmd}"
        );
        // A field NOT on the old five-name allowlist is redacted too. That
        // list said "users own this file", but `tokenade report` collects
        // debug.log into a support bundle by name, so what lands here is what
        // a user hands to someone else. Asserted by comparing against the
        // `command` field, which carried the same secret and is redacted:
        // whatever marker the redactor uses, both must end in it.
        let harmless = v["details"]["harmless"].as_str().unwrap();
        let marker = cmd.rsplit(' ').next().unwrap().trim_matches('\'');
        assert!(
            harmless.ends_with(marker),
            "a field outside the old allowlist was left in the clear"
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn url_field_gets_redacted() {
        // Regression for #476: webfetch handlers log raw URL; query
        // strings can carry `?api_key=…`, and userinfo can carry
        // `user:pwd@host` — both must be scrubbed before reaching disk.
        let (path, content) = run_with_log(|| {
            log(
                "test",
                "url_redact",
                json!({
                    "url": concat!("https://api.example.com/v1?api_key=sk_", "live_CCCCCCCCCCCCCCCCCCCCCCCC")
                }),
            )
        });
        let v: Value = content
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
            .find(|v| {
                v["component"].as_str() == Some("test") && v["event"].as_str() == Some("url_redact")
            })
            .expect("test/url_redact record not found in log");
        let url = v["details"]["url"].as_str().unwrap();
        assert!(
            !url.contains("sk_live_CCCCCCCC"),
            "url leaked secret to debug.log: {url}"
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn url_userinfo_form_gets_redacted_via_url_field() {
        // basic-auth URL form `https://user:pwd@host`.
        let (path, content) = run_with_log(|| {
            log(
                "test",
                "url_userinfo",
                json!({
                    "url": "https://bot:ghp_DDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDD@github.com/foo/bar"
                }),
            )
        });
        let v: Value = content
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
            .find(|v| {
                v["component"].as_str() == Some("test")
                    && v["event"].as_str() == Some("url_userinfo")
            })
            .expect("test/url_userinfo record not found in log");
        let url = v["details"]["url"].as_str().unwrap();
        assert!(
            !url.contains("ghp_DDDDD"),
            "userinfo password leaked: {url}"
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn iso_timestamp_round_numbers() {
        // 1970-01-01T00:00:00Z
        assert_eq!(iso_timestamp(0), "1970-01-01T00:00:00Z");
        // 2026-05-28T00:00:00Z = 1748390400 (approx). Check the shape only.
        let s = iso_timestamp(crate::platform::now_unix_secs());
        assert_eq!(s.len(), "1970-01-01T00:00:00Z".len());
        assert!(s.starts_with("20"));
    }
}
