//! Not compacting the same bash output twice in a session.

use super::*;
use crate::*;

/// Session key for the current wrap invocation, from `TOKENADE_SESSION` (set by
/// the bash hook). Alphanumeric-normalized to match the read-side session keys.
pub(super) fn wrap_session_key(session: Option<&str>) -> Option<String> {
    match session {
        Some(s) => {
            let k: String = s
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(40)
                .collect();
            (!k.is_empty()).then_some(k)
        }
        None => tokenade_core::session::ambient_session_key(),
    }
}

/// Filesystem marker proving THIS context delivered `hash`'s bytes to the model
/// (session-keyed, so a resumed/trimmed session doesn't inherit it).
///
/// #289 — and generation-keyed, which it was not. A subagent shares the
/// parent's `session_id` (the read path's own comment records a live audit,
/// three times), so a session-scoped proof answered YES for bytes a freshly
/// dispatched subagent had never received: its first command came back as a
/// `§ref` marker and five lines of tail, with no way to check the only claim it
/// was given. The read path had already drawn this line with `ctx_seen_path`'s
/// `g{gen}` segment; the bash path had not, and the two are the same shape.
pub(super) fn bash_seen_marker(skey: &str, hash: &str) -> std::path::PathBuf {
    let gen = crate::hooks::read_pretool::sidechain_generation(skey);
    crate::hooks::read_staging::read_pending_dir().join(format!(
        "bashseen-{skey}-g{gen}-{}",
        &hash[..hash.len().min(16)]
    ))
}

/// True iff a bash §ref fold for `hash` is safe: this session provably served
/// the bytes AND no `/compact` has dropped them since. No session (standalone
/// `tokenade wrap`) → never fold (can't prove the model still has the content).
pub(super) fn bash_dedup_safe(skey: &Option<String>, hash: &str) -> bool {
    let Some(skey) = skey else { return false };
    if crate::hooks::read_pretool::compaction_recent(skey) {
        return false;
    }
    bash_seen_marker(skey, hash).exists()
}

/// Record that THIS session just delivered `hash`'s bytes to the model, so a
/// later identical run can safely fold. No-op without a session.
pub(super) fn mark_bash_seen(skey: &Option<String>, hash: &str) {
    let Some(skey) = skey else { return };
    let p = bash_seen_marker(skey, hash);
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&p, b"1");
}

/// Where the consecutive-identical-answer counter for ONE command lives, in
/// ONE session.
///
/// Two sites open it — the exec-dedup escalation bumps it, the "the answer
/// changed" path removes it — and they must agree on the name or the streak
/// never resets.
///
/// `None` when no session id is set. A session-less run (manual CLI, a test)
/// must not escalate at all: the counter would be shared by every such run on
/// the machine and never reset, which is the same trap the head-slice advisory
/// cap documented as C12 and solved the same way.
///
/// It used to live in `std::env::temp_dir()` keyed by the COMMAND alone, so it
/// was shared across sessions. Measured 2026-08-26: after three runs of
/// `ls -la /usr/include` in one session, a BRAND-NEW session running it for the
/// FIRST time was answered `still unchanged (3 consecutive identical runs)`.
/// Two harms, not one — the sentence is false, and the escalation branch drops
/// the five-line tail, so the session that had never seen the output was the
/// one refused it.
pub(super) fn poll_streak_path(session: Option<&str>, cmd_key: &str) -> Option<std::path::PathBuf> {
    let owned;
    let skey = match session {
        Some(s) => s,
        None => {
            owned = tokenade_core::session::ambient_session_key()?;
            owned.as_str()
        }
    };
    Some(
        tokenade_core::platform::home_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join(".tokenade")
            .join("sessions")
            .join(format!(
                "pollstreak-{}-{}",
                tokenade_core::dedup_ref::hash_content(skey.as_bytes()),
                tokenade_core::dedup_ref::hash_content(cmd_key.as_bytes())
            )),
    )
}

/// Book a substitution's saving — and only when it IS one.
///
/// Three sites computed the same before/after pair by hand and guarded it with
/// the comparison spelled two different ways (`after < before` here,
/// `before > after` there). One rule, one spelling, one place to change it.
pub(super) fn record_shrink(
    channel: &str,
    op: &str,
    label: &str,
    class: Option<&str>,
    before: &str,
    after: &str,
) {
    let sav = tokenade_core::Savings::new(
        tokenade_core::tokenizer::estimate(before),
        tokenade_core::tokenizer::estimate(after),
    );
    if sav.tokens_after >= sav.tokens_before {
        return;
    }
    match class {
        Some(c) => gain_log::record_classed(channel, op, label, c, sav),
        None => gain_log::record(channel, op, label, sav),
    }
}

