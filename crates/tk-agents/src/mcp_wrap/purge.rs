//! **Remove our entries, and nothing else.**
//!
//! An uninstall that leaves an entry behind breaks the agent: the
//! wrapped command no longer exists. An uninstall that is too broad breaks
//! someone else's configuration. The boundary of each purge is therefore the
//! real subject, and it is re-read format by format — Hermes's YAML, JSON,
//! Reasonix's and Grok's TOML.

use super::*;

/// One config file's result from a [`remove_self_entry_all_known_configs`]
/// pass. `tokenade uninstall` previously left the `tokenade` self-entry
/// (the one `install` writes under `mcpServers` / `mcp`) in every agent
/// config, so after `brew uninstall tokenade` agents still tried to
/// spawn a binary that no longer existed.
#[derive(Debug, Default, Clone)]
pub struct SelfRemovalReport {
    pub config_path: String,
    pub removed: bool,
    pub error: Option<String>,
}

/// Strip the `tokenade` self-entry from every known agent config.
/// Symmetric to the `install`-time `write_mcp_config_*` writers that
/// insert it. Called by `tokenade uninstall` after the third-party
/// wraps are restored.
pub fn remove_self_entry_all_known_configs() -> Vec<SelfRemovalReport> {
    cleanup_config_paths()
        .into_iter()
        .map(|p| remove_self_entry_one_config(&p))
        .collect()
}

/// Dry-run variant of [`remove_self_entry_all_known_configs`] — reports
/// what WOULD be removed without modifying any file.
pub fn remove_self_entry_all_known_configs_dry() -> Vec<SelfRemovalReport> {
    cleanup_config_paths()
        .into_iter()
        .map(|p| remove_self_entry_one_config_dry(&p))
        .collect()
}

/// Strip the `tokenade` self-entry from a single config file. Mirrors
/// the per-file granularity of [`unwrap_one_config`].
pub fn remove_self_entry_one_config(path: &Path) -> SelfRemovalReport {
    remove_self_entry_one_config_inner(path, false)
}

/// Dry-run variant of [`remove_self_entry_one_config`] — populates the
/// report but skips the disk write.
pub fn remove_self_entry_one_config_dry(path: &Path) -> SelfRemovalReport {
    remove_self_entry_one_config_inner(path, true)
}

pub(super) fn remove_self_entry_one_config_inner(path: &Path, dry_run: bool) -> SelfRemovalReport {
    let mut report = SelfRemovalReport {
        config_path: path.display().to_string(),
        ..Default::default()
    };
    // TOML agents (Codex, Grok). `purge_grok_toml` already strips exactly
    // `[mcp_servers.tokenade]` and its sub-tables, so this is a delegation,
    // not a second implementation. Without it, uninstall printed
    // `parse: expected value at line 1 column 1` at every Codex user and
    // moved on — a self-entry it could not read is one it cannot remove.
    if path.extension().is_some_and(|e| e == "toml") {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                report.error = Some(format!("read: {e}"));
                return report;
            }
        };
        // Two TOML shapes under the same extension: `[mcp_servers.tokenade]`
        // (Codex, Grok) and a `[[plugins]]` entry whose `name` is
        // `tokenade` (Reasonix). This path only knows the extension, so it
        // tries both; the one that does not apply removes nothing.
        let purged = purge_grok_toml(&content).and_then(|(c, removed)| {
            purge_reasonix_toml(&c).map(|(c2, removed2)| (c2, removed || removed2))
        });
        match purged {
            Err(e) => report.error = Some(e),
            Ok((_, false)) => {}
            Ok((new_content, true)) => {
                report.removed = true;
                if !dry_run {
                    if let Err(e) = write_locked(path, &new_content) {
                        report.error = Some(format!("write: {e}"));
                    }
                }
            }
        }
        return report;
    }
    let mut root: Value = match read_config_root(path, "") {
        Ok(v) => v,
        Err(e) => {
            report.error = Some(e);
            return report;
        }
    };
    // Same walker as wrap/unwrap/purge: top level PLUS `projects[*]`, where
    // Claude Code's default `local` scope keeps its servers. The two-key
    // top-level loop this replaces could not see a self-entry there.
    let mut removed = false;
    for_each_server_map(&mut root, |servers| {
        if servers.remove("tokenade").is_some() {
            removed = true;
        }
    });
    report.removed = removed;
    if !removed || dry_run {
        return report;
    }
    let pretty = match serde_json::to_string_pretty(&root) {
        Ok(p) => p,
        Err(e) => {
            report.error = Some(format!("serialise: {e}"));
            return report;
        }
    };
    if let Err(e) = write_locked(path, &pretty) {
        report.error = Some(format!("write: {e}"));
    }
    report
}

