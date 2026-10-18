//! **Say what is wrapped, without changing anything.**
//!
//! The read-only counterpart of `sweep`: enough to answer "does tokenade
//! wrap anything on my machine, and what?" without a diagnostic
//! being allowed to write.

use super::*;

/// The four config files we know how to read. Detection mirrors the
/// existing install logic.
/// Per-entry wrap state surfaced to healthcheck / dashboard. The
/// `wrapper_command` is the literal string we wrote into the entry's
/// `command` field — comparing it to `env::current_exe()` tells the
/// user whether their wraps need refreshing (the absolute-path
/// migration).
#[derive(Debug, Clone)]
pub struct WrappedEntry {
    pub config_path: String,
    pub name: String,
    pub wrapper_command: String,
}

/// Walk every known agent config and yield each wrapped MCP entry
/// (identified by the [`ORIGINAL_KEY`] sentinel). Used by
/// `tokenade healthcheck` to flag stale wraps that point to a
/// tokenade binary the agent's spawning shell may not resolve.
pub fn inspect_wrapped_entries() -> Vec<WrappedEntry> {
    let mut out = Vec::new();
    for path in config_paths() {
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        // TOML agents (Codex, Grok). This inspector was JSON-only, so once the
        // wrap path learned TOML the two disagreed: `install` reported wrapping
        // two servers and `healthcheck` counted one. A wrap nobody can see is a
        // wrap nobody can diagnose — and this is the FIFTH function doing the
        // same job on the same files.
        if path.extension().is_some_and(|e| e == "toml") {
            if let Ok(doc) = toml::from_str::<toml::Value>(&content) {
                if let Some(servers) = doc.get("mcp_servers").and_then(|v| v.as_table()) {
                    for (name, entry) in servers {
                        let Some(t) = entry.as_table() else { continue };
                        if !t.contains_key(ORIGINAL_KEY) {
                            continue;
                        }
                        out.push(WrappedEntry {
                            config_path: path.display().to_string(),
                            name: name.clone(),
                            wrapper_command: t
                                .get("command")
                                .and_then(|c| c.as_str())
                                .unwrap_or("")
                                .to_string(),
                        });
                    }
                }
            }
            continue;
        }
        let Ok(mut root): Result<Value, _> = parse_json_tolerant(&content) else {
            continue;
        };
        // Same walker as wrap/unwrap/purge: top level PLUS `projects[*]`.
        let mut found: Vec<(String, String)> = Vec::new();
        for_each_server_map(&mut root, |servers| {
            for (name, entry) in servers.iter() {
                let Some(obj) = entry.as_object() else {
                    continue;
                };
                if !obj.contains_key(ORIGINAL_KEY) {
                    continue;
                }
                // The wrapper command lives in `command`. Object shape
                // → it's a String. Codex array shape → it's the first
                // element of an Array.
                let wrapper_command = match obj.get("command") {
                    Some(Value::String(s)) => s.clone(),
                    Some(Value::Array(arr)) => arr
                        .first()
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    _ => String::new(),
                };
                found.push((name.clone(), wrapper_command));
            }
        });
        for (name, wrapper_command) in found {
            out.push(WrappedEntry {
                config_path: path.display().to_string(),
                name,
                wrapper_command,
            });
        }
    }
    out
}
