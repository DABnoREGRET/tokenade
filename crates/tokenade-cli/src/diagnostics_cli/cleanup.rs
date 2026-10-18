//! **Uninstalling: remove what we installed, and nothing else.**
//!
//! Each removal is the exact mirror of an installation — a hook entry, an MCP
//! entry, a data directory, the binary. A backup is taken before every write,
//! and their number is bounded: an uninstaller that fills the disk with
//! backups is not careful, it is broken.

use super::*;

/// Carry out the cleanup of one detected competitor: run migrators (for
/// memory holders), then remove the signals (binary / MCP entry / hook /
/// data dir).
///
/// `assume_yes` skips interactive prompts (CI mode); when false the
/// caller is responsible for having already obtained consent — this fn
/// never prompts on its own (we run in non-tty contexts like a Claude
/// Code session).
///
/// Returns the list of human-readable outcome lines to display.
pub(crate) fn perform_cleanup(
    result: &competitors::DetectionResult,
    tokenade_root: &Path,
    dry_run: bool,
) -> Vec<String> {
    let mut log = Vec::new();
    let name = result.competitor.name;

    // Step 1: migration for memory holders (must succeed before we
    // touch the source data).
    if matches!(result.competitor.class, competitors::Class::MemoryHolder) {
        // Find a DataDir signal to migrate from; fall back to the first
        // path-based signal so the migrator gets something to read.
        let source: Option<PathBuf> = result
            .signals
            .iter()
            .find_map(|s| match s {
                competitors::Signal::DataDir { path, .. } => Some(path.clone()),
                _ => None,
            })
            .or_else(|| {
                result.signals.iter().find_map(|s| match s {
                    competitors::Signal::Binary { path, .. } => path.parent().map(PathBuf::from),
                    _ => None,
                })
            });
        // Memory-system migrators (codegraph / claude-mem / agentmemory →
        // tokenade) were retired with the custom memory subsystem in
        // favour of Claude Code's native auto-memory. The competitor
        // detection still runs (the cleanup phase strips their files),
        // but we no longer try to import their entries into a tokenade
        // store — there's no store to import into.
        let _ = source;
        let _ = name;
        let _ = tokenade_root;
    }

    // Step 2: remove every signal we found. Order matters: hook entries
    // before binary, so a half-done cleanup never leaves Claude Code
    // calling a binary we just removed.
    let mut hook_signals = Vec::new();
    let mut mcp_signals = Vec::new();
    let mut data_signals = Vec::new();
    let mut binary_signals = Vec::new();
    let mut rule_signals = Vec::new();
    for s in &result.signals {
        match s {
            competitors::Signal::HookScript { .. } => hook_signals.push(s),
            competitors::Signal::McpEntry { .. } => mcp_signals.push(s),
            competitors::Signal::DataDir { .. } => data_signals.push(s),
            competitors::Signal::Binary { .. } => binary_signals.push(s),
            competitors::Signal::RuleBlock { .. } | competitors::Signal::RuleFile { .. } => {
                rule_signals.push(s)
            }
        }
    }

    // Blocks before files: a CLAUDE.md that still says `@RTK.md` after the
    // file is gone is an unresolved import on every session.
    rule_signals.sort_by_key(|s| matches!(s, competitors::Signal::RuleFile { .. }));
    for s in rule_signals {
        log.push(remove_rule(s, dry_run));
    }
    for s in hook_signals {
        log.push(remove_hook_entry(s, dry_run));
    }
    for s in mcp_signals {
        log.push(remove_mcp_entry(s, name, dry_run));
    }
    for s in data_signals {
        log.push(remove_data_dir(s, dry_run));
    }
    for s in binary_signals {
        log.push(remove_binary(s, dry_run));
    }
    log
}

