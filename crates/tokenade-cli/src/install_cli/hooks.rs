//! **Installing Claude Code's hooks.**
//!
//! We rewrite a settings file the user owns and that another tool may also
//! write. The operation is idempotent, it preserves what is not ours, and it
//! is reversible — all three together, otherwise installing twice breaks
//! something.

use super::*;

/// Everything Claude Code gets: settings.json hooks, the shell proxy, the
/// WebSearch gate, and their dry-run reports.
///
/// 172 lines of one agent's wiring, lifted out of `install_mcp_for_targets`
/// where it sat between the plugin table and the Codex block. No early returns
/// and nothing read back — it only prints and writes.
pub(super) fn install_claude_code_hooks(plan: &InstallPlan) {
    let &InstallPlan {
        root,
        exe,
        home,
        debug_log_path,
        install_global,
        install_hook,
        dry_run,
        autocompact_pct: pct_flag,
        no_autocompact,
        ..
    } = plan;
    // #556 — TOKENADE_NO_HOOK is now checked inside each
    // installer; the outer guard reduces to just `--no-hook`.
    if install_hook {
        let (glyph, verb_pfx) = if dry_run {
            ("◐", "would install ")
        } else {
            ("✓", "")
        };
        match install_claude_code_hook(root, exe, debug_log_path, install_global, home, dry_run) {
            HookOutcome::Installed(path) => {
                println!(
                    "  {glyph} {verb_pfx}Claude Code PostToolUse hook → {}",
                    path.display()
                );
            }
            HookOutcome::Refreshed(path) => {
                println!(
                    "  ✓ refreshed Claude Code PostToolUse hook (matcher/command updated) → {}",
                    path.display()
                );
            }
            HookOutcome::AlreadyPresent => {
                println!("  ✓ Claude Code PostToolUse hook already present");
            }
            HookOutcome::OptedOut => {
                println!("  ✓ hook skipped via TOKENADE_NO_HOOK");
            }
            HookOutcome::Failed(e) => {
                eprintln!("  ✗ Claude Code hook install: {e}");
            }
        }
        // Auto-compaction threshold: ask Claude Code to summarize its own
        // conversation EARLIER — at 70% of the context window instead of the
        // native ~95% — so the cached prefix stays smaller (cheaper
        // cache_read every turn + cheaper re-creation on cache expiry).
        // Opt-out: TOKENADE_AUTOCOMPACT=0. Never clobbers a user-set value.
        if let Some(sp) = hook_settings_path(root, install_global, home) {
            if no_autocompact {
                // `--no-autocompact` REMOVES what we had set. Without this
                // removal, disabling afterwards only skipped the write: the
                // old value stayed in settings.json and the agent kept
                // summarising itself early, which is exactly the complaint
                // we are trying to address.
                match crate::install::strip_autocompact_override(home, dry_run) {
                    Some(note) => println!("  {glyph} {note}"),
                    None => println!(
                        "  {glyph} no tokenade auto-compact threshold to remove \
                         (a hand-set value is left alone)"
                    ),
                }
            } else {
                use crate::install::AutoCompact;
                match set_autocompact_window(&sp, dry_run, pct_flag) {
                    Ok(AutoCompact::Set(t, pct)) => println!(
                        "  {glyph} {verb_pfx}auto-compact threshold → {t} tokens \
                         ({pct}% of context window; change it with \
                         `tokenade install --autocompact-pct N`, remove it with \
                         `--no-autocompact`)"
                    ),
                    // Say what we left alone. Silent, the user who edited the
                    // file does not know whether their edit holds.
                    Ok(AutoCompact::HandSet(t)) => println!(
                        "  {glyph} auto-compact threshold left at your own {t} tokens \
                         (tokenade will not touch it again; \
                         `--autocompact-pct N` takes it back)"
                    ),
                    Ok(AutoCompact::Skipped) => {}
                    Err(e) => eprintln!("  ✗ auto-compact threshold: {e}"),
                }
            }
        }
        // Read-delivery hook: PostToolUse on Read. Second half of
        // the allow+updatedInput Read flow — delivers staged Read
        // compactions as additionalContext so the PreToolUse side
        // can ALLOW (and mark) the Read instead of denying it.
        // On global install, drops ~/.tokenade/postread-hook-ok,
        // which is what actually switches the PreToolUse side off
        // the legacy deny flow.
        // Four hook installs said the SAME 28 lines, differing only in the
        // installer and four labels. Verified before folding: normalising away the
        // function and the strings left ONE hash for these four, and a different
        // one for the base hook and the settings-path check — which is why those
        // keep their own blocks.
        //
        // The FULL label is stored per arm rather than a stem plus a shared
        // suffix: `pre_edit` fails with "pre-edit snapshot hook: {e}" where the
        // others say "… hook install: {e}". A stem would have silently rewritten
        // that one line.
        type ClaudeHook = fn(&Path, &str, Option<&str>, bool, Option<&Path>, bool) -> HookOutcome;
        const CLAUDE_HOOKS: &[(ClaudeHook, &str, &str, &str, &str)] = &[
            (
                install_claude_code_postread_hook as ClaudeHook,
                "Claude Code Read-delivery hook (PostToolUse Read)",
                "Claude Code Read-delivery hook (matcher/command updated)",
                "Claude Code Read-delivery hook",
                "Claude Code Read-delivery hook install",
            ),
            (
                install_claude_code_index_hook as ClaudeHook,
                "Claude Code auto-index hook (Edit/Write/MultiEdit)",
                "Claude Code auto-index hook (matcher/command updated)",
                "Claude Code auto-index hook",
                "Claude Code auto-index hook install",
            ),
            (
                install_claude_code_pre_edit_hook as ClaudeHook,
                "Claude Code pre-edit snapshot hook (Edit/Write/MultiEdit)",
                "Claude Code pre-edit snapshot hook (matcher/command updated)",
                "Claude Code pre-edit snapshot hook",
                "pre-edit snapshot hook",
            ),
            (
                install_claude_code_passive_hooks as ClaudeHook,
                "Claude Code passive hooks (UserPromptSubmit/SessionStart/PreCompact)",
                "Claude Code passive hooks (command updated)",
                "Claude Code passive hooks",
                "Claude Code passive hooks install",
            ),
        ];
        for (install, installed, refreshed, present, failed) in CLAUDE_HOOKS {
            match install(root, exe, debug_log_path, install_global, home, dry_run) {
                HookOutcome::Installed(path) => {
                    println!("  {glyph} {verb_pfx}{installed} → {}", path.display())
                }
                HookOutcome::Refreshed(path) => {
                    println!("  ✓ refreshed {refreshed} → {}", path.display())
                }
                HookOutcome::AlreadyPresent => println!("  ✓ {present} already present"),
                HookOutcome::OptedOut => {}
                HookOutcome::Failed(e) => eprintln!("  ✗ {failed}: {e}"),
            }
        }
        // WebSearch compaction hook: PostToolUse on WebSearch replaces
        // the tool_response via `hookSpecificOutput.updatedToolOutput`
        // (cross-call URL dedup + link slimming). CAPABILITY-GATED:
        // only Claude Code >= 2.1.121 honours the field, so the
        // installer probes the local binary (byte marker, version
        // fallback) and installs the entry only when it works — and
        // removes a stale entry when the capability disappeared.
        // Re-running install (the update path) re-evaluates the probe.
        {
            let (ws_capable, ws_outcome) = sync_claude_code_websearch_post_hook(
                root,
                exe,
                debug_log_path,
                install_global,
                home,
                dry_run,
            );
            match ws_outcome {
                HookOutcome::Installed(path) => println!(
                    "  {glyph} {verb_pfx}Claude Code WebSearch compaction hook (PostToolUse WebSearch) → {}",
                    path.display()
                ),
                HookOutcome::Refreshed(path) if ws_capable => println!(
                    "  ✓ refreshed Claude Code WebSearch compaction hook (matcher/command updated) → {}",
                    path.display()
                ),
                HookOutcome::Refreshed(path) => println!(
                    "  ✓ removed Claude Code WebSearch compaction hook (this Claude Code lacks updatedToolOutput) → {}",
                    path.display()
                ),
                HookOutcome::AlreadyPresent if ws_capable => {
                    println!("  ✓ Claude Code WebSearch compaction hook already present");
                }
                HookOutcome::AlreadyPresent => println!(
                    "  ⊘ WebSearch compaction hook skipped (this Claude Code lacks updatedToolOutput; re-run `tokenade install` after upgrading Claude Code)"
                ),
                HookOutcome::OptedOut => {}
                HookOutcome::Failed(e) => {
                    eprintln!("  ✗ Claude Code WebSearch compaction hook: {e}");
                }
            }
        }
        // MCP folding hook: PostToolUse on `mcp__.*`. An MCP tool's result
        // was folded by NO hook — the only coverage was `mcp-wrap install`,
        // which wraps the launch of each server. Same capability probe as the
        // WebSearch folder.
        {
            let (mcp_capable, mcp_outcome) = sync_claude_code_mcp_post_hook(
                root,
                exe,
                debug_log_path,
                install_global,
                home,
                dry_run,
            );
            match mcp_outcome {
                HookOutcome::Installed(path) => println!(
                    "  {glyph} {verb_pfx}Claude Code MCP folding hook (PostToolUse mcp__.*) → {}",
                    path.display()
                ),
                HookOutcome::Refreshed(path) if mcp_capable => println!(
                    "  ✓ refreshed Claude Code MCP folding hook (matcher/command updated) → {}",
                    path.display()
                ),
                HookOutcome::Refreshed(path) => println!(
                    "  ✓ removed Claude Code MCP folding hook (this Claude Code lacks updatedToolOutput) → {}",
                    path.display()
                ),
                HookOutcome::AlreadyPresent if mcp_capable => {
                    println!("  ✓ Claude Code MCP folding hook already present");
                }
                HookOutcome::AlreadyPresent => println!(
                    "  ⊘ MCP folding hook skipped (this Claude Code lacks updatedToolOutput; `tokenade mcp-wrap install` stays the route)"
                ),
                HookOutcome::OptedOut => {}
                HookOutcome::Failed(e) => {
                    eprintln!("  ✗ Claude Code MCP folding hook: {e}");
                }
            }
        }
        // Auto-index hook: PostToolUse on Edit|Write|MultiEdit. This
        // is the codegraph-parity mechanism — the index is kept warm
        // proactively so search-time latency stays near zero. Always
        // installed alongside the Bash compression hook (both are
        // PostToolUse, both gated on `--with-hook`).
        // Pre-edit snapshot hook: PreToolUse on Edit|Write|MultiEdit.
        // Snapshots the file content into dedup_ref BEFORE the edit so
        // that the PostToolUse file_diff hint can emit a diff even when
        // the agent never Read the file beforehand.
        // #565 — passive hooks (UserPromptSubmit / SessionStart /
        // PreCompact) used to be installed only when the user
        // passed `--with-lsp-enforcement-hook`. They're not LSP-
        // related; install them unconditionally next to the
        // PostToolUse hooks.
    } else {
        // --no-hook is opt-out, so the user explicitly disabled both
        // hooks. Confirm we honoured it — no nag-line suggesting
        // they re-enable.
        println!("  ⊘ Bash compression + auto-index hooks skipped (--no-hook).");
    }
}
