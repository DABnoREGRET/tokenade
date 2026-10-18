//! **The agent redoes EXACTLY the same work.**
//!
//! A failing command, a probe, a test: re-run turn after turn, each one
//! re-billing the whole context for an identical result. The counter is per
//! (session, normalized command), and the advice only goes out on the 3rd
//! run, then every 3rd.
//!
//! The delicate point is the RESET TO ZERO: if the output changed, the agent
//! is not going in circles, it is making progress — and the counter must
//! forget it, otherwise the advice accuses work that is moving forward.

use super::*;

/// Loop guard: agents sometimes re-run the EXACT same command (a failing
/// build/test, the same probe) turn after turn, each re-billing the whole
/// context for an identical result. We keep a per-(session, command) counter;
/// when the same normalized command fires 3× (then every 3rd) this session, emit
/// a ONE-LINE PreToolUse `additionalContext` telling the agent to change tack
/// rather than repeat. Local, zero model calls. Disable with
/// TOKENADE_NO_LOOP_NUDGE=1.
/// Where the loop counter for one (session, command) lives.
///
/// Shared, because the counter is written by the PreToolUse hook and CLEARED
/// by `wrap` — see [`reset_loop_guard_if_output_moved`]. Two spellings of this
/// path would mean the reset silently missed.
pub(crate) fn loop_guard_path(session: &str, cmd: &str) -> std::path::PathBuf {
    let sig = cmd.split_whitespace().collect::<Vec<_>>().join(" ");
    let skey: String = session
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(40)
        .collect();
    let chash = tokenade_core::dedup_ref::hash_content(sig.as_bytes());
    read_pending_dir().join(format!("loop-{skey}-{chash}"))
}

/// The command ran again and its output MOVED: this is a poll, not a loop.
///
/// The counter only ever looked at the command's INPUT, so a watcher — a build
/// log tailed every minute, a health probe, a test suite writing a file — was
/// told at its 3rd run that "re-running won't change the result" while every
/// run returned different bytes. MEASURED 2026-09-05 across this machine's
/// transcripts: of 50 firings whose command could be matched to its runs,
/// **5 were on a command whose outputs all differed** — one of them
/// `timeout 900 python3 scripts/watch.py … | tail -10`, 7 runs and 6 distinct
/// answers. A steer that lands BEFORE the command runs and is false 10% of the
/// time argues an agent out of the one thing that was working.
///
/// An Edit/Write already restarts the count (edit→test→edit is legitimate
/// iteration); this is the same idea for state the session does not own.
/// `ran_before`: this command has a delivery pointer from an earlier run.
/// `same_bytes`: that earlier run produced the same raw output as this one.
/// The decision lives HERE, not at the call site, so all four combinations are
/// testable: only a command that ran before AND answered differently is a
/// poll. A first run has nothing to have moved.
pub(crate) fn reset_loop_guard_if_output_moved(cmd: &str, ran_before: bool, same_bytes: bool) {
    if !(ran_before && !same_bytes) || hook_disabled_by_env() {
        return;
    }
    let Ok(session) = std::env::var("TOKENADE_SESSION") else {
        return;
    };
    let _ = std::fs::remove_file(loop_guard_path(&session, cmd));
    let alias = loop_alias_path(&session, cmd);
    if let Ok(counter) = std::fs::read_to_string(&alias) {
        let _ = std::fs::remove_file(read_pending_dir().join(counter.trim()));
    }
}

/// **The hook counts the WHOLE command; `wrap` only sees its segment.**
///
/// `cd X; cargo test | tail -3` is counted under that text, but rewritten as
/// `cd X ; tokenade wrap 'cargo test | tail -3'`: the reset above looked for
/// the SEGMENT's counter, which does not exist. Measured over 509 local
/// firings (2026-10-02): the most frequent are compound polls —
/// `SP=…; cat $SP/out`, `command sleep 118; …`, `cd …; cargo nextest …` —
/// whose output changed on every run, and which were told on the 3rd that
/// "re-running won't change the result". So the hook leaves, for each segment
/// it wraps, the address of the whole command's counter.
pub(crate) fn note_wrapped_segment(session: &str, full_cmd: &str, segment: &str) {
    let norm = |c: &str| c.split_whitespace().collect::<Vec<_>>().join(" ");
    if norm(full_cmd) == norm(segment) {
        return;
    }
    let counter = loop_guard_path(session, full_cmd);
    let Some(name) = counter.file_name().and_then(|n| n.to_str()) else {
        return;
    };
    let alias = loop_alias_path(session, segment);
    let _ = std::fs::create_dir_all(alias.parent().unwrap_or(&read_pending_dir()));
    let _ = tokenade_core::paths::write_atomic(&alias, name);
}

fn loop_alias_path(session: &str, segment: &str) -> std::path::PathBuf {
    let counter = loop_guard_path(session, segment);
    let name = counter.file_name().and_then(|n| n.to_str()).unwrap_or("");
    read_pending_dir().join(format!("alias-{name}"))
}

