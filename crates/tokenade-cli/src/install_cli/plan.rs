//! **Deciding what to install, before installing anything.**
//!
//! Targets are resolved from what is present on the machine, not from a list:
//! an absent agent is not offered, and a path already covered by a hook does
//! not need a second coverage through MCP. Cleaning up stale installations is
//! offered here too, before a new one gets added next to them.

use super::*;

/// Parsed `tokenade install` flag state. Pure data — every field is read
/// straight off the argv with no side effects, so threading it keeps
/// `cmd_install` an orchestrator instead of a flag-soup.
pub(crate) struct InstallFlags {
    pub install_hook: bool,
    pub wrap_mcps: bool,
    pub install_claude_md: bool,
    pub install_pretool_hook: bool,
    pub install_brevity: bool,
    pub profile: Option<String>,
    pub offer_cleanup: bool,
    pub assume_yes: bool,
    pub dry_run: bool,
    pub install_global: bool,
    pub debug_log_path: Option<String>,
    pub only: Option<String>,
    /// At what PERCENTAGE of the context window the agent should summarise
    /// itself. `None` = leave the default value (or
    /// `TOKENADE_AUTOCOMPACT_PCT`).
    pub autocompact_pct: Option<u32>,
    /// `--no-autocompact`: do not set the threshold, and REMOVE the one we had
    /// set. Without this removal, disabling afterwards left the old value in
    /// place and the agent kept summarising itself early.
    pub no_autocompact: bool,
}

/// Read the install flags off `rest`. Returns `Err(2)` (after printing the
/// hint) for an unknown `--profile` name — the one early-exit the flag block
/// owns; all other validation already ran in `validate_install_flags`.
pub(crate) fn parse_install_flags(rest: &[String]) -> Result<InstallFlags, i32> {
    let install_hook = !rest.iter().any(|a| a == "--no-hook");
    // L71 — auto-wrap third-party MCP servers behind `tokenade
    // mcp-proxy` by default. The wrap is idempotent and reversible
    // (`tokenade unwrap-mcps`, also auto-run by uninstall). Skip with
    // `--no-wrap-mcps` if the user wants to wrap them manually.
    let wrap_mcps = !rest.iter().any(|a| a == "--no-wrap-mcps");
    let install_claude_md = rest.iter().any(|a| a == "--with-claude-md");
    // PreToolUse Bash-rewrite hook (the proven allow+updatedInput
    // compaction path) is now installed by DEFAULT alongside the other
    // hooks — it's the real Bash-output saving, no longer gated. Disabled
    // only by `--no-hook`. `--with-lsp-enforcement-hook` is kept as a
    // deprecated no-op (still accepted so old scripts don't error) since
    // the behaviour it used to gate is now the default.
    let install_pretool_hook = install_hook;
    let install_brevity = rest.iter().any(|a| a == "--with-brevity-skill");
    // `--profile <coding|agents|analysis|benchmark>` selects a
    // specialised CLAUDE.md profile (sweep #3). Default profile
    // (no flag) gives the universal rules; named profiles add
    // domain-tailored sections on top.
    let profile: Option<String> = rest
        .iter()
        .position(|a| a == "--profile")
        .and_then(|i| rest.get(i + 1))
        .map(|s| s.to_lowercase());
    // #639 — typo'd profile names used to silently fall through to
    // the universal CLAUDE.md (profile_block returns "" for unknown
    // names). User typed `--profile codeing` expecting the coding
    // section and got nothing extra. Reject the typo early with a
    // Levenshtein hint, matching the `--only nonsense` UX.
    if let Some(p) = profile.as_deref() {
        const KNOWN_PROFILES: &[&str] = &["coding", "agents", "analysis", "benchmark"];
        if !KNOWN_PROFILES.contains(&p) {
            let near = KNOWN_PROFILES
                .iter()
                .map(|k| (*k, levenshtein(p, k)))
                .min_by_key(|(_, d)| *d)
                .filter(|(_, d)| *d <= 3)
                .map(|(k, _)| k);
            let suffix = match near {
                Some(n) => format!(" — did you mean `{n}`?"),
                None => String::new(),
            };
            eprintln!(
                "tokenade install --profile {p}: unknown profile{suffix} \
                 (known: {})",
                KNOWN_PROFILES.join(", ")
            );
            return Err(2);
        }
    }
    let offer_cleanup = !rest.iter().any(|a| a == "--no-cleanup");
    let assume_yes = rest.iter().any(|a| a == "--yes" || a == "-y");
    let dry_run = rest.iter().any(|a| a == "--dry-run");
    let install_global = !rest.iter().any(|a| a == "--per-project");
    // `--debug-log <path>` overrides the default path. `--no-debug-log`
    // turns it off entirely. Default: `~/.tokenade/debug.log` so the
    // user can `tail -f` and debug immediately without re-installing.
    let no_debug_log = rest.iter().any(|a| a == "--no-debug-log");
    let debug_log_path: Option<String> = if no_debug_log {
        None
    } else {
        rest.iter()
            .position(|a| a == "--debug-log")
            .and_then(|i| rest.get(i + 1))
            .map(|s| s.to_string())
            .or_else(|| {
                tokenade_core::platform::home_dir()
                    .map(|h| h.join(".tokenade").join("debug.log").display().to_string())
            })
    };
    let only: Option<String> = rest
        .iter()
        .position(|a| a == "--only")
        .and_then(|i| rest.get(i + 1))
        .map(|s| s.to_lowercase());
    // The auto-compaction threshold, as a percentage of the context window.
    // It used to be settable only through `TOKENADE_AUTOCOMPACT_PCT` at
    // install time — an environment variable one has to know about and that
    // leaves no trace of what was chosen.
    let autocompact_pct: Option<u32> = rest
        .iter()
        .position(|a| a == "--autocompact-pct")
        .and_then(|i| rest.get(i + 1))
        .and_then(|v| v.trim().parse::<u32>().ok());
    let no_autocompact = rest.iter().any(|a| a == "--no-autocompact");
    Ok(InstallFlags {
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
    })
}

