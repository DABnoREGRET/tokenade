//! `tokenade hookwrap` PostToolUse dispatcher + bypass helpers + read/edit
//! post-tool handlers. Extracted from main.rs (refactor split).
#![allow(unused_imports)]
use crate::hooks::*;
use crate::install::*;
use crate::wrap::*;
use crate::*;
mod bypass;
mod copilot;
mod dispatch;
mod read_hook;
pub(crate) use bypass::*;
pub(crate) use copilot::*;
pub(crate) use dispatch::*;
pub(crate) use read_hook::*;

/// Register tokenade with the detected agent by writing/merging an MCP config.
/// GitHub Copilot CLI `postToolUse` hook entry point.
///
/// Copilot is the one non-Claude agent whose post-tool hook can REPLACE the
/// tool result the model sees (docs.github.com hooks-reference): the hook
/// reads `toolResult.textResultForLlm` on stdin and returns
/// `{"modifiedResult":{"resultType":"success","textResultForLlm":<folded>}}`
/// on stdout. Copilot only fires `postToolUse` on SUCCESSFUL tools (failures
/// go to `postToolUseFailure`), so there is no error-preservation concern
/// here. Fail-open: any shape we don't recognize, or no net saving, prints
/// nothing → Copilot keeps the raw result unchanged.
/// Redact-only PostToolUse replacement for the Bash channel (SECURITY).
/// Scans the ALREADY-CAPTURED stdout/stderr for secret patterns; when any
/// match, emits `updatedToolOutput` with the same response shape and ONLY
/// the two streams redacted (all sibling fields ride through verbatim).
/// Returns `Some(0)` after emitting — the caller must stop, one
/// hookSpecificOutput line per invocation. `None` = clean output, proceed.
/// Claude Code < 2.1.121 ignores `updatedToolOutput` — fail-open by
/// protocol, same posture as hookwrap-websearch-post.
/// Serve the fold for a command the PreToolUse path refused to rewrite.
///
/// Four questions, each asked of the module that already owns it rather than
/// re-decided here:
///
/// * did PreToolUse decline this command for SAFETY? —
///   `bash_pretool::bash_segment_excluded_from_wrap`, the same declarer the
///   rewrite paths consult. A command the agent bypassed on purpose
///   (`tokenade raw`) is not in that set and stays untouched.
/// * does this host honour the field? — `claude_supports_updated_tool_output`,
///   the probe the installer already uses to decide whether to install the
///   WebSearch hook at all. On a host that ignores it we emit nothing, which
///   is exactly today's behaviour.
/// * are the folded bytes recoverable? — `attach_drop_recovery`, the same
///   function `wrap` calls, so `tokenade expand-ref <hash>` works on a result
///   folded here just as it does on one folded at source.
/// * is the fold worth a replacement at all? — see `MUST_SAVE_PCT`.
fn emit_folded_bash_output_if_pretool_declined(
    parsed: &serde_json::Value,
    combined: &str,
    r: &tokenade_core::filter::FilterResult,
    savings_pct: f64,
    worth_emitting: bool,
    started: std::time::Instant,
) -> Option<i32> {
    let updated = declined_fold_replacement(parsed, combined, r, worth_emitting)?;
    let served_bytes = updated
        .pointer("/stdout")
        .and_then(|v| v.as_str())
        .map(str::len)
        .unwrap_or(0);
    println!(
        "{}",
        serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "updatedToolOutput": updated,
            }
        })
    );
    tokenade_core::debug_log::log(
        "hookwrap",
        "pretool_declined_fold_served",
        serde_json::json!({
            "filter": r.filter,
            "input_bytes": combined.len(),
            "served_bytes": served_bytes,
            "savings_pct": savings_pct,
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }),
    );
    Some(0)
}

