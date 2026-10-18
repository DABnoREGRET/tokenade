//! Cache-health step of the UserPromptSubmit hook.
//!
//! Incrementally scans the session transcript for cache-thrash events
//! (full-context rewrites inside the live cache window — see
//! `tokenade_core::cache_thrash` for the detector) and books one
//! accounting-neutral `thrash:<trigger>` row per event in the LOCAL estimated
//! ledger (read by the local web dashboard). Nothing leaves the machine and
//! no behavior changes.
//!
//! Scan cursor lives in `~/.tokenade/sessions/<skey>-thrash.json` (same
//! lifecycle as the other per-session sentinels: reclaimed by the sessions/
//! TTL sweep; a lost cursor only resets the tail-capped scan).

use tokenade_core::cache_thrash::{scan_new, ScanState};

pub(crate) fn record_thrash_events(parsed: &serde_json::Value) {
    let Some(tp) = parsed.get("transcript_path").and_then(|x| x.as_str()) else {
        return;
    };
    let skey: String = crate::hooks::session_id_of(parsed)
        .unwrap_or("")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(40)
        .collect();
    if skey.is_empty() {
        return;
    }
    let Some(home) = tokenade_core::platform::home_dir() else {
        return;
    };
    let state_path = home
        .join(".tokenade")
        .join("sessions")
        .join(format!("{skey}-thrash.json"));
    let state: ScanState = std::fs::read_to_string(&state_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let (events, new_state) = scan_new(std::path::Path::new(tp), &state);
    for e in &events {
        tokenade_core::gain::record_thrash(
            e.trigger,
            e.rewritten as usize,
            e.prev_ctx as usize,
            e.gap_secs,
        );
    }
    if let Some(dir) = state_path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(js) = serde_json::to_string(&new_state) {
        let _ = std::fs::write(&state_path, js);
    }
    if !events.is_empty() {
        // Explanatory encart: a debug-log reader (often an AI agent triaging a
        // session) sees `hookwrap-userprompt` next to a large `rewritten` count
        // and can wrongly conclude tokenade rewrote the context and cost those
        // tokens. It did NOT. These tokens were re-created by Claude Code
        // invalidating its own prompt cache on a request-shape change (thinking
        // toggle / tool-skill list mutation / model fallback); tokenade only
        // DETECTS and measures the event. It is accounting-neutral (saved:0)
        // and not a cost tokenade added. `caused_by`/`tokenade_cost`/`note`
        // travel with the row so the log is self-explaining, not just this src.
        tokenade_core::debug_log::log(
            "hookwrap-userprompt",
            "cache_thrash_detected",
            serde_json::json!({
                "events": events.len(),
                "rewritten": events.iter().map(|e| e.rewritten).sum::<u64>(),
                "triggers": events.iter().map(|e| e.trigger).collect::<Vec<_>>(),
                "caused_by": "claude-code invalidating its own prompt cache on a request-shape change (thinking toggle / tool-skill list mutation / model fallback)",
                "tokenade_cost": 0,
                "note": "tokenade only DETECTS this; the hook does NOT rewrite context. Accounting-neutral (saved:0) — these tokens are Claude Code's cache re-creation, not a tokenade cost.",
            }),
        );
    }
}
