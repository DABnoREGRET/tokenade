//! **Cursor** — the only agent of this family that goes through HOOKS.
//!
//! The others receive a plugin file; Cursor has a hooks file with its own
//! shape, its own matchers, and a post-tool entry to add WITHOUT overwriting
//! the user's.
//!
//! That is why it is alone here: putting it with the plugins would suggest a
//! shared function is possible, and the first attempt would overwrite a
//! configuration.

use super::*;

pub(crate) fn install_cursor_adapter(
    _exe: &str,
    _dbg: Option<&str>,
    home: Option<&Path>,
    dry_run: bool,
) -> HookOutcome {
    // Cursor's installer resolves the binary itself and reports a bool.
    let Some(h) = home else {
        return HookOutcome::Failed("no home dir resolved for cursor hooks".into());
    };
    match install_cursor_hooks(h, dry_run) {
        Ok(true) => HookOutcome::Installed(cursor_hooks_path(h)),
        Ok(false) => HookOutcome::AlreadyPresent,
        Err(e) => HookOutcome::Failed(e),
    }
}

/// Every hook agent, its config file, and how to re-assert its hooks.
///
/// ONE list, because three consumers need it and three private copies had
/// already drifted apart: `PLUGIN_INSTALLS` (what `install` writes), the
/// upgrade refresh (what a new version repairs) and the dashboard badge (what
/// counts as "tokenade is wired here"). An agent added to one and not the
/// others fails silently in a different way each time — never installed, never
/// repaired, or reported absent while working.
///
/// Claude Code carries `None`: it is refreshed by its own, differently-shaped
/// path in `hook_health`, and duplicating it here would re-assert its suite
/// twice per upgrade. It still belongs to the list so the badge sees it.
pub(crate) fn hook_agent_configs(home: &Path) -> [(PathBuf, Option<LifecycleInstaller>); 7] {
    [
        (home.join(".claude").join("settings.json"), None),
        (
            home.join(".codex").join("hooks.json"),
            Some(install_codex_passive_hooks as LifecycleInstaller),
        ),
        (
            home.join(".factory").join("hooks.json"),
            Some(install_droid_hooks as LifecycleInstaller),
        ),
        (
            home.join(".copilot").join("hooks").join("tokenade.json"),
            Some(install_copilot_hooks as LifecycleInstaller),
        ),
        (
            home.join(".grok").join("hooks").join("tokenade.json"),
            Some(install_grok_hooks as LifecycleInstaller),
        ),
        (
            home.join(".qwen").join("settings.json"),
            Some(install_qwen_adapter as LifecycleInstaller),
        ),
        (
            cursor_hooks_path(home),
            Some(install_cursor_adapter as LifecycleInstaller),
        ),
    ]
}

