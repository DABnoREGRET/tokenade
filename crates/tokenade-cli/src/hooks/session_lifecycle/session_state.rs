//! Per-session counters that outlive a single hook call.
//!
//! Turn number, bytes delivered, context pressure, and the style marker —
//! all keyed by session id, all on disk because each hook is a new process.

use super::*;

/// The ONE way a session id becomes a filename: ASCII alphanumerics only,
/// bounded at 40, `session` when nothing survives.
///
/// It used to keep `-` and `_` and allow 80 characters, while eight other call
/// sites spelled the same idea inline as `.filter(is_ascii_alphanumeric).take(40)`.
/// Two spellings would be harmless if every caller saw the same id — they do
/// not. `TOKENADE_SESSION` is already hyphen-free, so a writer reading it got
/// `67caa…ce79`; a hook payload's `session_id` is the full UUID, so a writer
/// reading THAT got `67caa6a3-1a47-…`. Same session, two filenames.
///
/// MEASURED 2026-08-25 on this machine: 14 of 39 sessions had state under BOTH
/// spellings, split down the middle — `turn` and `compactions.jsonl` under the
/// hyphenated name, `delivered`, `compact-ts`, `sl-tally.json` and the overhead
/// counters under the bare one. `record_compounding_estimate` reads `turn` and
/// `delivered` together, so its `delivered` was ALWAYS 0: the counterfactual
/// context it compares against the auto-compact horizon collapsed to the
/// resident folds alone, the synthetic reset almost never fired, and compounding
/// credit kept accruing past the point a real context would have been squashed.
/// 189 572 tokens of counterfactual went missing on one session against a
/// 200 000-token horizon; 206 M across the fourteen.
pub(crate) fn sanitize_session_id(id: &str) -> String {
    let cleaned: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(40)
        .collect();
    if cleaned.is_empty() {
        "session".to_string()
    } else {
        cleaned
    }
}

/// A session's state written under the PREVIOUS spelling, if any.
///
/// Only the two files that carry history across turns need this: an in-flight
/// session must not forget its recorded compactions the moment the naming is
/// unified, because forgetting a reset is precisely what over-credits. Counters
/// that are re-derived every turn need no fallback.
pub(crate) fn legacy_session_id(id: &str) -> Option<String> {
    let legacy: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(80)
        .collect();
    (!legacy.is_empty() && legacy != sanitize_session_id(id)).then_some(legacy)
}

pub(crate) fn session_key_from_stdin(stdin_json: Option<&str>) -> String {
    stdin_json
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|v| crate::hooks::session_id_of(&v).map(String::from))
        .unwrap_or_else(|| "nosess".into())
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(40)
        .collect()
}

/// Per-session marker so the style preamble isn't re-injected every turn.
/// File `~/.tokenade/sessions/<id>-style` holds the last-injected effective
/// style id. `style_already_active` is true when it already equals `effective`
/// (skip the emit); `mark_style_active` records the new value after an emit.
/// The PreCompact hook deletes the file so the instruction is re-stated once
/// the transcript is summarised.
pub(crate) fn session_style_marker_path(parsed: &serde_json::Value) -> Option<std::path::PathBuf> {
    let session_id = crate::hooks::session_id_of(parsed).unwrap_or("");
    if session_id.is_empty() {
        return None;
    }
    let home = tokenade_core::platform::home_dir()?;
    Some(
        home.join(".tokenade")
            .join("sessions")
            .join(format!("{}-style", sanitize_session_id(session_id))),
    )
}

pub(crate) fn style_already_active(
    parsed: &serde_json::Value,
    effective: tokenade_core::style::Style,
) -> bool {
    let Some(p) = session_style_marker_path(parsed) else {
        return false;
    };
    std::fs::read_to_string(&p)
        .map(|s| s.trim() == effective.as_str())
        .unwrap_or(false)
}

