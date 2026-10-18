//! **The four places where a tool declares itself** on this machine.
//!
//! Claude Code's configuration (in several possible locations), an
//! MCP server entry, a hooks group, a global npm package. Every
//! detector goes through one of them, and writing them once keeps a
//! twenty-fifth detector from inventing its own way of looking — hence its
//! own way of being wrong.

use super::*;

// =======================================================================
// Helpers
// =======================================================================

/// The set of Claude / agent config files we know how to inspect for MCP
/// entries. Missing files are tolerated (the detector just gets `None`
/// from `read_json`).
pub fn claude_config_candidates(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".claude.json"),
        home.join(".claude").join("settings.json"),
        home.join(".config").join("claude").join("config.json"),
        home.join(".cursor").join("mcp.json"),
        home.join(".codex").join("config.json"),
        home.join(".windsurf").join("mcp_config.json"),
        home.join(".config").join("github-copilot").join("mcp.json"),
    ]
}

/// True if any of the standard MCP config shapes contains an entry named
/// `key`. Covers both `mcpServers.<key>` (Claude/Cursor/Codex/Windsurf)
/// and `mcp.<key>` (OpenCode).
pub(crate) fn mcp_has_entry(v: &Value, key: &str) -> bool {
    v.get("mcpServers")
        .and_then(|s| s.as_object())
        .is_some_and(|s| s.contains_key(key))
        || v.get("mcp")
            .and_then(|s| s.as_object())
            .is_some_and(|s| s.contains_key(key))
}

/// Iterate hook groups from a Claude settings.json. Yields `(event,
/// [(matcher, command), …])` tuples. Tolerates the wide shape variation
/// (PreToolUse, PostToolUse, SessionStart, …). Designed to be cheap on
/// missing/malformed input.
pub(crate) fn iter_hook_groups(v: &Value) -> BTreeMap<String, Vec<(String, String)>> {
    let mut out: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    let Some(hooks) = v.get("hooks").and_then(|h| h.as_object()) else {
        return out;
    };
    for (event, groups) in hooks {
        let Some(arr) = groups.as_array() else {
            continue;
        };
        for group in arr {
            let matcher = group
                .get("matcher")
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_string();
            let Some(hook_arr) = group.get("hooks").and_then(|h| h.as_array()) else {
                continue;
            };
            for h in hook_arr {
                let cmd = h
                    .get("command")
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .to_string();
                if !cmd.is_empty() {
                    out.entry(event.clone())
                        .or_default()
                        .push((matcher.clone(), cmd));
                }
            }
        }
    }
    out
}

/// Given an npm shim path like `.../bin/codegraph` symlinked to
/// `.../lib/node_modules/<scope>/<pkg>/something.js`, return the full
/// package name (e.g. `@colbymchenry/codegraph`). Falls back to None if
/// the path isn't a recognisable npm-global layout.
pub(crate) fn resolve_npm_global_package(shim_path: &Path) -> Option<String> {
    let target = std::fs::read_link(shim_path).ok()?;
    let abs = if target.is_absolute() {
        target
    } else {
        shim_path.parent()?.join(target)
    };
    // Look for `node_modules/<x>/<y or pkg>`. Pkg name is `<x>` (unscoped)
    // or `<x>/<y>` (scoped where `<x>` starts with @).
    let comps: Vec<_> = abs.components().collect();
    let nm_idx = comps.iter().position(|c| c.as_os_str() == "node_modules")?;
    let after = &comps[nm_idx + 1..];
    if after.is_empty() {
        return None;
    }
    let first = after[0].as_os_str().to_string_lossy().into_owned();
    if first.starts_with('@') && after.len() >= 2 {
        let second = after[1].as_os_str().to_string_lossy().into_owned();
        Some(format!("{first}/{second}"))
    } else {
        Some(first)
    }
}
