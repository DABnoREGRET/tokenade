//! **Answering the hook: fold, redact, or let through.**
//!
//! The host expects a precise response shape — a pass-through as is, a
//! substitution — and getting the shape wrong makes a successful fold look
//! like a tool error. Reading into a temporary file lives here because Gemini
//! returns a PATH, not content: that detour is what allows compacting without
//! rewriting the user's file.

use super::*;

/// Antigravity / Gemini-family `AfterTool` hook handler.
///
/// AfterTool fires after a tool runs. Its documented replace mechanism
/// (geminicli.com/docs/hooks/reference, shared by Antigravity) is
/// `{"decision":"deny","reason":<text>}` where `reason` REPLACES the tool
/// result sent back to the model. We fold `tool_response.llmContent` and
/// return it as `reason`. Error results (`tool_response.error`) are kept
/// verbatim. Fail-open: an unrecognized shape, an error, or no net saving →
/// `{"decision":"allow"}` (the real output is preserved).
/// The agent this hook process serves, from the `--agent <slug>` the installer
/// writes into the command.
///
/// Defaults to `gemini-cli` so a config written before 2026-08-08 keeps its
/// old behaviour rather than losing attribution entirely — the wrong one of
/// two agents beats none.
pub(crate) fn agent_from_argv() -> String {
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--agent" {
            if let Some(v) = args.next() {
                if tokenade_core::detect::Agent::from_slug(v.trim()).is_some() {
                    return v.trim().to_string();
                }
            }
        }
    }
    "gemini-cli".to_string()
}

pub(crate) fn cmd_gemini_after_tool() -> i32 {
    // This process IS the AfterTool hook of a Gemini-family agent. Attribute
    // whatever it books to that agent regardless of ambient env or parent
    // process — measured 2026-08-08, without this every row landed on the
    // agent the process tree happened to name (`claude-code` in the probe),
    // because `detect::from_env` falls back to walking parents. Its two
    // siblings, `cmd_gemini_hook` and `cmd_gemini_afterhook`, already did
    // this; the busiest of the three did not.
    std::env::set_var("TOKENADE_AGENT", agent_from_argv());
    let mut input = String::new();
    if crate::bounded_input::read_stdin_lossy(&mut input).is_err() {
        println!("{{\"decision\":\"allow\"}}");
        return 0;
    }
    {
        if let Some((out, before, after)) = gemini_after_fold(&input) {
            if after < before {
                tokenade_core::gain::record(
                    "gemini-aftertool",
                    "aftertool",
                    "aftertool",
                    tokenade_core::Savings::new(before, after),
                );
            }
            println!("{out}");
            return 0;
        }
    }
    // Redact-only replacement: fires when the fold declined (small output,
    // no net saving, error result) but secrets are present.
    if let Some(out) = gemini_redact_only(&input) {
        println!("{out}");
        return 0;
    }
    println!("{{\"decision\":\"allow\"}}");
    0
}