pub(crate) fn mark_style_active(
    parsed: &serde_json::Value,
    effective: tokenade_core::style::Style,
) {
    if let Some(p) = session_style_marker_path(parsed) {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&p, effective.as_str());
    }
}

/// Clear the per-session style marker (PreCompact): the summariser squashes
/// the previously-injected preamble out of context, so it must be re-stated.
pub(crate) fn clear_session_style_marker(session_id: &str) {
    if session_id.is_empty() {
        return;
    }
    if let Some(home) = tokenade_core::platform::home_dir() {
        let p = home
            .join(".tokenade")
            .join("sessions")
            .join(format!("{}-style", sanitize_session_id(session_id)));
        let _ = std::fs::remove_file(p);
    }
}

/// 0.6.4 — credit the per-turn estimates (output-lean + native batching) from
/// the session transcript in ONE read. Both are delta-based and keyed by
/// `session_id` (state files under `~/.tokenade/sessions/`), so re-firing
/// never double-counts and no session-end hook is needed — UserPromptSubmit
/// credits each prior turn.
///   - output-lean: the scaffold's "keep output lean" rules cut output by ~9%
///     (calibrated, see [`coefficients::output_lean_ratio`]); we apply that
///     ratio to produced output. `class=output`.
///   - batching: tokenade only nudges (zero-MCP — Claude Code batches), so the
///     `Σ sᵢ·(K−i)` avoided re-sends are read back from the transcript.
///     `class=batching`.
/// Single transcript read (`session_estimate`) — the hook runs every turn and
/// re-reading a multi-MB transcript twice per prompt would add latency.
/// How much a cumulative counter has grown since the last turn, or `None` when
/// nothing may be credited.
///
/// The cursor is the meter's zero. **When it is missing and the counter is
/// already non-zero, the backlog is NOT credited** — it is adopted as the new
/// zero. There are two real ways to arrive there and neither is work tokenade
/// did: tokenade installed part-way through a conversation, and a session left
/// idle past the state TTL (`gain::SESSIONS_TTL_SECS`) then resumed, whose
/// earlier turns were already credited under the cursor that has since been
/// swept. Reading a missing cursor as zero bills a whole transcript in one
/// turn — MEASURED on a real ledger: three single events implying more than a
/// million output tokens each, 14.7 % of the class between them.
///
/// A genuinely new session pays nothing for this: `UserPromptSubmit` fires
/// BEFORE the turn produces anything, so its first cumulative is 0 and the
/// adopted zero is 0.
///
/// Adopting a zero WRITES the cursor; reporting growth does not. That is
/// deliberate: the caller advances the cursor only once it has actually
/// recorded, so a delta too small to round to one token accumulates across
/// turns instead of being dropped.
pub(crate) fn cursor_growth(state: &std::path::Path, cumulative: u64) -> Option<u64> {
    let known = std::fs::read_to_string(state)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok());
    let Some(prev) = known else {
        if let Some(parent) = state.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(state, cumulative.to_string());
        return None;
    };
    (cumulative > prev).then(|| cumulative - prev)
}