/// Edit a Claude settings.json to drop the hook group whose command
/// matches the recorded one. Idempotent.
pub(crate) fn remove_hook_entry(signal: &competitors::Signal, dry_run: bool) -> String {
    let competitors::Signal::HookScript {
        settings_path,
        event,
        matcher,
        command,
    } = signal
    else {
        return String::new();
    };
    if dry_run {
        return format!(
            "    [dry-run] would remove hook from {} ({event}/{matcher}): {command}",
            settings_path.display()
        );
    }
    // Backup before mutating.
    backup_file(settings_path);
    let Ok(s) = fs::read_to_string(settings_path) else {
        return format!("    skip: cannot read {}", settings_path.display());
    };
    let Ok(mut v): Result<serde_json::Value, _> = serde_json::from_str(&s) else {
        return format!("    skip: malformed JSON in {}", settings_path.display());
    };
    let mut removed = 0;
    if let Some(groups) = v
        .pointer_mut(&format!("/hooks/{event}"))
        .and_then(|g| g.as_array_mut())
    {
        groups.retain(|grp| {
            let m = grp.get("matcher").and_then(|m| m.as_str()).unwrap_or("");
            if m != matcher {
                return true;
            }
            let Some(hooks_arr) = grp.get("hooks").and_then(|h| h.as_array()) else {
                return true;
            };
            // Drop the whole matcher group if ANY of its hooks runs the
            // target command. This is conservative but matches the "one
            // matcher group per tool" convention every installer uses.
            let matches_command = hooks_arr
                .iter()
                .any(|h| h.get("command").and_then(|c| c.as_str()).unwrap_or("") == command);
            if matches_command {
                removed += 1;
                false
            } else {
                true
            }
        });
    }
    if removed == 0 {
        return format!(
            "    hook entry not found in {} (already clean?)",
            settings_path.display()
        );
    }
    let Ok(pretty) = serde_json::to_string_pretty(&v) else {
        return "    skip: serialise failed".into();
    };
    if let Err(e) = write_json_atomic(settings_path, &pretty) {
        return format!("    write failed: {e}");
    }
    format!(
        "    removed hook from {} ({event}/{matcher})",
        settings_path.display()
    )
}

/// Edit a Claude/Cursor/Codex/etc. config to drop a specific MCP entry.
/// Tolerates both `mcpServers.<name>` and `mcp.<name>` shapes.
pub(crate) fn remove_mcp_entry(signal: &competitors::Signal, name: &str, dry_run: bool) -> String {
    let competitors::Signal::McpEntry {
        config_path,
        entry_key: _,
    } = signal
    else {
        return String::new();
    };
    if dry_run {
        return format!(
            "    [dry-run] would remove MCP entry `{name}` from {}",
            config_path.display()
        );
    }
    backup_file(config_path);
    let Ok(s) = fs::read_to_string(config_path) else {
        return format!("    skip: cannot read {}", config_path.display());
    };
    let Ok(mut v): Result<serde_json::Value, _> = serde_json::from_str(&s) else {
        return format!("    skip: malformed JSON in {}", config_path.display());
    };
    let mut removed = false;
    for key in ["mcpServers", "mcp"] {
        if let Some(obj) = v.get_mut(key).and_then(|o| o.as_object_mut()) {
            if obj.remove(name).is_some() {
                removed = true;
            }
        }
    }
    if !removed {
        return format!(
            "    MCP entry `{name}` not found in {} (already clean?)",
            config_path.display()
        );
    }
    let Ok(pretty) = serde_json::to_string_pretty(&v) else {
        return "    skip: serialise failed".into();
    };
    if let Err(e) = fs::write(config_path, pretty) {
        return format!("    write failed: {e}");
    }
    format!(
        "    removed MCP entry `{name}` from {}",
        config_path.display()
    )
}

/// Remove a competitor's data directory. Migration must have happened
/// first (the caller's responsibility). Uses `remove_dir_all` for dirs
/// and `remove_file` for plain files.
pub(crate) fn remove_data_dir(signal: &competitors::Signal, dry_run: bool) -> String {
    let competitors::Signal::DataDir { path, .. } = signal else {
        return String::new();
    };
    // Skip pseudo-paths like the `ANTHROPIC_BASE_URL=...` marker.
    if !path.exists() {
        return format!("    skip: {} doesn't exist", path.display());
    }
    if dry_run {
        return format!("    [dry-run] would remove {}", path.display());
    }
    let r = if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    match r {
        Ok(()) => format!("    removed {}", path.display()),
        Err(e) => format!("    remove failed for {}: {e}", path.display()),
    }
}

