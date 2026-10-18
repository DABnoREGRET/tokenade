//! **The hooks specific to Claude Code** — those with no equivalent
//! elsewhere.
//!
//! LSP enforcement, the post-MCP hook, the post-web-search hook, the
//! post-tool hook, and their REMOVALS. A hook is put in and taken out: an
//! installer without its removal leaves an agent calling a binary that was
//! just uninstalled, and the user reads the failure as their agent breaking.
//! So the two halves live side by side.

use super::*;

/// Install the default PreToolUse hook into `<root>/.claude/settings.json`,
/// matcher `Bash|Read|WebFetch|Task|Agent` → `tokenade hookwrap-pretool`.
/// Despite the legacy name (it once gated a Grep/Glob "LSP enforcement"
/// redirect behind `--with-lsp-enforcement-hook`), since the 2026-06 pivot
/// this is installed by DEFAULT (gated only by `--no-hook`), Grep/Glob are
/// excluded, and the flag is a deprecated no-op. The hook's live levers:
/// Bash rewrite, Read dedup/staging, WebFetch fetch-and-replace, and the
/// Task/Agent subagent-preamble injection. Per-call logic lives in
/// `cmd_hookwrap_pretool`. Idempotent (`_tag: tokenade-pretool`); the old
/// `tokenade-lsp` tag is migrated away on upgrade.
pub(crate) fn install_claude_code_lsp_enforcement_hook(
    root: &Path,
    exe: &str,
    debug_log_path: Option<&str>,
    global: bool,
    home: Option<&Path>,
    dry_run: bool,
) -> HookOutcome {
    let (path, mut settings) = match open_claude_settings(root, global, home, dry_run) {
        Ok(v) => v,
        Err(o) => return o,
    };
    if !settings.is_object() {
        return HookOutcome::Failed(format!(
            "{} root is not a JSON object — left untouched",
            path.display()
        ));
    }
    // On Windows the hook commands are direct exe calls (no inline `set`
    // env injection — see build_hook_command), so the debug-log path is
    // carried by the settings.json global `env` block instead. Idempotent;
    // a no-op on Unix and when no debug-log path was requested.
    ensure_debug_log_env(&mut settings, debug_log_path);
    let hooks = settings
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert(serde_json::json!({}));
    // #486: clobber non-object `hooks` with `{}` so the entry()
    // recursion below has a real map to insert into.
    if !hooks.is_object() {
        *hooks = serde_json::json!({});
    }
    let pre_array = ensure_array_entry(hooks.as_object_mut().unwrap(), "PreToolUse");
    // Migration: old installs used `_tag: "tokenade-lsp"` for a
    // Grep/Glob/Read-only matcher. If we find one, drop it so we can
    // install the new wider `Bash|Grep|Glob|Read` matcher under the
    // `tokenade-pretool` tag (preserves user intent: "tokenade is
    // intercepting tool calls").
    pre_array.retain(|g| {
        g.get("hooks")
            .and_then(|h| h.as_array())
            .map(|arr| {
                !arr.iter().any(|h| {
                    h.get("_tag")
                        .and_then(|t| t.as_str())
                        .map(|s| s == "tokenade-lsp")
                        .unwrap_or(false)
                })
            })
            .unwrap_or(true)
    });
    // #561 — canonical entry that the install path WANTS to have. We
    // compare an existing `tokenade-pretool`-tagged entry against this
    // shape: if it matches, return AlreadyPresent; if it has drifted
    // (matcher widened, command path changed, etc.), REMOVE the stale
    // entry so the push at the bottom installs the fresh one. Live
    // production caught this when the matcher was widened to include
    // `WebFetch` (#426) — existing users were stuck without it because
    // the old idempotency check was pure tag-presence.
    // Matcher covers exactly the tools `cmd_hookwrap_pretool` has a live
    // handler for: Bash (rewrite via the safe allow+updatedInput channel),
    // Read (dedup / head-slice staging, allow+updatedInput), WebFetch
    // (fetch-and-replace via deny+✓-marker), and Task/Agent (inject the
    // tokenade preamble into spawned subagents). Grep/Glob are DELIBERATELY
    // excluded: their only substitution channel is PreToolUse `deny`, which
    // Claude Code wraps in <error>…</error> — a successful redirect reads
    // as a failure and risks a wasted retry, so their handlers were removed.
    // (A 2026-06 pivot edit narrowed this to `Bash` alone, which silently
    // disabled the Read/WebFetch/subagent levers even though their handlers
    // and the staging↔postread dance stayed live — restored here.) An
    // existing drifted matcher is auto-healed: the drift check below removes
    // it and installs this canonical set.
    // `PowerShell` (#…, 2026-06): Claude Code's PowerShell tool, used on
    // Windows without Git Bash. Without it in the matcher, the PowerShell-tool
    // path got ZERO compaction (no Bash tool → no Bash hook → nothing). The
    // drift-heal below upgrades existing installs from the old matcher set.
    // Write/Edit joined in 0.7.6: the output-side Write steer (write_pretool)
    // and the Edit batching nudge live in hookwrap-pretool — without them in
    // the matcher those handlers never ran on real installs (0.7.5 shipped
    // the Write nudge dead-on-arrival; caught by config audit, not bench).
    // WebSearch joined in 0.8.13: the client-side search gate
    // (websearch_pretool) — same dead-on-arrival class as Write/Edit if
    // the matcher lags the router (see the structural guard test below).
    let canonical_matcher = "Bash|Read|Grep|WebFetch|WebSearch|Task|Agent|PowerShell|Write|Edit";
    let canonical_command = build_hook_command(exe, "hookwrap-pretool", debug_log_path);
    let mut stale_idx: Option<usize> = None;
    let mut already = false;
    for (i, g) in pre_array.iter().enumerate() {
        let (matched, has_tag) =
            group_matches_tokenade_hook(g, "tokenade-pretool", "hookwrap-pretool");
        if !matched {
            continue;
        }
        let matcher_matches = g
            .get("matcher")
            .and_then(|m| m.as_str())
            .map(|s| s == canonical_matcher)
            .unwrap_or(false);
        let command_matches = g
            .get("hooks")
            .and_then(|h| h.as_array())
            .and_then(|arr| arr.first())
            .and_then(|h| h.get("command"))
            .and_then(|c| c.as_str())
            .map(|s| s == canonical_command)
            .unwrap_or(false);
        if matcher_matches && command_matches && has_tag {
            already = true;
        } else {
            // Drift OR missing `_tag` (untagged-but-recognised) —
            // both heal in place (#775).
            stale_idx = Some(i);
        }
        break;
    }
    // #564 — do NOT early-return on PreToolUse `already`; the
    // function still needs to upsert UserPromptSubmit / SessionStart
    // / PreCompact below. Track the state in flags and combine at
    // the end. `pre_was_op` = false → PreToolUse was already
    // canonical (no work). `pre_was_refresh` = true → we replaced
    // a drifted entry.
    let pre_was_op = !already;
    let pre_was_refresh = stale_idx.is_some();
    if let Some(idx) = stale_idx {
        pre_array.remove(idx);
    }
    if pre_was_op {
        // Push the canonical entry. Matcher is `Bash`:
        // hookwrap-pretool rewrites the command to `tokenade wrap '<cmd>'`
        // via allow+updatedInput so the model sees compacted output.
        pre_array.push(serde_json::json!({
            "matcher": canonical_matcher,
            "hooks": [{
                "type": "command",
                "command": canonical_command,
                "_tag": "tokenade-pretool"
            }]
        }));
    }
    // #565 — UserPromptSubmit / SessionStart / PreCompact were
    // extracted to `install_claude_code_passive_hooks`. This
    // function's outcome reduces to just the PreToolUse state.
    let outcome = |p: PathBuf| -> HookOutcome {
        if !pre_was_op {
            HookOutcome::AlreadyPresent
        } else if pre_was_refresh {
            HookOutcome::Refreshed(p)
        } else {
            HookOutcome::Installed(p)
        }
    };
    if dry_run {
        return outcome(path);
    }
    if let Err(e) = write_json_atomic(&path, &serde_json::to_string_pretty(&settings).unwrap()) {
        return HookOutcome::Failed(format!("write {}: {e}", path.display()));
    }
    outcome(path)
}