/// Pure core of [`cmd_gemini_after_tool`], testable without stdin. Returns
/// `Some((deny_json, before_tok, after_tok))` with the folded replacement, or
/// `None` for passthrough (allow).
pub(crate) fn gemini_after_fold(input: &str) -> Option<(String, usize, usize)> {
    let v: serde_json::Value = serde_json::from_str(input).ok()?;
    // The web tools are owned by the specialized `gemini-afterhook`
    // compactor (prompt-scoped extract + stash + expand-ref banner); skip
    // them here so two AfterTool hooks never race to replace the same
    // result.
    if matches!(
        v["tool_name"].as_str(),
        Some("google_web_search") | Some("web_fetch")
    ) {
        return None;
    }
    let resp = &v["tool_response"];
    // Never fold an error result — the model must see it verbatim.
    if !resp.get("error").map(|e| e.is_null()).unwrap_or(true) {
        return None;
    }
    // `llmContent` is what the model sees: a plain string, or the structured
    // parts array (join its text parts).
    let text: String = match &resp["llmContent"] {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(parts) => {
            let joined: String = parts
                .iter()
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("");
            if joined.is_empty() {
                return None;
            }
            joined
        }
        _ => return None,
    };
    if text.len() < 800 {
        return None;
    }
    // One authority per payload: a result the PATH
    // shim or an earlier channel already folded has an owner. "Did it get
    // shorter" is a different question and lets a second pass chew on a
    // summary.
    if tokenade_core::fold_marker::already_folded(&text) {
        return None;
    }
    let before = tokenade_core::tokenizer::estimate(&text);
    let generic = tokenade_core::auto::compact(&text).1;
    // The command that produced these bytes is sitting in the payload, and this
    // hook was throwing it away — the exact information the five plugin bridges
    // were given a `--cmd` flag to stop throwing away.
    // Content detection cannot tell which program wrote a listing, so the
    // hundreds of per-command compactors were unreachable from Gemini CLI and
    // Qwen Code.
    //
    // Measured 2026-08-18 through the installed AfterTool hook, on a 33 726-byte
    // `ls -la`-shaped result: generic detection returned 23 465 bytes (30 %),
    // the named compactor returns ~300 (99 %). Claude Code has had the second
    // number since the `--cmd` work; these two agents had the first.
    //
    // Competition, not preference: `read --cmd` learned the hard way that a
    // more specific compactor is not always smaller (`ls -la` went 332 → 5 875
    // when the named one was made to win by default). Smallest delivered output
    // wins, which is the same rule and the same reason.
    let folded =
        crate::compactors::hint_programs(v["tool_input"]["command"].as_str().unwrap_or_default())
            .into_iter()
            .filter_map(|prog| tokenade_core::cmd::dispatch_named(prog, &text))
            .chain(std::iter::once(generic))
            .filter(|f| !f.is_empty())
            .min_by_key(|f| f.len())
            .unwrap_or_default();
    if folded.is_empty() || folded.len() >= text.len() {
        return None;
    }
    // SECURITY: the generic fold does not scrub — same print-site redact
    // as cmd_auto (#902) / cmd_filter (#904). Idempotent no-op when clean.
    let folded = tokenade_core::redact::redact_for_model(&folded);
    // Recovery pointer — the twin of `copilot_fold`, and it had the same gap.
    // The web branch above is skipped precisely because the specialized
    // compactor stashes and banners; the NON-web results this function does
    // fold had neither, so a Gemini CLI or Qwen user saw "N lines elided" with
    // no way back. Appends, never replaces.
    let dropped = before.saturating_sub(tokenade_core::tokenizer::estimate(&folded));
    let folded = crate::wrap::attach_drop_recovery(
        folded,
        &text,
        dropped,
        before,
        &tokenade_core::disclose::SourceHint::Shell,
        "gemini-hook",
    );
    let after = tokenade_core::tokenizer::estimate(&folded);
    let out = serde_json::json!({
        "decision": "deny",
        "reason": folded,
        "hookSpecificOutput": { "hookEventName": "AfterTool" }
    })
    .to_string();
    Some((out, before, after))
}

/// SECURITY fallback of [`cmd_gemini_after_tool`]: when the fold does not
/// replace the result (small output, no net saving), secrets
/// must still not reach the model. Only fires on a STRING `llmContent`
/// (a parts array may carry images — flattening it to a deny-reason would
/// destroy them); returns the redact-only deny JSON, or `None` when clean.
/// Unlike the fold, error results are NOT skipped: a failed curl echoing
/// an Authorization header is exactly the leak this exists to stop (the
/// redaction masks secret VALUES only — the diagnostic text survives).
pub(crate) fn gemini_redact_only(input: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(input).ok()?;
    if matches!(
        v["tool_name"].as_str(),
        Some("google_web_search") | Some("web_fetch")
    ) {
        return None; // owned by gemini-afterhook
    }
    let text = v["tool_response"]["llmContent"].as_str()?;
    let safe = tokenade_core::redact::redact_for_model(text);
    if safe == text {
        return None;
    }
    Some(
        serde_json::json!({
            "decision": "deny",
            "reason": safe,
            "hookSpecificOutput": { "hookEventName": "AfterTool" }
        })
        .to_string(),
    )
}