/// Answer a poll instead of re-serving its output.
///
/// `Some(rc)` = answered here (byte-identical re-run). `None` = fall through to
/// the normal delivery; `compacted_output` may have gained a loop hint on the
/// way out, which is why it comes in by `&mut`.
/// How to get the content back, in the cheapest spelling available.
///
/// When the command read ONE on-disk file, that file is the answer: it is
/// still there, `Read` takes offset/limit, and a re-Read of an unchanged path
/// costs a diff rather than the bytes. The stash pointer is named after it,
/// because what the stash holds is what was DELIVERED (already compacted),
/// which is a different thing from the file and occasionally the thing wanted.
fn way_back(file_target: Option<&std::path::Path>, prior_hash: &str) -> String {
    match file_target {
        Some(p) => format!(
            "`{}` has not changed since that run — `Read {}` (offset/limit) for the bytes; \
             `tokenade expand-ref {prior_hash}` returns what was DELIVERED (already compacted)",
            p.display(),
            p.display(),
        ),
        None => format!(
            "its output, as delivered and already compacted: `tokenade expand-ref {prior_hash}`"
        ),
    }
}

pub(super) fn serve_exec_dedup(
    compacted_output: &mut String,
    cwd: &std::path::Path,
    cmd_key: &str,
    safe_label: &str,
    exit_code: i32,
    session: Option<&str>,
    raw_fingerprint: &str,
    // The single on-disk file this command reads, when it reads exactly one
    // (`cat f`, `cat -n f`, `head -50 f`). MEASURED 2026-09-05 across this
    // machine's transcripts: 73 of 105 dedup answers were a `cat <file>`, and
    // 40 of those were followed within three turns by an `expand-ref` to get
    // the content back, median 2 319 tokens. A `cat` is not a poll — it is the
    // agent looking at a file it needs NOW — and the way back to a file is the
    // file, not a stash hash: `Read` on an unchanged path re-serves as a cheap
    // diff, where `expand-ref` re-emits the whole delivered text.
    file_target: Option<&std::path::Path>,
) -> Option<i32> {
    // TWO lookups, because two different questions are asked below.
    //
    // The per-command pointer answers "what did this command last deliver?" —
    // the near-identical loop nudge needs it whatever the bytes did. The
    // composite one answers "and were the command's OWN bytes the same?", which
    // is the only thing that licenses the word "unchanged".
    let (prior_hash, prior_content, age) =
        tokenade_core::dedup_ref::lookup_by_path_with_age(cwd, cmd_key)?;
    // **"Unchanged" points at output THIS context received.** The pointer is
    // keyed on (cwd, command) and nothing else, so a fresh session in the same
    // directory was answered "byte-identical to the previous run … see
    // expand-ref" about bytes only an EARLIER session had seen. Read in real
    // runs (a tool that launches agent after agent in one directory): "`identity
    // show` … was blocked by tokenade's deduplication ("unchanged… expand-ref")".
    // The `§ref` fold already demands this
    // proof (`bash_dedup_safe`: this session delivered the hash, no /compact
    // since); the poll answer now asks the same. Without a session there is
    // nothing to prove against, and the old behaviour stands.
    let skey = wrap_session_key(session);
    if skey.is_some() && !bash_dedup_safe(&skey, &prior_hash) {
        return None;
    }
    let byte_identical =
        tokenade_core::dedup_ref::lookup_by_path_with_age(cwd, &poll_key(cmd_key, raw_fingerprint))
            .is_some();
    // The claim is checked on the RAW output, not on what we delivered.
    //
    // This compared the two COMPACTED strings while telling the agent
    // "byte-identical output to the previous run of this command" — a claim
    // about the command, checked against the fold. A lossy fold makes those
    // two different things. REPRODUCED 2026-09-05: 400 lines of prose, one
    // line in the elided middle rewritten, and `cat` answered `unchanged —
    // byte-identical output to the previous run` with the rewritten line
    // nowhere in the delivery. An agent polling that file is told nothing
    // moved, and it did.
    //
    // The pointer is keyed on the raw fingerprint now, so equality here is
    // equality of the command's own bytes. A pointer written before this
    // change cannot match one, so it simply does not fire: a missed poll
    // answer costs tokens, a false one costs correctness.
    if byte_identical {
        // Poll-pattern escalation (GitHub demand: codex#13733 —
        // each background poll re-bills a full turn): consecutive
        // unchanged answers shrink further. 1st hit = banner+tail
        // (agent may need the reference), 2nd+ = one-liner (the
        // tail was already re-shown last time; nothing changed).
        let streak_path = poll_streak_path(session, cmd_key);
        let streak: u32 = streak_path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| t.trim().parse().ok())
            .unwrap_or(0);
        if let Some(p) = streak_path.as_ref() {
            if let Some(parent) = p.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(p, (streak + 1).to_string());
        }
        if streak >= 1 {
            // "its output", not "Full output". `register` below stores
            // `compacted_output` under this hash, so what comes back is
            // the DELIVERED text, not the command's raw bytes. Measured
            // 2026-08-04 on `git log --oneline -n 300`: the pointer
            // returned 82 lines of 300 — which is exactly the folded
            // delivery, and precisely what a reader of "Full output"
            // would not expect. The raw bytes are still reachable from
            // the `full output:` banner the FIRST run printed.
            let display = format!(
                "[tokenade:exec-dedup] still unchanged ({} consecutive identical \
                 runs). {}. Polling? Wait longer between checks or make the command \
                 block on the condition itself.",
                streak + 1,
                way_back(file_target, &prior_hash),
            );
            record_shrink(
                "wrap",
                "exec_dedup",
                safe_label,
                None,
                compacted_output,
                &display,
            );
            tokenade_core::debug_log::log(
                "wrap",
                "exec_dedup_poll_streak",
                serde_json::json!({"streak": streak + 1}),
            );
            println!("{display}");
            return Some(exit_code);
        }
        // Third tuple element became AGE-in-seconds when the read-diff
        // epoch-vs-age unit bug was fixed (0.7.5); this caller used to
        // subtract it from now() as if it were still an epoch, showing
        // an absurd age label in the poll answer.
        // Coarse buckets (byte-stable banners fold better on the
        // NEXT identical re-run — exact ages made each unique).
        let age_label = match age {
            0..=59 => "under a minute".to_string(),
            60..=299 => "a few minutes".to_string(),
            _ => "a while".to_string(),
        };
        // Tail = the poll answer (status lines live at the
        // end of most command output). Cap chars so a
        // single-line giant doesn't defeat the dedup.
        let tail_lines: Vec<&str> = compacted_output.trim_end().lines().collect();
        let tail_n = tail_lines.len().min(5);
        let mut tail = tail_lines[tail_lines.len() - tail_n..].join("\n");
        if tail.len() > 500 {
            let mut cut = tail.len() - 500;
            // Re-align to a char boundary after the byte cut.
            while !tail.is_char_boundary(cut) {
                cut += 1;
            }
            tail = format!("…{}", &tail[cut..]);
        }
        let display = format!(
            "[tokenade:exec-dedup] unchanged — byte-identical output to the \
             previous run of this command {age_label} ago (exit code {exit_code}). \
             Last {tail_n} lines:\n{tail}\n\
             ({way_back}. Polling? Prefer a longer wait or a condition in the \
             command itself over tight re-runs.)",
            way_back = way_back(file_target, &prior_hash),
        );
        // Refresh both the pointer timestamp (so the next poll
        // reports age since THIS run) and the content LRU.
        tokenade_core::dedup_ref::register(cwd, &prior_hash, compacted_output);
        tokenade_core::dedup_ref::register_path_pointer(
            cwd,
            &poll_key(cmd_key, raw_fingerprint),
            &prior_hash,
        );
        record_shrink(
            "wrap",
            "exec_dedup",
            safe_label,
            None,
            compacted_output,
            &display,
        );
        tokenade_core::debug_log::log(
            "wrap",
            "exec_dedup_hit",
            serde_json::json!({
                "hash": prior_hash,
                "age_secs": age,
                "exit_code": exit_code,
                "bytes": compacted_output.len(),
            }),
        );
        println!("{display}");
        return Some(exit_code);
    }
    // NEAR-identical re-run (the identical case returned above):
    // a failing edit→test→edit loop rarely reproduces byte-identical
    // output (timestamps/durations vary) but stays ~the same lines.
    // Tell the agent the run changed nothing — the strongest signal
    // to stop iterating in the same place. One line, appended to the
    // normal compacted output; fires at >=90% identical lines on
    // >=10-line outputs. TOKENADE_NO_LOOP_NUDGE mutes it too.
    else if std::env::var("TOKENADE_NO_LOOP_NUDGE").is_err() {
        let old_lines: std::collections::HashSet<&str> = prior_content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .collect();
        let new_lines: Vec<&str> = compacted_output
            .lines()
            .filter(|l| !l.trim().is_empty())
            .collect();
        if new_lines.len() >= 10 && !old_lines.is_empty() {
            let same = new_lines.iter().filter(|l| old_lines.contains(**l)).count();
            let pct = same * 100 / new_lines.len();
            if pct >= 90 {
                // Coarse buckets, not exact values: a byte-stable
                // banner lets the dedup layer fold the NEXT identical
                // re-run (an exact age/pct made every banner unique).
                let age_label = match age {
                    0..=59 => "under a minute",
                    60..=299 => "a few minutes",
                    _ => "a while",
                };
                let pct = pct / 5 * 5;
                compacted_output.push_str(&format!(
                    "\n[tokenade:loop] ~{pct}%+ of this output is line-identical to \
                     the run {age_label} ago — the changes since then did not move \
                     this result. Re-check whether the fix belongs where you're \
                     editing before another iteration."
                ));
                tokenade_core::debug_log::log(
                    "wrap",
                    "loop_similarity_hint",
                    serde_json::json!({"pct": pct, "age_secs": age}),
                );
            }
        }
    }
    None
}

