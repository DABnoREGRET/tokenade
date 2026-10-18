//! **What we SLIP to the agent without it asking.**
//!
//! Unsolicited advice costs tokens every turn and is read only once: half of
//! this file's job is therefore deciding WHEN to stay quiet. `should_fire`
//! carries that decision, and it depends on the current model's window — not
//! on a constant, because a reminder every 30 000 tokens is discreet on 1 M
//! and nagging on 200 k.
//!
//! The four advice families (read, shell, edit, oversized file) share this
//! gate and nothing else.

use super::*;

/// PreToolUse handler for `Read`. Detects repeat reads of the
/// SAME file (same path, same mtime, same size, same offset/limit
/// slice) and substitutes a `§ref:HASH§` marker instead of letting
/// Claude Code re-ship the bytes. The agent's prior turn already
/// has the content; `tokenade expand-ref <hash>` fetches it back if
/// they need it again.
///
/// Trade-off: we use `permissionDecision: deny + permissionDecisionReason`
/// to substitute (Read doesn't accept an `updatedInput` rewrite for
/// the same content). Claude Code surfaces that as "Error:" in the
/// UI. To keep the UX cost under the savings we gate on:
///   - file size ≥ MIN_BYTES_TO_DEDUPE (~200 bytes default)
///   - file is in the dedup cache (registered on a prior Read)
///
/// Tiny files and first reads pass through normally — no UI cost.
///
/// Symlink / non-file paths fall through too (we don't want to
/// dedup `/dev/random`).
/// #1072 — reactive batching nudge. Agents issue exactly one tool call per
/// turn (measured: 1 tool_use/message across every benchmark task), so a task
/// that reads N independent files pays N turns, each re-billing the whole
/// context. When the agent has issued several single-file Reads this session,
/// emit a ONE-LINE PreToolUse `additionalContext` pointing at native parallel
/// tool calls (read_many was an MCP tool, retired at the zero-MCP pivot; the
/// nudge text below steers to several `Read` calls in one message). Fires
/// exactly once per session (at the Nth Read) so it
/// never becomes recurring overhead. Disable with TOKENADE_NO_BATCH_NUDGE=1.
pub(crate) fn batching_read_nudge(parsed: &serde_json::Value) -> Option<String> {
    batching_family_nudge(
        parsed,
        "read",
        "[tokenade] You've Read several files one-per-turn. If you already know the other \
         files you need, emit all the `Read` tool calls in ONE message (parallel tool calls) \
         instead of one Read per turn — plan the full set of files up front.",
    )
}

/// Same steering for consecutive single-`Bash` turns — trace mining showed
/// Bash-only chains are the LARGEST avoidable re-send pool (2x the Read
/// chains) and had no nudge at all. Only fires via the same
/// streak/parallel-window mechanics as reads.
pub(crate) fn batching_bash_nudge(parsed: &serde_json::Value) -> Option<String> {
    batching_family_nudge(
        parsed,
        "bash",
        "[tokenade] You've run several commands one-per-turn. When commands don't depend \
         on each other's output, combine them in ONE call (`cmd1; cmd2` or one script) or \
         emit parallel Bash calls in one message — plan the batch before the first command.",
    )
}

/// And for consecutive single-`Edit` turns: independent edits — in several
/// files OR several hunks of one file — batch as parallel `Edit` calls in ONE
/// message. (Suggestion only — dependent edits stay valid.)
///
/// The advice used to send same-file hunks to `MultiEdit`, a tool Claude Code
/// no longer offers: zero `MultiEdit` calls in 962 local transcripts that use
/// `Edit` (2026-10-02). Half the advice pointed at nothing, and an agent that
/// cannot follow it books `ignored` — `batch-edit` sat at 19 % adherence.
pub(crate) const EDIT_BATCH_ADVICE: &str =
    "[tokenade] You've made several Edits one-per-turn. Independent edits batch: \
     several files, or several hunks of one file → parallel `Edit` calls in ONE \
     message. (Keep sequential edits when one depends on another's result.)";

pub(crate) fn batching_edit_nudge(parsed: &serde_json::Value) -> Option<String> {
    batching_family_nudge(parsed, "edit", EDIT_BATCH_ADVICE)
}

