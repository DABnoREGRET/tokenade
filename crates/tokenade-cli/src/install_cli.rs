//! `tokenade install` command + every per-command --help text + arg-flag
//! tables. Extracted from main.rs (refactor split).
#![allow(unused_imports)]
mod agent_plugins;
mod embedded_bin;
mod help_texts;
use crate::hooks::*;
use crate::install::*;
use crate::wrap::*;
use crate::*;
pub(crate) use agent_plugins::*;
pub(crate) use embedded_bin::*;
pub(crate) use help_texts::*;

// (env_help_text removed in 0.8.11 — the `env` CLI command no longer exists;
// `read` auto-detects env dumps and redacts secrets. The env compactor logic
// lives in tokenade-core, reached via `read`/`wrap`.)

/// #628 — dedicated help for `tokenade style`. Explains the three
/// preamble modes + the persistence behaviour so users know what
/// each setting actually injects.
mod assets;
mod flags;
mod hooks;
mod mcp;
mod plan;
mod saved_options;
pub(crate) use assets::*;
pub(crate) use flags::*;
pub(crate) use hooks::*;
pub(crate) use mcp::*;
pub(crate) use plan::*;
pub(crate) use saved_options::*;

pub(crate) fn cmd_install(rest: &[String]) -> i32 {
    // Audit (top-1 "user lost forever"): `sudo tokenade install` writes
    // hooks into /root and reports FULL SUCCESS — the user's real agent
    // never sees them and there is no signal afterwards. Refuse loudly;
    // TOKENADE_ALLOW_ROOT=1 overrides for actual root workflows.
    #[cfg(unix)]
    if std::env::var("SUDO_USER")
        .map(|u| u != "root")
        .unwrap_or(false)
        && std::env::var("TOKENADE_ALLOW_ROOT").is_err()
    {
        eprintln!(
            "✗ running under sudo: this would install into /root, not your own HOME —\n  your AI agent would never see the hooks (and everything would LOOK fine).\n  Re-run WITHOUT sudo: tokenade install\n  (intentional root install: TOKENADE_ALLOW_ROOT=1 tokenade install)"
        );
        return 1;
    }
    // L84 — explicit --help handling. Without this, the flag-parsing
    // below treats `--help` as an unknown flag (silently ignored)
    // and the install proceeds as if no flag was passed. A new user
    // discovering tokenade via `tokenade install --help` would
    // trigger an actual install — destructive UX.
    if crate::help_routing::asks_for_help(rest) {
        println!("{}", install_help_text());
        return 0;
    }
    // Windows: if a prior version wrote ~/.tokenade under a Git-Bash $HOME that
    // differs from %USERPROFILE%, migrate that ledger + license to the new
    // (USERPROFILE) location BEFORE we wire hooks there. No-op elsewhere /
    // after the first run.
    tokenade_core::platform::migrate_windows_home_once();
    // #644 — reject unknown flags up front. Until this, typos like
    // `--whith-claude-md` (extra `h`) were silently accepted; the
    // user thought the scaffold was being installed and only later
    // noticed it wasn't. Each install flag is either a boolean
    // toggle or a value-taking flag (--profile X, --debug-log
    // /path, --only agent). Anything else starting with `--` is a
    // typo; surface it with a Levenshtein hint (matches #639/#640).
    if let Err(code) = validate_install_flags(rest) {
        return code;
    }
    // The choices of the install before this one, unless overridden here:
    // `install.sh` re-runs a BARE `tokenade install` on every update, and
    // without this each update undid `--no-hook`, `--only`, `--no-autocompact`…
    let saved = tokenade_core::platform::home_dir()
        .map(|h| saved_install_args(&h))
        .unwrap_or_default();
    let (effective, inherited) = merge_saved_install_args(rest, &saved);
    if !inherited.is_empty() {
        println!(
            "keeping your earlier install choices: {} (`tokenade install {RESET_FLAG}` forgets them)",
            inherited.join(" ")
        );
    }
    let rest: &[String] = &effective;
    // `--with-hook` enables the Claude Code PostToolUse hook (opt-in for
    // backwards compat). `--with-claude-md` scaffolds a minimal startup
    // file structure. `--with-lsp-enforcement-hook` is a DEPRECATED no-op:
    // the PreToolUse hook it used to gate is now installed by default.
    // `--with-brevity-skill` writes a tokenade-authored compact-response style
    // hint into the CLAUDE.md scaffold (opt-in — terse prompts can degrade
    // reasoning, so off by default).
    // `--only <agent>` restricts the install to a single agent name
    // (case-insensitive: `claude-code`, `cursor`, …).
    // Defaults are tuned for **zero-config optimal install**: a user
    // (or AI agent) typing `tokenade install` gets every behaviour that
    // makes Tokenade most useful, with destructive ops still gated on
    // explicit consent (`--yes`).
    //
    // Negation flags exist for power users:
    //   `--no-hook`        disable the PostToolUse Bash+EditWrite hooks
    //   `--per-project`    write MCP config to ./.mcp.json instead of ~/
    //   `--no-cleanup`     skip the competitor detection phase
    //   `--no-debug-log`   don't set TOKENADE_DEBUG_LOG env in MCP/hook
    //
    // Opt-in (still default off because they modify user-owned files
    // or have contested UX):
    //   `--with-claude-md`              scaffold per-project CLAUDE.md
    //   `--with-brevity-skill`          add brevity instructions
    //   `--with-lsp-enforcement-hook`   DEPRECATED no-op (pretool hook now default)
    //   `--only <agent>`                restrict to one agent
    let flags = match parse_install_flags(rest) {
        Ok(f) => f,
        Err(code) => return code,
    };
    let InstallFlags {
        install_hook,
        wrap_mcps,
        install_claude_md,
        install_pretool_hook,
        install_brevity,
        profile,
        offer_cleanup,
        assume_yes,
        dry_run,
        install_global,
        debug_log_path,
        only,
        autocompact_pct,
        no_autocompact,
    } = flags;
    let root = root();
    // `stable_hook_exe`, not a bare `current_exe`. This `exe` is what gets
    // baked into every agent's hook command, and `stable_hook_exe` exists
    // precisely to keep those commands off a transient path — its own doc
    // records the live incident: "the dry-run dist binary self-healed the
    // user's config to its own path", after which a rebuild or `cargo clean`
    // kills every hook and the statusline.
    //
    // The helper was wired into the shim and the Cursor path and NOT here, so
    // eight of the nine hook agents kept the bug it was written to fix.
    // Reproduced 2026-08-08 by running `target/release/tokenade install`
    // against fixture homes: claude-code, codex, droid, grok, reasonix,
    // gemini-cli and qwen-code were all wired to
    // `…/target/release/tokenade`, while cursor alone got the installed
    // `~/.local/bin/tokenade`. That divergence, from ONE install run, is the
    // whole finding — the correct answer was already in the tree.
    let exe = crate::install_cli::embedded_bin::stable_hook_exe();

    // Portable: HOME on Unix, USERPROFILE on Windows.
    let home: Option<PathBuf> = tokenade_core::platform::home_dir();
    // Record the first-install date (sentinel) — `tokenade report` uses it
    // as the "collect logs since" cutoff. Idempotent: never overwrites. NOT
    // under --dry-run, which must not touch the home dir.
    if !rest.iter().any(|a| a == "--dry-run") {
        if let Some(h) = home.as_deref() {
            tokenade_core::install_state::ensure_install_sentinel(h);
        }
    }
    let targets = match resolve_install_targets(home.as_deref(), only.as_deref(), &root, dry_run) {
        Ok(t) => t,
        Err(rc) => return rc,
    };
    // Saved only once flags and targets resolve: a refused `--profile` or a
    // typo'd `--only` must not be remembered and refuse every later install.
    if !rest.iter().any(|a| a == "--dry-run") {
        if let Some(h) = tokenade_core::platform::home_dir() {
            save_install_args(&h, rest);
        }
    }

    println!(
        "detected agent(s): {}",
        targets
            .iter()
            .map(|a| a.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    // Competitor cleanup phase (before we install MCP entries — if we
    // strip a competitor's MCP entry from ~/.claude.json AFTER writing
    // our own, the user's config is briefly inconsistent. Cleanup first
    // keeps the file always-well-formed).
    //
    // Per-item prompts: each detected competitor gets its own y/N (or
    // structured proposal in non-TTY mode). This way the user can keep
    // some and remove others; previously a single global `--yes` was
    // all-or-nothing.
    let consent_flags = tokenade_core::prompt::ConsentFlags {
        assume_yes,
        dry_run,
    };
    offer_stale_cleanup(&root, consent_flags, offer_cleanup, dry_run);

    // Global CLAUDE.md preference block. Proposed independently of
    // `--no-cleanup` because every fresh install benefits from it — but only
    // when Claude Code is one of the agents this run is installing for.
    //
    // "ALWAYS proposed" is what this said, and always is what it did:
    // measured 2026-08-18, `tokenade install --only pi` in a HOME holding
    // nothing but `~/.pi/agent` created `~/.claude/CLAUDE.md` and wrote the
    // block into it. That file is the FIRST entry on Rule 1's never-write
    // list in `docs/AGENT-CONTRACT.md`, and the install had just been told,
    // by name, that this run was not about Claude Code.
    //
    // `resolve_install_targets` already adds ClaudeCode when T3 Code is
    // present — T3 drives Claude sessions and they load these rules — so
    // scoping on the target list keeps that case working rather than
    // special-casing it here.
    if targets.contains(&tokenade_core::detect::Agent::ClaudeCode) {
        propose_global_claude_md_block(home.as_deref(), consent_flags);
    }

    // Zero-MCP pivot (2026-06): tokenade no longer registers an MCP
    // server on ANY agent. The THOL long-bench measured the server as a
    // net parasite — its tool definitions are re-read as cache_read
    // tokens EVERY turn at ~0 % tool-call adoption, never amortized. All
    // value now flows through the PreToolUse/PostToolUse hooks (Claude
    // Code), the universal shell proxy (every agent), and the CLAUDE.md
    // CLI nudge. So instead of WRITING a `tokenade` MCP entry we PURGE
    // any stale one a pre-pivot install left behind — across every
    // client and wire format (JSON `mcpServers`/`mcp`/`mcp.servers`,
    // Hermes YAML, Grok TOML, plus `~/.claude.json` per-project maps) —
    // so migrating users stop spawning the now-removed `tokenade mcp`
    // command. Idempotent: a clean install with no stale entry is a no-op.
    let purge =
        tokenade_core::mcp_wrap::purge_tokenade_mcp_entries(&root, home.as_deref(), dry_run);
    let mut mcp_fail = 0;
    for r in &purge {
        if let Some(e) = &r.error {
            eprintln!("  ✗ MCP purge {} → {e}", r.config_path);
            mcp_fail += 1;
        } else if r.removed {
            let glyph = if dry_run { "◐" } else { "✓" };
            let verb = if dry_run {
                "would remove stale MCP entry from"
            } else {
                "removed stale MCP entry from"
            };
            println!("  {glyph} {verb} {}", r.config_path);
        }
    }
    let mcp_ok = install_mcp_for_targets(
        &targets,
        &InstallPlan {
            home: home.as_deref(),
            root: &root,
            exe: &exe,
            debug_log_path: debug_log_path.as_deref(),
            install_global,
            install_hook,
            install_pretool_hook,
            install_claude_md,
            install_brevity,
            profile: profile.as_deref(),
            dry_run,
            autocompact_pct,
            no_autocompact,
        },
    );

    // No-hook fallback: PATH shim (PO decision 2026-07 — supersedes the
    // shell ALIAS proxy). Hook-capable agents (Claude/Gemini/Grok/Qwen/
    // Droid) keep their hooks and are never given the shim; the shim is
    // installed only when a NO-HOOK agent is present among the targets.
    {
        // MIGRATION: strip any legacy alias proxy (< 0.8.6) from the rc
        // files + delete the ~/.tokenade/shellproxy.* fragments, so an old
        // install doesn't leave dead aliases that double-proxy or break the
        // shell. Idempotent — a clean machine is a no-op.
        let stripped = report_shell_proxy_removal(dry_run);
        if stripped > 0 {
            let verb = if dry_run { "would migrate" } else { "migrated" };
            println!("  ✓ {verb} {stripped} legacy shell-alias file(s)");
        }

        // The PATH shim was removed from the product in 0.9.5 and its opt-in
        // surface is gone entirely since. This CLEANUP stays: every
        // machine still running a shim installed by ≤0.9.4 needs an upgrade
        // that rips it out, and deleting this would strand them with a
        // hijacked PATH forever. Idempotent — a no-op on a clean machine.
        let home_p = home.as_deref();
        let removed = uninstall_path_shim(home_p, dry_run);
        if !removed.is_empty() {
            let verb = if dry_run { "would remove" } else { "removed" };
            println!("  ⊘ PATH shim {verb} — tokenade no longer wraps your PATH.");
        }
    }

    // Per-project rules scaffold. Same code path as the MCP server's
    // `initialize` handler — for users running an MCP-capable agent
    // this is a no-op (the MCP call already ran it), but for agents
    // without MCP OR users seeding a project ahead of first session,
    // this is the one-shot init. Under --dry-run we report what WOULD
    // happen without touching disk — the MCP `initialize` handler
    // re-runs it for free on the next agent session anyway.
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if dry_run {
        println!(
            "◐ would scaffold per-project rules in {} on first agent session (CLAUDE.md, AGENTS.md, .cursorrules, …).",
            cwd.display()
        );
    } else {
        let res = tokenade_core::project_scaffold::ensure(&cwd);
        if !res.skipped && !res.files_written.is_empty() {
            println!(
                "✓ scaffolded {} rules file(s) in {}: {}",
                res.files_written.len(),
                cwd.display(),
                res.files_written.join(", ")
            );
        } else if res.skipped {
            println!("⊙ project scaffold already present (skipped).");
        }
    }

    // L71 — auto-wrap third-party MCP servers found in the agent
    // configs. Skip if `--no-wrap-mcps`. Idempotent + reversible.
    wrap_configured_mcps(wrap_mcps, dry_run);

    offer_llm_proxy(&targets, dry_run);
    print_install_report(mcp_ok, mcp_fail, dry_run)
}

/// Offer rank 4 to the agents where it would add a FEATURE, and to no others.
///
/// The operator's ask, in his words: analyse what is installed, and where the
/// proxy buys a box that is dark today, say so in the vocabulary of the
/// compatibility page — then ask.
///
/// Three properties this deliberately has:
///
/// * **It is silent when it has nothing to sell.** On most agents a hook or a
///   plugin already folds every tool result, so rank 4 is a meter rather than a
///   compactor (one authority per payload). A meter is worth having — it is
///   the only source of the provider's real `usage` — but it is not one of the
///   ten boxes, and offering it as one would be selling a light that does not
///   come on. `features::gains` returns empty there and nothing is printed.
/// * **It never auto-installs.** This is the only rung whose installation can
///   BREAK the agent: its config would point at a proxy, and a proxy that is
///   not running fails every turn. That is a decision, so it is a question.
/// * **It says where the data goes, every time.** Pointing an agent's base URL
///   at something is exactly the moment a user is entitled to ask, and the
///   answer is short: loopback, this machine, then the provider they already
///   use.
/// Install the local proxy where it lights up a dark cell.
///
/// **It used to be only an OFFER, and the offer reached nobody.** Measured on
/// 2026-08-22 by replaying an upgrade from 1.1.6 to the current version:
/// `upgrade` swaps the binary and nothing else, the automatic repair does not
/// install the proxy, and the text below is printed only by `install`. A user
/// who lets the automatic update do its job therefore never learned the proxy
/// existed.
///
/// Three safeguards, because something that installs itself must behave:
///
/// 1. **only where it brings something** — `features::gains` is empty on
///    agents already covered, and a second channel on the same payload is the
///    double authority the one-authority rule forbids;
/// 2. **never an agent pointed at a dead port** — the install starts the
///    proxy, ASKS it whether it answers, and puts the original configuration
///    back otherwise;
/// 3. **a refusal is remembered** — `llm-proxy uninstall` records the agent
///    in a list this function re-reads, so we never undo the user's choice.
fn offer_llm_proxy(targets: &[detect::Agent], dry_run: bool) {
    use crate::llm_proxy::{cli::AutoOutcome, features};
    let candidates: Vec<detect::Agent> = targets
        .iter()
        .copied()
        .filter(|a| features::offer(*a).is_some())
        .collect();
    if candidates.is_empty() {
        return;
    }
    println!();
    if dry_run {
        println!("The local LLM proxy would be installed for these agents (dry-run):");
        for a in &candidates {
            if let Some(text) = features::offer(*a) {
                print!("{text}");
            }
        }
        return;
    }
    println!(
        "Local LLM proxy — installing where it adds something these agents cannot have otherwise:"
    );
    println!();
    for o in crate::llm_proxy::cli::ensure_installed_for(&candidates) {
        match o {
            AutoOutcome::Installed {
                agent,
                listen,
                gains,
            } => println!(
                "  ✓ {} — {gains} feature(s) added, through a LOCAL proxy on {listen} \
                 (every request stays on this machine before going to the provider you \
                 already use). Undo: tokenade llm-proxy uninstall --agent {}",
                agent.as_str(),
                agent.as_str()
            ),
            AutoOutcome::Already(agent) => {
                println!("  ✓ {} — already installed", agent.as_str())
            }
            AutoOutcome::Declined(agent) => println!(
                "  ⊙ {} — you removed it before, so it stays off. Re-add with \
                 `tokenade llm-proxy install --agent {}`",
                agent.as_str(),
                agent.as_str()
            ),
            // The case that matters most: we SAY it, because a silent
            // rollback reads as "nothing was attempted".
            AutoOutcome::RolledBack { agent, why } => println!(
                "  ◐ {} — not installed: {why}. Your config was put back exactly as it was.",
                agent.as_str()
            ),
        }
    }
}

/// Final outcome banner for `cmd_install`: the post-install summary line(s)
/// and the stale-MCP nudge. Returns the process exit code (0 on success or partial
/// success, 1 only when every agent failed).
pub(crate) fn print_install_report(mcp_ok: usize, mcp_fail: i32, dry_run: bool) -> i32 {
    println!();
    if mcp_fail == 0 {
        if dry_run {
            println!(
                "◐ dry-run preview: would install for {mcp_ok} agent(s). \
                 Re-run without --dry-run to apply."
            );
        } else {
            println!("✓ installed for {mcp_ok} agent(s). You're set.");
            // #503 — after a successful install (which typically means
            // the user just upgraded the binary), warn about any MCP
            // processes still running the OLD binary in memory.
            println!();
            print_stale_mcp_nudge(None);
        }
        0
    } else {
        println!("done with {mcp_ok} success(es) and {mcp_fail} failure(s).");
        if mcp_ok > 0 {
            0
        } else {
            1
        }
    }
}

#[cfg(test)]
mod temp_exe_tests {
    use super::*;

    /// Root cause of the "exec: …/.tokenade.new.9087: not found" incident:
    /// the auto-updater probes the freshly-downloaded binary under its
    /// transient `.<name>.new.<pid>` name, and the hook-rewrite captured
    /// THAT path. Detection + canonicalisation to the stable destination
    /// is what stops it and heals it.
    // NB: uses forward-slash paths only — on a Unix test runner `\` is a
    // filename char, not a separator, so a backslash "path" wouldn't parse
    // as one here. The helper itself is separator-correct per platform (it
    // relies on `Path::file_name`/`parent`), so `.exe` handling is covered
    // via a forward-slash path carrying an `.exe` name.
    #[test]
    fn detects_updater_temp_binary_names() {
        assert!(is_temp_new_exe_path("/npm/vendor/.tokenade.new.9087"));
        assert!(is_temp_new_exe_path(".tokenade.new.1"));
        assert!(is_temp_new_exe_path("/npm/vendor/.tokenade.exe.new.42"));
        // Not temp: the stable binary, a bare name, or a lookalike.
        assert!(!is_temp_new_exe_path("/usr/local/bin/tokenade"));
        assert!(!is_temp_new_exe_path("tokenade"));
        assert!(!is_temp_new_exe_path("/vendor/tokenade.exe"));
        // Leading dot required; pid tail must be all digits and non-empty.
        assert!(!is_temp_new_exe_path("my.new.123")); // no leading dot
        assert!(!is_temp_new_exe_path("/v/.tokenade.new.")); // empty pid
        assert!(!is_temp_new_exe_path("/v/.tokenade.new.abc")); // non-digit pid
    }

    #[test]
    fn canonicalizes_temp_to_stable_destination() {
        // The headline resolution the hook-writer now performs (npm layout).
        assert_eq!(
            canonicalize_temp_new_exe(
                "/home/studio/.npm-global/lib/node_modules/@tokenade/cli/vendor/.tokenade.new.9087"
            )
            .as_deref(),
            Some("/home/studio/.npm-global/lib/node_modules/@tokenade/cli/vendor/tokenade")
        );
        // Native curl-installer layout (~/.local/bin) resolves the same way.
        assert_eq!(
            canonicalize_temp_new_exe("/home/u/.local/bin/.tokenade.new.55").as_deref(),
            Some("/home/u/.local/bin/tokenade")
        );
        // `.exe` suffix preserved (strip only the leading dot + `.new.<pid>`).
        assert_eq!(
            canonicalize_temp_new_exe("/npm/vendor/.tokenade.exe.new.42").as_deref(),
            Some("/npm/vendor/tokenade.exe")
        );
        // Non-temp input yields None (leave current_exe alone).
        assert_eq!(canonicalize_temp_new_exe("/usr/local/bin/tokenade"), None);
    }
}

#[cfg(test)]
mod cursor_hook_tests {
    use super::*;

    #[test]
    fn cursor_hooks_merge_idempotent_and_preserves_user_entries() {
        // The hook command names `stable_hook_exe()`, which can resolve through
        // PATH: a test elsewhere changing PATH between the two installs made
        // the second one rewrite the entry (full suite, 2026-10-06).
        let _env = tokenade_core::platform::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let home = std::env::temp_dir().join(format!("tok-cursor-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(home.join(".cursor")).unwrap();
        // Pre-existing user hook config must survive the merge.
        let path = cursor_hooks_path(&home);
        fs::write(
            &path,
            r#"{"hooks":{"preToolUse":[{"command":"my-own-hook","matcher":"Shell"}],"afterFileEdit":[{"command":"fmt"}]}}"#,
        )
        .unwrap();

        let added = install_cursor_hooks(&home, false).unwrap();
        assert!(added, "first install adds the tokenade entry");
        let v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let pre = v.pointer("/hooks/preToolUse").unwrap().as_array().unwrap();
        assert_eq!(pre.len(), 2, "user entry preserved + tokenade added");
        assert!(pre.iter().any(|e| e["command"] == "my-own-hook"));
        assert!(pre
            .iter()
            .any(|e| e["command"].as_str().unwrap().contains("tokenade")));
        // unrelated hook key untouched.
        assert!(v.pointer("/hooks/afterFileEdit").is_some());

        // Idempotent: a second install is a no-op.
        let added2 = install_cursor_hooks(&home, false).unwrap();
        assert!(!added2, "second install is a no-op");
        let v2: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            v2.pointer("/hooks/preToolUse")
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            2
        );

        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn any_agent_detector_spans_all_agents_not_just_claude() {
        let home = std::env::temp_dir().join(format!("tok-anyagent-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(&home).unwrap();
        // Pin XDG into the tempdir so the opencode/kilo plugin lookups can't
        // read (or the test mutate) a real install on the dev machine.
        std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));

        // No agent config at all → badge hidden.
        assert_eq!(any_agent_has_tokenade_in(&home), None);

        // A Claude settings file with no tokenade hook → present but off.
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::write(
            home.join(".claude").join("settings.json"),
            r#"{"hooks":{}}"#,
        )
        .unwrap();
        assert_eq!(any_agent_has_tokenade_in(&home), Some(false));

        // tokenade wired into a NON-Claude agent (Cursor) → on, even though
        // Claude itself has no hook. This is the multi-agent fix.
        fs::create_dir_all(home.join(".cursor")).unwrap();
        fs::write(
            cursor_hooks_path(&home),
            r#"{"hooks":{"preToolUse":[{"command":"tokenade hookwrap-pretool"}]}}"#,
        )
        .unwrap();
        assert_eq!(any_agent_has_tokenade_in(&home), Some(true));

        // A dropped plugin file alone (Kilo) is enough to read as installed.
        let _ = fs::remove_dir_all(&home);
        let kdir = kilo_plugin_dir(&home);
        fs::create_dir_all(&kdir).unwrap();
        fs::write(kdir.join("tokenade.ts"), "// bridge").unwrap();
        assert_eq!(any_agent_has_tokenade_in(&home), Some(true));

        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn install_cline_plugin_writes_folding_plugin() {
        let home = std::env::temp_dir().join(format!("tok-cline-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(&home).unwrap();
        install_cline_plugin(&home, false).unwrap();
        let dir = home.join(".tokenade").join("cline-plugin");
        let ts = fs::read_to_string(dir.join("tokenade.ts")).unwrap();
        assert!(ts.contains("registerMessageBuilder"), "plugin body present");
        assert!(!ts.contains("@@TOKENADE_BIN@@"), "binary path injected");
        let pkg = fs::read_to_string(dir.join("package.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&pkg).unwrap();
        assert!(v["cline"]["plugins"].is_array(), "cline.plugins registered");
        // Dropped-file marker recognized by the status/doctor probe.
        assert_eq!(any_agent_has_tokenade_in(&home), Some(true));
        let _ = fs::remove_dir_all(&home);
    }
}