/// Where a delivery is being deduped, and under what name it gets booked.
///
/// `fold_or_register_dedup` had one caller and therefore two constants: the
/// channel was always `wrap` and the op always `cmd_dedup`. It has two callers
/// since `tokenade read` — the command every plugin BRIDGE calls — was wired
/// into the same dedup layer, and the moment a constant varies it stops being
/// one. Threading seven loose arguments through two sites is the parameter
/// list this file already argues against on `ReadRequest`, so what varies per
/// site travels as the site.
#[derive(Clone, Copy)]
pub(crate) struct DedupSite<'a> {
    /// Gain-ledger channel: `wrap` or `read`.
    pub(crate) channel: &'a str,
    /// Gain-ledger op: `cmd_dedup` or `read_dedup`.
    pub(crate) op: &'a str,
    /// Directory the content is registered against.
    pub(crate) cwd: &'a std::path::Path,
    /// Path-pointer key for this delivery.
    pub(crate) key: &'a str,
    /// The shell command that produced the output, for the cross-channel `cat`
    /// seed. Empty on the read path — there is no shell command there, and
    /// `register_bash_served_files` reads an empty string as nothing to do.
    pub(crate) shell_cmd: &'a str,
    /// Redacted label for the ledger row.
    pub(crate) safe_label: &'a str,
    /// Which compactor won, for the debug log.
    pub(crate) filter_label: &'a str,
    /// The session this delivery belongs to, when the caller knows it.
    ///
    /// `None` means "read `TOKENADE_SESSION`", which is right for `wrap`,
    /// where the shell hook sets it. It exists because the Gemini hook knows
    /// the session from its own payload and used to SET the environment
    /// variable around this call to smuggle it in — a process-global write on
    /// a live path, which raced with everything else reading the same
    /// variable and made a dedup guard fail intermittently for three days.
    pub(crate) session: Option<&'a str>,
    /// Hash of the RAW output, before any compaction. The poll pointer is
    /// keyed on it so "byte-identical output to the previous run of this
    /// command" is a statement about the command rather than about the fold.
    pub(crate) raw_fingerprint: &'a str,
}