/// Which agents this run installs into: everything detected, then the T3Code /
/// `--only` adjustments.
///
/// `Err(rc)` = nothing to install into and the reason is already printed. The
/// stale-MCP purge still ran on that path, because cleanup is not installation.
pub(super) fn resolve_install_targets(
    home: Option<&Path>,
    only: Option<&str>,
    root: &Path,
    dry_run: bool,
) -> Result<Vec<detect::Agent>, i32> {
    let agents_present = detect::detect_all_present(home);
    if agents_present.is_empty() {
        // The stale-MCP purge is CLEANUP, not installation — run it even
        // with zero agents detected (a leftover `tokenade mcp` entry in
        // ~/.claude.json is exactly what a machine mid-migration looks
        // like). Regression note: this used to be masked by the silent
        // post-upgrade refresh doing the purge before dispatch; the
        // build-dir guard removed that accident and the e2e caught it.
        let _ = tokenade_core::mcp_wrap::purge_tokenade_mcp_entries(root, home, dry_run);
        println!("no AI coding agent detected on this machine.");
        println!("Tokenade supports: Claude Code, Cursor, Codex, Devin Desktop, OpenCode, Copilot, Kilo Code, Hermes, Cline, Aider, Pi, OpenClaw, Qwen Code, Antigravity, Grok, T3 Code, Droid.");
        return Err(0);
    }

    let mut targets = agents_present;
    // T3 Code is an ORCHESTRATOR: its Claude driver spawns the real
    // `claude` with settingSources [user, project, local] (verified in
    // t3code source, 2026-07) — so tokenade's Claude wiring IS the T3
    // wiring. If T3 is present, make sure ClaudeCode is in the target set
    // even when the claude binary itself wasn't detected (T3 can manage
    // its own install).
    if let Some(want) = only {
        // `windsurf` was the agent's name until Devin Desktop, its fork (same
        // paths `~/.codeium/windsurf/`), became its own: an older script that
        // types `--only windsurf` targets that same product. `devin` is the
        // name on the launcher. Deprecated aliases, announced.
        let want = match want {
            "windsurf" | "devin" => {
                eprintln!(
                    "tokenade install --only {want}: deprecated alias of `--only devin-desktop`."
                );
                "devin-desktop"
            }
            other => other,
        };
        targets.retain(|a| a.as_str() == want);
        if targets.is_empty() {
            // #640 — Levenshtein hint for one-character typos like
            // `claude-cod` → `claude-code`. The found-agents list is
            // already in the error, but a typo hint surfaces the
            // right answer instead of asking the user to spot it.
            let found = detect::detect_all_present(home);
            // typo_distance (not plain levenshtein): agent names like
            // `claude-code` are long, so a typed `clude` (5 chars) has
            // dist 6 vs the full name but only 1 vs the `claude`
            // prefix — without the prefix window, a coincidentally
            // shorter unrelated name like `codex` wins falsely.
            let near = found
                .iter()
                .map(|a| {
                    (
                        a.as_str(),
                        tokenade_core::text_distance::typo_distance(want, a.as_str()),
                    )
                })
                .min_by_key(|(_, d)| *d)
                .filter(|(_, d)| *d <= 3)
                .map(|(name, _)| name);
            let suffix = match near {
                Some(n) => format!(" — did you mean `{n}`?"),
                None => String::new(),
            };
            eprintln!(
                "tokenade install --only {want}: no such agent detected{suffix} (found: {})",
                found
                    .iter()
                    .map(|a| a.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            return Err(1);
        }
    }
    // AFTER the `--only` filter, and that ordering is the whole point.
    //
    // This ran BEFORE it until 2026-08-20, so `--only t3-code` pushed
    // ClaudeCode into the set and then `retain` took it straight back out. The
    // note below still printed, promising "the Claude hooks below" on a run
    // that wrote no `.claude/settings.json` at all — measured in a throwaway
    // home: `install --only t3-code` created `~/.tokenade` and nothing else.
    // An agent whose entire wiring is another agent's hooks got no wiring, and
    // the message said otherwise.
    if targets.contains(&tokenade_core::detect::Agent::T3Code)
        && !targets.contains(&tokenade_core::detect::Agent::ClaudeCode)
    {
        targets.push(tokenade_core::detect::Agent::ClaudeCode);
    }
    if targets.contains(&tokenade_core::detect::Agent::T3Code) {
        eprintln!(
            "  ℹ T3 Code detected — its Claude sessions load your user settings, so the \
         Claude hooks below cover them. If you configured a T3 Claude environment \
         with a CUSTOM home path, run `HOME=<that path> tokenade install` once to \
         cover that environment too."
        );
        // The same fact, in a shape a reader OTHER than a human can act on.
        //
        // The prose above has said this since the delegation existed, and
        // anything checking where an install wrote still had to know the
        // exception by heart. It did not: `--only t3-code` writes
        // `~/.claude/`, which every "did this install stay in its own
        // territory" check reads as another agent's files. Saying which agent
        // owns the files, in one grep-able line, is what makes that check able
        // to tell a delegation from a leak.
        eprintln!("  ↳ t3-code is wired through claude-code's files (it loads them itself).");
    }
    Ok(targets)
}

/// Offer to remove what a previous install left behind, if anything.
pub(super) fn offer_stale_cleanup(
    root: &Path,
    consent_flags: tokenade_core::prompt::ConsentFlags,
    offer_cleanup: bool,
    dry_run: bool,
) {
    if offer_cleanup {
        let ctx = competitors::DetectionCtx::default();
        let found = competitors::detect_all(&ctx);
        let actionable: Vec<&competitors::DetectionResult> = found
            .iter()
            .filter(|r| {
                matches!(
                    r.competitor.class,
                    competitors::Class::DropInBetter
                        | competitors::Class::MemoryHolder
                        | competitors::Class::AntiPattern
                )
            })
            .collect();
        if actionable.is_empty() {
            println!("fix-up: no incompatible token tools to resolve.");
        } else {
            println!();
            println!("── Incompatible tools — suggested fixes ──");
            for r in &actionable {
                let badge = match r.competitor.class {
                    competitors::Class::DropInBetter => "[redundant]",
                    competitors::Class::MemoryHolder => "[overlapping]",
                    competitors::Class::AntiPattern => "[wasteful]",
                    _ => "",
                };
                println!("• {badge} {}", r.competitor.name);
                println!("  {}", r.competitor.one_liner);
                for s in &r.signals {
                    println!("  - {}", signal_one_line(s));
                }
                // Ask. Default: Yes for DropInBetter / MemoryHolder
                // (we're better, migration runs first for the holder);
                // Yes also for AntiPattern (we strongly recommend
                // removal). The user can override per item.
                let question = format!(
                "  `{}` overlaps with Tokenade. Migrate (if applicable) and remove it to avoid conflicts?",
                r.competitor.name
            );
                let decision = tokenade_core::prompt::ask(&question, false, consent_flags);
                if decision == tokenade_core::prompt::Decision::No {
                    println!("    skipped");
                    continue;
                }
                let outcome = perform_cleanup(r, root, dry_run);
                for line in outcome {
                    println!("{line}");
                }
            }
            println!();
        }
    }
}

/// The MCP configurations that ANOTHER route already folds, and which must
/// therefore not be wrapped behind the proxy.
///
/// Each one behind ITS capability probe:
/// * Claude Code, as soon as its `PostToolUse` hook on `mcp__.*` is in place;
/// * Cursor (below).
/// (Gemini CLI was one, before its removal from the product.)
///
/// Wrapping as well would fold twice — one more process per server for a
/// second pass that yields nothing, and a saving counted twice. The proxy and
/// the hook are two routes to the same fold, never two stages of the same
/// route.
///
/// Cursor is NOT there, and that is deliberate: its `postToolUse` is to date
/// only fired in the editor, the CLI serving only its shell events. Removing
/// the wrapper there would trade a coverage that works for one that perhaps
/// does not.
/// **`dry_run` is not a presentation detail here.** The Gemini probe CACHES
/// its answer in `~/.tokenade/cap-gemini-aftertool`, because a
/// `gemini --version` costs almost a second and the answer only changes on an
/// update. Called from the preview, it made the promise of `install --dry-run`
/// a lie: modify nothing under `HOME`. The e2e test refused it, and it is
/// right — the preview of a command that writes nothing must not write.
///
/// On a dry run, the preview therefore assumes BOTH agents covered. That is the
/// right direction of error: it announces "I will not wrap this", and a real
/// run that discovers the capability is absent will wrap — announcing less
/// than what we do beats announcing a write we will not do.
///
/// **The order of the two questions matters: a capability probe LAUNCHES
/// another agent's binary.** These three `if`s asked for the capability first
/// and the paths afterwards — so an `install --only claude-code` executed
/// `gemini --version`, and gemini, even just to print its version, writes
/// `~/.gemini/projects.json` (and leaves a `projects.json.<uuid>.tmp` there).
/// MEASURED on 2026-09-12 on the macOS VM, the only bench machine where the
/// gemini CLI is installed: the `scoping` cell of **17 agents out of 24** fell
/// to `Ko` — "install --only X also wrote 2 file(s) belonging to another
/// agent". Linux and Windows said green for lack of that binary, and that is
/// all their silence meant.
///
/// `config_paths_for_agent` only returns paths that EXIST: if there are none,
/// there is nothing to exclude and the probe's answer cannot change anything.
/// Asking for the paths first keeps the same result, while only starting an
/// agent's binary on a machine where its MCP config is present.
pub(crate) fn mcp_paths_covered_by_a_hook(dry_run: bool) -> Vec<std::path::PathBuf> {
    type Cap = fn() -> bool;
    let mut out = Vec::new();
    // Cursor is the third, on a MEASURED capability rather than a reported
    // one. An earlier version had installed the folding hook AND left the
    // proxy wrapped on top, on the claim that Cursor's `postToolUse` does not
    // fire in CLI; the installed bundle rebuilds an MCP result from
    // `updated_mcp_tool_output`, and the claim does not survive reading it.
    // Two folds of the same bytes is what was found earlier with Gemini.
    for (agent, capable) in [
        (
            "claude",
            crate::install::capability::claude_supports_updated_tool_output as Cap,
        ),
        (
            "cursor",
            crate::install::capability::cursor_supports_mcp_post_replace as Cap,
        ),
    ] {
        let paths = tokenade_core::mcp_wrap::config_paths_for_agent(agent);
        if paths.is_empty() {
            continue;
        }
        if dry_run || capable() {
            out.extend(paths);
        }
    }
    out
}

/// Wrap every configured MCP server so its tool results come back folded.
pub(super) fn wrap_configured_mcps(wrap_mcps: bool, dry_run: bool) {
    let covered = mcp_paths_covered_by_a_hook(dry_run);
    if wrap_mcps && !dry_run {
        // A wrapper PUT IN PLACE BEFORE the hook existed must go, otherwise
        // today's install leaves yesterday's double fold in place.
        for r in tokenade_core::mcp_wrap::unwrap_paths(&covered) {
            if !r.unwrapped.is_empty() {
                println!(
                    "  unwrapped {} MCP entry(ies) in {} — the PostToolUse hook folds them now",
                    r.unwrapped.len(),
                    r.config_path
                );
            }
        }
        let reports =
            tokenade_core::mcp_wrap::wrap_all_known_configs_except(&["tokenade"], &covered);
        let mut total = 0usize;
        for r in &reports {
            if let Some(err) = &r.error {
                eprintln!("  {} — mcp-wrap error: {err}", r.config_path);
                continue;
            }
            if !r.wrapped.is_empty() {
                println!(
                    "  wrapped {} MCP entry(ies) in {}: {}",
                    r.wrapped.len(),
                    r.config_path,
                    r.wrapped.join(", ")
                );
                total += r.wrapped.len();
            }
        }
        if total > 0 {
            println!(
                "✓ wrapped {total} third-party MCP server(s) behind tokenade mcp-wrap \
             (run `tokenade unwrap-mcps` to reverse; uninstall does this automatically)"
            );
        }
    } else if dry_run && wrap_mcps {
        // Real dry-run helper from #435 (mirror of unwrap path #416).
        // The OLD code passed `&["tokenade-dry-run"]` as `excluded_names`
        // thinking the magic string would block the write — but that
        // arg is the exclusion list, NOT a dry-run signal, so the
        // function still wrote when fresh third-party MCPs were found.
        let reports =
            tokenade_core::mcp_wrap::wrap_all_known_configs_dry_except(&["tokenade"], &covered);
        for r in &reports {
            if !r.wrapped.is_empty() {
                println!(
                    "  ◐ would wrap {} entry(ies) in {}",
                    r.wrapped.len(),
                    r.config_path
                );
            }
        }
    }
}
