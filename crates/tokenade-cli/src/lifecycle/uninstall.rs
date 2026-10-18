//! **Uninstalling: giving the machine back its previous state.**
//!
//! The mirror of installation, and the least tested path of any product. An
//! uninstall that leaves an entry behind breaks the agent: the wrapped command
//! no longer exists. The aliases served afterwards are here for the same
//! reason — an already-open shell keeps its own.

use super::*;

// `tokenade uninstall aliases` — remove ONLY the shell proxy (rc-file
// lines + ~/.tokenade/shellproxy.*), leaving MCP wiring, hooks and
// CLAUDE.md intact. Escape hatch when the alias system misbehaves.
/// `Some(rc)` when that is what was asked, `None` for a full uninstall.
pub(crate) fn serve_uninstall_aliases(rest: &[String]) -> Option<i32> {
    if !rest.first().is_some_and(|a| {
        matches!(
            a.as_str(),
            "aliases" | "alias" | "shell-proxy" | "shellproxy"
        )
    }) {
        return None;
    }
    let flags = &rest[1..];
    if let Err(code) = validate_simple_bool_flags(flags, "uninstall aliases", UNINSTALL_FLAGS) {
        return Some(code);
    }
    let dry_run = flags.iter().any(|a| a == "--dry-run");
    let prefix = if dry_run {
        "[tokenade uninstall aliases --dry-run]"
    } else {
        "[tokenade uninstall aliases]"
    };
    println!("{prefix} removing the no-hook fallback (rc lines + shim/fragment files)");
    let touched =
        report_shell_proxy_removal(dry_run) + crate::install::report_path_shim_removal(dry_run);
    if touched == 0 {
        println!("  shell proxy not installed — nothing to remove.");
    } else if dry_run {
        println!("◐ {touched} file(s) WOULD be changed (re-run without --dry-run to apply).");
    } else {
        println!(
            "✓ shell proxy removed from {touched} file(s). \
                 Open shells keep their aliases until restarted."
        );
    }
    Some(0)
}

/// One uninstall pass: what it was asked to do, and whether any step failed.
///
/// Honest exit code (audit): partial failures were printed but the
/// command still returned 0 — scripts checking $? believed a clean
/// uninstall on a half-done one.
pub(crate) struct Uninstall {
    dry_run: bool,
    headless: bool,
    keep_data: bool,
    home: Option<std::path::PathBuf>,
    had_failures: bool,
}

impl Uninstall {
    /// A line from a removal helper. `couldn't …` is a failure the exit code
    /// must carry; anything else is a step that happened.
    fn report_removal_line(&mut self, line: &str, ok_prefix: &str) {
        if line.starts_with("couldn't") {
            self.had_failures = true;
            eprintln!("  ✗ {line}");
        } else {
            println!("{ok_prefix}{} {line}", glyph(self.dry_run));
        }
    }