/// Probe-and-wire of the MCP folder (`hookwrap-mcp-post`).
///
/// **What this hook replaces.** An MCP tool result was folded by no hook
/// — no `matcher` caught `mcp__<server>__<tool>` — and the only coverage
/// was `tokenade mcp-wrap install`, which wraps the LAUNCH of each server.
/// The Claude Code documentation is explicit on the point that makes the
/// proxy unnecessary here: "MCP tool output is passed through without
/// schema validation", so `updatedToolOutput` accepts whatever shape we
/// hand back, where a native tool requires its output schema.
///
/// Same capability probe as the WebSearch folder. It is CONSERVATIVE: the
/// replacement existed for MCP before 2.1.121 (that version's changelog says
/// "previously MCP-only"), but no source says since when. An unknown field on
/// an older host produces a visible error notice, and a red notice on every
/// tool call costs more than it returns: below the probe, `mcp-wrap` stays
/// the route, exactly as before.
pub(crate) fn sync_claude_code_mcp_post_hook(
    root: &Path,
    exe: &str,
    debug_log_path: Option<&str>,
    global: bool,
    home: Option<&Path>,
    dry_run: bool,
) -> (bool, HookOutcome) {
    let capable = super::capability::claude_supports_updated_tool_output();
    let outcome = if capable {
        install_claude_code_posttool_hook(
            root,
            exe,
            debug_log_path,
            global,
            home,
            dry_run,
            // MCP tools are named `mcp__<server>__<tool>` and the
            // `matcher` is a regular expression.
            "mcp__.*",
            "hookwrap-mcp-post",
            "tokenade-mcp-post",
        )
    } else {
        remove_claude_code_posttool_hook(
            root,
            global,
            home,
            dry_run,
            "hookwrap-mcp-post",
            "tokenade-mcp-post",
        )
    };
    (capable, outcome)
}