/// Which pointer a folded repeat gets, by route.
///
/// The COMMAND route carries the tail: a command that repeats is a poll —
/// `tail -30 <log>`, `until [ -s <log> ]` — and its last lines are the answer
/// to the question the repeat asked. `exec-dedup`, the sibling answer for the
/// very same "byte-identical to the previous run" situation, has always given
/// the last five lines.
///
/// The READ route does not. Its floor was measured and settled on its own
/// terms (`dedup_ref::worth_a_ref`), the file is still on disk for the agent
/// to name, and an e2e guard pins the marker's size against
/// `REF_MARKER_BYTES`.
fn marker_for(channel: &str, hash: &str, content: &str) -> String {
    if channel == "wrap" {
        tokenade_core::dedup_ref::ref_marker_with_tail(hash, content.len(), content)
    } else {
        tokenade_core::dedup_ref::ref_marker(hash, content.len())
    }
}

#[cfg(test)]
mod marker_route_tests {
    use super::*;

    #[test]
    fn only_the_command_route_carries_the_poll_tail() {
        let body = format!("{}\nSTILL RUNNING\n", "distinct line\n".repeat(400));
        let w = marker_for("wrap", "abc123abc123", &body);
        let r = marker_for("read", "abc123abc123", &body);
        assert!(w.contains("STILL RUNNING"), "the poll lost its answer: {w}");
        assert!(
            !r.contains("STILL RUNNING"),
            "the read route grew a tail: {r}"
        );
        assert!(
            r.len() <= tokenade_core::dedup_ref::REF_MARKER_BYTES,
            "the read marker is {} bytes, above the declared bound",
            r.len()
        );
    }
}