    /// The verb of a removal line: `--dry-run` must not claim one happened.
    fn removes(&self) -> &'static str {
        if self.dry_run {
            "would remove"
        } else {
            "removed"
        }
    }

    fn banner(&self) {
        let prefix = if self.dry_run {
            "[tokenade uninstall --dry-run]"
        } else {
            "[tokenade uninstall]"
        };
        if !self.headless {
            println!(
                "{prefix} {}",
                if self.dry_run {
                    "would restore third-party MCPs + remove tokenade entries"
                } else {
                    "restoring third-party MCPs + removing tokenade entries"
                }
            );
        }
    }

    // 1. Always-first: restore wrapped MCPs to their originals.
    //    Doing this BEFORE removing the tokenade entry means a
    //    crashed uninstall still leaves the user with working MCPs.
    fn unwrap_mcps(&self) {
        unwrap_third_party_mcps(self.dry_run);
    }

    // 2. #552 — strip the fenced tokenade block from
    //    ~/.claude/CLAUDE.md. Install's `propose_global_claude_md_block`
    //    appends the block; without this step uninstall would leave
    //    a permanent tokenade ad behind even after the binary is gone.
    fn strip_global_rules(&mut self) {
        if self.dry_run {
            for (path, removes) in
                tokenade_core::project_scaffold::plan_retract_global_rules(self.home.as_deref())
            {
                let verb = if removes {
                    "remove"
                } else {
                    "strip tokenade rules from"
                };
                println!("  ◐ would {verb} {}", path.display());
            }
            return;
        }
        for path in tokenade_core::project_scaffold::retract_global_rules(self.home.as_deref()) {
            let verb = if path.exists() {
                "stripped tokenade rules from"
            } else {
                "removed"
            };
            println!("  ✓ {verb} {}", path.display());
        }
    }

    fn strip_global_claude_md(&mut self) {
        match strip_global_claude_md_block(self.home.as_deref(), self.dry_run) {
            StripOutcome::NoHome | StripOutcome::AbsentOrUnreadable | StripOutcome::NoBlock => {
                // Silent: nothing to do.
            }
            StripOutcome::MalformedFences => {
                eprintln!(
                    "  ⚠ ~/.claude/CLAUDE.md has an opening `{}` without a matching close — \
                 leaving alone. Remove the block by hand.",
                    GLOBAL_CLAUDE_MD_FENCE_START
                );
            }
            StripOutcome::WouldStrip {
                path,
                bytes_removed,
            } => {
                println!(
                    "  ◐ would strip {bytes_removed}-byte tokenade block from {}",
                    path.display()
                );
            }
            StripOutcome::Stripped {
                path,
                bytes_removed,
            } => {
                println!(
                    "  ✓ stripped {bytes_removed}-byte tokenade block from {}",
                    path.display()
                );
            }
            // Say REMOVED, not "stripped": the file is gone, and reporting a
            // strip would describe a file that is still there.
            StripOutcome::RemovedEmpty {
                path,
                bytes_removed,
            } => {
                println!(
                    "  ✓ removed {} — the {bytes_removed}-byte tokenade block was all it held",
                    path.display()
                );
            }
            StripOutcome::WouldRemoveEmpty {
                path,
                bytes_removed,
            } => {
                println!(
                    "  ◐ would remove {} — the {bytes_removed}-byte tokenade block is all it holds",
                    path.display()
                );
            }
            StripOutcome::WriteFailed(err) => {
                self.had_failures = true;
                eprintln!("  ✗ couldn't rewrite ~/.claude/CLAUDE.md: {err}");
            }
        }
    }

    // 3. #611 — strip the `tokenade` self-entry from every known agent
    //    config. `install` writes one under `mcpServers` / `mcp` in
    //    each detected config; before this iteration `uninstall`
    //    didn't remove it, so after `brew uninstall tokenade` the
    //    agents kept trying to spawn a binary that no longer existed.
    /// Remove the llm-proxy autostart unit.
    ///
    /// `autostart.rs` states the rule in its own words — "an autostart entry
    /// that survives the uninstall is a process the user cannot find and did
    /// not ask for" — and `uninstall` was not honouring it: the unit stayed,
    /// and said nothing. It is `Restart=always` with `RestartSec=1`, so once
    /// the binary is gone systemd retries the dead path every second until it
    /// hits the start limit. Same failure `#611` fixed for the MCP
    /// self-entries: after `brew uninstall tokenade`, something on the machine
    /// keeps trying to spawn a binary that is not there.
    ///
    /// **Handing agents back to their provider.** `llm-proxy install` writes a
    /// base-url pointing at `127.0.0.1` into the agent's configuration; an
    /// uninstall that leaves it does not leave residue, it leaves a BROKEN
    /// agent — it keeps sending its model traffic to a proxy that no longer
    /// runs, and every turn fails.
    ///
    /// Measured on 2026-09-12 by the compatibility gate of the 1.1.13 dry-run,
    /// on qwen: after `uninstall`, `.qwen/settings.json` kept
    /// `"env": {"OPENAI_BASE_URL": "http://127.0.0.1:8787"}`. The revert
    /// existed (`llm-proxy uninstall`), nobody called it here.
    ///
    /// The list comes from the installation REGISTRY, never from an enumeration
    /// of known agents: we only undo what we did, and an agent the user pointed
    /// at a proxy themselves is not ours.
    fn revert_llm_proxy_installs(&mut self) {
        let Some(home) = self.home.as_deref() else {
            return;
        };
        let records = crate::llm_proxy::install::load_records(home).unwrap_or_default();
        for r in &records {
            // An agent removed from the product no longer has a variant, but
            // its record can still be undone (`legacy_site`).
            let known = tokenade_core::detect::Agent::all()
                .iter()
                .any(|a| a.as_str() == r.agent);
            if !known && crate::llm_proxy::install::legacy_site(&r.agent).is_none() {
                continue;
            }
            if self.dry_run {
                println!(
                    "  ◐ would point {} back at {}",
                    r.agent,
                    if r.upstream.is_empty() {
                        "its provider"
                    } else {
                        &r.upstream
                    }
                );
                continue;
            }
            match crate::llm_proxy::install::uninstall_slug(home, &r.agent) {
                Ok(Some(rec)) => println!(
                    "  ✓ {} points back at its provider ({})",
                    rec.agent,
                    if rec.upstream.is_empty() {
                        "default"
                    } else {
                        &rec.upstream
                    }
                ),
                Ok(None) => {}
                Err(e) => {
                    self.had_failures = true;
                    eprintln!("  ✗ {}: {e}", r.agent);
                }
            }
        }
    }

    /// The supervisor is told FIRST and the commands are PRINTED, not run —
    /// the convention `llm-proxy autostart --remove` already follows, because
    /// removing a unit out from under a running service leaves a process
    /// nothing can name.
    fn remove_autostart_unit(&mut self) {
        let Some(home) = self.home.as_deref() else {
            return;
        };
        let Some(platform) = crate::llm_proxy::autostart::Platform::current() else {
            return;
        };
        // ALL units: there is one per port since each agent got its own
        // proxy, and removing only one leaves N-1 that come back at the next
        // session start.
        let paths: Vec<_> = crate::llm_proxy::autostart::unit_paths_all(platform, home)
            .into_iter()
            .filter(|p| p.exists())
            .collect();
        let Some(path) = paths.first().cloned() else {
            return;
        };
        let _ = &path;
        if self.dry_run {
            for p in &paths {
                println!("  ◐ would remove the autostart unit {}", p.display());
            }
            return;
        }
        println!("  ⚠ stop the service first — this product does not run these for you:");
        for p in &paths {
            // The port is in the unit's NAME: it is what says which stop
            // command to name, one per proxy.
            let name = p
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let listen = name
                .rsplit_once('-')
                .map(|(_, port)| format!(":{port}"))
                .unwrap_or_default();
            for c in crate::llm_proxy::autostart::disable_commands(platform, &listen) {
                println!("      {c}");
            }
        }
        for p in &paths {
            match std::fs::remove_file(p) {
                Ok(()) => println!("  ✓ removed the autostart unit {}", p.display()),
                Err(e) => {
                    self.had_failures = true;
                    eprintln!("  ✗ couldn't remove {}: {e}", p.display());
                }
            }
        }
    }

    fn remove_self_entries(&self) {
        remove_tokenade_self_entries(self.dry_run);
    }

    // 4. #612 — strip every `_tag: "tokenade*"` group from
    //    ~/.claude/settings.json. Symmetric to the four
    //    `install_claude_code_*hook*` installers; without it every
    //    Bash/Read/Grep call after `brew uninstall tokenade` would
    //    fire a hook spawning a missing binary.
    // Markers that survive an uninstall poison later runs: a leftover
    // claude-hooks-installed makes any surviving tokenade binary WARN
    // daily that "hooks vanished" (they were removed on purpose).
    fn clear_stale_markers(&self) {
        if self.dry_run {
            return;
        }
        let Some(h) = self.home.as_deref() else {
            return;
        };
        for m in [
            "claude-hooks-installed",
            "hooks-vanished-warned",
            "installed-at",
            // Remembered choices belong to THIS install. Kept, a later fresh
            // install would inherit them, and the statusline record would read
            // our own uninstall as the user's removal and never re-wire it.
            "install-options.json",
            "statusline-wired.json",
        ] {
            let _ = std::fs::remove_file(h.join(".tokenade").join(m));
        }
    }

    /// Hook/statusLine entries in the two settings files that carry them.
    fn strip_hook_settings(&mut self) {
        // Qwen statusLine (same strip/restore semantics as Claude below).
        match crate::install::uninstall::strip_qwen_statusline(self.home.as_deref(), self.dry_run) {
            StripHooksOutcome::WouldStrip { path, .. } => {
                println!(
                    "  ◐ would remove tokenade statusLine/hooks from {}",
                    path.display()
                );
            }
            StripHooksOutcome::Stripped { path, .. } => {
                println!(
                    "  ✓ removed tokenade statusLine/hooks from {}",
                    path.display()
                );
            }
            StripHooksOutcome::WriteFailed(e) => {
                self.had_failures = true;
                eprintln!("  ✗ ~/.qwen/settings.json: {e}");
            }
            _ => {}
        }
        // Antigravity CLI's hooks.json is a map of named bundles; only ours goes.
        if let Some(p) =
            crate::install::uninstall::strip_antigravity_hook(self.home.as_deref(), self.dry_run)
        {
            println!(
                "  {} {} the tokenade hook bundle from {}",
                glyph(self.dry_run),
                self.removes(),
                p.display()
            );
        }
        // Copilot CLI / Cursor CLI carry the SAME `statusLine` command entry,
        // in their own config files. Left behind, each is a status bar that
        // spawns a binary that is no longer there.
        for p in crate::install::unwire_statusline_hosts(self.home.as_deref(), self.dry_run) {
            println!(
                "  {} {} tokenade statusLine from {}",
                glyph(self.dry_run),
                self.removes(),
                p.display()
            );
        }
        match strip_tokenade_hooks_from_settings(self.home.as_deref(), self.dry_run) {
            StripHooksOutcome::NoHome
            | StripHooksOutcome::AbsentOrUnreadable
            | StripHooksOutcome::NoGroups => {
                // Silent: nothing to do.
            }
            StripHooksOutcome::Malformed(err) => {
                self.had_failures = true;
                eprintln!("  ⚠ ~/.claude/settings.json: {err} — leaving hooks alone.");
            }
            StripHooksOutcome::WouldStrip {
                path,
                groups_removed,
            } => {
                println!(
                    "  ◐ would remove {groups_removed} tokenade-tagged hook group(s) from {}",
                    path.display()
                );
            }
            StripHooksOutcome::Stripped {
                path,
                groups_removed,
            } => {
                println!(
                    "  ✓ removed {groups_removed} tokenade-tagged hook group(s) from {}",
                    path.display()
                );
            }
            StripHooksOutcome::WriteFailed(err) => {
                self.had_failures = true;
                eprintln!("  ✗ couldn't rewrite ~/.claude/settings.json: {err}");
            }
        }
    }

    // 5. Remove the shell proxy: alias/eval lines in every rc file plus
    //    the ~/.tokenade/shellproxy.* fragment files. Same code path as
    //    `tokenade uninstall aliases`; without it, every aliased command
    //    in a new shell would spawn a binary that no longer exists.
    fn strip_shell_proxy(&self) {
        if report_shell_proxy_removal(self.dry_run)
            + crate::install::report_path_shim_removal(self.dry_run)
            == 0
        {
            println!("  no shell proxy / PATH shim found in rc files.");
        }
    }

    // 5a-bis. Per-project install: `--per-project` writes
    //     `<root>/.claude/settings.json`; walking up from cwd catches the
    //     common "uninstall from the project I installed in" flow (a
    //     project elsewhere still needs uninstall run from inside it).
    /// The nearest `.claude/settings.json` between the working directory and
    /// its PROJECT root — never above it, and never Claude's global file.
    ///
    /// The walk used to climb to the filesystem root. From a directory under
    /// the account home with no project file on the way, it reached
    /// `~/.claude/settings.json` — the user's global config — and stripped it
    /// as a "project" file. A test with a disposable HOME under `~/.cache`
    /// uninstalled tokenade from the operator's real settings that way, twice
    /// (2026-10-03, `e2e_uninstall_strips_global_claude_md_block`, once
    /// `TMPDIR` sat under the home).
    fn strip_project_hooks(&self) {
        let Ok(cwd) = std::env::current_dir() else {
            return;
        };
        let root = tokenade_core::platform::project_root_of(&cwd);
        let global = self
            .home
            .as_deref()
            .map(|h| tokenade_core::platform::claude_config_dir_for(h).join("settings.json"));
        for d in cwd.ancestors() {
            let p = d.join(".claude").join("settings.json");
            if global.as_ref() != Some(&p) && p.is_file() {
                match crate::install::strip_tokenade_hooks_from_settings_at(&p, self.dry_run) {
                    StripHooksOutcome::Stripped { groups_removed, .. } => println!(
                        "  ✓ removed {groups_removed} tokenade hook group(s) from {}",
                        p.display()
                    ),
                    StripHooksOutcome::WouldStrip { groups_removed, .. } => println!(
                        "  ◐ would remove {groups_removed} tokenade hook group(s) from {}",
                        p.display()
                    ),
                    _ => {}
                }
                break;
            }
            if d == root {
                break;
            }
        }
    }

    // 5b. Non-Claude agents: strip hook entries (cursor/codex/grok/gemini/
    //     qwen) and delete plugin files (opencode/pi/kilo/openclaw/hermes).
    //     Same rationale as #612: an orphaned entry spawns a missing binary
    //     on every tool call once the package is removed.
    fn strip_other_agents(&self) {
        if let Some(h) = self.home.as_deref() {
            for line in crate::install::uninstall_agent_integrations(h, self.dry_run) {
                println!("  {} {line}", glyph(self.dry_run));
            }
        }
        // 5c. Auto-compact override (only when stamped `_tokenadeAutoCompact`).
        if let Some(line) =
            crate::install::strip_autocompact_override(self.home.as_deref(), self.dry_run)
        {
            println!("  {} {line}", glyph(self.dry_run));
        }
    }

    // Clear the install state (sentinel + self-heal stamp) so a later
    // SessionStart / `tokenade wrap` does NOT resurrect the hooks we just
    // removed — without this, `ensure_hooks` would treat the uninstall as
    // drift and rebuild everything.
    fn clear_install_state(&self) {
        if !self.dry_run {
            if let Some(h) = self.home.as_deref() {
                tokenade_core::install_state::clear_install_state(h);
            }
        }
    }

    // 6. Wipe tokenade's OWN data: ~/.tokenade + <cache>/tokenade. FAIL-OPEN
    //    (a dir that can't be removed logs a line, never aborts). `--keep-data`
    //    preserves the savings ledger; the default is a clean wipe so
    //    "uninstall" actually leaves the machine as it found it.
    fn wipe_data(&mut self) {
        if self.keep_data {
            if !self.headless {
                println!("  · keeping data (~/.tokenade, ~/.cache/tokenade) — --keep-data set.");
            }
            return;
        }
        let cache = tokenade_core::platform::cache_dir();
        let lines = crate::install::uninstall::remove_tokenade_data(
            self.home.as_deref(),
            cache.as_deref(),
            self.dry_run,
        );
        for line in lines {
            self.report_removal_line(&line, "  ");
        }
    }

    // 7. The binary itself. Skipped in --headless: npm's `preuninstall` invokes
    //    us headless and then removes the binary on its own, so we must not
    //    delete it out from under npm nor print package-manager guidance.
    fn handle_binary(&mut self) {
        if self.headless {
            return;
        }
        let (_method, line) = crate::install::uninstall::handle_binary_removal(self.dry_run);
        println!();
        self.report_removal_line(&line, "Next: ");
        println!(
            "The agent configs {} through tokenade for any third-party MCP.",
            if self.dry_run {
                "would no longer route"
            } else {
                "no longer route"
            }
        );
    }
}

