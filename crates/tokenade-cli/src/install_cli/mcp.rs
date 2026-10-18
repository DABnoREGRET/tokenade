//! **Declaring tokenade as an MCP server, for every agent.**
//!
//! Fifteen agents, as many files and shapes. The difficulty is not writing the
//! entry but knowing whether it is ALREADY there, under another name or
//! another shape — a duplicate entry starts two servers fighting over the
//! same state.

use super::*;

#[allow(clippy::too_many_arguments)]
/// What one `tokenade install` run was asked to do.
///
/// These twelve values were twelve positional parameters — five of them
/// `bool`, consecutive, which is the exact shape a caller transposes without
/// the compiler noticing. Named fields, destructured at the top so the body
/// below reads unchanged.
pub(super) struct InstallPlan<'a> {
    pub(super) home: Option<&'a Path>,
    pub(super) root: &'a Path,
    pub(super) exe: &'a str,
    pub(super) debug_log_path: Option<&'a str>,
    pub(super) install_global: bool,
    pub(super) install_hook: bool,
    pub(super) install_pretool_hook: bool,
    pub(super) install_claude_md: bool,
    pub(super) install_brevity: bool,
    pub(super) profile: Option<&'a str>,
    pub(super) dry_run: bool,
    /// The auto-compaction threshold requested on the command line, and its
    /// refusal. Carried by the plan like everything else:
    /// `install_claude_code_hooks` is the only function that writes `settings.json`.
    pub(super) autocompact_pct: Option<u32>,
    pub(super) no_autocompact: bool,
}