/// Fold this output to a §ref marker when the session provably already has it,
/// otherwise serve it and register it so the NEXT identical run can fold.
pub(crate) fn fold_or_register_dedup(compacted_output: String, site: DedupSite<'_>) -> String {
    let DedupSite {
        channel,
        op,
        cwd,
        key: cmd_key,
        shell_cmd,
        session,
        safe_label,
        filter_label,
        raw_fingerprint,
    } = site;
    let hash = tokenade_core::dedup_ref::hash_content(compacted_output.as_bytes());
    // Cross-channel dedup seed (also here: wrap is the only pass for
    // agents without a posttool hook): files served via `cat` can
    // answer a later Read.
    crate::hooks::register_bash_served_files(&serde_json::Value::Null, cwd, shell_cmd);
    // Poll streak resets when the answer actually changed.
    {
        if let Some(streak_path) = poll_streak_path(session, cmd_key) {
            let _ = std::fs::remove_file(&streak_path);
        }
    }
    // Point the exec-dedup key at this run's output so the next
    // identical run of the same command answers as a poll.
    let skey = wrap_session_key(session);
    // Fold to a §ref ONLY when THIS session provably delivered these exact
    // bytes to the model AND no /compact has dropped them since. The
    // "seen" marker is session-keyed, so a resumed (and possibly
    // transcript-trimmed) session never folds against a prior session's
    // deliveries either. A snapshot/baseline (`pre-edit-snapshot`,
    // `cmd > file`) registers content for expand-ref recovery but is NEVER
    // marked seen — so it can't masquerade as "you already have this" for
    // content the model never actually received (the `cat`-a-just-written-
    // file round-trip). Mirrors the `ctx_claim_ok` gate the Read/image
    // dedup already uses; the bash path was the only one on raw presence.
    let marker_or_content = if tokenade_core::dedup_ref::worth_a_ref(compacted_output.len())
        && bash_dedup_safe(&skey, &hash)
        && tokenade_core::dedup_ref::lookup(cwd, &hash).is_some()
    {
        // Cache HIT — substitute marker.
        tokenade_core::debug_log::log(
            channel,
            "dedup_hit",
            serde_json::json!({
                "hash": hash,
                "bytes_saved": compacted_output.len(),
                "filter": filter_label,
            }),
        );
        // The tail rides along on the COMMAND route only. A command that
        // repeats is a poll — `tail -30 <log>`, `until [ -s <log> ]` — and its
        // tail is the answer to the question the repeat asked. A repeated READ
        // is a different judgement: the file is still on disk, the agent can
        // name it, and that route's floor was measured and settled separately
        // (`worth_a_ref`). See `dedup_ref::ref_marker_with_tail`.
        let marker = marker_for(channel, &hash, &compacted_output);
        // Exact saving: the full compacted output collapses to an
        // ~80-char ref marker. Until 0.6.4 this was only debug_log'd,
        // so a real, measurable saving went uncounted. `class=dedup`
        // keeps it distinct from compaction in the dashboard.
        record_shrink(
            channel,
            op,
            safe_label,
            Some("dedup"),
            &compacted_output,
            &marker,
        );
        marker
    } else if let Some(marker) = (channel == "wrap")
        .then(|| super::range_read::covered(&skey, shell_cmd, cwd, &compacted_output))
        .flatten()
    {
        // A `sed -n 'A,Bp'` inside lines this session already printed.
        record_shrink(
            channel,
            op,
            safe_label,
            Some("dedup"),
            &compacted_output,
            &marker,
        );
        marker
    } else {
        // Not safe to fold — serve the content, record it for recovery, and
        // mark THIS session as having seen it so a later identical run
        // (pre-compaction) can safely fold.
        tokenade_core::dedup_ref::register(cwd, &hash, &compacted_output);
        mark_bash_seen(&skey, &hash);
        if channel == "wrap" {
            super::range_read::register(&skey, shell_cmd, cwd, &compacted_output);
        }
        // **Not the same bytes — but the same standing fields.** A JSON poll
        // (`some-cli inbound --json`, 4.4 KB of `principles` per call) reprints
        // its unchanged guidance every time. A large
        // top-level field equal to what THIS command delivered last, in THIS
        // session and before any /compact (the same `bash_dedup_safe` proof as
        // the exact fold above), becomes a pointer to that delivery. The full
        // output stays registered under its own hash, so the next run compares
        // against everything the agent now holds.
        // The COMMAND route only: a re-Read of an edited file has its own diff.
        match (channel == "wrap")
            .then(|| tokenade_core::dedup_ref::lookup_by_path(cwd, cmd_key))
            .flatten()
            .filter(|(prev_hash, _)| *prev_hash != hash && bash_dedup_safe(&skey, prev_hash))
            .and_then(|(prev_hash, prev)| {
                tokenade_core::json::fold_unchanged_fields(&prev, &compacted_output, &prev_hash)
            }) {
            Some((folded, _fields)) if folded.len() < compacted_output.len() => {
                record_shrink(
                    channel,
                    op,
                    safe_label,
                    Some("dedup"),
                    &compacted_output,
                    &folded,
                );
                folded
            }
            _ => compacted_output,
        }
    };
    // The poll pointer carries the RAW fingerprint IN ITS KEY, and still points
    // at the DELIVERED text — so `expand-ref` answers "its output, as
    // delivered" exactly as `serve_exec_dedup` promises, and a changed command
    // simply misses the lookup instead of matching a fold that happens to
    // deliver the same way.
    //
    // The fingerprint is NOT used as a blob key: the recovery stash already
    // stores the command's RAW bytes under that very hash, and writing the
    // compacted view there replaced them — three recovery tests caught it.
    // Both pointers: the per-command one for "what did it last deliver", and
    // the composite one as the witness that these exact bytes were produced.
    tokenade_core::dedup_ref::register_path_pointer(cwd, cmd_key, &hash);
    tokenade_core::dedup_ref::register_path_pointer(
        cwd,
        &poll_key(cmd_key, raw_fingerprint),
        &hash,
    );
    marker_or_content
}