/// Gemini CLI BeforeTool hook handler.
///
/// Gemini CLI calls this with a JSON payload on stdin:
/// `{"tool_name":"run_shell_command","tool_input":{"command":"..."}}`
///
/// For `run_shell_command` calls, we rewrite the command to route through
/// `tokenade wrap` so the model receives compressed output. All other tools
/// are allowed through unchanged.
/// `BeforeAgent` and `SessionStart` for Gemini CLI and Qwen Code.
///
/// These two events were NOT registered, and the reason they now are is a
/// measurement rather than a reading of the docs. `HookEventName` in the
/// shipped `@google/gemini-cli-core/dist/src/hooks/types.js` lists eleven
/// events — `BeforeTool`, `AfterTool`, `BeforeAgent`, `Notification`,
/// `AfterAgent`, `SessionStart`, `SessionEnd`, `PreCompress`, `BeforeModel`,
/// `AfterModel`, `BeforeToolSelection` — and tokenade used two of them. The
/// compat bench read that as "no prompt-time hook exists for this agent" and
/// "no lifecycle hook for this agent", on two agents across three OS, which
/// was true of our INSTALL and false of the host.
///
/// `additionalContext` is why these two and not `BeforeModel`. Both
/// `BeforeAgentOutput` and `SessionStartOutput` declare it, so the preamble
/// travels as an extra piece of context and nothing else is touched.
/// `BeforeModel` can rewrite the whole request — `fromHookLLMRequest` REBUILDS
/// `contents` from the hook's `messages` — so a hook that answers it and gets
/// the message list wrong silently truncates the conversation. A style
/// preamble is not worth that.
pub(crate) fn cmd_gemini_lifecycle() -> i32 {
    if hook_disabled_by_env() {
        return 0;
    }
    let Some(input) = crate::hooks::stdin::read_event("gemini-lifecycle") else {
        return 0;
    };
    let parsed: serde_json::Value = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(_) => return 0,
    };
    std::env::set_var("TOKENADE_AGENT", agent_from_argv());
    // The preamble emitted below is booked as injected context — under THIS
    // session, or it is dropped (no key) and never charged again.
    if let Some(sid) = crate::hooks::session_id_of(&parsed) {
        let skey: String = sid
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(40)
            .collect();
        tokenade_core::session::set_session_key_authoritative(&skey);
    }
    let event = parsed
        .get("hook_event_name")
        .and_then(|v| v.as_str())
        .unwrap_or("BeforeAgent")
        .to_string();
    // The context is emptied: compacted (`PreCompress` / `PreCompact`), or
    // cleared under the same session id. A compaction event takes no output.
    let cleared =
        event == "SessionStart" && parsed.get("source").and_then(|v| v.as_str()) == Some("clear");
    if event == "PreCompress" || event == "PreCompact" || cleared {
        crate::hooks::session_lifecycle::note_context_reset(
            crate::hooks::session_id_of(&parsed).unwrap_or(""),
        );
        if !cleared {
            return 0;
        }
    }
    // The turn's own text where the event carries one. `SessionStart` has no
    // prompt, and `Auto` classifies an empty string as ambiguous — which
    // injects nothing. That is the right answer for a per-turn preamble and
    // the wrong one for the single event that opens the session, so the
    // session opener falls back to the coding preset it would otherwise
    // never reach.
    let prompt = parsed
        .get("prompt")
        .and_then(|v| v.as_str())
        .or_else(|| parsed.get("message").and_then(|v| v.as_str()))
        .unwrap_or("");
    let root = root();
    let configured = tokenade_core::style::load(&root);
    let effective = match configured.resolve(prompt) {
        tokenade_core::style::Style::Off
            if event == "SessionStart" && configured != tokenade_core::style::Style::Off =>
        {
            tokenade_core::style::Style::Coding
        }
        other => other,
    };
    let preamble = effective.preamble();
    if preamble.is_empty() {
        return 0;
    }
    let out = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": event,
            "additionalContext": preamble,
        }
    });
    crate::log_emitted_additional_context("gemini-style-preamble", &event, preamble);
    println!("{out}");
    tokenade_core::debug_log::log(
        "gemini-lifecycle",
        "preamble_emitted",
        serde_json::json!({"event": event, "bytes": preamble.len()}),
    );
    0
}

