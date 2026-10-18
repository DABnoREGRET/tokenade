//! **Who the current session is, and where its crumbs live.**
//!
//! Two primitives, and nothing else: the session key — as the host
//! declared it to the hook — and the directory of small state files it
//! leaves behind (webfetch cache, websearch seen-set, overhead
//! counters).
//!
//! They lived in `gain`, the LEDGER module, because the ledger was
//! the first to ask for them. The measurable result: `disclose`, the
//! fold gate, named `gain` to know which session to attribute a
//! stash fingerprint to. A fold gate has no business knowing the
//! accounting; knowing which session we are in is not
//! accounting. It is the third time in this refactoring that a primitive
//! stored in a product layer holds a cycle on its own — after
//! `archetypes` and the table fold.
//!
//! `gain` keeps what is really its own: eviction of stale sessions,
//! a session's declared model, and the attribution of ledger rows.

/// (key, authoritative). Two levels (statusline audit, live capture): the
/// `TOKENADE_SESSION` environment variable is only a WEAK seed —
/// a hook process inherits the session exported by the PARENT shell and
/// must be correctable by its own payload, otherwise the rows
/// of a nested agent are misattributed (a first-wins `OnceLock`
/// did exactly that).
static SESSION_KEY: std::sync::Mutex<Option<(String, bool)>> = std::sync::Mutex::new(None);

/// Weak seed (env): only fills a gap, never overwrites.
pub fn set_session_key(skey: &str) {
    if skey.is_empty() {
        return;
    }
    let mut g = SESSION_KEY.lock().unwrap_or_else(|p| p.into_inner());
    if g.is_none() {
        *g = Some((skey.to_string(), false));
    }
}

/// Forget the session key this process carries. The key is process-wide:
/// a test that set `sessAAA` left it to every later test in its binary, and
/// those — running with the real `HOME` — filed their stash refs under it in
/// the operator's own `~/.tokenade/sessions/` (55 KB of them, 2026-10-03).
pub fn clear_session_key() {
    *SESSION_KEY.lock().unwrap_or_else(|p| p.into_inner()) = None;
}

/// Authoritative (hook payload): overwrites any weak seed.
pub fn set_session_key_authoritative(skey: &str) {
    if skey.is_empty() {
        return;
    }
    let mut g = SESSION_KEY.lock().unwrap_or_else(|p| p.into_inner());
    *g = Some((skey.to_string(), true));
}

/// Where the read hooks stage their pending state, and where the sidechain
/// marker lives. Env override for test isolation (mirrors TOKENADE_DEDUP_DB).
pub fn read_pending_dir() -> std::path::PathBuf {
    if let Some(p) = crate::env_compat::path_from_env("TOKENADE_READ_PENDING_DIR") {
        return p;
    }
    crate::platform::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".tokenade")
        .join("read-pending")
}

/// `count:ts` of a sidechain marker (a bare number is an old `ts`, count 1).
pub fn read_sidechain_state(p: &std::path::Path) -> (u64, u64) {
    let Ok(txt) = std::fs::read_to_string(p) else {
        return (0, 0);
    };
    let t = txt.trim();
    match t.split_once(':') {
        Some((c, ts)) => (c.parse().unwrap_or(1), ts.parse().unwrap_or(0)),
        None => (1, t.parse().unwrap_or(0)),
    }
}

/// How many subagents of session `skey` are running: ≥ 1 while one runs (the
/// Task/Agent hook raises the `sidechain-<key>` marker, SubagentStop lowers
/// it). A subagent shares its parent's session id but NOT its context, so
/// whatever this process adds to or removes from "the context" belongs to the
/// subagent while this is non-zero.
pub fn sidechain_generation(skey: &str) -> u64 {
    if skey.is_empty() {
        return 0;
    }
    read_sidechain_state(&read_pending_dir().join(format!("sidechain-{skey}"))).0
}

/// The calling agent's session, from the environment: the key the hook's
/// rewrite exports (`TOKENADE_SESSION`), else the id Claude Code exports to
/// every Bash call (`CLAUDE_CODE_SESSION_ID`). A command the agent types as
/// `tokenade …` is never rewritten, so it only has the second: MEASURED
/// 2026-10-08, 727 ledger rows in 14 days (`tokenade wrap`, `web`, `read`…)
/// carried no session, so none was banked for the re-reads it saves. Same
/// normalisation as the hook's key (alphanumerics, 40 at most).
pub fn ambient_session_key() -> Option<String> {
    ["TOKENADE_SESSION", "CLAUDE_CODE_SESSION_ID"]
        .iter()
        .find_map(|v| {
            let k: String = std::env::var(v)
                .ok()?
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(40)
                .collect();
            (!k.is_empty()).then_some(k)
        })
}

pub fn session_key() -> Option<String> {
    SESSION_KEY
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .map(|(s, _)| s.clone())
}

/// The directory of small per-session state files: the webfetch
/// URL→stash cache (`<sid>-webfetch.json`), the websearch seen-set
/// (`<sid>-websearch.json`), the shared engine-health file,
/// and the overhead counters (`<key>-ovh`). One to a few files
/// per session id.
pub fn sessions_dir() -> std::path::PathBuf {
    crate::platform::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".tokenade")
        .join("sessions")
}

/// Per-session accumulator of injected overhead tokens (appended at the end of
/// the file, one number per line) — read by the per-turn re-billing, which
/// therefore never has to walk the estimated ledger.
pub fn session_overhead_counter_path(skey: &str) -> std::path::PathBuf {
    sessions_dir().join(format!("{skey}-ovh"))
}

/// The same injections, each with the second it happened (`<ts> <tokens>` per
/// line) — what `gain::injected_rereads` counts each one's re-reads from.
pub fn session_injected_log_path(skey: &str) -> std::path::PathBuf {
    sessions_dir().join(format!("{skey}-ovh-ts"))
}
