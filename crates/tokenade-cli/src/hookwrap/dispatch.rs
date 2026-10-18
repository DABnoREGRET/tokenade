//! **`tokenade hookwrap`: the body of the hook.**
//!
//! What the host calls after every tool. It must return a VALID response no
//! matter what — a malformed output makes a successful fold look like a tool
//! error — and never block: a hook that takes its time is paid for on every
//! turn of the agent.

use super::*;

/// PostToolUse dispatcher: the Bash channel's whole pipeline, in order.
///
/// **Length: 447 → 363 lines**, on a measurement that had to be taken twice.
///
/// The first count said the three seams — the search steer (46 l.), the
/// `tokenade`-wrapping-itself branch (21 l.) and the below-threshold exit
/// (23 l.) — read 11, 8 and 6 of this function's locals, and concluded that
/// extracting them would only spell the coupling louder. That count was wrong:
/// it included each block's OWN locals. Counting only what is declared *before*
/// a block and read *inside* it gives **6, 6 and 5** — and four of them are the
/// SAME four every time: `parsed`, `cmd`, `safe_cmd_for_log`, `started`.
///
/// Four values threaded through three blocks are not parameters, they are
/// state. Hence [`PostToolInvocation`] and its three methods, which take zero
/// to three arguments of their own rather than the six a free function needed.
/// `serve_below_threshold` keeps `r`/`savings_pct`/`worth_emitting` as
/// arguments on purpose: those describe a filter RESULT, not the invocation,
/// and absorbing them would make the struct two things at once.
///
/// What remains here is the pipeline itself: read, parse, route by tool,
/// capture, filter, account, emit. That sequence IS this function's one
/// responsibility, and it is why the rest is not split further.
///
/// One hard constraint on any future split:
/// `install::lifecycle_hooks::posttool_matcher_covers_every_tool_its_handler_dispatches_on`
/// scans THIS function's source for the tool names it dispatches on
/// (`Bash|Read`). The `tool_is(tool, "Read")` and `is_shell_tool_name` checks
/// must stay in this body, or the hook contract is guarded by nothing.
pub(crate) fn cmd_hookwrap() -> i32 {
    // Runtime kill-switch env var.
    // A user (or a CI script) can prepend the env var to mute the hook
    // without re-installing. Useful when debugging a hook ordering
    // issue or when the agent is doing trusted bulk work that doesn't
    // need compaction.
    //
    // SECURITY: the kill-switch mutes COMPACTION, not the secret scrub.
    // We still read the payload below and run the redact-only pass —
    // otherwise `TOKENADE_HOOK_DISABLED=1 cat .env` (or an agent that
    // sets the var to dodge folding) ships credentials to the model
    // verbatim. Everything else (hints, replay, analytics, gain rows)
    // honours the switch.
    let disabled = hook_disabled_by_env();
    if !disabled {
        // Self-heal backstop: if SessionStart was dropped but this PostToolUse
        // hook still fires, re-assert the suite (throttled). Best-effort.
        let _ = ensure_hooks(tokenade_core::platform::home_dir().as_deref(), true);
    }
    let started = std::time::Instant::now();
    // The OUTPUT cap, not the event one: this channel carries a command's
    // result — a build log or a `grep -r` is the normal case here. See
    // `hooks::stdin` for why the two caps differ and why they now live
    // side by side.
    let Some(input) = crate::hooks::stdin::read_capped(
        "hookwrap",
        crate::hooks::stdin::MAX_OUTPUT_STDIN_BYTES,
        started,
    ) else {
        return 0;
    };
    let Some(parsed) = crate::hooks::stdin::parse_event(&input) else {
        tokenade_core::debug_log::log(
            "hookwrap",
            "parse_err",
            serde_json::json!({"elapsed_ms": started.elapsed().as_millis() as u64}),
        );
        return 0;
    };
    // Session-scope every row this hook process books (statusline ticker).
    if !disabled {
        if let Some(sid) = crate::hooks::session_id_of(&parsed) {
            let skey: String = sid
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(40)
                .collect();
            tokenade_core::session::set_session_key_authoritative(&skey);
        }
    }

    let tool = parsed
        .get("tool_name")
        .and_then(|s| s.as_str())
        .unwrap_or("");
    // Read hook: passive nudge — when the model reads a code file
    // larger than 5k tokens, suggest `skeleton` for next time.
    // Doesn't touch the content (model still gets full Read result).
    // ~50 tokens of additionalContext per qualifying Read; the model
    // self-selects skeleton on the next exploration of that area.
    // `tool_is`, not `==`: Grok lowercases EVERY tool name, which is the field
    // bug that once had `tool == "Bash"` eat every shell event here. The Bash
    // half was fixed (`is_shell_tool_name` is case-tolerant); this one was left
    // literal, so on that host a Read fell past this branch, failed the shell
    // test below, and logged `skip_non_bash` — the read nudge never fired.
    if crate::hooks::tool_is(tool, "Read") {
        if disabled {
            return 0;
        }
        return handle_read_posttool(&parsed, started);
    }
    if !crate::hooks::is_shell_tool_name(tool) {
        tokenade_core::debug_log::log(
            "hookwrap",
            "skip_non_bash",
            serde_json::json!({
                "tool": tool,
                "elapsed_ms": started.elapsed().as_millis() as u64,
            }),
        );
        return 0;
    }
    let cmd = parsed
        .get("tool_input")
        .and_then(|i| i.get("command"))
        .and_then(|c| c.as_str())
        .unwrap_or("");
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

    // SECURITY: redact-only replacement, UNCONDITIONAL — runs before (and
    // regardless of) the kill-switch return, the `TOKENADE_HOOK_DISABLED=1`
    // command-prefix bypass and `tokenade raw`. The
    // rewritten (`tokenade wrap`) path already ships redacted bytes, so
    // this is a no-op there; it exists for every output that reaches the
    // model WITHOUT passing through a compactor: excluded/interactive
    // commands, absolute-path invocations that dodge the PATH shim,
    // and deliberate bypasses. Secrets are
    // never bypassable.
    if let Some(rc) = emit_redacted_bash_output_if_needed(&parsed, stdout, stderr, started) {
        return rc;
    }
    if disabled {
        return 0;
    }

    // Register path baselines for files written by this command so that
    // the next Read hits L31 diff instead of passthrough. Runs before the
    // bypass / search-hint early-returns so we never miss a write even
    // when the command's output itself is not compacted.
    register_bash_write_baselines(cmd);
    // #977 — honor the bash one-shot env-prefix bypass. The hook
    // process can't see `TOKENADE_HOOK_DISABLED=1` set on the user's
    // command (separate process tree), so detect the literal prefix
    // here and treat it like the env-var path. Tick the bypass-nudge
    // counter so over-bypass still surfaces in the 24h reminder.
    // One question, asked once. Every SEGMENT counts, because that is what the
    // wrap path honours — `cd /repo` on one line and the prefix on the next
    // leaves that segment unwrapped, and the counting used to read only the
    // whole command's leading prefix. And it is asked HERE only: the
    // PreToolUse handler asked it too, so one bypass counted as two. Measured
    // live on the installed binary before the fix: the leading form moved the
    // 24h counter by 2, the segment form by 0, and 2 490 of the 2 948 bypasses
    // in the corpus are the second shape.
    if let Some(seg) = bypassed_segment(cmd) {
        // `TOKENADE_HOOK_DISABLED=1 tokenade wrap …` bypasses NOTHING — the
        // hook is off precisely because tokenade is being invoked directly.
        // Both PreToolUse sites carried this guard; this one never did.
        if !invokes_tokenade_itself(seg) {
            // The nudge's own channel is this hook's stderr, which is not
            // surfaced: measured 2026-09-02 on this install, 885 recorded
            // fires of `nudge:bypass:fire` and not one ever seen by the agent
            // it is addressed to, while every nudge that travels in
            // `hookSpecificOutput.additionalContext` arrives. This handler
            // already emits that shape elsewhere, so deliver it here too.
            //
            // ONCE per session, unlike the stderr write. "Loud on purpose" was
            // written for a channel that costs nothing; delivered context is
            // not free, and this window held 164 bypasses.
            if let Some(msg) = maybe_emit_bypass_nudge() {
                if bypass_nudge_is_new_for_session(&parsed) {
                    let decision = serde_json::json!({
                        "hookSpecificOutput": {
                            "hookEventName": "PostToolUse",
                            "additionalContext": msg,
                        }
                    });
                    log_emitted_additional_context("hookwrap-bypassnudge", "PostToolUse", &msg);
                    println!("{decision}");
                    // Delivered: this is WHERE the firing is counted.
                    crate::wrap::book_bypass_delivered(1);
                }
            }
        }
        if cmdline_requests_hook_bypass(cmd) {
            // P2b — before bailing, write what the bypass COST to the
            // lost-savings ledger (tool_response is right here; this is
            // the only place the raw output of a bypassed run is visible).
            record_bypass_lost_savings(cmd, &parsed, started);
            return 0;
        }
    }
    // REPLAY store: remember eligible read-only runs so an unchanged-state
    // re-run can skip execution (hooks/replay.rs). The command here is the
    // REWRITTEN one — unwrap to the inner user command for a stable key.
    {
        let session = crate::hooks::session_id_of(&parsed).unwrap_or("nosess");
        let skey: String = session
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(40)
            .collect();
        let inner = unwrap_tokenade_wrap(cmd).unwrap_or(cmd);
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        // Context-scoped replay key (transcript folded in) — the class-1
        // definitive scheme; skey alone only when no transcript_path.
        let replay_ctx = crate::hooks::ctx_replay_key(&parsed, &skey);
        crate::hooks::replay::store_replay(&replay_ctx, &cwd, inner, &combined);
        // Volume-pressure signal: what actually got delivered this call.
        crate::hooks::bump_session_delivered(&skey, combined.len());
        // Cross-channel dedup seed: files served via `cat` can answer a
        // later Read (the Read-before-Edit gate forces exactly that shape).
        crate::hooks::register_bash_served_files(&parsed, &cwd, inner);
    }

    // SECURITY: redact `cmd` BEFORE any debug_log site uses it. Pre-fix,
    // sshpass / sudo -S passwords leaked into ~/.tokenade/debug.log via
    // the empty_output / passthrough / log_only / search_hint_emitted
    // events (4 separate sites). Compute the safe form once + reuse so
    // future code additions can't accidentally re-introduce the leak.
    let safe_cmd_for_log = {
        let argv: Vec<String> = cmd.split_whitespace().map(String::from).collect();
        let r = redact::redact(&redact::redact_argv(&argv).join(" "));
        // #1021 — mirror of the pretool fix; same UTF-8 panic risk.
        let cut = tokenade_core::caps::truncate_at_char_boundary(&r, 200);
        if cut.len() < r.len() {
            format!("{cut}…[+{} bytes]", r.len() - cut.len())
        } else {
            r
        }
    };

    // Search-tool nudge MUST run BEFORE the empty-output early return.
    // A grep / find / rg that returns 0 matches has stdout="", so
    // skipping on empty would silently lose the most important nudge
    // case ("0 matches → try semantic_search"). The hint emission is
    // a tiny additive context (~60 tokens), so we're not duplicating
    // data even when combined is empty.
    let invocation = PostToolInvocation {
        parsed: &parsed,
        cmd,
        combined: &combined,
        stdout,
        safe_cmd_for_log: &safe_cmd_for_log,
        started,
    };
    if let Some(rc) = invocation.serve_search_suggestion() {
        return rc;
    }

    if combined.is_empty() {
        tokenade_core::debug_log::log(
            "hookwrap",
            "empty_output",
            serde_json::json!({
                "command": &safe_cmd_for_log,
                "elapsed_ms": started.elapsed().as_millis() as u64,
            }),
        );
        return 0;
    }
    // Tokenise the command into argv and apply the same filter as `tokenade proxy`.
    //
    // Compound commands: route + label on the first MEANINGFUL segment, not a
    // leading `cd`/`export`/`set` state-modifier (or a bare operator token).
    // `cd /tmp && grep x f` produces grep's output, so it must report + route as
    // `grep` — the old whole-string `split_whitespace` keyed everything on
    // `argv[0]` (`cd`), which is why the dashboard showed a `cd` row with huge
    // savings and ~3% coverage. `split_chain_segments` is quote/redirect/
    // control-flow aware (handles `&&`/`||`/`;`, leaves `|` pipelines and `2>&1`
    // intact, bails safely on heredocs/subshells/background-`&`); when it can't
    // safely split we fall back to the whole command. `segment_must_not_be_wrapped`
    // also skips an inner `tokenade wrap …` segment, so `cd && tokenade wrap X`
    // resolves to the tokenade token and is caught by the already-wrapped guard
    // below instead of being re-compacted under a `cd` label.
    let effective_cmd: &str = command_rewrite::meaningful_segment(cmd);
    let argv: Vec<String> = effective_cmd.split_whitespace().map(String::from).collect();

    // L79 — skip the redundant re-compaction when the cmd is itself
    // a `tokenade wrap` invocation. PreToolUse already rewrote most
    // bash commands to flow through `tokenade wrap`, which runs the
    // proper per-command compactor on the output. Re-running
    // filter_command here:
    //   - wastes CPU on 30 auto-detector regex scans
    //   - mis-classifies the tokenade banner as Kind::Ini (production
    //     data showed 747/764 auto:ini events were tokenade wrap
    //     outputs, not real ini files)
    //   - pollutes the dashboard with false per-format labels.
    // The first segment of an Axe-2 chain rewrite (`tokenade wrap A
    // && tokenade wrap B`) also matches here — fine, the OUTER
    // chain's `&&` was preserved so bash ran both wraps and we got
    // their concatenated outputs. The inner wraps already logged
    // their own gain entries.
    let cmd_prog = argv
        .first()
        .map(|p| p.rsplit(['/', '\\']).next().unwrap_or(p).to_string())
        .unwrap_or_default();
    if let Some(rc) = invocation.serve_tokenade_self_call(&argv, &cmd_prog) {
        return rc;
    }

    // The panic boundary, plus the salvage that actually returns the bytes
    // under `panic = "abort"`. This is the highest-traffic compaction channel
    // in the product; it went through the compactor bare while `wrap` — the
    // same work, one process earlier — had both.
    let r = {
        let _salvage = tokenade_core::crash_guard::arm_salvage(&combined, 0);
        filter::filter_command_fail_open(&argv, &combined)
    };
    // Classify BEFORE anything else runs: `is_command_specific` consults a
    // per-call thread-local that the next `filter_command` would reset.
    let cmd_name = command_rewrite::program_name(cmd);
    let cmd_specific = r.is_command_specific();
    let safe_label = redact::redact(&redact::redact_argv(&argv).join(" "));

    // Silent passthrough when our compaction would NOT net a saving.
    // The banner `[tokenade:filter −X% tokens]\n` is ~30-40 chars; on
    // a tiny output (say `cargo build` that printed 120 chars), adding
    // that banner makes the agent's context strictly larger. Returning
    // no `additionalContext` lets Claude Code see the raw tool result
    // unchanged — the right thing to do when we have nothing to add.
    //
    // Threshold: require ≥ 5 % real savings AND ≥ 10 tokens absolute
    // saved. Both must hold so we don't quibble over single-token wins.
    let savings_pct = r.savings.percent();
    let absolute_saved = r
        .savings
        .tokens_before
        .saturating_sub(r.savings.tokens_after);
    // "One authority per payload", on the channel that carries the
    // most traffic. Until now this channel enforced it only by ACCIDENT: an
    // already-folded payload usually fails the ≥5 % test below, so no second
    // row got booked. Measured 2026-08-08 on a fresh HOME — `ss -tanp`,
    // `lsblk -O`, `systemctl list-units --all` folded by `wrap`, then fed to
    // this hook: zero extra rows. But "usually" is not the invariant: hand the
    // same hook a folded payload a second authority CAN still shave 5 % off
    // and it books a row for bytes the first authority already credited (the
    // `copilot_fold` comment records exactly that shape — a second fold of a
    // git log shaving 54 more bytes). `copilot_fold` has asked this question
    // since it was written; the Bash path never did.
    let already_folded = tokenade_core::fold_marker::already_folded(&combined);
    let worth_emitting = fold_is_worth_booking(&r.savings, already_folded);
    let _ = absolute_saved;

    if let Some(rc) =
        invocation.serve_below_threshold(&r, savings_pct, worth_emitting, already_folded)
    {
        return rc;
    }

    gain_log::record_cmd(
        "hook",
        &r.filter,
        &safe_label,
        r.savings,
        &cmd_name,
        cmd_specific,
    );
    // ARCHITECTURAL FIX (2026-05-29): we used to emit `additionalContext`
    // here, but `additionalContext` is ADDITIVE in Claude Code — it
    // appends to the model's context WITHOUT replacing the raw tool
    // result. Net effect: the model paid for raw + compact for every
    // intercepted command, strictly worse than no hook at all.
    //
    // The architectural fix lives in `cmd_hookwrap_pretool`'s Bash
    // handler: PreToolUse intercepts BEFORE execution and returns
    // `permissionDecision: allow` + `updatedInput` rewriting
    // `Bash(cmd)` → `Bash(tokenade wrap 'cmd')`. Claude Code then runs
    // the rewritten command, so `tokenade wrap`'s already-compact output
    // IS the captured tool_response → real substitution, real savings.
    // (The older `deny` + reason channel was dropped: Claude Code wraps
    // a deny reason in <error>…</error>, making a successful redirect
    // read as a failure and risking a wasted retry. allow+updatedInput
    // is the clean substitution channel.)
    //
    // We keep this PostToolUse hook ALIVE for two purposes that don't
    // require emitting context:
    //   1. Ledger logging (gain_log::record above) — analytics about
    //      what compaction WOULD have saved for commands that the
    //      PreToolUse path didn't intercept (e.g. excluded interactive
    //      tools, or runs in clients that haven't installed the
    //      PreToolUse hook yet).
    //   2. Telemetry / debugging.
    //
    // Suppressing additionalContext means subsequent install paths
    // can leave PostToolUse on without re-introducing the additive
    // double-cost bug.
    // An earlier finding proved on the shipped Claude
    // Code binary that `PostToolUse.updatedToolOutput` genuinely REPLACES a
    // tool result, and decided NOT to move the Bash path onto it. Every
    // objection it listed is about replacing the PreToolUse rewrite wholesale:
    // quote preservation through the outer shell, the inner shell on Windows,
    // interactive/TTY detection, the `TOKENADE_SESSION=` prefix, and what a
    // host command guard does with a rewrite.
    //
    // None of them apply to the commands PreToolUse DECLINED for safety. Those
    // are never rewritten, so nothing folds their output — while this very
    // function computes the fold and writes it to a log. Measured on a
    // realistic `sshpass -p … ssh -T host 'ps -eo pid,tty,stat,time,cmd'`:
    // 31 779 bytes in, 155 out, `log_only`.
    if let Some(rc) = emit_folded_bash_output_if_pretool_declined(
        &parsed,
        &combined,
        &r,
        savings_pct,
        worth_emitting,
        started,
    ) {
        return rc;
    }
    let elapsed_ms = started.elapsed().as_millis() as u64;
    tokenade_core::debug_log::log(
        "hookwrap",
        "log_only",
        serde_json::json!({
            "command": &safe_cmd_for_log,
            "filter": r.filter,
            "input_bytes": combined.len(),
            "compact_bytes": r.output.len(),
            "tokens_before": r.savings.tokens_before,
            "tokens_after": r.savings.tokens_after,
            "elapsed_ms": elapsed_ms,
        }),
    );
    emit_host_cut_advisory(stdout, &safe_cmd_for_log, started);
    0
}