/// **Following this advice means doing SOMETHING ELSE — so re-running nothing.**
///
/// The advice says "try a DIFFERENT approach". The agent that obeys does not
/// go through the same command again: that command's counter is never touched
/// again, and the `ok` was only booked in the one case where it re-ran the
/// SAME command anyway after an edit. Measured over 30 days: 2 072 firings,
/// 46 followed, 543 ignored — 72 % with no outcome, because success was
/// invisible by construction.
///
/// The session's write generation is the missing signal: it moves as soon as
/// an `Edit`/`Write` happens, whatever the next command. A pending firing that
/// is followed by a write is a followed firing.
fn resolve_pending_tack_change(skey: &str, gen: u64) {
    let p = read_pending_dir().join(format!("loop-guard-pending-{skey}"));
    let Ok(prev) = std::fs::read_to_string(&p) else {
        return;
    };
    let Ok(fired_at) = prev.trim().parse::<u64>() else {
        return;
    };
    if gen > fired_at {
        tokenade_core::gain::record_nudge(
            "loop-guard",
            "ok",
            tokenade_core::gain::NudgeMetric::Occurrence(1),
        );
        let _ = std::fs::remove_file(&p);
    }
}

/// Records that an advice awaits an outcome, at the generation where it spoke.
fn mark_pending_tack_change(skey: &str, gen: u64) {
    let dir = read_pending_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = tokenade_core::paths::write_atomic(
        &dir.join(format!("loop-guard-pending-{skey}")),
        &gen.to_string(),
    );
}

pub(crate) fn loop_guard_nudge(parsed: &serde_json::Value, cmd: &str) -> Option<String> {
    if hook_disabled_by_env() || std::env::var("TOKENADE_NO_LOOP_NUDGE").is_ok() {
        return None;
    }
    // Normalize whitespace so re-runs with cosmetic spacing differences still
    // collide, but distinct commands stay distinct.
    let sig = cmd.split_whitespace().collect::<Vec<_>>().join(" ");
    if sig.is_empty() {
        return None;
    }
    let session = crate::hooks::session_id_of(parsed).unwrap_or("nosess");
    let skey: String = session
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(40)
        .collect();
    let p = loop_guard_path(session, cmd);
    let _ = std::fs::create_dir_all(p.parent().unwrap_or(&read_pending_dir()));
    // Generation-aware count: an Edit/Write in this session bumps the
    // write-generation, and a command re-run AFTER an edit is legitimate
    // iteration (edit→test→edit→test), not a loop. Restart the count when
    // the generation moved. File payload: "<gen>:<count>" ("<count>" from
    // older builds parses as gen 0).
    let gen = crate::hooks::session_lifecycle::current_write_generation(&skey);
    resolve_pending_tack_change(&skey, gen);
    let n = std::fs::read_to_string(&p)
        .ok()
        .and_then(|s| {
            let t = s.trim();
            match t.split_once(':') {
                Some((g, c)) => Some((g.parse::<u64>().ok()?, c.parse::<u32>().ok()?)),
                None => Some((0, t.parse::<u32>().ok()?)),
            }
        })
        .map(|(g, c)| {
            if g == gen {
                c
            } else {
                // The generation moved: the agent edited something. The
                // advice is resolved by `resolve_pending_tack_change`, which
                // ALSO sees the sessions where the agent switched commands —
                // this arm only saw those where it re-runs the SAME one.
                0
            }
        })
        .unwrap_or(0)
        + 1;
    let _ = tokenade_core::paths::write_atomic(&p, &format!("{gen}:{n}"));
    // Fire at the 3rd identical run, then every 3rd after (3, 6, 9…) if it keeps
    // repeating — steer without spamming every turn.
    if n >= 3 && n.is_multiple_of(3) {
        // **`fire` counts DELIVERIES, and `ignored` the previous firing.**
        //
        // This site used to write `fire` only at n == 3 and `ignored` on every
        // re-fire: `fire` and `ignored` therefore counted two different
        // populations, and a loop of twelve repetitions produced ONE firing
        // and THREE ignored. Measured over 30 days: 2 072 firings, 46
        // followed, 543 ignored — an "8 % adherence" comparable to no other,
        // since everywhere else (`batch-*` family) the three verbs cover the
        // same set.
        //
        // Here as there: we fire every time we speak, and the previous firing
        // is deemed ignored at the moment we need to speak again.
        if n > 3 {
            tokenade_core::gain::record_nudge(
                "loop-guard",
                "ignored",
                tokenade_core::gain::NudgeMetric::Occurrence(n.saturating_sub(3) as usize),
            );
        }
        tokenade_core::gain::record_nudge(
            "loop-guard",
            "fire",
            tokenade_core::gain::NudgeMetric::Occurrence(n as usize),
        );
        mark_pending_tack_change(&skey, gen);
        Some(format!(
            "[tokenade] You've run this exact command {n}× this session with the same input — \
             re-running won't change the result. Read the last output/error and try a DIFFERENT \
             approach (different flags, inspect state, or fix the root cause) instead of repeating."
        ))
    } else {
        None
    }
}
