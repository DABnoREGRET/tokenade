//! **One function per hook installed in Claude Code.**
//!
//! Index, post-read, post-fetch, pre-edit, and the main hook. They look alike
//! and are NOT interchangeable: each has its own matcher, and an over-broad
//! matcher intercepts a tool we cannot do anything with — which costs a
//! round-trip per call without folding anything.

use super::*;

pub(crate) fn install_claude_code_index_hook(
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
    // #486 defensive: handle non-object hooks.
    if !hooks.is_object() {
        *hooks = serde_json::json!({});
    }
    let post_array = ensure_array_entry(hooks.as_object_mut().unwrap(), "PostToolUse");
    // #561/#562 — content-aware idempotency (same as install_claude_code_hook).
    let canonical_matcher = "Edit|Write|MultiEdit";
    let canonical_command = build_hook_command(exe, "index-touch", debug_log_path);
    let mut stale_idx: Option<usize> = None;
    let mut already = false;
    for (i, g) in post_array.iter().enumerate() {
        let (matched, has_tag) = group_matches_tokenade_hook(g, "tokenade-index", "index-touch");
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
            // Drift OR missing `_tag` — heal in place (#775).
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
            "_tag": "tokenade-index"
        }]
    }));
    let outcome = |p: PathBuf| {
        if was_refresh {
            HookOutcome::Refreshed(p)
        } else {
            HookOutcome::Installed(p)
        }
    };
    if dry_run {
        return outcome(path);
    }
    let content = serde_json::to_string_pretty(&settings).unwrap();
    match idempotent_write(&path, &content) {
        Ok(true) => outcome(path),
        // Settings file already contains identical bytes — treat as
        // "installed, no-op". Re-running `tokenade install` should be
        // safe + silent rather than churning timestamps + re-prompting
        // the agent's settings-watcher.
        Ok(false) => outcome(path),
        Err(e) => HookOutcome::Failed(format!("write {}: {e}", path.display())),
    }
}

/// Install the PostToolUse `Read` hook (`hookwrap-postread`) that
/// delivers staged Read compactions as `additionalContext` — the
/// second half of the allow+updatedInput Read flow (see
/// `emit_read_compaction`). Tagged `tokenade-postread` for clean
/// uninstall.
///
/// On success with `global: true` (and not dry-run) this drops the
/// `~/.tokenade/postread-hook-ok` marker the PreToolUse side checks
/// before switching away from the legacy deny flow. The marker is
/// deliberately NOT written for per-project installs: the PreToolUse
/// hook can fire in ANY project, and staged context must never be
/// emitted where no postread hook will deliver it.
pub(crate) fn install_claude_code_postread_hook(
    root: &Path,
    exe: &str,
    debug_log_path: Option<&str>,
    global: bool,
    home: Option<&Path>,
    dry_run: bool,
) -> HookOutcome {
    // #556 — TOKENADE_NO_HOOK opt-out (see install_claude_code_hook).
    if is_hook_opted_out() {
        return HookOutcome::OptedOut;
    }
    let Some(path) = hook_settings_path(root, global, home) else {
        return HookOutcome::Failed("no home dir resolved for global install".into());
    };
    let finish = |outcome: HookOutcome| {
        let hook_live = matches!(
            outcome,
            HookOutcome::Installed(_) | HookOutcome::Refreshed(_) | HookOutcome::AlreadyPresent
        );
        if hook_live && global && !dry_run {
            let marker = postread_marker_path();
            if let Some(parent) = marker.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(&marker, env!("CARGO_PKG_VERSION"));
        }
        outcome
    };
    if !dry_run {
        if let Some(parent) = path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                return HookOutcome::Failed(format!("mkdir {}: {e}", parent.display()));
            }
        }
    }
    let mut settings: serde_json::Value = match super::json_util::read_json_object_or_bail(&path) {
        Ok(v) => v,
        Err(e) => return HookOutcome::Failed(e),
    };
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
    // #486 defensive: handle non-object hooks.
    if !hooks.is_object() {
        *hooks = serde_json::json!({});
    }
    let post_array = ensure_array_entry(hooks.as_object_mut().unwrap(), "PostToolUse");
    // #561/#562 — content-aware idempotency (same as install_claude_code_hook).
    // Grep joins Read: both stage a compacted view at PreToolUse that this hook
    // delivers as additionalContext. Glob joins them for a different reason —
    // it needs no staging, it rewrites its own result in place via
    // `updatedToolOutput` (see `handle_glob_posttool`). Changing this string
    // makes an existing install REFRESH itself on the next `tokenade install`,
    // which is how the matcher migrates without a bespoke migration.
    let canonical_matcher = "Read|Grep|Glob";
    let canonical_command = build_hook_command(exe, "hookwrap-postread", debug_log_path);
    let mut stale_idx: Option<usize> = None;
    let mut already = false;
    for (i, g) in post_array.iter().enumerate() {
        let (matched, has_tag) =
            group_matches_tokenade_hook(g, "tokenade-postread", "hookwrap-postread");
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
            // Drift OR missing `_tag` — heal in place (#775).
            stale_idx = Some(i);
        }
        break;
    }
    if already {
        return finish(HookOutcome::AlreadyPresent);
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
            "_tag": "tokenade-postread"
        }]
    }));
    let outcome = |p: PathBuf| {
        if was_refresh {
            HookOutcome::Refreshed(p)
        } else {
            HookOutcome::Installed(p)
        }
    };
    if dry_run {
        return outcome(path);
    }
    let content = serde_json::to_string_pretty(&settings).unwrap();
    match idempotent_write(&path, &content) {
        Ok(true) => finish(outcome(path)),
        Ok(false) => finish(outcome(path)),
        Err(e) => HookOutcome::Failed(format!("write {}: {e}", path.display())),
    }
}