/// Remove a tool's rules: the block it wrote into a shared file (from the
/// `open` line to the `close` line, both included — the rest of the file
/// stays byte for byte, and a file left blank goes), or its own file.
pub(crate) fn remove_rule(signal: &competitors::Signal, dry_run: bool) -> String {
    match signal {
        competitors::Signal::RuleFile { path } => {
            if !path.exists() {
                return String::new();
            }
            if dry_run {
                return format!("    [dry-run] would remove {}", path.display());
            }
            match fs::remove_file(path) {
                Ok(()) => format!("    removed {}", path.display()),
                Err(e) => format!("    remove failed for {}: {e}", path.display()),
            }
        }
        competitors::Signal::RuleBlock { file, open, close } => {
            let Ok(text) = fs::read_to_string(file) else {
                return String::new();
            };
            let Some(kept) = without_block(&text, open, close) else {
                return String::new();
            };
            if dry_run {
                return format!("    [dry-run] would drop `{open}` from {}", file.display());
            }
            backup_file(file);
            let r = if kept.trim().is_empty() {
                fs::remove_file(file)
            } else {
                fs::write(file, kept)
            };
            match r {
                Ok(()) => format!("    dropped `{open}` from {}", file.display()),
                Err(e) => format!("    write failed for {}: {e}", file.display()),
            }
        }
        _ => String::new(),
    }
}

/// `None` when the block is not there (or never closes: a block without its
/// end is not one we can bound, and guessing would eat the user's text).
fn without_block(text: &str, open: &str, close: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.iter().position(|l| l.trim() == open)?;
    let end = start + lines[start..].iter().position(|l| l.trim() == close)?;
    let before = lines[..start].join("\n");
    let after = lines[end + 1..].join("\n");
    let (before, after) = (
        before.trim_end(),
        after.trim_start_matches(['\n', ' ', '\t']),
    );
    let mut out = match (before.is_empty(), after.is_empty()) {
        (true, _) => after.to_string(),
        (false, true) => before.to_string(),
        (false, false) => format!("{before}\n\n{after}"),
    };
    if !out.is_empty() && text.ends_with('\n') && !out.ends_with('\n') {
        out.push('\n');
    }
    Some(out)
}

/// Remove a binary. Dispatches by installer: npm/cargo/brew get the
/// proper command; RawCopy uses `rm`, with a sudo prefix only if the
/// binary's path is outside `$HOME`.
pub(crate) fn remove_binary(signal: &competitors::Signal, dry_run: bool) -> String {
    let competitors::Signal::Binary { path, installer } = signal else {
        return String::new();
    };
    let (command, args, needs_sudo) = match installer {
        competitors::BinaryInstaller::NpmGlobal { package } => (
            "npm".to_string(),
            vec!["uninstall".into(), "-g".into(), package.clone()],
            false,
        ),
        competitors::BinaryInstaller::Homebrew { formula } => (
            "brew".to_string(),
            vec!["uninstall".into(), formula.clone()],
            false,
        ),
        competitors::BinaryInstaller::CargoInstall { krate } => (
            "cargo".to_string(),
            vec!["uninstall".into(), krate.clone()],
            false,
        ),
        competitors::BinaryInstaller::RawCopy { needs_sudo } => (
            "rm".to_string(),
            vec![path.display().to_string()],
            *needs_sudo,
        ),
    };
    if dry_run {
        let sudo = if needs_sudo { "sudo " } else { "" };
        return format!(
            "    [dry-run] would run: {sudo}{command} {}",
            args.join(" ")
        );
    }
    // tokenade never elevates itself: the user runs the privileged step.
    if needs_sudo {
        return format!(
            "    needs administrator rights — run: sudo {command} {}",
            args.join(" ")
        );
    }
    let mut cmd = Command::new(&command);
    cmd.args(&args);
    match cmd.status() {
        Ok(s) if s.success() => format!("    {command} OK for {}", path.display()),
        Ok(s) => format!(
            "    {command} exited {} for {}",
            s.code().unwrap_or(-1),
            path.display()
        ),
        Err(e) => format!("    {command} failed for {}: {e}", path.display()),
    }
}

/// How many `<path>.tokenade-backup-<ts>` snapshots to keep per file. The
/// self-heal (every ~6h) and post-upgrade refresh paths used to copy
/// `~/.claude/settings.json` unconditionally with a unique-per-second name
/// and NO retention, so hundreds of (mostly identical) backups piled up in
/// the user's global config dir (field report 2026-07-14). We keep only the
/// most recent one — the pre-mutation state, all a rollback needs.
pub(crate) const MAX_BACKUPS_PER_FILE: usize = 1;