/// After how many consecutive turns this family speaks up.
///
/// A single place, because a test that primes a streak must be able to ask
/// for the threshold rather than copy it — the first version of the rule
/// below turned red a test that wrote `2` for every family.
pub(crate) fn nudge_at_for(family: &str) -> u32 {
    if family == "edit" {
        2
    } else {
        3
    }
}

pub(crate) fn batching_family_nudge(
    parsed: &serde_json::Value,
    family: &str,
    msg: &str,
) -> Option<String> {
    if hook_disabled_by_env() || std::env::var("TOKENADE_NO_BATCH_NUDGE").is_ok() {
        return None;
    }
    // **The edit advice arrives at the worst moment, and the measurement says
    // so.**
    //
    // Measured over 30 days of real usage: `batch-read` 69 % adherence at
    // 251.0k context tokens, `batch-bash` 44 % at 158.4k, and `batch-edit`
    // **20 % at 284.9k** — the lowest, at the highest context. Context alone
    // does not explain the gap (read and edit both fire above 250k): a
    // sequence of edits is a plan ALREADY settled. Redirecting a read is free;
    // undoing decided edits is not.
    //
    // So we speak one edit earlier, before the sequence closes. This is a
    // HYPOTHESIS, not a given: the control is `batch-read`, which does not
    // move, and the comparison will be made on the next 30-day measurement.
    let nudge_at = nudge_at_for(family);
    let session = crate::hooks::session_id_of(parsed).unwrap_or("nosess");
    let key: String = session
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(40)
        .collect();
    let dir = read_pending_dir();
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join(format!("batch-streak-{family}-{key}"));
    // `streak:last_unix_millis`. Reads landing within 400 ms of the previous
    // one are PARALLEL calls from one message — the harness dispatches those
    // back-to-back (tens of ms apart), while sequential turns need a full
    // model round-trip (≥1 s even in fast sessions). Parallel → the agent
    // is ALREADY batching → RESET the streak. A 2 s threshold was tried
    // first and misread fast sequential turns as parallel: a bench run
    // where the agent read 13 files one-per-turn at ~2 s/turn never got
    // nudged at all (+26% on that task).
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    // `streak:last_unix_millis[:pending_fire]` — pending_fire is the streak
    // value at the last unresolved fire (0 = none); it closes the adherence
    // loop: a parallel burst after a fire books `ok`, a periodic re-fire
    // books `ignored`. Two-field files from older builds parse as pending 0.
    let (prev_streak, last_ts, mut pending, prev_ignored) = std::fs::read_to_string(&p)
        .ok()
        .and_then(|s| {
            let mut it = s.trim().split(':');
            Some((
                it.next()?.parse::<u32>().ok()?,
                it.next().and_then(|t| t.parse::<u64>().ok()).unwrap_or(0),
                it.next().and_then(|t| t.parse::<u32>().ok()).unwrap_or(0),
                it.next().and_then(|t| t.parse::<u32>().ok()).unwrap_or(0),
            ))
        })
        .unwrap_or((0, 0, 0, 0));
    let parallel = last_ts != 0 && now.saturating_sub(last_ts) < 400;
    let streak = if parallel {
        0 // same-message parallel read → agent already batches
    } else {
        prev_streak + 1
    };
    let kind = format!("batch-{family}");
    // Ignored fires in a row. Drives the backoff below; zeroed the moment the
    // steer is followed, and forgotten after a long enough quiet gap.
    let mut ignored_streak = prev_ignored;
    if parallel && pending > 0 {
        // Parallel burst after a fire — the steer was followed. Book once.
        tokenade_core::gain::record_nudge(
            &kind,
            "ok",
            tokenade_core::gain::NudgeMetric::Items(pending as usize),
        );
        pending = 0;
        ignored_streak = 0;
    }
    // A session can run for hours; a backoff earned during one phase of work
    // must not silence the steer for the rest of it. After a long quiet gap
    // the agent is effectively starting again, so the cadence does too.
    const BACKOFF_FORGET_MS: u64 = 45 * 60 * 1_000;
    if last_ts != 0 && now.saturating_sub(last_ts) > BACKOFF_FORGET_MS {
        ignored_streak = 0;
    }
    // Fire at the Nth read, then PERIODICALLY (every NUDGE_EVERY reads) if the
    // one-per-turn pattern persists — a single early nudge gets forgotten
    // (measured: agents stay 100% single-call all session). Periodic-but-sparse
    // keeps steering without spamming: re-state the cheaper batched pattern
    // every ~dozen reads, not every turn. Better nudging, not a heavier prompt.
    const NUDGE_EVERY: u32 = 12;
    // Back OFF on repeated `ignored` rather than escalate the wording (PO,
    // 2026-08-02). A steer that keeps costing ~90 tokens and books `ignored`
    // every time is a pure loss; one that fades is at worst neutral, and
    // harder wording risks reading as scolding. Doubling is bounded at 4x so
    // the steer never disappears outright, and `ignored_streak` is cleared
    // both by compliance and by a long quiet gap — so a session that changes
    // shape gets the early cadence back instead of inheriting a penalty.
    //
    // 2 -> 4 (2026-08-04, PO #36). MEASURED over 22 days / 63 sessions on this
    // install: `batch-bash` fired 2 367 times and was followed 185 times — 8 %.
    // 36 sessions never followed it ONCE and carried 1 135 of those fires, one
    // of them taking 304. The 4x ceiling let a session that had ignored the
    // steer three hundred times keep receiving it at one per 48 calls.
    // `batch-read`, by contrast, is followed 30 % of the time and is untouched
    // by this: `ignored_streak` resets on compliance, so a session that follows
    // the steer never reaches the deeper backoff at all.
    //
    // Raising the ceiling rather than falling silent keeps the ruling of
    // 2026-08-02 intact — the steer still never disappears outright, it just
    // stops being loud at an agent that has proved it will not listen.
    const BACKOFF_MAX_SHIFT: u32 = 4;
    // **And the context-window budget decides the rest.**
    //
    // The cadence above says WHEN a family would like to speak; it did not
    // know what the OTHER families had already said, nor whether its own
    // sentence was still in front of the model. Result measured by the module
    // itself: 2 367 `batch-bash` firings for 185 followed, and one session
    // that received 304. An agent that hears the same sentence over and over
    // ends up commenting on it instead of applying it.
    //
    // `nudge_budget_take` sets the two missing rules — one sentence per family
    // and per window, three nudges in total — and the window closes at
    // COMPACTION, where `reset_session_hints` clears the markers: what the
    // summary carried away can be said again. So the advice does not
    // disappear, it stops being repeated while it is still there.
    let every = NUDGE_EVERY << ignored_streak.min(BACKOFF_MAX_SHIFT);
    let fire = should_fire(streak, pending, nudge_at, every)
        && crate::hooks::read_staging::nudge_budget_take(
            session,
            &kind,
            current_context_tokens(parsed).unwrap_or(0),
            nudge_repeat_distance(parsed),
        );
    if !fire {
        let _ = tokenade_core::paths::write_atomic(
            &p,
            &format!("{streak}:{now}:{pending}:{ignored_streak}"),
        );
        return None;
    }
    if pending > 0 {
        // Re-firing means `every` more one-per-turn calls landed since the
        // last fire — that fire was ignored.
        tokenade_core::gain::record_nudge(
            &kind,
            "ignored",
            tokenade_core::gain::NudgeMetric::Items(streak.saturating_sub(pending) as usize),
        );
        ignored_streak = ignored_streak.saturating_add(1);
    }
    let _ = tokenade_core::paths::write_atomic(
        &p,
        &format!("{streak}:{now}:{streak}:{ignored_streak}"),
    );
    // Personalised stakes: quote the agent's OWN context size (from the
    // transcript's latest usage record) instead of an abstract warning —
    // measured adherence on the generic wording was ~zero (field audit
    // 2026-07-11: 94% → 89% singleton rate around a fire).
    let ctx = current_context_tokens(parsed);
    tokenade_core::gain::record_nudge(
        &kind,
        "fire",
        tokenade_core::gain::NudgeMetric::ContextTokens(ctx.unwrap_or(0) as usize),
    );
    match ctx {
        // Quote the size AND the price class. Those tokens are cache reads,
        // which this product weights at a TENTH of an input token everywhere
        // else it reports a number (see `gain::WEIGHTED_LEGEND_EN`); calling
        // the raw count "tokens you could be saving" overstated the stake by
        // 10x, in the one place a user is being asked to trust our arithmetic.
        Some(t) if t >= 8_000 => Some(format!(
            "{msg} Each extra turn re-sends your ENTIRE context as cache reads — \
             right now ~{}k of them, about {}k tokens' worth at cache-read prices.",
            t / 1_000,
            (t / 10).max(1_000) / 1_000
        )),
        _ => Some(format!(
            "{msg} Each extra turn re-bills the whole context from cache."
        )),
    }
}