/// #955 v2 (case 3) — install the PostToolUse `WebFetch` failure net
/// (`hookwrap-postwebfetch`): when the NATIVE WebFetch ran (selective
/// mode passthrough) and its result looks like a fetch failure
/// (403/bot-block/timeout/near-empty), a one-line `additionalContext`
/// hint points the agent at `tokenade web <url>` (browser-profile
/// fetch). Tagged `tokenade-postwebfetch` for clean uninstall.
pub(crate) fn install_claude_code_postwebfetch_hook(
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
    let canonical_matcher = "WebFetch";
    let canonical_command = build_hook_command(exe, "hookwrap-postwebfetch", debug_log_path);
    let mut stale_idx: Option<usize> = None;
    let mut already = false;
    for (i, g) in post_array.iter().enumerate() {
        let (matched, has_tag) =
            group_matches_tokenade_hook(g, "tokenade-postwebfetch", "hookwrap-postwebfetch");
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
            "_tag": "tokenade-postwebfetch"
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
    let content = serde_json::to_string_pretty(&settings).unwrap();
    match idempotent_write(&path, &content) {
        Ok(_) => outcome,
        Err(e) => HookOutcome::Failed(format!("write {}: {e}", path.display())),
    }
}

/// Install a PreToolUse hook that fires BEFORE Edit/Write/MultiEdit to
/// snapshot the file into dedup_ref. This enables `file_diff` hints even
/// when the agent never Read the file prior to editing.
/// Tagged `tokenade-pre-edit` for clean uninstall.
pub(crate) fn install_claude_code_pre_edit_hook(
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
    if !hooks.is_object() {
        *hooks = serde_json::json!({});
    }
    let pre_array = ensure_array_entry(hooks.as_object_mut().unwrap(), "PreToolUse");
    let canonical_matcher = "Edit|Write|MultiEdit";
    let canonical_command = build_hook_command(exe, "pre-edit-snapshot", debug_log_path);
    let mut stale_idx: Option<usize> = None;
    let mut already = false;
    for (i, g) in pre_array.iter().enumerate() {
        let (matched, has_tag) =
            group_matches_tokenade_hook(g, "tokenade-pre-edit", "pre-edit-snapshot");
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
        pre_array.remove(idx);
    }
    pre_array.push(serde_json::json!({
        "matcher": canonical_matcher,
        "hooks": [{
            "type": "command",
            "command": canonical_command,
            "_tag": "tokenade-pre-edit"
        }]
    }));
    let outcome = |p: PathBuf| {
        if was_refresh {
            HookOutcome::Refreshed(p)
        } else {
            HookOutcome::Installed(p)
        }
    };
    if dry_run {
        return outcome(path);
    }
    let content = serde_json::to_string_pretty(&settings).unwrap();
    match idempotent_write(&path, &content) {
        Ok(true) | Ok(false) => outcome(path),
        Err(e) => HookOutcome::Failed(format!("write {}: {e}", path.display())),
    }
}

pub(crate) fn install_claude_code_hook(
    root: &Path,
    exe: &str,
    debug_log_path: Option<&str>,
    global: bool,
    home: Option<&Path>,
    dry_run: bool,
) -> HookOutcome {
    // #556 — TOKENADE_NO_HOOK opt-out moved INTO the installer (was a
    // caller-side gate that didn't cover every installer — the LSP
    // enforcement hook ignored it). Returning OptedOut makes the
    // HookOutcome variant honest at the type level.
    if is_hook_opted_out() {
        return HookOutcome::OptedOut;
    }
    let Some(path) = hook_settings_path(root, global, home) else {
        return HookOutcome::Failed("no home dir resolved for global install".into());
    };
    if !dry_run {
        if let Some(parent) = path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                return HookOutcome::Failed(format!("mkdir {}: {e}", parent.display()));
            }
        }
    }
    // Defensive backup: if an existing settings.json is present, snapshot
    // it before we mutate. `backup_file` dedups (no copy when identical to
    // the newest existing backup) and caps retention, so the ~6h self-heal
    // re-running doesn't pile up hundreds of copies. Skipped silently if
    // there's no existing file (nothing to lose) AND in dry-run (no mutation).
    if !dry_run && path.exists() {
        crate::diagnostics_cli::backup_file(&path);
    }
    let mut settings: serde_json::Value = match super::json_util::read_json_object_or_bail(&path) {
        Ok(v) => v,
        Err(e) => return HookOutcome::Failed(e),
    };
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
    // #486 defensive: handle non-object hooks.
    if !hooks.is_object() {
        *hooks = serde_json::json!({});
    }
    let post_array = ensure_array_entry(hooks.as_object_mut().unwrap(), "PostToolUse");

    // #561/#562 — canonical entry the install path WANTS. Compare an
    // existing `tokenade`-tagged entry against this; match → bail
    // AlreadyPresent, drift → remove stale + push fresh + return
    // Refreshed.
    // Matcher is `Bash|Read`: `cmd_hookwrap` handles Bash (gain log +
    // search-tool hint) and Read (the skeleton/auto read-hint via
    // `handle_read_posttool`). The 2026-06 pivot narrowed this to `Bash`,
    // which silently disabled the read-hint even though its handler stayed
    // live — restored here. (Read also has the separate `hookwrap-postread`
    // hook for staged-content delivery; both firing is fine — the read-hint
    // bails when the result already carries a `§ref:` marker, so they don't
    // double up.)
    let canonical_matcher = "Bash|Read";
    let canonical_command = build_hook_command(exe, "hookwrap", debug_log_path);
    let mut stale_idx: Option<usize> = None;
    let mut already = false;
    for (i, g) in post_array.iter().enumerate() {
        let (matched, has_tag) = group_matches_tokenade_hook(g, "tokenade", "hookwrap");
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
            // Drift OR missing `_tag` — heal in place (#775).
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
            "_tag": "tokenade"
        }]
    }));

    let outcome = |p: PathBuf| {
        if was_refresh {
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