pub(crate) fn cmd_uninstall(rest: &[String]) -> i32 {
    // `--yes` accepted as a no-op: uninstall is non-interactive already,
    // but install takes it and muscle memory shouldn't abort an uninstall.
    let rest: Vec<String> = rest.iter().filter(|a| *a != "--yes").cloned().collect();
    let rest: &[String] = &rest;
    if let Some(rc) = serve_uninstall_aliases(rest) {
        return rc;
    }
    // L87 — --dry-run was silently ignored (signature was `_rest`).
    // Running `tokenade uninstall --dry-run` actually unwrapped
    // configs, surprising users expecting a preview.
    //
    // #655 — and `--dryrun` (typo) was ALSO silently accepted, so the
    // user thought they were previewing and instead performed a real
    // uninstall (the strip-pass takes a backup, so recovery is
    // possible, but the UX is hostile). Reject unknown flags up front.
    if let Err(code) = validate_simple_bool_flags(rest, "uninstall", UNINSTALL_FLAGS) {
        return code;
    }
    let mut run = Uninstall {
        dry_run: rest.iter().any(|a| a == "--dry-run"),
        // `--headless` (npm preuninstall): no banner, no next-step guidance, never
        // touch the binary — npm removes its own binary right after this returns.
        headless: rest.iter().any(|a| a == "--headless"),
        // `--keep-data`: undo the integration + handle the binary, but PRESERVE
        // ~/.tokenade and ~/.cache/tokenade (savings ledger).
        keep_data: rest.iter().any(|a| a == "--keep-data"),
        home: tokenade_core::platform::home_dir(),
        had_failures: false,
    };
    // The door. See `paths::refuse_real_home_from_a_test_binary` — a test
    // without a `HOME` override uninstalled a live install on 2026-08-19, and
    // the only signal anybody got was the user's savings figure going away.
    if let Some(home) = run.home.as_deref() {
        if let Err(why) =
            tokenade_core::paths::refuse_real_home_from_a_test_binary(home, "uninstall")
        {
            eprintln!("{why}");
            return 2;
        }
    }
    run.banner();
    run.unwrap_mcps();
    run.strip_global_claude_md();
    run.strip_global_rules();
    run.remove_self_entries();
    run.revert_llm_proxy_installs();
    run.remove_autostart_unit();
    run.clear_stale_markers();
    run.strip_hook_settings();
    run.strip_shell_proxy();
    run.strip_project_hooks();
    run.strip_other_agents();
    run.clear_install_state();
    // LAST, and after every writer has had its turn. An uninstall that changed
    // nothing of the user's must leave their bytes exactly as it found them,
    // and the only moment that is knowable is once nobody else is going to
    // write. See `paths::restore_pristine_files`.
    if !run.dry_run {
        if let Some(home) = run.home.as_deref() {
            let n = tokenade_core::paths::restore_pristine_files(home);
            if n > 0 {
                println!("  ✓ restored {n} config file(s) to their original bytes");
            }
        }
    }
    run.wipe_data();
    run.handle_binary();
    // AFTER the last writer, for the reason the restore above exists: a pass
    // that runs earlier finds its own copies taken again by whatever writes
    // next. Measured 2026-08-19 — cleaning them inside the restore walk left a
    // `settings.json.tokenade-original` beside every config anyway.
    if !run.dry_run {
        if let Some(home) = run.home.as_deref() {
            tokenade_core::paths::forget_pristine_files(home);
        }
    }
    i32::from(run.had_failures)
}

