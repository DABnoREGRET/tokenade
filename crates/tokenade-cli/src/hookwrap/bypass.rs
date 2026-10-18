//! **What the bypass COST.**
//!
//! When the fold did not happen, it must be measured anyway: a silent bypass
//! is an invisible loss, and it is the only number that says whether a
//! reminder is worth pushing. The reminder is capped per session, so that it
//! informs instead of nagging.

use super::*;

/// Claude Code PostToolUse hook entry point.
///
/// Reads the hook input JSON on stdin (containing the tool result), runs the
/// captured Bash output through STS's auto compactor, and emits the modified
/// result back on stdout. The exact JSON contract follows Claude Code's hook
/// spec — when the spec or the input shape doesn't match what we expect, we
/// emit no decision (pass-through) so we never break the user's session.
///
/// Hook input shape (current Claude Code):
/// ```json
/// {
///   "tool_name": "Bash",
///   "tool_input": {"command": "<cmd>", ...},
///   "tool_response": {"stdout": "...", "stderr": "...", ...}
/// }
/// ```
/// Output we emit:
/// ```json
/// {
///   "hookSpecificOutput": {
///     "hookEventName": "PostToolUse",
///     "additionalContext": "<compacted output>"
///   }
/// }
/// ```
/// P2b — given a bypassed command line, return the inner argv the agent
/// actually ran: strips a leading env-prefix
/// (`TOKENADE_HOOK_DISABLED=1 cargo test`) and/or a
/// `tokenade raw|bypass|noproxy` prefix. `None` when the line isn't a
/// bypass form (or nothing remains after the prefix).
/// Single construction point for the delivered payload — TWO identical
/// literals used to sit in different functions and a content-anchored
/// edit landed in the wrong one (live incident during the R-round).
pub(crate) fn combined_output(stdout: &str, stderr: &str) -> String {
    format!("{stdout}{stderr}")
}

pub(crate) fn bypass_inner_argv(cmd: &str) -> Option<Vec<String>> {
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    // Skip the env-assignment prefix, noting whether it carries a
    // truthy disable var (same grammar as cmdline_requests_hook_bypass).
    let mut i = 0;
    let mut env_disable = false;
    while i < tokens.len() && tokens[i].contains('=') {
        if let Some((key, val)) = tokens[i].split_once('=') {
            if matches!(key, "TOKENADE_HOOK_DISABLED" | "STS_HOOK_DISABLED")
                && matches!(val.to_ascii_lowercase().as_str(), "1" | "true" | "yes")
            {
                env_disable = true;
            }
        }
        i += 1;
    }
    let rest = &tokens[i..];
    let prog = rest
        .first()
        .map(|p| p.rsplit(['/', '\\']).next().unwrap_or(p))?;
    if prog == "tokenade" && matches!(rest.get(1).copied(), Some("raw" | "bypass" | "noproxy")) {
        let inner: Vec<String> = rest[2..].iter().map(|s| s.to_string()).collect();
        return (!inner.is_empty()).then_some(inner);
    }
    if env_disable && !rest.is_empty() {
        return Some(rest.iter().map(|s| s.to_string()).collect());
    }
    None
}

/// #299 — is this fold worth BOOKING, asked in one place.
///
/// The rule was spelled twice, and the two had already drifted. The Bash
/// delivery path carries `!already_folded` with a comment explaining exactly
/// why — "hand the same hook a folded payload a second authority CAN still
/// shave 5 % off and it books a row for bytes the first authority already
/// credited" — and the lost-savings path, which books rows into the same
/// ledger, never learned the clause. The comment even records the asymmetry:
/// "`copilot_fold` has asked this question since it was written; the Bash path
/// never did." One of the three had it, then two, and the third was still
/// waiting.
///
/// The three numbers are here and nowhere else: a fold must SHRINK, save at
/// least 5 %, and save at least 10 tokens absolutely.
pub(crate) fn fold_is_worth_booking(s: &tokenade_core::Savings, already_folded: bool) -> bool {
    const MIN_PERCENT: f64 = 5.0;
    const MIN_ABSOLUTE: usize = 10;
    !already_folded
        && s.tokens_after < s.tokens_before
        && s.percent() >= MIN_PERCENT
        && s.tokens_before.saturating_sub(s.tokens_after) >= MIN_ABSOLUTE
}