/// **A hook's answer must speak its host's language.**
///
/// Gemini CLI reads `hookSpecificOutput.tool_input` and merges the object onto
/// the original input (`BeforeToolHookOutput::getModifiedToolInput()`). Qwen
/// Code reads `hookSpecificOutput.updatedInput` and additionally REQUIRES
/// `permissionDecision` + `permissionDecisionReason` — its shipped docs mark
/// them "REQUIRED" (<https://qwenlm.github.io/qwen-code-docs/en/users/features/hooks/>).
/// The same object posted under the other name rewrites nothing and says
/// nothing. "I am touching nothing", in the host's dialect.
///
/// [`substitution_output`] with `null` would produce `updatedInput: null`,
/// which is not a passthrough but a substitution by nothing. Both hosts have a
/// shape for that, and it is not the same one: that is all this knows.
pub(crate) fn passthrough_output() -> serde_json::Value {
    if agent_from_argv() == "qwen-code" {
        return serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "allow",
                "permissionDecisionReason": "tokenade left this call untouched",
            }
        });
    }
    serde_json::json!({ "decision": "allow" })
}

pub(crate) fn substitution_output(changed: serde_json::Value) -> serde_json::Value {
    if agent_from_argv() == "qwen-code" {
        return serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "allow",
                "permissionDecisionReason": "tokenade compacted this call's payload",
                "updatedInput": changed,
            }
        });
    }
    serde_json::json!({
        "decision": "allow",
        "hookSpecificOutput": {
            "hookEventName": "BeforeTool",
            "tool_input": changed,
        }
    })
}

pub(crate) fn cmd_gemini_hook() -> i32 {
    let mut input = String::new();
    if crate::bounded_input::read_stdin_lossy(&mut input).is_err() {
        // If we can't read stdin, allow passthrough — don't block Gemini.
        println!("{{\"decision\":\"allow\"}}");
        return 0;
    }
    let v: serde_json::Value = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(_) => {
            println!("{{\"decision\":\"allow\"}}");
            return 0;
        }
    };
    // This process IS the Gemini hook, so attribute any savings we record to
    // gemini-cli regardless of ambient env (TOKENADE_AGENT is highest-precedence
    // in detect::from_env).
    std::env::set_var("TOKENADE_AGENT", agent_from_argv());
    let tool_name = v["tool_name"].as_str().unwrap_or("");
    if tool_name == "run_shell_command" {
        if let Some(cmd) = v["tool_input"]["command"].as_str() {
            // Don't double-wrap.
            if !cmd.starts_with("tokenade wrap ")
                && !cmd.contains(" wrap '")
                && !cmd.starts_with("tokenade gemini-hook")
                && !cmd.contains("TOKENADE_HOOK_DISABLED")
            {
                // **On Windows, rewriting NOTHING beats rewriting wrong.**
                //
                // These hosts run `run_shell_command` through `cmd.exe /c` on
                // win32, where `'…'` quotes nothing. MEASURED 2026-09-12 on a
                // Windows VM: the rewrite went out, and the command returned
                // ZERO bytes with exit code 1 ("'tokenade' is not
                // recognized") — qwen-code's `cmd` cell marked `Ko`, i.e.
                // worse than no hook at all: the agent loses its command's
                // output. An announced passthrough lets rank 4 and
                // `tokenade wrap` do the work.
                //
                // A correct rewrite for `cmd.exe` (doubled quotes, escaped
                // `%`) remains to be written AND verified on Windows; guessing
                // it here would trade one outage for another.
                if cfg!(target_os = "windows") {
                    tokenade_core::debug_log::log(
                        "gemini-hook",
                        "windows_passthrough",
                        serde_json::json!({
                            "why": "cmd.exe does not read this rewrite's POSIX quoting",
                            "cmd_len": cmd.len(),
                        }),
                    );
                    println!("{}", passthrough_output());
                    return 0;
                }
                // The ABSOLUTE path, as the Claude Code hook already does
                // (`bash_pretool`): a bare name assumes `tokenade` is on the
                // agent shell's PATH, which is false as soon as it is
                // installed elsewhere — and the failure does not say why.
                let exe = std::env::current_exe()
                    .ok()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "tokenade".to_string());
                // Single-quote escape: ' → '\''
                let escaped = cmd.replace('\'', "'\\''");
                let exe_q = exe.replace('\'', "'\\''");
                let wrapped = format!("'{exe_q}' wrap '{escaped}'");
                let out = substitution_output(serde_json::json!({ "command": wrapped }));
                println!("{out}");
                return 0;
            }
        }
    } else if is_delegation_tool(tool_name) {
        // Gemini CLI's experimental `delegate_to_agent`. Its arguments are
        // whatever the agent DEFINITION declares, plus the reserved
        // `agent_name`, so there is no fixed prompt field to aim at — we
        // prefix the first argument that names instructions and leave a
        // definition carrying none alone.
        //
        // `BeforeToolHookOutput::getModifiedToolInput()` reads
        // `hookSpecificOutput.tool_input` and the host MERGES it over the
        // original, so naming the one changed key is right.
        let ti = &v["tool_input"];
        if let Some((key, original)) = delegation_prompt_field(ti) {
            let subagent_type = ti["agent_name"].as_str().unwrap_or("");
            if let Some(briefed) = crate::hooks::brief_prompt(subagent_type, original) {
                let out = substitution_output(serde_json::json!({ key: briefed }));
                println!("{out}");
                return 0;
            }
        }
    } else if tool_name == "read_file" || tool_name == "read_many_files" {
        let ti = &v["tool_input"];
        // Only redirect a WHOLE-file read. With offset/limit the line range
        // targets the ORIGINAL file; pointing it at a compacted temp would read
        // the wrong lines (Gemini merges our input over the original — caveat
        // from coreToolHookTriggers.ts). Skip the redirect when a range is set.
        let has_range = !ti.get("offset").map(|x| x.is_null()).unwrap_or(true)
            || !ti.get("limit").map(|x| x.is_null()).unwrap_or(true);
        if !has_range {
            if let Some(fp) = ti["file_path"].as_str() {
                let cwd = v["cwd"].as_str();
                // Image reads → downscale an oversized PNG/JPEG to a smaller
                // temp (fewer vision tokens). Falls back to text compaction for
                // non-images.
                if let Some(temp) = gemini_downscale_image_to_temp(fp, cwd)
                    .or_else(|| compact_read_to_temp(fp, cwd, v["session_id"].as_str()))
                {
                    let out = substitution_output(serde_json::json!({ "file_path": temp }));
                    println!("{out}");
                    return 0;
                }
            }
        }
    }
    println!("{{\"decision\":\"allow\"}}");
    0
}