/// `tokenade unwrap-mcps` — restore every wrapped MCP entry to its
/// original `command` + `args`. Invoked automatically by
/// `tokenade uninstall` BEFORE the tokenade self-entry is removed.
pub(crate) fn cmd_unwrap_mcps(rest: &[String]) -> i32 {
    // Symmetric to uninstall --dry-run (#416): without this flag check,
    // `unwrap-mcps --dry-run` silently ignores the flag and modifies
    // configs anyway. Reuses the unwrap_all_known_configs_dry helper
    // added when fixing the uninstall path.
    //
    // #655 — also reject unknown flags + positional args so typos like
    // `--dryrun` don't silently fall through to a real unwrap.
    if let Err(code) = validate_simple_bool_flags(rest, "unwrap-mcps", UNWRAP_MCPS_FLAGS) {
        return code;
    }
    let dry_run = rest.iter().any(|a| a == "--dry-run");
    let reports = if dry_run {
        tokenade_core::mcp_wrap::unwrap_all_known_configs_dry()
    } else {
        tokenade_core::mcp_wrap::unwrap_all_known_configs()
    };
    let mut total = 0usize;
    for r in &reports {
        if let Some(err) = &r.error {
            eprintln!("  {} — error: {err}", r.config_path);
            continue;
        }
        if !r.unwrapped.is_empty() {
            let glyph = if dry_run { "◐" } else { " " };
            let verb = if dry_run { "would unwrap" } else { "unwrapped" };
            println!(
                "  {glyph} {} — {verb} {} entry(ies): {}",
                r.config_path,
                r.unwrapped.len(),
                r.unwrapped.join(", ")
            );
            total += r.unwrapped.len();
        }
    }
    if total == 0 {
        println!("nothing to unwrap.");
    } else if dry_run {
        println!(
            "◐ dry-run preview: would unwrap {total} MCP entry(ies) across {} config file(s). \
             Re-run without --dry-run to apply.",
            reports.len()
        );
    } else {
        println!(
            "✓ unwrapped {total} MCP entry(ies) across {} config file(s).",
            reports.len()
        );
    }
    0
}