/// Whether this call is a nudge fire: the Nth one-per-turn call, then one every
/// `every` calls AFTER the previous fire.
///
/// The gap is measured from `pending` — the streak value at the last unresolved
/// fire — and NOT as a modulo of the absolute streak. That distinction is the
/// whole backoff.
///
/// MEASURED 2026-08-04 on this install: of 2 096 `batch-bash` fires booked
/// `ignored`, **1 976 came exactly 12 calls after the previous one** — the base
/// cadence — even though `every` had already been doubled. `(streak - NUDGE_AT)
/// % every == 0` tests a position on the absolute streak, so raising `every`
/// only removes intermediate multiples that were already fired: after firing at
/// 15 with `every` newly 24, the next position satisfying the modulo is 27, a
/// gap of 12 again. Simulated over 3 000 calls, gaps were `12, 12, 24, 48, 96`
/// where the comment above promises `12, 24, 48, 96` — **every escalation
/// arrived one fire late**, and the extra fire always landed at the cadence the
/// agent had just demonstrated it was ignoring.
///
/// `pending == 0` means nothing is outstanding — either the steer has never
/// fired, or the last one was followed (which resets the streak in the same
/// call). Both are answered by the first-fire rule.
pub(crate) fn should_fire(streak: u32, pending: u32, nudge_at: u32, every: u32) -> bool {
    if pending == 0 {
        // Nothing outstanding: the steer has never fired, or the last one was
        // followed (which resets the streak to 0 in the same call). Either way
        // the first-fire rule answers it.
        //
        // The modulo survives HERE and only here, for state files written by a
        // build that stored fewer fields: those parse with `pending == 0` while
        // the streak is already past the threshold. Firing on `streak >=
        // nudge_at` in that case turns the steer into one nudge PER CALL —
        // caught by `batch_nudge_fires_once_at_threshold`, which truncates the
        // file to two fields exactly as an older build would have left it.
        return streak == nudge_at
            || (streak > nudge_at && (streak - nudge_at).is_multiple_of(every));
    }
    streak.saturating_sub(pending) >= every
}