/// Snapshot a file to `<path>.tokenade-backup-<ts>` before mutating it. Best-
/// effort: ignored on read/write failures (the worst-case outcome of
/// missing a backup is the user has to `git diff` to see what changed).
///
/// Dedups and prunes: if the newest existing backup is byte-identical to the
/// current content, no new copy is written (the common case for the periodic
/// self-heal, which re-runs even when nothing changed). Either way, older
/// backups beyond `MAX_BACKUPS_PER_FILE` are deleted — so the historical pile
/// from before this dedup existed is reclaimed on the next backup attempt.
pub(crate) fn backup_file(path: &Path) {
    let Ok(current) = fs::read(path) else {
        return;
    };
    let mut backups = list_backups(path); // oldest → newest
    let newest_identical = backups
        .last()
        .and_then(|p| fs::read(p).ok())
        .map(|prev| prev == current)
        .unwrap_or(false);
    if !newest_identical {
        let ts = tokenade_core::platform::now_unix_secs();
        let mut backup = path.as_os_str().to_owned();
        backup.push(format!(".tokenade-backup-{ts}"));
        let backup = PathBuf::from(backup);
        if fs::copy(path, &backup).is_ok() {
            backups.push(backup);
        }
    }
    // Prune even when we skipped writing — this is what clears the historical
    // accumulation of identical backups on the next run.
    while backups.len() > MAX_BACKUPS_PER_FILE {
        let _ = fs::remove_file(backups.remove(0));
    }
}

/// Existing `<path>.tokenade-backup-*` snapshots in `path`'s directory,
/// sorted oldest → newest by their unix-seconds suffix.
pub(crate) fn list_backups(path: &Path) -> Vec<PathBuf> {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name().and_then(|n| n.to_str())) else {
        return Vec::new();
    };
    let prefix = format!("{name}.tokenade-backup-");
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(u64, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let ts = p
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_prefix(&prefix))
                .map(|s| s.parse::<u64>().unwrap_or(0))?;
            Some((ts, p))
        })
        .collect();
    out.sort_by_key(|(ts, _)| *ts);
    out.into_iter().map(|(_, p)| p).collect()
}

#[cfg(test)]
mod rule_tests {
    use super::*;

    #[test]
    fn remove_rule_drops_only_the_block_and_the_tools_file() {
        let dir = std::env::temp_dir().join(format!("tokenade-rule-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let claude = dir.join("CLAUDE.md");
        let rtk = dir.join("RTK.md");
        fs::write(&claude, "@RTK.md\n\n## Mine\n@RTK.md is my note\n").unwrap();
        fs::write(&rtk, "# RTK\n").unwrap();
        let gemini = dir.join("GEMINI.md");
        fs::write(
            &gemini,
            "# lean-ctx — Context Engineering Layer\n<!-- lean-ctx-rules-v11 -->\n| Read | ctx_read |\n<!-- /lean-ctx -->\n\n<!-- tokenade-scaffold -->\nours\n<!-- /tokenade-scaffold -->\n",
        )
        .unwrap();
        let only = dir.join("AGENTS.md");
        fs::write(&only, "# lean-ctx — Context Engineering Layer\n<!-- lean-ctx-rules-v11 -->\nx\n<!-- /lean-ctx -->\n").unwrap();
        let block = |file: &Path, open: &str, close: &str| competitors::Signal::RuleBlock {
            file: file.to_path_buf(),
            open: open.into(),
            close: close.into(),
        };
        let lean = "# lean-ctx — Context Engineering Layer";
        let sigs = [
            block(&claude, "@RTK.md", "@RTK.md"),
            competitors::Signal::RuleFile { path: rtk.clone() },
            block(&gemini, lean, "<!-- /lean-ctx -->"),
            block(&only, lean, "<!-- /lean-ctx -->"),
        ];
        for s in &sigs {
            remove_rule(s, false);
        }
        assert_eq!(
            fs::read_to_string(&claude).unwrap(),
            "## Mine\n@RTK.md is my note\n"
        );
        assert!(!rtk.exists());
        assert_eq!(
            fs::read_to_string(&gemini).unwrap(),
            "<!-- tokenade-scaffold -->\nours\n<!-- /tokenade-scaffold -->\n"
        );
        assert!(!only.exists(), "a file that held only the block goes");
        for s in &sigs {
            assert_eq!(remove_rule(s, false), "", "second pass must be a no-op");
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