/// The DECISION, separated from the printing so it can be measured. The
/// reason: a rule fused to its output site is asserted on
/// what the terminal shows, which is not the same thing.
fn declined_fold_replacement(
    parsed: &serde_json::Value,
    combined: &str,
    r: &tokenade_core::filter::FilterResult,
    worth_emitting: bool,
) -> Option<serde_json::Value> {
    // ONE question, asked once. `worth_emitting` is `fold_is_worth_booking`,
    // which already consults `already_folded` — so a second
    // `|| already_folded` here is a copy of a rule, not a rule. Both were
    // tried and both were removed on the mutation's own evidence: deleting the
    // 50 % bar stayed green (it bought nothing, and it could not buy safety
    // either, since every byte the fold drops is reachable through the
    // recovery pointer attached below), and deleting `|| already_folded`
    // stayed green because the declarer had already answered it.
    if !worth_emitting {
        return None;
    }
    let cmd = parsed
        .pointer("/tool_input/command")
        .and_then(|v| v.as_str())?;
    if !crate::hooks::bash_pretool::bash_segment_excluded_from_wrap(cmd) {
        return None;
    }
    if !crate::install::capability::claude_supports_updated_tool_output() {
        return None;
    }
    let dropped = r
        .savings
        .tokens_before
        .saturating_sub(r.savings.tokens_after);
    // The bytes came from a shell command, which is what `Shell` names — the
    // same hint `wrap` passes for its own stdout.
    let hint = tokenade_core::disclose::SourceHint::Shell;
    let served = crate::wrap::nudges::attach_drop_recovery(
        r.output.clone(),
        combined,
        dropped,
        r.savings.tokens_before,
        &hint,
        // The name of the compactor the router chose, not the channel's: it
        // is the one the ledger records, and two vocabularies for the same
        // thing would make the two measurements diverge.
        &r.filter,
    );
    let mut updated = parsed
        .get("tool_response")
        .or_else(|| parsed.get("toolResponse"))
        .cloned()?;
    let obj = updated.as_object_mut()?;
    // `wrap` merges stderr into stdout and compacts the two as one stream, so
    // a result folded here carries the same shape as one folded at source.
    obj.insert(
        "stdout".to_string(),
        serde_json::Value::String(tokenade_core::redact::redact_for_model(&served)),
    );
    obj.insert(
        "stderr".to_string(),
        serde_json::Value::String(String::new()),
    );
    Some(updated)
}

/// Drive the real decision from a command and its stdout. Test seam only —
/// it builds the same `parsed` shape the hook receives and runs the same
/// filter, so nothing about the rule is re-spelled here.
#[cfg(test)]
pub(crate) fn serve_declined_fold_for_test(cmd: &str, stdout: &str) -> Option<String> {
    let parsed = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": { "command": cmd },
        "tool_response": { "stdout": stdout, "stderr": "", "interrupted": false }
    });
    let argv: Vec<String> = cmd.split_whitespace().map(String::from).collect();
    let r = filter::filter_command_fail_open(&argv, stdout);
    let worth_emitting = fold_is_worth_booking(
        &r.savings,
        tokenade_core::fold_marker::already_folded(stdout),
    );
    declined_fold_replacement(&parsed, stdout, &r, worth_emitting).and_then(|u| {
        u.pointer("/stdout")
            .and_then(|v| v.as_str())
            .map(str::to_string)
    })
}

fn emit_redacted_bash_output_if_needed(
    parsed: &serde_json::Value,
    stdout: &str,
    stderr: &str,
    started: std::time::Instant,
) -> Option<i32> {
    if stdout.is_empty() && stderr.is_empty() {
        return None;
    }
    let red_out = redact::redact_for_model(stdout);
    let red_err = redact::redact_for_model(stderr);
    if red_out == stdout && red_err == stderr {
        return None;
    }
    let mut updated = parsed
        .get("tool_response")
        .or_else(|| parsed.get("toolResponse"))
        .cloned()?;
    let obj = updated.as_object_mut()?;
    obj.insert("stdout".to_string(), serde_json::Value::String(red_out));
    obj.insert("stderr".to_string(), serde_json::Value::String(red_err));
    println!(
        "{}",
        serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "updatedToolOutput": updated,
            }
        })
    );
    tokenade_core::debug_log::log(
        "hookwrap",
        "redact_only_replacement",
        serde_json::json!({
            "stdout_bytes": stdout.len(),
            "stderr_bytes": stderr.len(),
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }),
    );
    Some(0)
}

struct PostToolInvocation<'a> {
    parsed: &'a serde_json::Value,
    /// The shell command as the agent typed it.
    cmd: &'a str,
    /// stdout + stderr of that command, already joined.
    combined: &'a str,
    /// stdout ALONE. The host caps each field separately, so the truncation
    /// check must not read a large stdout beside a large stderr as one payload
    /// over the cap — the mirror of the mistake `omni` #727 warns about.
    stdout: &'a str,
    /// `cmd`, redacted and length-capped: the only spelling that may reach a log.
    safe_cmd_for_log: &'a str,
    started: std::time::Instant,
}

