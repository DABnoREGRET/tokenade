//! **Don't repeat the same advice.**
//!
//! An agent that receives the same sentence over and over starts COMMENTING
//! on it instead of applying it. Two rules, and neither is a global quota: one
//! sentence per family and per window, then a DISTANCE before saying it again,
//! measured in context tokens and proportional to the model's window.
//!
//! A global ceiling was written then removed the same day: it makes the
//! arrival of one piece of advice depend on UNRELATED advice, so a useful
//! warning can be swallowed because two other families spoke first — and the
//! swallowed family will never know it was silenced.

use super::*;

/// Once-per-session gate for an additive hint line. Returns `true` the
/// FIRST time a given `(session_id, hint_key)` pair is seen and `false`
/// on every later call in the same session.
///
/// Why disk, not a `Mutex`: each hook fires as a fresh, short-lived
/// process, so an in-memory counter (like `nudge::step`'s) resets every
/// invocation and never actually throttles. We persist a marker file
/// keyed by the session so repeats are suppressed across processes.
///
/// The DETECTION still runs on every command/read — we keep all of the
/// steering value of the matcher. We only stop re-injecting the SAME
/// advice into a context that already carries it (once it is in the
/// context, there is no need to say it again). Distinct
/// hint families use distinct keys so each is shown once.
///
/// Fail-open: any IO error (no home dir, disk full, permissions) returns
/// `true` so the hint is shown rather than silently lost — at worst we
/// under-throttle, which is strictly safer than swallowing the steer.
/// Sanitise a raw `session_id` into the on-disk marker prefix used by
/// the hint throttle. Alphanumeric-only (no `-`) so the first `-` in a
/// marker filename always separates session from `hint_key` — letting
/// `reset_session_hints` match `"{session}-*"` without bleeding into a
/// different session whose id is a prefix of this one.
pub(crate) fn hint_session_key(session_id: &str) -> String {
    let session: String = session_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(40)
        .collect();
    if session.is_empty() {
        "nosess".to_string()
    } else {
        session
    }
}

/// Re-arm every hint for `session_id` by deleting its marker files.
///
/// Called from the `PreCompact` hook: when Claude Code's auto-summariser
/// runs, the hint lines we injected earlier are squashed out of the live
/// context, so the once-per-session gate would otherwise keep them
/// suppressed FOREVER even though the agent can no longer see them. After
/// a reset, the next time each triggering condition reappears the hint is
/// shown once more, then re-throttled until the following compaction.
/// (The context gets compacted, the hints leave it, so they can be shown
/// again when the condition reappears.)
///
/// Fail-silent: any IO error just leaves markers in place (under-resets,
/// strictly safer than a panic in a compaction-blocking hook).
pub(crate) fn reset_session_hints(session_id: &str) {
    let prefix = format!("{}-", hint_session_key(session_id));
    let dir = read_pending_dir().join("hints");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_str()
            .map(|n| n.starts_with(&prefix))
            .unwrap_or(false)
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

pub(crate) fn hint_fresh_this_session(session_id: &str, hint_key: &str) -> bool {
    let session = hint_session_key(session_id);
    let dir = read_pending_dir().join("hints");
    if std::fs::create_dir_all(&dir).is_err() {
        return true; // fail-open
    }
    let p = dir.join(format!("{session}-{hint_key}"));
    // `create_new` is atomic: it succeeds exactly once and returns
    // `AlreadyExists` on every later call — no read-modify-write race
    // between concurrent hook processes racing on the same session.
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&p)
    {
        Ok(_) => true,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(_) => true, // fail-open on other IO errors
    }
}

// **The repetition budget of a piece of advice, within the window the model
// sees.**
//
// Design rule (2026-09-12): don't spam the same nudges — there must be a
// session-wide count (taking compaction into account) and smart handling.
// The reported case: an agent that receives the same sentence over and over
// starts COMMENTING on it instead of applying it.
//
// Two rules, and neither is a global quota:
//
// 1. **one sentence per family and per window** — advice already present in
//    the context does not need to be said again;
// 2. **a DISTANCE before saying it again**, measured in context tokens and
//    proportional to the model's window.
//
// A global ceiling — three pieces of advice per window, all families
// combined — was written then REMOVED the same day, on the intuition that it
// would create more problems than it solves. That intuition is sound: the
// ceiling makes the arrival of one piece of advice depend on UNRELATED
// advice, so a useful warning can be swallowed because two other families
// spoke first — and the swallowed family will never know it was silenced.
// Each piece of advice is governed by its own distance.
//
// **What this gate never sees**: the banners carrying a retrieval hash
// (`tokenade expand-ref <hash>`). Their text is NEW every time — it names
// other bytes — and muzzling them would cut the agent off from the raw
// bytes. They live in `wrap::nudges`, which does not call this; the test
// `a_hash_carrying_banner_is_never_muzzled` holds that.