/// Current context size (tokens) from the transcript's most recent usage
/// record: cache_read + cache_creation + fresh input of the last assistant
/// API call. Tail-reads at most 256 KB of the transcript. `None` on any
/// miss — callers fall back to unquantified wording.
pub(crate) fn current_context_tokens(parsed: &serde_json::Value) -> Option<u64> {
    let path = parsed.get("transcript_path").and_then(|s| s.as_str())?;
    let mut f = std::fs::File::open(path).ok()?;
    use std::io::{Read as _, Seek as _};
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(256 * 1024);
    f.seek(std::io::SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::with_capacity((len - start) as usize);
    f.read_to_end(&mut buf).ok()?;
    let s = String::from_utf8_lossy(&buf);
    for line in s.lines().rev() {
        if !line.contains("cache_read_input_tokens") {
            continue;
        }
        let Ok(ev) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(u) = ev.pointer("/message/usage") else {
            continue;
        };
        let g = |k: &str| u.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        let total =
            g("cache_read_input_tokens") + g("cache_creation_input_tokens") + g("input_tokens");
        if total > 0 {
            return Some(total);
        }
    }
    None
}

/// The session's model, read from the SAME transcript tail as
/// [`current_context_tokens`] — a single file pass for both.
pub(crate) fn current_model(parsed: &serde_json::Value) -> Option<String> {
    let path = parsed.get("transcript_path").and_then(|s| s.as_str())?;
    let mut f = std::fs::File::open(path).ok()?;
    use std::io::{Read as _, Seek as _};
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(256 * 1024);
    f.seek(std::io::SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::with_capacity((len - start) as usize);
    f.read_to_end(&mut buf).ok()?;
    let s = String::from_utf8_lossy(&buf);
    for line in s.lines().rev() {
        let Ok(ev) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(m) = ev.pointer("/message/model").and_then(|m| m.as_str()) {
            if !m.is_empty() {
                return Some(m.to_string());
            }
        }
    }
    None
}

/// **How much context must pass before repeating the SAME sentence.**
///
/// Design rule (2026-09-12): the distance is measured in context tokens and
/// depends on the model's window. A quarter of the window: far enough for the
/// previous sentence to be drowned in what the model has read since, close
/// enough for advice to stay available in a long session.
///
/// The window comes from `estimator_window_for_model`, which fails SMALL on an
/// unknown model (200k). That is the right direction here too: an
/// underestimated window repeats the advice a little early, an overestimated
/// one would silence it for the whole session.
pub(crate) fn nudge_repeat_distance(parsed: &serde_json::Value) -> u64 {
    let model = current_model(parsed).unwrap_or_default();
    tokenade_core::autocompact::estimator_window_for_model(&model) / 4
}

/// Source-line count above which a code file is flagged for refactoring.
pub(crate) const LARGE_FILE_LINE_WARN: usize = 1000;
/// Byte pre-filter: below this, a code file can't plausibly exceed the line
/// threshold (~24 B/line), so we skip the read+count entirely. Keeps the hot
/// Read path cheap — only genuinely large files pay the line count, and only
/// until they've been nudged once.
pub(crate) const LARGE_FILE_BYTE_PREFILTER: u64 = 24_000;

/// Reactive nudge: when the agent Reads a CODE file over ~1000 lines, suggest
/// splitting it into focused modules. Large files are a compounding cost — they
/// re-ship more context on every read/edit and are harder to navigate — and a
/// refactor is usually overdue. Fires at most ONCE per (session, file) so it
/// never spams. Code files only (refactor advice is meaningless for data/logs).
/// Disable with TOKENADE_NO_LARGE_FILE_NUDGE=1.
pub(crate) fn large_file_refactor_nudge(parsed: &serde_json::Value) -> Option<String> {
    if hook_disabled_by_env() || std::env::var("TOKENADE_NO_LARGE_FILE_NUDGE").is_ok() {
        return None;
    }
    // Never on the FIRST read of the file this session: this nudge runs the
    // read natively (one-JSON hook contract), and it targets precisely the
    // BIG files where skipping compaction costs the most (measured: a
    // 1500-line first read went raw for a ~60-token steer). The first read
    // gets staged/compacted; the nudge waits for a subsequent read, whose
    // dedup/diff delivery is cheap anyway.
    {
        let fp = parsed
            .pointer("/tool_input/file_path")
            .and_then(|p| p.as_str())
            .unwrap_or("");
        let session = crate::hooks::session_id_of(parsed).unwrap_or("nosess");
        let skey: String = session
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(40)
            .collect();
        let seen_marker = crate::hooks::read_staging::read_pending_dir().join(format!(
            "seenread-{skey}-{}",
            tokenade_core::dedup_ref::hash_content(fp.as_bytes())
        ));
        if !seen_marker.exists() {
            tokenade_core::platform::ensure_parent_dir(&seen_marker);
            let _ = std::fs::write(&seen_marker, b"1");
            return None;
        }
    }
    let file_path = parsed
        .get("tool_input")
        .and_then(|i| i.get("file_path"))
        .and_then(|p| p.as_str())?;
    let ext = std::path::Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())?;
    // Code files only.
    tokenade_core::code_strip::language_for_extension(ext)?;
    let meta = std::fs::metadata(file_path).ok()?;
    if !meta.is_file() || meta.len() < LARGE_FILE_BYTE_PREFILTER {
        return None;
    }
    // Once per (session, file): cheap marker check before the line count.
    let session = crate::hooks::session_id_of(parsed).unwrap_or("nosess");
    let key = tokenade_core::dedup_ref::hash_content(format!("{session}|{file_path}").as_bytes());
    let dir = read_pending_dir();
    let _ = std::fs::create_dir_all(&dir);
    let marker = dir.join(format!("bigfile-{key}"));
    if marker.exists() {
        return None;
    }
    let content = std::fs::read_to_string(file_path).ok()?;
    let lines = content.lines().count();
    // Mark checked regardless, so a sub-threshold big file isn't re-counted
    // every read this session.
    let _ = std::fs::write(&marker, lines.to_string());
    if lines <= LARGE_FILE_LINE_WARN {
        return None;
    }
    let name = std::path::Path::new(file_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(file_path);
    Some(format!(
        "[tokenade] `{name}` is {lines} lines. Files over ~1000 lines hurt twice: every \
         read/edit re-ships more context (cost), and they're hard to navigate (slower, more \
         errors). Consider splitting it into focused modules — extract cohesive groups of \
         functions into their own files. This is usually a refactor signal worth acting on."
    ))
}