/// Probe-and-wire for the WebSearch PostToolUse compactor
/// (`hookwrap-websearch-post`): the hook replaces the WebSearch
/// tool_response via `hookSpecificOutput.updatedToolOutput`, which only
/// Claude Code >= 2.1.121 honours — so the installer VALIDATES the
/// capability (see `capability::claude_supports_updated_tool_output`) and
/// installs the entry only when it works, or REMOVES a previously
/// installed entry when the capability disappeared (downgrade / different
/// claude on PATH). Re-running install (the update path) re-evaluates.
/// Returns `(capable, outcome)` so the caller can phrase its report.
pub(crate) fn sync_claude_code_websearch_post_hook(
    root: &Path,
    exe: &str,
    debug_log_path: Option<&str>,
    global: bool,
    home: Option<&Path>,
    dry_run: bool,
) -> (bool, HookOutcome) {
    let capable = super::capability::claude_supports_updated_tool_output();
    let outcome = if capable {
        install_claude_code_websearch_post_hook(root, exe, debug_log_path, global, home, dry_run)
    } else {
        remove_claude_code_websearch_post_hook(root, global, home, dry_run)
    };
    (capable, outcome)
}

/// Install the PostToolUse `WebSearch` compaction hook
/// (`hookwrap-websearch-post`). Tagged `tokenade-websearch-post` for
/// idempotent re-install / drift-heal / clean uninstall (the generic
/// `strip_tokenade_hooks_from_settings` covers the tag AND the
/// ` hookwrap` command fallback). Mirrors
/// `install_claude_code_postwebfetch_hook` structurally. Callers pass an
/// `exe` from `stable_hook_exe` (0.8.5 rule: never a temp
/// `.tokenade.new.<pid>` / build-dir path).
pub(crate) fn install_claude_code_websearch_post_hook(
    root: &Path,
    exe: &str,
    debug_log_path: Option<&str>,
    global: bool,
    home: Option<&Path>,
    dry_run: bool,
) -> HookOutcome {
    install_claude_code_posttool_hook(
        root,
        exe,
        debug_log_path,
        global,
        home,
        dry_run,
        "WebSearch",
        "hookwrap-websearch-post",
        "tokenade-websearch-post",
    )
}