pub(super) fn install_mcp_for_targets(targets: &[detect::Agent], plan: &InstallPlan) -> usize {
    let &InstallPlan {
        home,
        root,
        exe,
        debug_log_path,
        install_global,
        install_hook,
        install_pretool_hook,
        install_claude_md,
        install_brevity,
        profile,
        dry_run,
        ..
    } = plan;
    // Wrap-only agents have no output hook and no MCP — surface the
    // one-liner so the installer doesn't silently skip them.
    //
    // "Wrap-only" is the claim to keep true. Two arms here had stopped being
    // it, and the proof was in this command's OWN output (measured 2026-08-08,
    // `install --dry-run` with both agents present):
    //
    //   ◐ openclaw — use `tokenade wrap openclaw …` to compact output
    //   ◐ openclaw — installed tool-result compaction plugin (…)
    //
    // Two adjacent lines, the second contradicting the first: OpenClaw is a
    // row of `PLUGIN_INSTALLS` and of `has_plugin()`, so `coverage()` answers
    // `Plugin`. Its arm is gone — a plugin agent is not wrap-only.
    //
    // Aider's arm said "use `tokenade wrap aider …` to compact /run output",
    // which names the wrong mechanism: `wrap` compacts the whole captured
    // stdout of ONE process at exit, so wrapping the agent has no per-`/run`
    // granularity. What actually folds aider's `/run` is the universal shell
    // proxy intercepting each command — which the `--with-hook` note at the
    // bottom of this same output already said, while this line said otherwise.
    for agent in targets {
        match agent {
            // This line said "no hook" and stopped there, which was true and
            // incomplete: since the install writes the startup launcher
            // (`load:` → `/run`), something does LAUNCH the binary on this
            // agent, and saying so here avoids a `◐` that contradicts the `✓`
            // printed two lines below.
            detect::Agent::Aider => println!(
                "  ◐ {} — no hook; `/run` output is compacted by the universal shell proxy, \
                 `tokenade wrap <cmd>` compacts one command by hand, and the startup command \
                 below runs the repo map through tokenade on every session",
                agent.as_str()
            ),
            // Pi gets a dedicated install line below (it now has a compaction
            // extension, not just the shell proxy), so no "wrap-only" note here.
            detect::Agent::Hermes => println!(
                "  ✓ {} — terminal output compacted via the universal shell proxy; \
                 usage + savings read from ~/.hermes/state.db",
                agent.as_str()
            ),
            _ => {}
        }
    }
    // Hermes (no opt-in): drop the read_file/MCP compaction plugin and enable
    // it in config.yaml. The terminal channel + gain ledger already work via
    // the universal shell proxy and the state.db reader; this plugin adds the
    // file-read / MCP-result compaction Phase 1 can't reach in pure Rust.
    for (agent, install, label, what) in PLUGIN_INSTALLS {
        if !targets.contains(agent) {
            continue;
        }
        let Some(h) = home else { continue };
        match install(h, dry_run) {
            Ok(true) => {
                let g = if dry_run { "◐" } else { "✓" };
                println!("  {g} {label} — {what}");
            }
            // Files on disk, host did NOT take them: the bridge is inert, and
            // saying so is the whole point. `{what}` describes a REGISTERED
            // plugin, so printing it here would claim the one thing that just
            // failed.
            Ok(false) => println!(
                "  ◐ {label} — plugin files written, but {label} did not register them: \
                 NOT active, tool results are not compacted for it"
            ),
            Err(e) => eprintln!("  ✗ {label} plugin install: {e}"),
        }
    }
    // Pi (no opt-in): drop the tool-result compaction extension. Pi auto-
    // discovers ~/.pi/agent/extensions/ (no manifest, no enable-list), so the
    // terminal channel (shell proxy) + read_file/tool-result compaction both
    // work once this lands.
    // opencode (no opt-in): drop the tool-result compaction plugin into its
    // auto-discovery dir (~/.config/opencode/plugin) — read_file/tool-result
    // compaction; terminal stays on the shell proxy.
    // OpenClaw (no opt-in): write + register the agent-tool-result middleware
    // plugin. Covers read_file/tool-result compaction; terminal stays on the
    // shell proxy.
    // Cursor: a programmatic preToolUse hook that rewrites Shell commands to
    // `tokenade wrap …` so their output is compacted (Cursor has no after-hook
    // to substitute output). Merges into ~/.cursor/hooks.json, preserving any
    // existing user hooks.
    // Kilo Code: drop the tool-result compaction plugin into its auto-discovery
    // dir (mirrors OpenCode). read_file/tool-result compaction; terminal stays
    // on the shell proxy.
    // Each detected agent is wired through the hooks + shell proxy below;
    // the count drives the final "installed for N agent(s)" summary.
    let mcp_ok = targets.len();

    // Optional Claude Code hook (only meaningful when Claude Code is in
    // the targets list AND --with-hook is on).
    //
    // The second half of this comment used to read "other agents only get the
    // MCP server entry today — they don't expose a Bash-output hook surface we
    // can wire". That was true before the pivot and is not now: nine agents
    // get lifecycle hooks (`hook_agent_configs`) and six get plugin bridges
    // (`plugin_bridges`). What the note below is actually about is narrower —
    // `--with-hook` names the CLAUDE CODE hook specifically, so passing it
    // while installing for someone else does nothing.
    let claude_in_targets = targets.contains(&detect::Agent::ClaudeCode);
    if !claude_in_targets && install_hook {
        // Name the TARGETS, not the ambient agent. This printed
        // `detect::detect(root)` — whichever agent happens to be running the
        // install — so `tokenade install --only <agent>`, run from inside
        // Claude Code, said "the claude-code agent here has no compatible
        // PostToolUse hook". That is the one agent for which the claim is
        // false, and Claude Code is by far the likeliest thing to be running
        // an install, so the sentence was usually both wrong and about the
        // wrong agent. Measured 2026-08-08 on a fixture home carrying only
        // one other agent's marker.
        let named = targets
            .iter()
            .map(|a| a.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        // The parenthetical was a claim about the HOST, and it is false for
        // at least two of them: tokenade registers a `postToolUse` for
        // Copilot (folding results through `modifiedResult`) and a
        // `PostToolUse` for Codex, and since then a `PreToolUse` for Copilot
        // too. The comment above had already been corrected to say the note is
        // about the FLAG; the printed sentence had not, which is the same shape
        // as the stale `install_cli` line this came from.
        println!(
            // "and the shell proxy" was removed: nothing has installed it
            // since 2026-07 (`install/shell_proxy.rs` says so at its top), and
            // a note naming a vanished mechanism sends the reader looking for
            // it.
            "  note: --with-hook names the Claude Code hook specifically, so it does nothing here; {named} is wired through its own hooks."
        );
    }
    if claude_in_targets {
        install_claude_code_hooks(plan);
    }

    // Codex passive hooks (~/.codex/hooks.json). Recent Codex CLI exposes the
    // SAME lifecycle hooks as Claude Code (Pre/PostToolUse/SessionStart/
    // UserPromptSubmit/PreCompact) with the same stdin+stdout contract, so we
    // wire the identical `hookwrap*` subcommands — deterministic passive
    // compaction for Codex, no MCP (pivot-compatible).
    if targets.contains(&detect::Agent::Codex) && !is_hook_opted_out() {
        let glyph = if dry_run { "◐" } else { "✓" };
        let verb_pfx = if dry_run { "would install " } else { "" };
        let outcome = install_codex_passive_hooks(exe, debug_log_path, home, dry_run);
        let hooks_present = matches!(
            outcome,
            HookOutcome::Installed(_) | HookOutcome::Refreshed(_) | HookOutcome::AlreadyPresent
        );
        match &outcome {
            HookOutcome::Installed(path) => println!(
                "  {glyph} {verb_pfx}Codex passive hooks (Pre/PostToolUse/SessionStart/…) → {}",
                path.display()
            ),
            HookOutcome::Refreshed(path) => {
                println!("  ✓ refreshed Codex passive hooks → {}", path.display())
            }
            HookOutcome::AlreadyPresent => println!("  ✓ Codex passive hooks already present"),
            HookOutcome::OptedOut => {}
            HookOutcome::Failed(e) => eprintln!("  ✗ Codex passive hooks install: {e}"),
        }
        // Codex refuses to RUN a non-managed hook until it is trusted (SHA-256
        // of the hook identity persisted in config.toml). A hook we install but
        // never trust is inert — no compaction, zero savings. There is no
        // official trust CLI (openai/codex#21615), so we write the trust state
        // ourselves. Without this, every Codex hook we ship does nothing.
        // The dry-run must NAME this write. It rewrites `~/.codex/config.toml`
        // — the file holding the user's model, providers and MCP servers —
        // and the preview said nothing about it (MEASURED 2026-08-07 by
        // diffing `--dry-run` output against the real run). A file the
        // preview does not name is a file the user did not consent to, which
        // is rule 1 of docs/AGENT-CONTRACT.md.
        if hooks_present && dry_run {
            println!(
                "  ◐ would trust the Codex hooks by writing `hooks.state` entries into {} \
                 (required — an untrusted Codex hook never runs)",
                crate::codex_trust::config_path_for_default()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "~/.codex/config.toml".into())
            );
        }
        if hooks_present && !dry_run {
            match crate::codex_trust::ensure_trust_default() {
                Ok(n) if n > 0 => println!(
                    "  ✓ trusted {n} Codex hook(s) (compaction active — no /hooks step needed)"
                ),
                Ok(_) => {}
                Err(e) => eprintln!(
                    "  ⚠ could not auto-trust Codex hooks ({e}) — run `/hooks` in Codex to \
                     trust tokenade, else compaction stays inactive"
                ),
            }
        }
    }

    // Grok Build CLI hooks (~/.grok/hooks/tokenade.json — NOT
    // `user-settings.json`, which grok does not read; this was measured with
    // `grok inspect` on a throwaway home). Same matcher-grouped JSON schema as
    // Claude/Codex, a camelCase payload, and an output contract that accepts
    // no substitution at all — so only the two side-effect events are wired.
    for (agent, install, name, what) in HOOK_INSTALLS {
        if !targets.contains(agent) || is_hook_opted_out() {
            continue;
        }
        let glyph = if dry_run { "◐" } else { "✓" };
        let verb_pfx = if dry_run { "would install " } else { "" };
        match install(exe, debug_log_path, home, dry_run) {
            HookOutcome::Installed(path) => {
                println!("  {glyph} {verb_pfx}{name} {what} → {}", path.display())
            }
            HookOutcome::Refreshed(path) => {
                println!("  ✓ refreshed {name} hooks → {}", path.display())
            }
            HookOutcome::AlreadyPresent => println!("  ✓ {name} hooks already present"),
            HookOutcome::OptedOut => {}
            HookOutcome::Failed(e) => eprintln!("  ✗ {name} hooks install: {e}"),
        }
    }

    // Live savings ticker for the agents OTHER than Claude Code that spawn a
    // status-line COMMAND and hand it the session JSON on stdin — the contract
    // `tokenade statusline` already answers, so only the wiring is per-agent.
    // Gated on the config file EXISTING: an agent that has never run has no
    // status line to attach to, and the next `install` (or session self-heal)
    // picks it up once it does. See `install/statusline_hosts.rs` for what was
    // measured, host by host, and for the measured negatives.
    for line in crate::install::wire_statusline_hosts(home, exe, dry_run) {
        match line {
            Ok(msg) => println!(
                "  {} {}{msg}",
                if dry_run { "◐" } else { "✓" },
                if dry_run { "would wire " } else { "" }
            ),
            Err(e) => eprintln!("  ✗ statusline: {e}"),
        }
    }

    // Factory Droid CLI hooks (~/.factory/hooks.json). Claude-Code-compatible
    // hook system (factory.ai hooks-reference); shell tool is `Execute`.

    // GitHub Copilot CLI hooks (~/.copilot/hooks/tokenade.json). PascalCase
    // events opt into Claude-compatible payloads + updatedInput (≥1.0.24).

    // CLAUDE.md scaffold (framework-aware). Pure new files — never
    // overwrites; if any target already exists we skip with a notice.
    if install_claude_md && dry_run {
        println!("  ◐ would scaffold CLAUDE.md / .claude/* (skipped: --dry-run)");
    } else if install_claude_md {
        let fw = framework::detect(root);
        match write_claude_md_scaffold_with_profile(root, fw, install_brevity, profile) {
            ClaudeMdOutcome::Wrote(paths) => {
                for p in paths {
                    println!("  ✓ scaffold → {}", p.display());
                }
            }
            ClaudeMdOutcome::Skipped(reason) => {
                println!("  ⊘ CLAUDE.md scaffold skipped: {reason}");
            }
            ClaudeMdOutcome::Failed(e) => {
                eprintln!("  ✗ CLAUDE.md scaffold: {e}");
            }
        }
    }

    // PreToolUse Bash-rewrite hook. Rewrites `Bash(cmd)` to
    // `Bash(tokenade wrap 'cmd')` via the safe allow+updatedInput channel
    // so the model sees the compacted command output. Claude Code and also
    // GitHub Copilot — whose PascalCase compat map carries
    // `PreToolUse:"preToolUse"` and whose response handler reads
    // `hookSpecificOutput.updatedInput`, i.e. this same channel. Installed by
    // default with the hook (gated only by --no-hook).
    if install_pretool_hook && claude_in_targets {
        match install_claude_code_lsp_enforcement_hook(
            root,
            exe,
            debug_log_path,
            install_global,
            home,
            dry_run,
        ) {
            HookOutcome::Installed(path) => {
                let (glyph, verb_pfx) = if dry_run {
                    ("◐", "would install ")
                } else {
                    ("✓", "")
                };
                println!(
                    "  {glyph} {verb_pfx}Claude Code PreToolUse Bash-rewrite hook → {}",
                    path.display()
                );
            }
            HookOutcome::Refreshed(path) => {
                let (glyph, verb_pfx) = if dry_run {
                    ("◐", "would refresh ")
                } else {
                    ("✓", "refreshed ")
                };
                println!(
                    "  {glyph} {verb_pfx}Claude Code PreToolUse Bash-rewrite hook (matcher/command updated) → {}",
                    path.display()
                );
            }
            HookOutcome::AlreadyPresent => {
                println!("  ✓ Claude Code PreToolUse Bash-rewrite hook already present");
            }
            HookOutcome::OptedOut => {
                println!("  ✓ Bash-rewrite hook skipped via TOKENADE_NO_HOOK");
            }
            HookOutcome::Failed(e) => {
                eprintln!("  ✗ Claude Code Bash-rewrite hook: {e}");
            }
        }
    }

    // Qwen Code tool hook (Gemini CLI family) — rewrites `run_shell_command`
    // calls to `tokenade wrap '<cmd>'` before execution, so the model sees
    // compressed output without needing PostToolUse support. Not Antigravity — the IDE has no BeforeTool seam. Its CLI, `agy`, is a
    // DIFFERENT product and it does have hooks; that one is wired through
    // `install_antigravity_cli_hooks`, whose contract was read out of the
    // shipped `agy` binary. "Antigravity has no CLI hook support" was a claim
    // about this table, not about the host.
    // Best-effort: a build predating the hooks feature ignores the
    // (harmless) entry.
    for (agent, install, label, event) in BEFORE_TOOL_INSTALLS {
        if !targets.contains(agent) || !install_hook {
            continue;
        }
        match install(exe, home, dry_run) {
            HookOutcome::Installed(path) => {
                let (glyph, verb_pfx) = if dry_run {
                    ("◐", "would install ")
                } else {
                    ("✓", "")
                };
                println!("  {glyph} {verb_pfx}{label} {event} → {}", path.display());
            }
            HookOutcome::Refreshed(path) => {
                println!("  ✓ refreshed {label} {event} → {}", path.display());
            }
            HookOutcome::AlreadyPresent => {
                println!("  ✓ {label} {event} already present");
            }
            HookOutcome::OptedOut => {}
            HookOutcome::Failed(e) => {
                eprintln!("  ✗ {label} {event} install: {e}");
            }
        }
    }
    mcp_ok
}