/// P2b — measure the savings LOST to a hook bypass and append them to
/// the dedicated `~/.tokenade/bypass_lost.jsonl` ledger (NEVER
/// `gain.jsonl` — see `gain::lost_ledger_path` for why). Runs the same
/// `filter_command` the hook would have applied, against the output
/// the agent actually received, so the number is "what this exact
/// bypass cost", not an estimate. Same ≥5 % / ≥10-token noise floor as
/// the normal emit path: a bypass that would have saved nothing is a
/// free bypass and stays out of the panel.
/// True at most ONCE per session, so a delivered nudge cannot flood the very
/// context it exists to protect. Same marker directory and the same sanitised
/// session key the batching nudge uses; a session id we cannot read falls back
/// to one shared key, which errs toward saying it less rather than more.
pub(crate) fn bypass_nudge_is_new_for_session(parsed: &serde_json::Value) -> bool {
    let session = crate::hooks::session_id_of(parsed).unwrap_or("nosess");
    let key: String = session
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(40)
        .collect();
    let dir = crate::hooks::read_staging::read_pending_dir();
    let _ = std::fs::create_dir_all(&dir);
    let marker = dir.join(format!("bypass-nudge-{key}"));
    if marker.exists() {
        return false;
    }
    std::fs::write(&marker, b"1").is_ok()
}

pub(crate) fn record_bypass_lost_savings(
    cmd: &str,
    parsed: &serde_json::Value,
    started: std::time::Instant,
) {
    let Some(inner_argv) = bypass_inner_argv(cmd) else {
        return;
    };
    let response = parsed
        .get("tool_response")
        .or_else(|| parsed.get("toolResponse"));
    let stdout = response
        .and_then(|r| r.get("stdout"))
        .and_then(|s| s.as_str())
        .unwrap_or("");
    let stderr = response
        .and_then(|r| r.get("stderr"))
        .and_then(|s| s.as_str())
        .unwrap_or("");
    let combined = combined_output(stdout, stderr);
    if combined.is_empty() {
        return;
    }
    // Same boundary `wrap` uses, for the same reason: a compactor that blows up
    // must cost the agent nothing. See `crash_guard::arm_salvage` — with
    // `panic = "abort"` the salvage is what actually returns the bytes.
    let r = {
        let _salvage = tokenade_core::crash_guard::arm_salvage(&combined, 0);
        filter::filter_command_fail_open(&inner_argv, &combined)
    };
    // #299 — the shared rule, `!already_folded` included. This path books into
    // the same ledger as the delivery path and used to ask a weaker question,
    // so an output a first authority had already folded could be booked a
    // second time as a bypass loss.
    let already_folded = tokenade_core::fold_marker::already_folded(&combined);
    if !fold_is_worth_booking(&r.savings, already_folded) {
        return;
    }
    let lost = r
        .savings
        .tokens_before
        .saturating_sub(r.savings.tokens_after);
    let safe_label = redact::redact(&redact::redact_argv(&inner_argv).join(" "));
    gain_log::record_lost("hook", "bypass_lost", &safe_label, r.savings);
    tokenade_core::debug_log::log(
        "hookwrap",
        "bypass_lost",
        serde_json::json!({
            "command": tokenade_core::caps::truncate_at_char_boundary(&safe_label, 200),
            "filter": r.filter,
            "tokens_lost": lost,
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }),
    );
}

/// The values every stage of one PostToolUse invocation reads.
///
/// `parsed`, `cmd`, `combined`, `safe_cmd_for_log` and `started` were threaded
/// through three separate blocks of [`cmd_hookwrap`] as locals. Four of the
/// five appear in all three, which is the definition of state rather than
/// parameters — see that function's verdict for the count.
/// #287 — say that the HOST cut this result, which it marks nowhere.
///
/// Returns `true` when a note was emitted, so a caller about to return may stop
/// rather than print a second decision. Called from BOTH terminal points of the
/// Bash PostToolUse handler: the below-threshold passthrough and the log-only
/// tail. The passthrough is not the lesser of the two — it is exactly the
/// population this note exists for, since output the fold cannot improve is
/// output that reaches the model at full length.
///
/// Placed at the ends, never early: a first wiring returned before the fold
/// accounting and `the_plain_hookwrap_agents_fold_and_credit_themselves`
/// caught it — a fold that happened wrote no row.
pub(crate) fn emit_host_cut_advisory(
    stdout: &str,
    safe_cmd_for_log: &str,
    started: std::time::Instant,
) -> bool {
    let Some(note) = tokenade_core::agent_caps::host_cut_advisory(
        tokenade_core::detect::from_env().as_str(),
        tokenade_core::agent_caps::Action::Command,
        stdout,
    ) else {
        return false;
    };
    let decision = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PostToolUse",
            "additionalContext": note,
        }
    });
    log_emitted_additional_context("hookwrap-host-cut", "PostToolUse", &note);
    println!("{decision}");
    tokenade_core::debug_log::log(
        "hookwrap",
        "host_cut_advisory",
        serde_json::json!({
            "command": safe_cmd_for_log,
            "stdout_chars": stdout.chars().count(),
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }),
    );
    true
}