/// Context-pressure factor from the session's turn depth. Under 40 turns
/// (every THOL bench task, most short sessions) → 1.0: benched behavior is
/// untouched. Deeper sessions tighten the stash thresholds progressively —
/// inline weight late in a session re-bills on every remaining turn.
/// TOKENADE_ADAPTIVE_THRESHOLDS=0 disables.
pub(crate) fn session_pressure_factor(session_id: &str) -> f64 {
    if std::env::var("TOKENADE_ADAPTIVE_THRESHOLDS")
        .map(|v| v == "0")
        .unwrap_or(false)
    {
        return 1.0;
    }
    let Some(home) = tokenade_core::platform::home_dir() else {
        return 1.0;
    };
    let safe = sanitize_session_id(session_id);
    let turn = std::fs::read_to_string(
        home.join(".tokenade")
            .join("sessions")
            .join(format!("{safe}-turn")),
    )
    .ok()
    .and_then(|t| t.trim().parse::<u64>().ok())
    .unwrap_or(0);
    let turn_factor: f64 = match turn {
        0..=39 => 1.0,
        40..=69 => 0.75,
        70..=99 => 0.5,
        _ => 0.35,
    };
    // Volume axis (adaptive-filter steal): delivered tool-output bytes ≈
    // 4 bytes/token. A 3-turn session that already delivered 200k+ tokens
    // is under MORE context pressure than a 50-turn chatty one. Thresholds
    // deliberately high so short benches (< ~160k delivered tokens) keep
    // factor 1.0 — same guarantee as the turn axis.
    let delivered_tokens = session_delivered_bytes(
        &home
            .join(".tokenade")
            .join("sessions")
            .join(format!("{safe}-delivered")),
    ) / 4;
    let volume_factor = match delivered_tokens {
        0..=159_999 => 1.0,
        160_000..=299_999 => 0.75,
        300_000..=499_999 => 0.5,
        _ => 0.35,
    };
    turn_factor.min(volume_factor)
}

/// Export the factor for THIS hook process (read staging thresholds run
/// in-process) — and callers embed it in rewritten wrap commands so the
/// child process sees it too.
pub(crate) fn export_pressure(parsed: &serde_json::Value) -> f64 {
    let sid = crate::hooks::session_id_of(parsed).unwrap_or("");
    let f = if sid.is_empty() {
        1.0
    } else {
        session_pressure_factor(sid)
    };
    if f < 1.0 {
        std::env::set_var("TOKENADE_PRESSURE", format!("{f:.2}"));
    }
    f
}

pub(crate) fn bump_session_turn(parsed: &serde_json::Value) {
    let session_id = crate::hooks::session_id_of(parsed).unwrap_or("");
    if session_id.is_empty() {
        return;
    }
    let Some(home) = tokenade_core::platform::home_dir() else {
        return;
    };
    let dir = home.join(".tokenade").join("sessions");
    let p = dir.join(format!("{}-turn", sanitize_session_id(session_id)));
    let turn = std::fs::read_to_string(&p)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0)
        + 1;
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(&p, turn.to_string());
}

/// Cumulative DELIVERED tool-output bytes for a session — the volume-based
/// pressure signal (SupervisorAgent-inspired LLM-free adaptive filter): a
/// session can be few turns yet context-fat (one giant log).
///
/// **Append-only, one decimal per line** — as the doc above this function has
/// always claimed and the code did not do. It ran a read-modify-write while
/// being called from `cmd_hookwrap`, which is ONE PROCESS PER TOOL CALL: an
/// agent issuing parallel Bash calls has several of them racing on this file
/// and the losers' bytes vanish. MEASURED with 20 concurrent hooks on one
/// session: a sibling counter built the same way recorded 13. Under-counting
/// here makes a context-fat session look light, so the adaptive thresholds
/// that exist to bound late-session weight never tighten.
///
/// The neighbouring compaction bank already documents the rule this now
/// follows: an append below `PIPE_BUF` is atomic, so concurrent writers can
/// neither lose an update nor tear a line.
///
/// Reading is `sum of lines`, which also makes the migration free: a file left
/// by an older build holds a single integer, and one line summing to itself is
/// that same number.
pub(crate) fn bump_session_delivered(session_id: &str, bytes: usize) {
    use std::io::Write;
    if session_id.is_empty() || bytes == 0 {
        return;
    }
    let Some(home) = tokenade_core::platform::home_dir() else {
        return;
    };
    let dir = home.join(".tokenade").join("sessions");
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join(format!("{}-delivered", sanitize_session_id(session_id)));
    // Single-write append: `writeln!(f, "{bytes}")` is two `write()` calls, and
    // these counters are appended by concurrent hook PROCESSES. Interleaved
    // digits produce a line that still parses as a number and is silently
    // wrong — which is exactly the failure this counter has already had once
    // ("a context-heavy session could look light"). See
    // `platform::append_line`.
    tokenade_core::platform::append_line(&p, &bytes.to_string());
}

