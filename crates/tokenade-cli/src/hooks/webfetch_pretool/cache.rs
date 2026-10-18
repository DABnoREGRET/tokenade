//! Per-session URL to stash index, so a re-fetch of the same page inside
//! one session is answered without a second network round trip.

use super::*;

/// Directory for the tiny per-session webfetch URL→stash index. Env
/// override `TOKENADE_WEBFETCH_CACHE_DIR` for test isolation.
pub(crate) fn webfetch_cache_dir() -> std::path::PathBuf {
    if let Some(p) = tokenade_core::env_compat::path_from_env("TOKENADE_WEBFETCH_CACHE_DIR") {
        return p;
    }
    tokenade_core::platform::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".tokenade")
        .join("sessions")
}

pub(crate) fn webfetch_cache_path(session_id: &str) -> std::path::PathBuf {
    // session_id is a UUID from Claude Code but sanitize anyway.
    let safe: String = session_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    webfetch_cache_dir().join(format!("{safe}-webfetch.json"))
}

/// How long a cached fetch stays authoritative.
///
/// The entry's `ts` was written from the start but never READ, so an entry
/// lived for the whole session — and that made the cache a trap: when a site
/// reader fails transiently (rate limit, timeout) the generic pipeline
/// distils the JS shell and caches THAT under the URL. Since the lookup runs
/// before `site_readers::plan`, the reader stayed disabled for that URL until
/// the session ended. An hour is long enough to spare a re-fetch inside a
/// working burst, short enough that a long session recovers on its own.
const WEBFETCH_CACHE_TTL_SECS: u64 = 3_600;

/// Look up a same-session prior fetch of `url`. Returns the stashed distilled
/// markdown when the index has an entry younger than
/// [`WEBFETCH_CACHE_TTL_SECS`] whose stash file still exists.
pub(crate) fn webfetch_cache_lookup(session_id: &str, url: &str) -> Option<String> {
    let idx = std::fs::read_to_string(webfetch_cache_path(session_id)).ok()?;
    let idx: serde_json::Value = serde_json::from_str(&idx).ok()?;
    let entry = idx.get(url)?;
    let ts = entry.get("ts").and_then(|t| t.as_u64())?;
    let now = tokenade_core::platform::now_unix_secs();
    // Saturating: a clock that moved backwards must expire the entry, never
    // grant it an unbounded lease.
    if now.saturating_sub(ts) >= WEBFETCH_CACHE_TTL_SECS {
        return None;
    }
    let path = entry.get("path").and_then(|p| p.as_str())?;
    std::fs::read_to_string(path).ok()
}

/// Record `url` → stash `path` in the session index. Best-effort: any
/// I/O error silently skips the cache (next fetch just re-curls).
pub(crate) fn webfetch_cache_record(session_id: &str, url: &str, path: &std::path::Path) {
    if session_id.is_empty() {
        return;
    }
    let idx_path = webfetch_cache_path(session_id);
    if let Some(parent) = idx_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut idx: serde_json::Value = std::fs::read_to_string(&idx_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let now = tokenade_core::platform::now_unix_secs();
    if let Some(map) = idx.as_object_mut() {
        // The index is otherwise insert-only; drop entries past the TTL (they can
        // never be served — `webfetch_cache_lookup` rejects them) so a long
        // session's file cannot grow without bound.
        map.retain(|_, v| {
            v.get("ts")
                .and_then(|t| t.as_u64())
                .is_some_and(|ts| now.saturating_sub(ts) < WEBFETCH_CACHE_TTL_SECS)
        });
        map.insert(
            url.to_string(),
            serde_json::json!({
                "path": path.display().to_string(),
                "ts": now,
            }),
        );
        // Atomic write (temp beside it → rename over), NOT a bare `fs::write`.
        // Claude Code runs tool calls in parallel; a crash or an interleaved
        // write mid-`fs::write` leaves a truncated JSON, which fails to parse on
        // the next read and SILENTLY RESETS the whole session cache to `{}`. A
        // rename is atomic on one filesystem, so a reader sees either the old or
        // the new file whole — never a half-written one.
        let data = serde_json::to_string(&idx).unwrap_or_default();
        let tmp = idx_path.with_extension(format!("json.{}.tmp", std::process::id()));
        if std::fs::write(&tmp, data.as_bytes()).is_ok()
            && std::fs::rename(&tmp, &idx_path).is_err()
        {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}