/// Read `file_path`, compact it, and write the compacted form to a temp file
/// inside `<cwd>/.tokenade/reads/` (kept within the workspace so Gemini's
/// `read_file` path-resolution accepts it).
///
/// Named for Gemini because Gemini was the only host that could use it. Cursor
/// is the second: measured 2026-08-19 against a real `cursor-agent`, its
/// `preToolUse` fires for `tool_name: "Read"` and an `updated_input.file_path`
/// in the answer redirects the read — the agent quoted the redirect target's
/// line back, not the original's. Two hosts, one redirect, one function. Returns the absolute temp path when
/// compaction actually shrank the content (and records the saving); `None`
/// otherwise (caller leaves the read untouched). Best-effort — any IO error or
/// non-gain → `None`.
pub(crate) fn compact_read_to_temp(
    file_path: &str,
    cwd: Option<&str>,
    session: Option<&str>,
) -> Option<String> {
    use tokenade_core::auto;
    // The stash is the way back: served as stored (see `is_a_stashed_copy`).
    if crate::hooks::read_staging::is_a_stashed_copy(file_path) {
        return None;
    }
    // Lossy UTF-8 so a legacy-encoded / non-UTF-8 file still compacts (the
    // compactor is byte-tolerant) instead of erroring out the whole read.
    let raw = String::from_utf8_lossy(&fs::read(file_path).ok()?).into_owned();
    if raw.len() < 4096 {
        return None; // small read — nothing worth redirecting
    }
    let (_kind, compacted, _sav) = auto::compact_with_path_hint(&raw, file_path);
    // BOUND it, don't just compact it. A per-format compactor is a bet that we
    // recognised the format, and on a plain `.txt` it loses: measured
    // 2026-08-19 on a 104 000-byte log, `compact_with_path_hint` returned it
    // unchanged, `after >= before` closed the gate, and Gemini read all 104 000
    // bytes — while the same file through `tokenade read` came back at 1 475.
    // Two agents (gemini-cli, qwen-code) got NO compaction on any format the
    // compactor did not recognise, which is precisely the class the universal
    // backstop exists for. `bound_delivery` is that backstop, and it is now the
    // one place the rule lives rather than the fourth copy of it.
    let compacted = crate::compactors::bound_delivery(
        compacted,
        tokenade_core::disclose::SourceHint::File(PathBuf::from(file_path)),
        "gemini-read-compact",
    );
    // A REPEAT of the same read is answered with a pointer, not the content
    // again — the same rule, and the same implementation, as every other
    // delivery path. It needs the session, because the claim it makes is about
    // a context: Gemini hands us `session_id` on every hook payload, so this
    // one can prove what it asserts. Without a session it never folds.
    let compacted = match session {
        Some(sid) => {
            let cwd_path = cwd.map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
            // The session is PASSED, not smuggled through the environment.
            // This used to `set_var("TOKENADE_SESSION", sid)` around the call
            // and put it back afterwards — a process-global write on a live
            // path, which raced with every other reader of that variable.
            let raw_fp = tokenade_core::dedup_ref::hash_content(compacted.as_bytes());
            let folded = crate::wrap::fold_or_register_dedup(
                compacted,
                crate::wrap::DedupSite {
                    channel: "read",
                    op: "read_dedup",
                    cwd: &cwd_path,
                    key: &format!("gemini-read:{file_path}"),
                    shell_cmd: "",
                    safe_label: &redact::redact(file_path),
                    filter_label: "gemini-read-compact",
                    session: Some(sid),
                    // This bridge is handed already-compacted text and has no
                    // rawer form to offer; keyed on what it has.
                    raw_fingerprint: &raw_fp,
                },
            );
            folded
        }
        None => compacted,
    };
    let before = tokenade_core::tokenizer::estimate(&raw);
    let after = tokenade_core::tokenizer::estimate(&compacted);
    if after >= before {
        return None; // no real gain — let Gemini read the original
    }
    // Workspace-relative temp dir (absolute path required by read_file.build).
    let base = cwd.map(PathBuf::from).unwrap_or_else(|| {
        Path::new(file_path)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default()
    });
    let dir = base.join(".tokenade").join("reads");
    fs::create_dir_all(&dir).ok()?;
    let stem = Path::new(file_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("file");
    let temp = dir.join(format!("{stem}.compact.txt"));
    fs::write(&temp, &compacted).ok()?;
    log_gain(
        file_path,
        "gemini-read-compact",
        tokenade_core::Savings::new(before, after),
    );
    Some(temp.to_string_lossy().into_owned())
}

/// Downscale an oversized PNG/JPEG read to a temp inside `<cwd>/.tokenade/reads/`
/// so the model sees a smaller image (fewer vision tokens). Returns the temp
/// path when a real shrink happened; `None` for non-images, images already
/// within the target, or any IO/decode error. Lossy — controlled by
/// `TOKENADE_IMAGE_MAX_EDGE` (default = Anthropic's 1568px cap, i.e. a no-op for
/// already-capped providers; lower it to trade quality for tokens).
pub(crate) fn gemini_downscale_image_to_temp(file_path: &str, cwd: Option<&str>) -> Option<String> {
    use tokenade_core::image_downscale as img;
    let base = cwd.map(PathBuf::from).unwrap_or_else(|| {
        Path::new(file_path)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default()
    });
    let staged = img::prepare_downscaled_read(Path::new(file_path), &base)?;
    // Estimated vision-token saving from the pixel-area reduction.
    let before = img::vision_token_estimate(staged.orig_w, staged.orig_h) as usize;
    let after = img::vision_token_estimate(staged.new_w, staged.new_h) as usize;
    log_gain(
        file_path,
        "image-downscale",
        tokenade_core::Savings::new(before, after).billed(),
    );
    Some(staged.redirect_path.to_string_lossy().into_owned())
}

/// A tool that starts a SUBAGENT. Broad on purpose: the name varies by host
/// (`delegate_to_agent` on Gemini CLI and Qwen Code, `task` elsewhere), and a
/// false positive costs nothing — without an instructions field the caller
/// does nothing.
pub(crate) fn is_delegation_tool(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["task", "agent", "delegate", "subagent", "spawn"]
        .iter()
        .any(|w| n.contains(w))
}

/// The argument carrying a subagent's instructions, as it presents itself.
/// `agent_name` is Gemini's reserved discriminator and never the instructions.
pub(crate) fn delegation_prompt_field(input: &serde_json::Value) -> Option<(&'static str, &str)> {
    for key in [
        "prompt",
        "message",
        "instructions",
        "task",
        "goal",
        "context",
    ] {
        if let Some(s) = input.get(key).and_then(|v| v.as_str()) {
            if !s.trim().is_empty() {
                return Some((key, s));
            }
        }
    }
    None
}
