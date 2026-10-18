//! **A tool call is an API call** — the clock of an agent whose transcript
//! shows no call-by-call timeline (no prompt hook, or no readable timeline).
//! Split out of `credit.rs` at the 1.2.0 ship, where it was born.

use super::*;
pub(crate) fn ticked_path(safe: &str) -> std::path::PathBuf {
    tokenade_core::platform::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".tokenade")
        .join("sessions")
        .join(format!("{safe}-ticked"))
}

pub(crate) fn dir_ticked(safe: &str) -> bool {
    ticked_path(safe).exists()
}

/// **A tool call is an API call** — the clock of an agent whose transcript
/// shows no call-by-call timeline. The charge for tokenade's injected context
/// used to advance 12 re-reads per USER message: an autonomous run, one
/// message driving a hundred calls, was charged for 12. Each tool call now
/// charges the API calls a tool call stands for ([`CALLS_PER_TOOL_CALL`]) of
/// everything injected and still resident, up to the compaction horizon —
/// the same count its folds are credited. Claude Code and Codex are counted exactly
/// from their transcripts and never tick.
///
/// The tokenade block in the agent's rules file is in that context too: on an
/// agent with no `SessionStart` hook it was never charged at all. Booked once
/// per session, from the file as written.
pub(crate) fn record_tool_tick(parsed: &serde_json::Value) {
    use tokenade_core::detect::Agent;
    let agent = tokenade_core::detect::from_env();
    if matches!(agent, Agent::ClaudeCode | Agent::Codex | Agent::Unknown) {
        return;
    }
    let Some(sid) = crate::hooks::session_id_of(parsed) else {
        return;
    };
    let threshold = assumed_threshold(parsed, sid);
    let transcript = parsed
        .get("transcript_path")
        .and_then(|t| t.as_str())
        .filter(|t| !t.is_empty());
    // A bridge that ticks once per API REQUEST (Cline's message builder) and
    // not per tool call: one tick is one call.
    let calls = if parsed["per_call"] == true {
        1.0
    } else {
        CALLS_PER_TOOL_CALL
    };
    tool_tick(sid, agent, threshold, transcript, calls);
}

/// `tokenade session-tick --session <id> [--agent <slug>]` — the same tick,
/// for a plugin bridge (opencode, Kilo, Pi, OpenClaw, Cline, Hermes) whose
/// host runs no hook process of ours on a tool call. Always exits 0.
pub(crate) fn cmd_session_tick(rest: &[String]) -> i32 {
    let arg = |k: &str| {
        rest.iter()
            .position(|a| a == k)
            .and_then(|i| rest.get(i + 1))
            .cloned()
    };
    if let Some(a) = arg("--agent") {
        std::env::set_var("TOKENADE_AGENT", a);
    }
    let Some(sid) = arg("--session")
        .or_else(|| std::env::var("TOKENADE_SESSION").ok())
        .filter(|s| !s.trim().is_empty())
    else {
        return 0;
    };
    let skey: String = sid
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(40)
        .collect();
    tokenade_core::session::set_session_key_authoritative(&skey);
    // `--reset`: the host compacted or cleared the conversation.
    if rest.iter().any(|a| a == "--reset") {
        super::note_context_reset(&sid);
        return 0;
    }
    record_tool_tick(&serde_json::json!({
        "session_id": sid,
        "transcript_path": arg("--transcript"),
        "per_call": rest.iter().any(|a| a == "--per-call"),
    }));
    0
}