/// Purge any `tokenade` self-entry from EVERY known agent's MCP config,
/// across all wire formats — the zero-MCP pivot's install/update hook.
///
/// Unlike [`remove_self_entry_all_known_configs`] (JSON-only, four
/// hardcoded paths, written for `uninstall`) this is agent-driven via
/// [`crate::detect`] and FORMAT-AWARE: JSON `mcpServers` / `mcp` /
/// nested `mcp.servers`, Hermes YAML `mcp_servers:`, Grok TOML
/// `[mcp_servers.tokenade]`, plus the per-project `projects[*]` maps in
/// `~/.claude.json`. Idempotent: a config with no `tokenade` entry is a
/// no-op (`removed = false`, no write). `install` and the post-upgrade
/// refresh call this so a user migrating from a pre-pivot version stops
/// spawning the now-removed `tokenade mcp` command.
pub fn purge_tokenade_mcp_entries(
    root: &Path,
    home: Option<&Path>,
    dry_run: bool,
) -> Vec<SelfRemovalReport> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for agent in crate::detect::Agent::all() {
        if !agent.integration().mcp {
            continue;
        }
        let Some(path) = agent.mcp_config_path(root, home) else {
            continue;
        };
        if !seen.insert(path.clone()) {
            // Several agents can resolve to the same file (rare); only
            // touch it once.
            continue;
        }
        if !path.exists() {
            continue;
        }
        // The EARLIEST hand on an agent config during an install, and
        // therefore the only one whose copy is really the user's. Everything
        // downstream that tried to take one — three writers and a hook
        // installer — found a file this pass had already edited, and
        // "original" meant our own output. Measured 2026-08-19 on Gemini CLI
        // and Qwen: a user's `{"mcpServers":{}}` came back `{}` after an
        // uninstall, the empty map dropped here and never put back.
        if !dry_run {
            crate::paths::backup_once(&path);
        }
        out.push(purge_one_config(&path, agent.mcp_config_format(), dry_run));
    }
    // Claude Code's USER-scope MCP store lives in `~/.claude.json`
    // (a top-level `mcpServers` map plus per-project `projects[*]`
    // maps). No agent's `mcp_config_path()` points there —
    // `mcp_config_path(ClaudeCode)` yields the PROJECT-scoped
    // `<root>/.mcp.json` instead — so a pre-pivot `tokenade` self-entry
    // written into the global store would otherwise survive the purge.
    let global_claude = home
        .map(|h| h.to_path_buf())
        .or_else(crate::platform::home_dir)
        .map(|h| h.join(".claude.json"));
    if let Some(path) = global_claude {
        if path.exists() && seen.insert(path.clone()) {
            out.push(purge_one_config(
                &path,
                crate::detect::McpConfigFormat::JsonMcpServers,
                dry_run,
            ));
        }
    }
    out
}

pub(super) fn purge_one_config(
    path: &Path,
    format: crate::detect::McpConfigFormat,
    dry_run: bool,
) -> SelfRemovalReport {
    use crate::detect::McpConfigFormat;
    let mut report = SelfRemovalReport {
        config_path: path.display().to_string(),
        ..Default::default()
    };
    let content = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            report.error = Some(format!("read: {e}"));
            return report;
        }
    };
    let result = match format {
        McpConfigFormat::YamlHermesMcpServers => purge_hermes_yaml(&content),
        McpConfigFormat::TomlMcpServers => purge_grok_toml(&content),
        other => purge_json_config(&content, other),
    };
    let (new_content, removed) = match result {
        Ok(v) => v,
        Err(e) => {
            report.error = Some(e);
            return report;
        }
    };
    report.removed = removed;
    if !removed || dry_run {
        return report;
    }
    if let Err(e) = write_locked(path, &new_content) {
        report.error = Some(format!("write: {e}"));
    }
    report
}

/// Remove the `tokenade` key from the object reached by walking `path`
/// (e.g. `["mcp", "servers"]`). Returns whether anything was removed.
pub(super) fn remove_tokenade_at(root: &mut Value, path: &[&str]) -> bool {
    let mut cur = root;
    for key in path {
        cur = match cur.get_mut(*key) {
            Some(v) if v.is_object() => v,
            _ => return false,
        };
    }
    match cur.as_object_mut() {
        Some(o) => o.remove("tokenade").is_some(),
        None => false,
    }
}