/// The poll pointer's key: the command, and the fingerprint of the output it
/// produced. Two runs match only when the command AND its own bytes match.
pub(crate) fn poll_key(cmd_key: &str, raw_fingerprint: &str) -> String {
    format!("{cmd_key}|raw:{raw_fingerprint}")
}

/// The consecutive-identical-runs counter belongs to ONE session.
#[cfg(test)]
mod poll_streak_scope {
    use super::poll_streak_path;

    /// Two sessions running the SAME command must not share a counter.
    ///
    /// The path is derived, not read from disk, so this pins the rule without
    /// touching the filesystem or racing another test on the environment.
    #[test]
    fn the_counter_is_keyed_by_session_as_well_as_command() {
        // Derives paths from HOME, so it must pin HOME like its neighbours:
        // a sibling test swapping it mid-run made the same session yield two
        // different paths.
        let _home = tokenade_core::platform::ScratchHome::new("streak-key");
        let with = |sess: &str| poll_streak_path(Some(sess), "ls -la /usr/include");
        let a = with("session-aaaa").expect("a session gets a counter");
        let b = with("session-bbbb").expect("a session gets a counter");
        assert_ne!(
            a, b,
            "two sessions share one streak file, so a fresh session inherits \
             another's history"
        );
        let a2 = with("session-aaaa").expect("stable");
        assert_eq!(a, a2, "the same session must keep its own counter");
    }

    /// And the same session must still separate its commands.
    #[test]
    fn the_counter_is_still_keyed_by_command() {
        let _home = tokenade_core::platform::ScratchHome::new("streak-cmd");
        let one = poll_streak_path(Some("s"), "ls /a").unwrap();
        let two = poll_streak_path(Some("s"), "ls /b").unwrap();
        assert_ne!(one, two);
    }

    /// No session id: no escalation at all. A session-less run is a manual CLI
    /// invocation or a test, and a counter shared by every one of them on the
    /// machine never resets — the C12 trap the head-slice cap documented.
    #[test]
    fn without_a_session_there_is_no_counter() {
        let _home = tokenade_core::platform::ScratchHome::new("streak-nosess");
        assert!(
            poll_streak_path(None, "ls -la /usr/include").is_none()
                || tokenade_core::session::ambient_session_key().is_some()
        );
    }