impl PostToolInvocation<'_> {
    /// Steer a failing or over-broad search instead of serving its own empty
    /// output. `Some(rc)` = answered here, nothing else must print.
    ///
    /// Throttled by hint FAMILY per session: detection keeps firing (the steer
    /// stays correct), only the repetition is suppressed once the advice is
    /// already in the agent's context.
    fn serve_search_suggestion(&self) -> Option<i32> {
        if let Some(hint) = search_suggestion_hint(self.cmd, self.combined) {
            // Anti-spam: a churny session re-runs the same failing grep many
            // times; the matcher still fires each time (we keep the steer),
            // but once the advice is already in context we don't re-inject
            // it. Key by hint FAMILY (empty vs broad) so each is shown once
            // per session. DETECTION stays on; only repetition is throttled.
            let session_id = crate::hooks::session_id_of(self.parsed).unwrap_or("");
            let hint_key = if self
                .combined
                .lines()
                .filter(|l| !l.trim().is_empty())
                .count()
                == 0
            {
                "search-empty"
            } else {
                "search-broad"
            };
            if !hint_fresh_this_session(session_id, hint_key) {
                tokenade_core::debug_log::log(
                    "hookwrap",
                    "search_hint_throttled",
                    serde_json::json!({
                        "command": &self.safe_cmd_for_log,
                        "hint_key": hint_key,
                        "elapsed_ms": self.started.elapsed().as_millis() as u64,
                    }),
                );
                return Some(0);
            }
            let decision = serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "PostToolUse",
                    "additionalContext": hint,
                }
            });
            log_emitted_additional_context("hookwrap-search-hint", "PostToolUse", &hint);
            println!("{decision}");
            tokenade_core::debug_log::log(
                "hookwrap",
                "search_hint_emitted",
                serde_json::json!({
                    "command": &self.safe_cmd_for_log,
                    "matches": self.combined.lines().filter(|l| !l.trim().is_empty()).count(),
                    "elapsed_ms": self.started.elapsed().as_millis() as u64,
                }),
            );
            return Some(0);
        }
        None
    }

    /// tokenade wrapping ITSELF — the second bypass spelling.
    ///
    /// `tokenade raw|bypass|noproxy <cmd>` books the same lost-savings
    /// measurement as the env-prefix form handled earlier. A plain
    /// `tokenade wrap` falls through: that is compaction, not a bypass.
    fn serve_tokenade_self_call(&self, argv: &[String], cmd_prog: &str) -> Option<i32> {
        if cmd_prog == "tokenade" {
            // P2b — `tokenade raw|bypass|noproxy <self.cmd>` is the OTHER bypass
            // spelling (the env-prefix form bailed above). Same lost-savings
            // measurement; `tokenade wrap` invocations fall through to the
            // plain bail below (they're compaction, not bypass).
            if matches!(
                argv.get(1).map(String::as_str),
                Some("raw" | "bypass" | "noproxy")
            ) {
                record_bypass_lost_savings(self.cmd, self.parsed, self.started);
            }
            tokenade_core::debug_log::log(
                "hookwrap",
                "wrap_already_applied",
                serde_json::json!({
                    "command": &self.safe_cmd_for_log,
                    "elapsed_ms": self.started.elapsed().as_millis() as u64,
                }),
            );
            return Some(0);
        }
        None
    }

    /// The fold was not worth serving: emit nothing, book nothing.
    ///
    /// Deliberately NO `gain.jsonl` row — the `passthrough` debug.log entry
    /// below already carries the same information, and a gain row here only
    /// dilutes the dashboard averages (measured over a 4000-line production
    /// window).
    ///
    /// `r`, `savings_pct` and `worth_emitting` stay ARGUMENTS on purpose: they
    /// describe this filter RESULT, not the invocation, and folding them into
    /// the struct would make it two things at once.
    fn serve_below_threshold(
        &self,
        r: &tokenade_core::filter::FilterResult,
        savings_pct: f64,
        worth_emitting: bool,
        already_folded: bool,
    ) -> Option<i32> {
        if !worth_emitting {
            // Below-threshold: SKIP the gain.jsonl record. The same info is
            // already captured by the `passthrough` debug.log entry below,
            // so the gain entry is pure noise that dilutes dashboard
            // averages. Production telemetry (4000-line window): 188
            // auto:ini + 313 filter:passthrough + 136 filter:generic events,
            // all below-5pct → ~17 % of all hook records were noise.
            // Mirrors the #411 (MCP log_mcp) and #417 (CLI raw) fixes.
            let elapsed_ms = self.started.elapsed().as_millis() as u64;
            tokenade_core::debug_log::log(
                "hookwrap",
                "passthrough",
                serde_json::json!({
                    "command": &self.safe_cmd_for_log,
                    "filter": r.filter,
                    "tokens_before": r.savings.tokens_before,
                    "tokens_after": r.savings.tokens_after,
                    "reason": if already_folded {
                        "already-folded"
                    } else if savings_pct < 5.0 {
                        "below-5pct"
                    } else {
                        "below-10-abs"
                    },
                    "elapsed_ms": elapsed_ms,
                }),
            );
            emit_host_cut_advisory(self.stdout, self.safe_cmd_for_log, self.started);
            return Some(0);
        }
        None
    }
}
