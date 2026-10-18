//! On-disk audit trace for a spawned light-agent.
//!
//! `tokenade agent` is non-blocking: the launcher creates a trace directory,
//! spawns a detached worker, and immediately prints the trace-dir path + the
//! worker PID (for auditability / `kill`). The worker streams a transcript of
//! every turn (model reply, tool call, tool result, token usage) into that
//! directory and writes the final result there, so the whole run is auditable
//! after the fact.
//!
//! Layout of `<root>/agents/agent-<pid>-<ts>/`:
//!   meta.json         task + sandbox + model + status + pid (updated live)
//!   transcript.jsonl  one JSON event per line, appended as the run proceeds
//!   result.json       final answer + token accounting (written on finish)

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// `<tokenade-root>/agents` — same root as the gain ledger (honours the test
/// override + `TOKENADE_HOME`).
fn agents_root() -> PathBuf {
    tokenade_core::gain::current_ledger_path()
        .parent()
        .map(|d| d.join("agents"))
        .unwrap_or_else(|| PathBuf::from(".tokenade/agents"))
}

/// Create a fresh trace directory and return its path.
pub fn new_dir() -> std::io::Result<PathBuf> {
    let dir = agents_root().join(format!("agent-{}-{}", std::process::id(), now_millis()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn meta_path(dir: &Path) -> PathBuf {
    dir.join("meta.json")
}
pub fn transcript_path(dir: &Path) -> PathBuf {
    dir.join("transcript.jsonl")
}
pub fn result_path(dir: &Path) -> PathBuf {
    dir.join("result.json")
}

pub fn write_meta(dir: &Path, meta: &serde_json::Value) {
    let _ = std::fs::write(meta_path(dir), meta.to_string());
}

pub fn read_meta(dir: &Path) -> Option<serde_json::Value> {
    let s = std::fs::read_to_string(meta_path(dir)).ok()?;
    serde_json::from_str(&s).ok()
}

/// Merge `patch`'s top-level keys into meta.json (e.g. status, pid), always
/// stamping `updated`.
pub fn patch_meta(dir: &Path, patch: serde_json::Value) {
    let mut m = read_meta(dir).unwrap_or_else(|| serde_json::json!({}));
    if let (Some(dst), Some(src)) = (m.as_object_mut(), patch.as_object()) {
        for (k, v) in src {
            dst.insert(k.clone(), v.clone());
        }
        dst.insert(
            "updated".into(),
            serde_json::json!(tokenade_core::platform::now_unix_secs()),
        );
    }
    write_meta(dir, &m);
}

/// Append one event to the transcript. String values are run through the
/// secret redactor first — a model reply or tool result can echo a token/key,
/// and this file persists on disk, so it must not store secrets in cleartext.
pub fn append_event(dir: &Path, event: serde_json::Value) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(transcript_path(dir))
    {
        let mut ev = redact_value(event);
        if let Some(obj) = ev.as_object_mut() {
            obj.insert(
                "ts".into(),
                serde_json::json!(tokenade_core::platform::now_unix_secs()),
            );
        }
        let mut line = ev.to_string();
        line.push('\n');
        let _ = f.write_all(line.as_bytes());
    }
}

/// Recursively redact every string in a JSON value.
fn redact_value(v: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match v {
        Value::String(s) => Value::String(tokenade_core::redact::redact(&s)),
        Value::Array(a) => Value::Array(a.into_iter().map(redact_value).collect()),
        Value::Object(o) => Value::Object(
            o.into_iter()
                .map(|(k, val)| (k, redact_value(val)))
                .collect(),
        ),
        other => other,
    }
}

pub fn write_result(dir: &Path, result: &serde_json::Value) {
    let _ = std::fs::write(result_path(dir), redact_value(result.clone()).to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gc_prunes_old_but_keeps_recent_and_unknown() {
        let root = std::env::temp_dir().join(format!(
            "tok-gc-test-{}-{}",
            std::process::id(),
            now_millis()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let now = now_millis();
        let old = std::fs::create_dir_all(root.join("agent-1-0")); // ts=0 → ancient
        let recent = std::fs::create_dir_all(root.join(format!("agent-2-{now}")));
        let unknown = std::fs::create_dir_all(root.join("not-an-agent"));
        old.and(recent).and(unknown).unwrap();
        // also a stray file that must survive
        std::fs::write(root.join("keep.txt"), b"x").unwrap();

        gc_in(&root, 60, 0); // max_age 60s, no throttle

        assert!(
            !root.join("agent-1-0").exists(),
            "ancient trace must be pruned"
        );
        assert!(
            root.join(format!("agent-2-{now}")).exists(),
            "recent trace kept"
        );
        assert!(
            root.join("not-an-agent").exists(),
            "non-agent dir untouched"
        );
        assert!(root.join("keep.txt").exists(), "stray file untouched");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn gc_throttles_and_no_dir_is_noop() {
        // Missing root: must not panic or create it.
        let missing = std::env::temp_dir().join(format!(
            "tok-gc-absent-{}-{}",
            std::process::id(),
            now_millis()
        ));
        gc_in(&missing, 60, 3600);
        assert!(!missing.exists(), "gc must not create the agents dir");
    }
}

const GC_MAX_AGE_SECS: u64 = 7 * 24 * 3600; // prune traces older than 7 days
const GC_THROTTLE_SECS: u64 = 12 * 3600; // scan at most twice a day

/// Prune agent trace directories older than 7 days. Called once at tokenade
/// startup; throttled to at most one filesystem scan per 12h via a stamp file.
///
/// This is SAFE for token accounting: gains live in `gain_estimated.jsonl`
/// (written by [`crate::agent`] when a run finishes), a separate file GC never
/// touches. Trace dirs are audit artifacts only — deleting stale ones loses no
/// accounted savings.
pub fn gc_old_traces() {
    gc_in(&agents_root(), GC_MAX_AGE_SECS, GC_THROTTLE_SECS);
}

fn gc_in(root: &Path, max_age_secs: u64, throttle_secs: u64) {
    let stamp = root.join(".last_gc");
    // Throttle: skip if we scanned recently.
    if throttle_secs > 0 {
        if let Some(age) = std::fs::metadata(&stamp)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| SystemTime::now().duration_since(m).ok())
        {
            if age.as_secs() < throttle_secs {
                return;
            }
        }
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return; // no agents dir yet — nothing to prune, don't create it
    };
    let now_ms = now_millis();
    for e in entries.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        // Only touch `agent-<pid>-<ts_millis>` dirs; age from the trailing
        // creation timestamp in the name (robust to FS mtime quirks). Unknown
        // names are left alone.
        let Some(ts_ms) = name
            .strip_prefix("agent-")
            .and_then(|r| r.rsplit('-').next())
            .and_then(|ts| ts.parse::<u128>().ok())
        else {
            continue;
        };
        let age_secs = (now_ms.saturating_sub(ts_ms) / 1000) as u64;
        if age_secs >= max_age_secs {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
    let _ = std::fs::write(&stamp, b"");
}