    /// The WIRING, not the path rule: `serve_exec_dedup` itself must not
    /// escalate for a session that has not repeated anything.
    ///
    /// The three tests above pin `poll_streak_path` and all stay green if the
    /// caller never consults it — the same trap that made two earlier
    /// guards worthless until one of them crossed the seam. An end-to-end
    /// version was attempted and abandoned. The reason first written here —
    /// "the dedup pass does not run under the e2e harness" — was WRONG, and
    /// a later round corrected it: the dedup did not fire on that FIXTURE, in any
    /// environment. This guard lives here because this is where the counter is
    /// read, which is the right place regardless.
    #[test]
    fn a_fresh_session_is_not_told_it_repeated() {
        // The streak file lives under HOME, and this test used the REAL one.
        // `hookwrap`'s SessionStart sweep prunes `~/.tokenade/sessions/` once
        // a day; whichever test triggered it first deleted this test's streak
        // file mid-flight, and the assertion below — "the session that DID
        // repeat must have one" — failed. An earlier fix removed the `TOKENADE_SESSION`
        // race and I wrote that the flake was gone; it was not, there were
        // TWO causes and this is the second.
        //
        // `ScratchHome` holds `platform::ENV_LOCK` for its whole life, which
        // is the sanctioned way to touch a process-global here.
        let _home = tokenade_core::platform::ScratchHome::new("streak-wire");
        let dir = std::env::temp_dir().join(format!(
            "tokenade-streakwire-{}-{}",
            std::process::id(),
            line!()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let key = format!("streak-wire-{}", std::process::id());
        let body = "line one\nline two\nline three\n".repeat(40);

        let serve = |sess: &str| -> Option<String> {
            let hash = tokenade_core::dedup_ref::hash_content(body.as_bytes());
            tokenade_core::dedup_ref::register(&dir, &hash, &body);
            // Both pointers, as production writes them: the per-command one,
            // and the composite witness that these exact raw bytes were the
            // ones produced. Raw and delivered are the same string here.
            tokenade_core::dedup_ref::register_path_pointer(&dir, &key, &hash);
            tokenade_core::dedup_ref::register_path_pointer(
                &dir,
                &super::poll_key(&key, &hash),
                &hash,
            );
            // And the proof that THIS session received them, which production
            // writes on the delivering run (`fold_or_register_dedup`) and the
            // poll answer now requires.
            super::mark_bash_seen(&super::wrap_session_key(Some(sess)), &hash);
            let mut out = body.clone();
            super::serve_exec_dedup(&mut out, &dir, &key, "ls", 0, Some(sess), &hash, None)?;
            Some(out)
        };
        // Two runs in one session: the second must escalate.
        let _ = serve("wire-alpha");
        let _ = serve("wire-alpha");
        let third = serve("wire-alpha");
        // A session that never ran it must NOT inherit that streak.
        let fresh_path = poll_streak_path(Some("wire-beta"), &key).unwrap();
        assert!(
            !fresh_path.exists(),
            "the fresh session already has a streak file at {}",
            fresh_path.display()
        );
        let alpha_path = poll_streak_path(Some("wire-alpha"), &key).unwrap();
        assert!(
            alpha_path.exists(),
            "the session that DID repeat must have one — otherwise this test \
             proves nothing about the caller consulting the counter"
        );
        assert_ne!(alpha_path, fresh_path);
        let _ = third;
        let _ = std::fs::remove_file(&alpha_path);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod a_subagent_never_inherits_the_parents_proof {
    use super::{bash_dedup_safe, bash_seen_marker, mark_bash_seen};

    fn open_sidechain(skey: &str, count: u64) {
        let dir = crate::hooks::read_staging::read_pending_dir();
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join(format!("sidechain-{skey}")), format!("{count}:0"));
    }

    /// The shape #289 fixes: the parent runs a command and receives its bytes,
    /// a subagent is dispatched and runs the same command. Before the
    /// generation segment the subagent was told it already had the output and
    /// got a `§ref` marker for bytes that had never been in its context.
    #[test]
    fn a_parents_delivery_does_not_answer_for_a_subagent() {
        let _home = tokenade_core::platform::ScratchHome::new("subagent-proof");
        let skey = "sess289".to_string();
        let hash = "abcdef0123456789abcdef";

        mark_bash_seen(&Some(skey.clone()), hash);
        assert!(
            bash_dedup_safe(&Some(skey.clone()), hash),
            "the parent may fold its own repeat"
        );

        open_sidechain(&skey, 1);
        assert!(
            !bash_dedup_safe(&Some(skey.clone()), hash),
            "a subagent must be served in full: it never received these bytes"
        );
    }

    /// …and the parent keeps its own proof once the subagent is gone.
    #[test]
    fn the_parent_keeps_its_proof_after_the_subagent_ends() {
        let _home = tokenade_core::platform::ScratchHome::new("subagent-proof-end");
        let skey = "sess289b".to_string();
        let hash = "0123456789abcdef012345";

        mark_bash_seen(&Some(skey.clone()), hash);
        open_sidechain(&skey, 1);
        assert!(!bash_dedup_safe(&Some(skey.clone()), hash));

        let dir = crate::hooks::read_staging::read_pending_dir();
        let _ = std::fs::remove_file(dir.join(format!("sidechain-{skey}")));
        assert!(
            bash_dedup_safe(&Some(skey.clone()), hash),
            "generation 0 is the parent's own, and it did deliver"
        );
    }

    /// A subagent that runs a command TWICE may still fold its own repeat —
    /// the line is drawn at the context boundary, not at subagents in general.
    #[test]
    fn a_subagent_may_still_fold_its_own_repeat() {
        let _home = tokenade_core::platform::ScratchHome::new("subagent-own");
        let skey = "sess289c".to_string();
        let hash = "feedfacefeedfacefeedfa";

        open_sidechain(&skey, 1);
        mark_bash_seen(&Some(skey.clone()), hash);
        assert!(bash_dedup_safe(&Some(skey.clone()), hash));
    }

    /// The generation rides the marker's NAME, so the two dedup paths spell it
    /// the same way and a stale-generation marker is simply unreachable.
    #[test]
    fn the_generation_is_part_of_the_marker_name() {
        let _home = tokenade_core::platform::ScratchHome::new("subagent-name");
        let skey = "sess289d";
        let hash = "cafebabecafebabecafeba";
        let g0 = bash_seen_marker(skey, hash);
        open_sidechain(skey, 2);
        let g2 = bash_seen_marker(skey, hash);
        assert_ne!(g0, g2);
        assert!(g0.to_string_lossy().contains("-g0-"), "{g0:?}");
        assert!(g2.to_string_lossy().contains("-g2-"), "{g2:?}");
    }
}

#[cfg(test)]
mod one_spelling_for_the_way_back {
    /// Every dedup answer names the way back through `way_back`.
    ///
    /// Both answer shapes (the first hit, which carries a tail, and the 2nd+
    /// "still unchanged" one-liner, which does not) have to make the same
    /// choice between "the file is still on disk" and "here is the stash
    /// hash". They were two independent `format!` strings, and only one of
    /// them can be reached from the e2e harness: `TOKENADE_SESSION` is removed
    /// from every spawn on purpose, so the streak counter never escalates
    /// there and the one-liner branch is unreachable end to end. A mutation
    /// that reverted JUST that branch to the hash-only wording survived the
    /// e2e test. This guard is what kills it: the recovery pointer may be
    /// spelled in exactly one place.
    #[test]
    fn the_stash_pointer_is_spelled_once() {
        let src = tokenade_core::srcsweep::production_source(include_str!("dedup.rs"));
        const PTR: &str = "expand-ref {prior_hash}";
        let (before, rest) = src
            .split_once("fn way_back(")
            .expect("`way_back` is where the pointer is spelled");
        let (body, after) = rest.split_once("\n}\n").expect("`way_back` ends");
        assert_eq!(
            before.matches(PTR).count(),
            0,
            "a stash pointer is spelled before `way_back`"
        );
        assert_eq!(
            after.matches(PTR).count(),
            0,
            "a stash pointer is spelled outside `way_back` - both answer shapes must \
             go through it, or the branch the e2e cannot reach drifts on its own"
        );
        // Anti-vacuity: the counts above are also zero if the pointer left
        // `way_back` entirely.
        assert!(
            body.matches(PTR).count() >= 2,
            "`way_back` must still spell the pointer on both of its arms"
        );
    }
}

#[cfg(test)]
mod unchanged_points_at_this_contexts_bytes {
    use super::*;

    /// A tool that launches agent after agent in one directory: the second one
    /// received "unchanged … expand-ref" on an output it had never seen.
    #[test]
    fn a_fresh_session_is_not_told_unchanged() {
        let home = tokenade_core::platform::ScratchHome::new("exec-dedup-ctx");
        let cwd = home.path().to_path_buf();
        let out = "identity: alice\nplan: basic\nquota: 10M\n".repeat(40);
        let cmd_key =
            tokenade_core::dedup_ref::command_key(&cwd.to_string_lossy(), "some-cli identity show");
        let raw_fp = tokenade_core::dedup_ref::hash_content(out.as_bytes());
        let hash = tokenade_core::dedup_ref::hash_content(out.as_bytes());
        tokenade_core::dedup_ref::register(&cwd, &hash, &out);
        tokenade_core::dedup_ref::register_path_pointer(&cwd, &cmd_key, &hash);
        tokenade_core::dedup_ref::register_path_pointer(&cwd, &poll_key(&cmd_key, &raw_fp), &hash);
        mark_bash_seen(&Some("sessiona".to_string()), &hash);

        // `Some` = the poll answer was printed in place of the output.
        let serve = |session: &str| {
            let mut shown = out.clone();
            serve_exec_dedup(
                &mut shown,
                &cwd,
                &cmd_key,
                "some-cli",
                0,
                Some(session),
                &raw_fp,
                None,
            )
        };
        assert!(
            serve("sessionb").is_none(),
            "session B never received these bytes"
        );
        assert_eq!(
            serve("sessiona"),
            Some(0),
            "session A did: the poll answer stands"
        );
    }
}

#[cfg(test)]
mod a_poll_reprints_only_what_moved {
    use super::*;

    fn site<'a>(cwd: &'a std::path::Path, key: &'a str, session: Option<&'a str>) -> DedupSite<'a> {
        DedupSite {
            channel: "wrap",
            op: "cmd_dedup",
            cwd,
            key,
            shell_cmd: "",
            session,
            safe_label: "some-cli inbound --json",
            filter_label: "auto:json",
            raw_fingerprint: "fp",
        }
    }

    /// `some-cli inbound --json`: `principles` (4.4 kB) reprinted on every
    /// call.
    #[test]
    fn an_unchanged_field_of_the_previous_run_becomes_a_pointer() {
        let home = tokenade_core::platform::ScratchHome::new("rerun-fields");
        let cwd = home.path().to_path_buf();
        let key = tokenade_core::dedup_ref::command_key(
            &cwd.to_string_lossy(),
            "some-cli inbound --json",
        );
        let guide = "un principe assez long pour peser dans la sortie. ".repeat(20);
        let run = |n: u32| {
            serde_json::json!({ "count": n, "items": (0..n).collect::<Vec<_>>(), "principles": guide }).to_string()
        };
        let first = fold_or_register_dedup(run(0), site(&cwd, &key, Some("sessa")));
        assert!(
            first.contains("un principe assez long"),
            "the first run is delivered whole"
        );
        let second = fold_or_register_dedup(run(2), site(&cwd, &key, Some("sessa")));
        assert!(
            second.contains("\"count\":2") && second.contains("expand-ref"),
            "{second}"
        );
        assert!(!second.contains("un principe assez long"), "{second}");
        // Another session never received the guidance: it gets it whole.
        let other = fold_or_register_dedup(run(3), site(&cwd, &key, Some("sessb")));
        assert!(other.contains("un principe assez long"), "{other}");
    }
}