/// Can this advice be said now? Consumes the token if it can.
///
/// Order matters: the DISTANCE is checked before the per-family token,
/// otherwise a refusal due to distance would still burn the family's « once »,
/// and it would stay silent for the whole next window.
pub(crate) fn nudge_budget_take(
    session_id: &str,
    kind: &str,
    ctx_now: u64,
    repeat_distance: u64,
) -> bool {
    let session = hint_session_key(session_id);
    let dir = read_pending_dir().join("hints");
    if std::fs::create_dir_all(&dir).is_err() {
        return true; // fail-open: an unreadable budget must not muzzle
    }
    // **The distance in tokens, for the SAME sentence.** Advice already given
    // is only repeated once the context has grown enough to drown it — a
    // quarter of the model's window. A context that SHRANK is a compaction:
    // the sentence left with the summary, it can be said again right away.
    //
    // This rule only concerns advice with IDENTICAL text. A banner carrying a
    // retrieval hash says something new every time; it does not go through
    // here.
    let stamp = dir.join(format!("{session}-nudgectx-{kind}"));
    let last = std::fs::read_to_string(&stamp)
        .ok()
        .and_then(|t| t.trim().parse::<u64>().ok());
    if let Some(last) = last {
        let grew_enough = ctx_now >= last.saturating_add(repeat_distance);
        let compacted = ctx_now < last;
        if !grew_enough && !compacted {
            return false;
        }
    }
    let fresh = hint_fresh_this_session(session_id, &format!("nudge-{kind}"));
    if fresh || last.is_some() {
        let _ = tokenade_core::paths::write_atomic(&stamp, &ctx_now.to_string());
    }
    fresh || last.is_some()
}

pub(crate) fn read_flow_wants_legacy_deny() -> bool {
    match std::env::var("TOKENADE_READ_FLOW") {
        Ok(v) if v.eq_ignore_ascii_case("deny") => return true,
        Ok(v) if v.eq_ignore_ascii_case("allow") => return false,
        _ => {}
    }
    !postread_marker_path().exists()
}

/// Staged entries are keyed by (session, file) so two agents reading
/// the same file concurrently don't steal each other's context.
/// Deny-audit (HIGH): the key MUST discriminate the slice — two PARALLEL
/// Reads of the same file at different offsets (the batching pattern we
/// ourselves recommend) shared one key: the second staging overwrote the
/// first, delivering the wrong slice to the wrong tool_use_id.
pub(crate) fn read_pending_key(
    session_id: &str,
    file_path: &str,
    offset: u64,
    limit: u64,
) -> String {
    tokenade_core::dedup_ref::hash_content(
        format!("{session_id}|{file_path}|{offset}|{limit}").as_bytes(),
    )
}

pub(crate) fn stage_read_pending(
    session_id: &str,
    file_path: &str,
    offset: u64,
    limit: u64,
    context: &str,
) -> bool {
    let dir = read_pending_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return false;
    }
    // Opportunistic GC — the dir only ever holds in-flight entries.
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let stale = e
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.elapsed().ok())
                .map(|d| d.as_secs() > READ_PENDING_TTL_SECS)
                .unwrap_or(false);
            if stale {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let ts = tokenade_core::platform::now_unix_secs();
    let record = serde_json::json!({
        "file_path": file_path,
        "ts": ts,
        "context": context,
    });
    let key = read_pending_key(session_id, file_path, offset, limit);
    // Write-then-rename so the postread hook never reads a half-written
    // record (both hooks are separate short-lived processes).
    let tmp = dir.join(format!("{key}.json.tmp-{}", std::process::id()));
    let dst = dir.join(format!("{key}.json"));
    if std::fs::write(&tmp, record.to_string()).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    std::fs::rename(&tmp, &dst).is_ok()
}