pub(super) fn purge_json_config(
    content: &str,
    format: crate::detect::McpConfigFormat,
) -> Result<(String, bool), String> {
    use crate::detect::McpConfigFormat;
    let mut root: Value = parse_json_tolerant(content).map_err(|e| format!("parse: {e}"))?;
    let mut removed = false;
    match format {
        McpConfigFormat::JsonOpenCodeMcp => {
            removed |= remove_tokenade_at(&mut root, &["mcp"]);
        }
        McpConfigFormat::JsonOpenClawMcp => {
            removed |= remove_tokenade_at(&mut root, &["mcp", "servers"]);
        }
        // JsonMcpServers, JsonGeminiSettings: top-level `mcpServers`,
        // plus the per-project `projects[*].mcpServers` maps that only
        // `~/.claude.json` carries (a no-op everywhere else).
        _ => {
            removed |= remove_tokenade_at(&mut root, &["mcpServers"]);
            if let Some(projects) = root.get_mut("projects").and_then(|p| p.as_object_mut()) {
                for proj in projects.values_mut() {
                    removed |= remove_tokenade_at(proj, &["mcpServers"]);
                }
            }
        }
    }
    if !removed {
        return Ok((content.to_string(), false));
    }
    let pretty = serde_json::to_string_pretty(&root).map_err(|e| format!("serialise: {e}"))?;
    Ok((pretty, true))
}

/// Strip the `tokenade:` block written under `mcp_servers:` by the
/// Add `plugin` to Hermes' `plugins.enabled` allow-list in `config.yaml`
/// (user plugins are opt-in via that list — Hermes won't load one that isn't
/// named). Text-surgical so unrelated config is preserved. Returns
/// `(new_content, changed)`; idempotent — a no-op (changed=false) when the
/// plugin is already enabled. Handles: empty/absent config, a `plugins:` block
/// with no `enabled:`, a block-style `enabled:` list, and a flow-style
/// `enabled: [..]` list.
pub fn enable_plugin_in_hermes_config(content: &str, plugin: &str) -> (String, bool) {
    let lines: Vec<&str> = content.lines().collect();
    let indent_of = |l: &str| l.len() - l.trim_start().len();

    // Locate a top-level `plugins:` key.
    let plugins_idx = lines
        .iter()
        .position(|l| indent_of(l) == 0 && l.trim() == "plugins:");

    let Some(pidx) = plugins_idx else {
        // No plugins block — append a fresh one.
        let mut s = content.to_string();
        if !s.is_empty() && !s.ends_with('\n') {
            s.push('\n');
        }
        s.push_str(&format!("plugins:\n  enabled:\n    - {plugin}\n"));
        return (s, true);
    };

    // Scan the plugins block (lines indented deeper than `plugins:`) for an
    // `enabled:` key.
    let mut enabled_idx = None;
    let mut j = pidx + 1;
    while j < lines.len() {
        let l = lines[j];
        if l.trim().is_empty() {
            j += 1;
            continue;
        }
        if indent_of(l) == 0 {
            break; // left the plugins block
        }
        if l.trim() == "enabled:" || l.trim_start().starts_with("enabled:") {
            enabled_idx = Some(j);
            break;
        }
        j += 1;
    }

    let mut out: Vec<String> = lines.iter().map(|s| s.to_string()).collect();

    let Some(eidx) = enabled_idx else {
        // plugins block without enabled: — insert it right after `plugins:`.
        let child = " ".repeat(indent_of(lines[pidx]) + 2);
        out.insert(pidx + 1, format!("{child}enabled:"));
        out.insert(pidx + 2, format!("{child}  - {plugin}"));
        return (out.join("\n") + "\n", true);
    };

    let eline = lines[eidx].trim_start();
    let rest = eline["enabled:".len()..].trim();
    if !rest.is_empty() && rest.starts_with('[') {
        // Flow-style: enabled: [a, b]. Already present?
        let inner = rest.trim_start_matches('[').trim_end_matches(']');
        if inner.split(',').any(|t| t.trim() == plugin) {
            return (content.to_string(), false);
        }
        let new_inner = if inner.trim().is_empty() {
            plugin.to_string()
        } else {
            format!("{}, {plugin}", inner.trim_end())
        };
        let new_line = format!(
            "{}enabled: [{new_inner}]",
            " ".repeat(indent_of(lines[eidx]))
        );
        out[eidx] = new_line;
        return (out.join("\n") + "\n", true);
    }

    // Block-style list: collect `- item` lines beneath `enabled:`.
    let enabled_indent = indent_of(lines[eidx]);
    let mut k = eidx + 1;
    let mut last_item = eidx;
    while k < lines.len() {
        let l = lines[k];
        if l.trim().is_empty() {
            k += 1;
            continue;
        }
        let ind = indent_of(l);
        if ind <= enabled_indent {
            break;
        }
        if l.trim_start().starts_with("- ") {
            if l.trim_start()[2..].trim() == plugin {
                return (content.to_string(), false); // already enabled
            }
            last_item = k;
        }
        k += 1;
    }
    let item_indent = " ".repeat(enabled_indent + 2);
    out.insert(last_item + 1, format!("{item_indent}- {plugin}"));
    (out.join("\n") + "\n", true)
}