/// The shared body of Claude Code's `PostToolUse` folders: same structure,
/// three differing strings (the `matcher`, the subcommand and the
/// idempotence label). It served ONE caller and serves two since the MCP
/// hook; writing the same hundred lines a second time would have made the
/// two diverge at the first fix.
#[allow(clippy::too_many_arguments)]
pub(crate) fn install_claude_code_posttool_hook(
    root: &Path,
    exe: &str,
    debug_log_path: Option<&str>,
    global: bool,
    home: Option<&Path>,
    dry_run: bool,
    canonical_matcher: &str,
    sub: &str,
    tag: &str,
) -> HookOutcome {
    let (path, mut settings) = match open_claude_settings(root, global, home, dry_run) {
        Ok(v) => v,
        Err(o) => return o,
    };
    ensure_debug_log_env(&mut settings, debug_log_path);
    let hooks = settings
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert(serde_json::json!({}));
    if !hooks.is_object() {
        *hooks = serde_json::json!({});
    }
    let post_array = ensure_array_entry(hooks.as_object_mut().unwrap(), "PostToolUse");
    let canonical_command = build_hook_command(exe, sub, debug_log_path);
    let mut stale_idx: Option<usize> = None;
    let mut already = false;
    for (i, g) in post_array.iter().enumerate() {
        let (matched, has_tag) = group_matches_tokenade_hook(g, tag, sub);
        if !matched {
            continue;
        }
        let matcher_matches = g
            .get("matcher")
            .and_then(|m| m.as_str())
            .map(|s| s == canonical_matcher)
            .unwrap_or(false);
        let command_matches = g
            .get("hooks")
            .and_then(|h| h.as_array())
            .and_then(|arr| arr.first())
            .and_then(|h| h.get("command"))
            .and_then(|c| c.as_str())
            .map(|s| s == canonical_command)
            .unwrap_or(false);
        if matcher_matches && command_matches && has_tag {
            already = true;
        } else {
            stale_idx = Some(i);
        }
        break;
    }
    if already {
        return HookOutcome::AlreadyPresent;
    }
    let was_refresh = stale_idx.is_some();
    if let Some(idx) = stale_idx {
        post_array.remove(idx);
    }
    post_array.push(serde_json::json!({
        "matcher": canonical_matcher,
        "hooks": [{
            "type": "command",
            "command": canonical_command,
            "_tag": tag
        }]
    }));
    let outcome = if was_refresh {
        HookOutcome::Refreshed(path.clone())
    } else {
        HookOutcome::Installed(path.clone())
    };
    if dry_run {
        return outcome;
    }
    match write_json_atomic(&path, &serde_json::to_string_pretty(&settings).unwrap()) {
        Ok(()) => outcome,
        Err(e) => HookOutcome::Failed(format!("write {}: {e}", path.display())),
    }
}

/// Remove the WebSearch PostToolUse entry (capability disappeared or a
/// downgraded Claude Code was detected at install/update time).
/// `AlreadyPresent` = nothing to remove; `Refreshed(path)` = removed.
pub(crate) fn remove_claude_code_websearch_post_hook(
    root: &Path,
    global: bool,
    home: Option<&Path>,
    dry_run: bool,
) -> HookOutcome {
    remove_claude_code_posttool_hook(
        root,
        global,
        home,
        dry_run,
        "hookwrap-websearch-post",
        "tokenade-websearch-post",
    )
}

/// The generic counterpart of `install_claude_code_posttool_hook`.
pub(crate) fn remove_claude_code_posttool_hook(
    root: &Path,
    global: bool,
    home: Option<&Path>,
    dry_run: bool,
    sub: &str,
    tag: &str,
) -> HookOutcome {
    let Some(path) = hook_settings_path(root, global, home) else {
        return HookOutcome::Failed("no home dir resolved for global install".into());
    };
    let Ok(raw) = fs::read_to_string(&path) else {
        return HookOutcome::AlreadyPresent; // no settings file → nothing to remove
    };
    let mut settings: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            return HookOutcome::Failed(format!(
                "unparsable {} — left untouched: {e}",
                path.display()
            ))
        }
    };
    let Some(post_array) = settings
        .get_mut("hooks")
        .and_then(|h| h.get_mut("PostToolUse"))
        .and_then(|a| a.as_array_mut())
    else {
        return HookOutcome::AlreadyPresent;
    };
    let before = post_array.len();
    post_array.retain(|g| !group_matches_tokenade_hook(g, tag, sub).0);
    if post_array.len() == before {
        return HookOutcome::AlreadyPresent;
    }
    if dry_run {
        return HookOutcome::Refreshed(path);
    }
    match write_json_atomic(&path, &serde_json::to_string_pretty(&settings).unwrap()) {
        Ok(()) => HookOutcome::Refreshed(path),
        Err(e) => HookOutcome::Failed(format!("write {}: {e}", path.display())),
    }
}