pub(crate) fn tool_tick(
    sid: &str,
    agent: tokenade_core::detect::Agent,
    threshold: u64,
    transcript: Option<&str>,
    calls_per_tick: f64,
) {
    let safe: String = sid
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(40)
        .collect();
    if safe.is_empty() {
        return;
    }
    book_loaded_scaffold_once(sid, agent);
    let marker = ticked_path(&safe);
    if !marker.exists() {
        tokenade_core::platform::ensure_parent_dir(&marker);
        let _ = std::fs::write(&marker, b"1");
    }
    // Exact where the agent keeps its calls on disk (opencode, Kilo, pi): the
    // same count as Claude Code's, call by call, up to the real compaction.
    if let Some(tl) = bridge_timeline(sid, agent, threshold, transcript) {
        credit_exact(&safe, &fold_log_of(sid), &tl, "tool-tick");
        charge_exact(&safe, &tl, "tool-tick");
        return;
    }
    // Counted exactly before (its cursor exists), unreadable this once — a
    // locked database, a step not yet written: the proxy would count again
    // calls the exact count already holds. Wait for the next tick.
    if tokenade_core::session::sessions_dir()
        .join(format!("{safe}-rebill-exact-v2"))
        .exists()
    {
        return;
    }
    let proxy = RereadProxy {
        calls_per_turn: calls_per_tick,
        ..RereadProxy::measured(threshold)
    };
    rebill_resident(&safe, proxy, "tool-tick");
    // No prompt hook: nothing ever ran the credit for this session's folds.
    // The tool call is its turn clock, at the calls a tool call stands for.
    if !has_prompt_hook(agent) {
        session_state::bump_session_turn(&serde_json::json!({ "session_id": sid }));
        compound_by_proxy(sid, threshold, proxy, "tool-tick");
    }
}

/// The call timeline of a bridge agent that stores its API calls locally.
fn bridge_timeline(
    sid: &str,
    agent: tokenade_core::detect::Agent,
    threshold: u64,
    transcript: Option<&str>,
) -> Option<tokenade_core::gain::Timeline> {
    use tokenade_core::detect::Agent;
    let home = tokenade_core::platform::home_dir()?;
    let (stl, model) = match agent {
        Agent::OpenCode => tokenade_core::agent_opencode::session_timeline(&home, sid)?,
        Agent::KiloCode => tokenade_core::agent_kilo::session_timeline(&home, sid)?,
        Agent::Pi => {
            tokenade_core::agent_pi::session_timeline_from_file(std::path::Path::new(transcript?))?
        }
        _ => return None,
    };
    let model = model.unwrap_or_default();
    Some(
        tokenade_core::gain::Timeline {
            calls: stl.calls,
            compactions: stl.compactions.iter().map(|&(t, _)| t).collect(),
            threshold,
            billed_per_estimated: tokenade_core::tokenizer::claude_tokens_per_estimated(&model),
            rewrites: stl.rewrites,
            rewrite_weight: tokenade_core::gain::rewrite_weight(&model),
        }
        .with_inferred_compactions(),
    )
}

/// API calls one tool call stands for, on both sides: the median.
/// MEASURED 2026-10-06, 30 days of local Claude Code transcripts (173 sessions
/// of 20+ tool calls, 150 295 tool calls): API calls that ran a tool, per tool
/// call — p25 0.44, median **0.69**, p75 0.92 (several tools per call). The
/// credit took the p25 and the charge one call per tool call; one quantity,
/// one count.
const CALLS_PER_TOOL_CALL: f64 = 0.69;

/// The agents whose `UserPromptSubmit` hook runs the per-message accounting.
fn has_prompt_hook(agent: tokenade_core::detect::Agent) -> bool {
    use tokenade_core::detect::Agent;
    matches!(
        agent,
        Agent::ClaudeCode | Agent::Codex | Agent::Droid | Agent::Copilot
    )
}

/// The scaffold this agent's rules file loads into every session, booked once
/// per session as injected overhead — `SessionStart` does it for the agents
/// that have one, under the same marker.
fn book_loaded_scaffold_once(sid: &str, agent: tokenade_core::detect::Agent) {
    let Some(home) = tokenade_core::platform::home_dir() else {
        return;
    };
    let marker = home.join(".tokenade").join("sessions").join(format!(
        "{}-overhead",
        session_state::sanitize_session_id(sid)
    ));
    if marker.exists() {
        return;
    }
    tokenade_core::platform::ensure_parent_dir(&marker);
    let _ = std::fs::write(&marker, b"1");
    let cwd = std::env::current_dir().ok();
    let cost = tokenade_core::project_scaffold::loaded_block_tokens(agent, &home, cwd.as_deref());
    if cost > 0 {
        gain_log::record_estimated(
            "tool-tick",
            "scaffold_overhead",
            "",
            "overhead",
            tokenade_core::Savings::new(cost, 0),
        );
    }
}