/// Total bytes delivered so far in this session — the sum of every appended
/// record. Tolerates the single-integer file older builds wrote.
pub(crate) fn session_delivered_bytes(path: &std::path::Path) -> u64 {
    std::fs::read_to_string(path)
        .map(|s| s.lines().filter_map(|l| l.trim().parse::<u64>().ok()).sum())
        .unwrap_or(0)
}

/// Read one of a session's numeric counters, whatever shape it is stored in.
///
/// Two faults lived in the two hand-written copies of this read, and they were
/// independent, so fixing either alone changed nothing:
///
/// * the FILENAME came from whichever id that caller happened to hold, and the
///   two spellings disagreed (see [`sanitize_session_id`]);
/// * the PARSE was `trim().parse::<u64>()`, which reads a single-value file and
///   fails outright on an append-only one. `-turn` is single-valued; `-delivered`
///   is a log of per-turn byte counts, 518 lines on the session this was measured
///   on. Summing the lines is correct for both — a one-line file sums to itself.
///
/// The consequence was silent and one-directional: `delivered` came back 0, so
/// the counterfactual context size that decides when a session WOULD have been
/// auto-compacted lost its largest term, the synthetic reset stopped firing, and
/// compounding savings kept accruing past the horizon. It over-credited.
pub(crate) fn session_counter(dir: &std::path::Path, session_id: &str, name: &str) -> u64 {
    let sum = |k: &str| -> Option<u64> {
        let p = dir.join(format!("{k}-{name}"));
        // One reader, not a second copy of the same three lines. They WERE two
        // copies, and that cost a mutation test its meaning: mutating "the"
        // line-summing read hit the twin instead, which had no test of its own,
        // so the mutant passed and the guard looked toothless when it was not.
        p.exists().then(|| session_delivered_bytes(&p))
    };
    sum(&sanitize_session_id(session_id))
        .or_else(|| legacy_session_id(session_id).as_deref().and_then(sum))
        .unwrap_or(0)
}

/// Remember which model is running this session.
///
/// Written by the statusline, which the host calls every turn with `model.id`
/// in its context — the one place that is TOLD. Read by the compounding
/// estimator, whose hook payload is not. Best-effort and idempotent: a rewrite
/// of the same bytes is the common case, and `write_atomic` gives the file back
/// unchanged when nothing differs.
pub(crate) fn note_session_model(session_id: &str, model_id: &str) {
    if session_id.is_empty() || model_id.is_empty() {
        return;
    }
    let Some(home) = tokenade_core::platform::home_dir() else {
        return;
    };
    let dir = home.join(".tokenade").join("sessions");
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join(format!("{}-model", sanitize_session_id(session_id)));
    // Only rewrite on a change: the statusline runs at a few Hz and this file
    // is otherwise constant for the life of the session.
    if std::fs::read_to_string(&p).ok().as_deref().map(str::trim) == Some(model_id) {
        return;
    }
    let _ = tokenade_core::paths::write_atomic(&p, model_id);
}

/// The model this session is running, if anything has been able to say.
///
/// Empty when nothing recorded one — the caller keeps its conservative
/// default, which is the right answer for an unknown model and the wrong one
/// only when we could have known better.
pub(crate) fn session_model(session_id: &str) -> String {
    let Some(home) = tokenade_core::platform::home_dir() else {
        return String::new();
    };
    let _ = &home;
    // A single spelling of "where this session's model is written": the
    // ledger reads it too, to stamp every line as it is written.
    let read = tokenade_core::gain::session_model_for;
    read(&sanitize_session_id(session_id))
        .or_else(|| legacy_session_id(session_id).as_deref().and_then(read))
        .unwrap_or_default()
}