/// Merge tokenade's Cursor hooks into ~/.cursor/hooks.json, preserving any
/// existing user hooks. Idempotent — returns `Ok(true)` when it actually
/// changed something, `Ok(false)` when everything was already present.
///
/// TWO entries:
/// * `preToolUse` rewrites a Shell command into `tokenade wrap <cmd>` (via
///   `updated_input`) and redirects a Read to a folded temp file.
/// * `postToolUse` folds MCP tool results via `updated_mcp_tool_output`.
///   **This line long said that "Cursor has no after-hook to substitute
///   output": that was true, it no longer is.** The field is documented "For
///   MCP tools only: replaces the tool output seen by the model", and
///   `postToolUse` "fires for all tools" — hence no `matcher` and a guard on
///   the SHAPE of the result rather than on a name prefix Cursor promises
///   nowhere.
pub(crate) fn install_cursor_hooks(home: &Path, dry_run: bool) -> Result<bool, String> {
    let path = cursor_hooks_path(home);
    if dry_run {
        return Ok(true);
    }
    // Unparsable hooks.json → bail (never replace a user's hand-edited file
    // with a tokenade-only one — same hardening as the Claude/Grok writers).
    let mut root: serde_json::Value = crate::install::json_util::read_json_object_or_bail(&path)?;
    let hooks = root
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));
    if !hooks.is_object() {
        *hooks = serde_json::json!({});
    }
    let arr = hooks
        .as_object_mut()
        .unwrap()
        .entry("preToolUse")
        .or_insert_with(|| serde_json::json!([]));
    if !arr.is_array() {
        *arr = serde_json::json!([]);
    }
    let list = arr.as_array_mut().unwrap();
    // `Shell` alone left every Cursor Read unreached — measured against a real
    // `cursor-agent`, `preToolUse` fires for `tool_name: "Read"` and the
    // handler can redirect it to a compacted temp, but the matcher never let
    // the event through. It is a regex over the tool name, so both go in one
    // alternation.
    const CURSOR_MATCHER: &str = "Shell|Read";
    let mut migrated = false;
    for e in list.iter_mut() {
        let is_ours = e
            .get("command")
            .and_then(|c| c.as_str())
            .map(|c| c.contains("tokenade"))
            .unwrap_or(false);
        if !is_ours {
            continue;
        }
        // Heal in place. An install that ran before the Read branch existed
        // carries the narrow matcher, and returning early on "already present"
        // would leave that user on the shell-only hook for ever — the same
        // shape as every other matcher migration in this file.
        let stale = e.get("matcher").and_then(|m| m.as_str()) != Some(CURSOR_MATCHER);
        if stale {
            if let Some(o) = e.as_object_mut() {
                o.insert(
                    "matcher".to_string(),
                    serde_json::Value::String(CURSOR_MATCHER.to_string()),
                );
            }
            migrated = true;
        }
    }
    // "Nothing to add and nothing to repair" is no longer decided HERE: the
    // `postToolUse` entry is examined next, and leaving early would miss it on
    // every machine where the `preToolUse` was already set — that is, on every
    // machine already installed.
    let already_pre = list.iter().any(|e| {
        e.get("command")
            .and_then(|c| c.as_str())
            .map(|c| c.contains("tokenade"))
            .unwrap_or(false)
    });
    // Absolute binary path: Cursor launched from a desktop icon often lacks
    // the user's shell PATH, so a bare `tokenade` silently never fires.
    // `stable_hook_exe` keeps this off transient updater-temp / build-dir
    // paths (see its doc comment).
    if !migrated && !already_pre {
        let exe = stable_hook_exe();
        list.push(serde_json::json!({
            "command": format!("{exe} hookwrap-pretool"),
            "matcher": CURSOR_MATCHER,
            "description": "Route shell commands and file reads through tokenade for token-optimized output"
        }));
    }
    let post_added = ensure_cursor_posttool_entry(&mut root);
    if !migrated && !post_added && already_pre {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    crate::write_json_atomic(
        &path,
        &serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(true)
}

/// Adds the MCP folder's `postToolUse` entry if it is missing, or repairs its
/// command if the binary moved. Returns `true` when the document changed.
///
/// No `matcher`: Cursor's `postToolUse` fires for ALL tools, and an MCP tool's
/// name carries no declared prefix there. The handler is what discards
/// whatever is not an MCP result, by its shape.
pub(crate) fn ensure_cursor_posttool_entry(root: &mut serde_json::Value) -> bool {
    let exe = stable_hook_exe();
    let want = format!("{exe} hookwrap-cursor-mcp-post");
    let hooks = root
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));
    if !hooks.is_object() {
        *hooks = serde_json::json!({});
    }
    let arr = hooks
        .as_object_mut()
        .unwrap()
        .entry("postToolUse")
        .or_insert_with(|| serde_json::json!([]));
    if !arr.is_array() {
        *arr = serde_json::json!([]);
    }
    let list = arr.as_array_mut().unwrap();
    for e in list.iter_mut() {
        let Some(cmd) = e.get("command").and_then(|c| c.as_str()) else {
            continue;
        };
        if !cmd.contains("hookwrap-cursor-mcp-post") {
            continue;
        }
        if cmd == want {
            return false;
        }
        // The binary moved: repair in place rather than add a duplicate.
        if let Some(o) = e.as_object_mut() {
            o.insert("command".to_string(), serde_json::Value::String(want));
        }
        return true;
    }
    list.push(serde_json::json!({
        "command": want,
        "description": "Fold verbose MCP tool results before the model reads them"
    }));
    true
}