/// Hermes YAML writer — the key line plus every more-deeply-indented
/// line beneath it. Text-surgical (no YAML round-trip) so unrelated
/// formatting is preserved.
pub(super) fn purge_hermes_yaml(content: &str) -> Result<(String, bool), String> {
    let lines: Vec<&str> = content.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut removed = false;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let indent = line.len() - line.trim_start().len();
        if indent > 0 && line.trim() == "tokenade:" {
            removed = true;
            i += 1;
            // Drop the block body: contiguous lines indented deeper than
            // the `tokenade:` key. A blank line or a shallower key ends it.
            while i < lines.len() {
                let l = lines[i];
                let ind = l.len() - l.trim_start().len();
                if !l.trim().is_empty() && ind > indent {
                    i += 1;
                } else {
                    break;
                }
            }
            continue;
        }
        out.push(line.to_string());
        i += 1;
    }
    if !removed {
        return Ok((content.to_string(), false));
    }
    let mut result = out.join("\n");
    if content.ends_with('\n') {
        result.push('\n');
    }
    Ok((result, true))
}

/// Strip a `[[plugins]]` entry whose `name` is `tokenade` from a Reasonix
/// `config.toml`. The entry runs from its `[[plugins]]` header to the next
/// header of any kind, sub-tables included, so nothing survives as an orphan.
pub(super) fn purge_reasonix_toml(content: &str) -> Result<(String, bool), String> {
    // Same line-based half as the two transformers, same BOM hazard — and here
    // it is the UNWRAP side, so the damage is a half-removed install. Note that
    // `str::trim` does NOT help: U+FEFF is a format character (Cf), not
    // whitespace, so `lines[0].trim() == "[[plugins]]"` stays false.
    let (bom, content) = split_bom(content);
    let lines: Vec<&str> = content.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut removed = false;
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() == "[[plugins]]" {
            let mut j = i + 1;
            let mut is_self = false;
            while j < lines.len() && !lines[j].trim_start().starts_with('[') {
                let t = lines[j].trim();
                if t.starts_with("name") {
                    if let Some(v) = t.split('=').nth(1) {
                        is_self = v.trim().trim_matches(['"', '\'']) == "tokenade";
                    }
                }
                j += 1;
            }
            // Sub-tables of this entry belong to it and go with it.
            while j < lines.len() && lines[j].trim_start().starts_with("[plugins.") {
                j += 1;
                while j < lines.len() && !lines[j].trim_start().starts_with('[') {
                    j += 1;
                }
            }
            if is_self {
                removed = true;
                i = j;
                continue;
            }
            for line in lines.iter().take(j).skip(i) {
                out.push((*line).to_string());
            }
            i = j;
            continue;
        }
        out.push(lines[i].to_string());
        i += 1;
    }
    if !removed {
        return Ok((format!("{bom}{content}"), false));
    }
    let mut result = format!("{bom}{}", out.join("\n"));
    if content.ends_with('\n') {
        result.push('\n');
    }
    Ok((result, true))
}

/// Strip the `[mcp_servers.tokenade]` section appended by the Grok TOML
/// writer — its header line, its body, AND every sub-table the writer
/// emits beneath it (`[mcp_servers.tokenade.env]`, …). Each matching
/// header (the section or any `[mcp_servers.tokenade.*]` sub-table) is
/// dropped together with the lines up to the next section header, so a
/// sub-table never survives as an orphan.
pub(super) fn purge_grok_toml(content: &str) -> Result<(String, bool), String> {
    fn is_tokenade_header(trimmed: &str) -> bool {
        trimmed == "[mcp_servers.tokenade]" || trimmed.starts_with("[mcp_servers.tokenade.")
    }
    let lines: Vec<&str> = content.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut removed = false;
    let mut i = 0;
    while i < lines.len() {
        if is_tokenade_header(lines[i].trim()) {
            removed = true;
            i += 1;
            while i < lines.len() && !lines[i].trim_start().starts_with('[') {
                i += 1;
            }
            continue;
        }
        out.push(lines[i].to_string());
        i += 1;
    }
    if !removed {
        return Ok((content.to_string(), false));
    }
    let mut result = out.join("\n");
    if content.ends_with('\n') {
        result.push('\n');
    }
    Ok((result, true))
}
