//! **Rewrite TOML without reformatting it.**
//!
//! A TOML configuration file carries the order, the comments and
//! the spacing of the person who wrote it. Reading it with a library
//! then re-emitting it destroys all three. This module therefore edits the TEXT,
//! section by section — it is trickier than a serialisation round-trip
//! and it is the only way to give back a file its owner recognises.

use super::*;

/// `skipped_unrecognised_shape` and left byte-for-byte alone. Refusing
/// loudly beats a half-understood rewrite of a file the agent must parse.
pub(super) fn transform_toml_mcp_servers(
    content: &str,
    only: Option<&str>,
    wrap: bool,
    report: &mut WrapReport,
) -> Result<Option<String>, String> {
    let (bom, content) = split_bom(content);
    let parsed: toml::Value = toml::from_str(content).map_err(|e| format!("parse: {e}"))?;
    let servers = match parsed.get("mcp_servers").and_then(|v| v.as_table()) {
        Some(t) => t,
        None => return Ok(None),
    };
    let wrapper = wrapper_command_string();
    let agent = super::discover::agent_for_config_path(std::path::Path::new(&report.config_path));
    let mut out: Vec<String> = Vec::with_capacity(content.lines().count() + 4);
    let mut current: Option<String> = None;
    let mut changed = false;

    for line in content.lines() {
        if line.trim_start().starts_with('[') {
            current = toml_server_header(line);
            out.push(line.to_string());
            continue;
        }
        let Some(name) = current.clone() else {
            out.push(line.to_string());
            continue;
        };
        if let Some(want) = only {
            if name != want {
                out.push(line.to_string());
                continue;
            }
        }
        let Some(entry) = servers.get(&name).and_then(|v| v.as_table()) else {
            out.push(line.to_string());
            continue;
        };
        let key = line.split('=').next().unwrap_or("").trim();
        let has_original = entry.contains_key(ORIGINAL_KEY);

        if wrap {
            if name == "tokenade" {
                if key == "command" && !report.skipped_self.contains(&name) {
                    report.skipped_self.push(name.clone());
                }
                out.push(line.to_string());
                continue;
            }
            let kind = classify_command(
                entry.get("command").and_then(|c| c.as_str()),
                entry
                    .get("args")
                    .and_then(|a| a.as_array())
                    .and_then(|a| a.first())
                    .and_then(|v| v.as_str()),
            );
            if kind == EntryCommand::HostApp {
                if key == "command" && !report.skipped_host_app.contains(&name) {
                    report.skipped_host_app.push(name.clone());
                }
                out.push(line.to_string());
                continue;
            }
            if kind == EntryCommand::OurOwn {
                if key == "command" && !report.skipped_self.contains(&name) {
                    report.skipped_self.push(name.clone());
                }
                out.push(line.to_string());
                continue;
            }
            if has_original || kind == EntryCommand::RoutedByHand {
                if key == "command" && !report.skipped_already_wrapped.contains(&name) {
                    report.skipped_already_wrapped.push(name.clone());
                }
                out.push(line.to_string());
                continue;
            }
            let Some(orig_bin) = entry.get("command").and_then(|c| c.as_str()) else {
                if key == "command" && !report.skipped_unrecognised_shape.contains(&name) {
                    report.skipped_unrecognised_shape.push(name.clone());
                }
                out.push(line.to_string());
                continue;
            };
            let orig_args: Vec<String> = entry
                .get("args")
                .and_then(|a| a.as_array())
                .map(|a| {
                    a.iter()
                        .map(|v| v.as_str().unwrap_or_default().to_string())
                        .collect()
                })
                .unwrap_or_default();
            match key {
                "command" => {
                    let mut new_args = vec![
                        WRAPPER_SUBCOMMAND.to_string(),
                        PROXY_ARGV_SEPARATOR.to_string(),
                        orig_bin.to_string(),
                    ];
                    new_args.extend(orig_args.iter().cloned());
                    out.push(format!("command = {}", toml_str(&wrapper)));
                    out.push(format!("args = {}", toml_array(&new_args)));
                    out.push(format!(
                        "{ORIGINAL_KEY} = {{ command = {}, args = {} }}",
                        toml_str(orig_bin),
                        toml_array(&orig_args)
                    ));
                    // The app's name, for the rows; only where the server sets no
                    // `env` of its own, which this line would collide with.
                    if let (Some(a), false) = (agent.as_deref(), entry.contains_key("env")) {
                        out.push(format!("env = {{ {AGENT_ENV} = {} }}", toml_str(a)));
                    }
                    changed = true;
                    report.wrapped.push(name.clone());
                }
                // The rewritten `args` was emitted alongside `command`;
                // drop the user's original line rather than duplicate the key.
                "args" => {}
                _ => out.push(line.to_string()),
            }
            continue;
        }

        // unwrap
        let Some(original) = entry.get(ORIGINAL_KEY).and_then(|v| v.as_table()) else {
            out.push(line.to_string());
            continue;
        };
        let Some(orig_bin) = original.get("command").and_then(|c| c.as_str()) else {
            out.push(line.to_string());
            continue;
        };
        let orig_args: Vec<String> = original
            .get("args")
            .and_then(|a| a.as_array())
            .map(|a| {
                a.iter()
                    .map(|v| v.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        // Same rule as the JSON path, and the same declarant behind it: what
        // the entry launches TODAY beats the snapshot taken when it was
        // wrapped. Both TOML shapes go through here, so neither can drift.
        let (orig_bin, orig_args) = toml_live_or_snapshot(entry, orig_bin, orig_args);
        let orig_bin = orig_bin.as_str();
        match key {
            "command" => {
                out.push(format!("command = {}", toml_str(orig_bin)));
                out.push(format!("args = {}", toml_array(&orig_args)));
                changed = true;
                report.unwrapped.push(name.clone());
            }
            "args" => {}
            "env"
                if entry
                    .get("env")
                    .and_then(|e| e.as_table())
                    .is_some_and(|t| t.len() == 1 && t.contains_key(AGENT_ENV)) => {}
            k if k == ORIGINAL_KEY => {}
            _ => out.push(line.to_string()),
        }
    }

    if !changed {
        return Ok(None);
    }
    let mut result = format!("{bom}{}", out.join("\n"));
    if content.ends_with('\n') {
        result.push('\n');
    }
    Ok(Some(result))
}

/// Wrap / unwrap the `[[plugins]]` entries of a Reasonix `config.toml`.
///
/// Same line-surgical contract as [`transform_toml_mcp_servers`] — the file
/// belongs to the user, so only the lines we own are rewritten — with one
/// structural difference: this is an ARRAY of tables, so the server's name is
/// a key INSIDE the entry rather than part of its header. The entry is
/// therefore identified by its position in the parsed array, which makes the
/// rewrite independent of where `name` sits among the keys.
///
/// A file with no `plugins` array returns `Ok(None)` and is left alone, which
/// is what lets the same `.toml` caller try both shapes in turn.
pub(super) fn transform_toml_plugins_array(
    content: &str,
    only: Option<&str>,
    wrap: bool,
    report: &mut WrapReport,
) -> Result<Option<String>, String> {
    let (bom, content) = split_bom(content);
    let parsed: toml::Value = toml::from_str(content).map_err(|e| format!("parse: {e}"))?;
    let plugins = match parsed.get("plugins").and_then(|v| v.as_array()) {
        Some(a) if !a.is_empty() => a,
        _ => return Ok(None),
    };
    let wrapper = wrapper_command_string();
    let mut out: Vec<String> = Vec::with_capacity(content.lines().count() + 4);
    let mut idx: Option<usize> = None;
    let mut in_subtable = false;
    let mut changed = false;

    for line in content.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            if t == "[[plugins]]" {
                idx = Some(idx.map_or(0, |i| i + 1));
                in_subtable = false;
            } else if t.starts_with("[plugins.") {
                // Sub-table of the entry we are in: body, not a server.
                in_subtable = true;
            } else {
                idx = None;
                in_subtable = false;
            }
            out.push(line.to_string());
            continue;
        }
        let (Some(i), false) = (idx, in_subtable) else {
            out.push(line.to_string());
            continue;
        };
        let Some(entry) = plugins.get(i).and_then(|v| v.as_table()) else {
            out.push(line.to_string());
            continue;
        };
        let name = entry
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        if let Some(want) = only {
            if name != want {
                out.push(line.to_string());
                continue;
            }
        }
        let key = line.split('=').next().unwrap_or("").trim();
        let has_original = entry.contains_key(ORIGINAL_KEY);

        if wrap {
            if name == "tokenade" {
                if key == "command" && !report.skipped_self.contains(&name) {
                    report.skipped_self.push(name.clone());
                }
                out.push(line.to_string());
                continue;
            }
            let kind = classify_command(
                entry.get("command").and_then(|c| c.as_str()),
                entry
                    .get("args")
                    .and_then(|a| a.as_array())
                    .and_then(|a| a.first())
                    .and_then(|v| v.as_str()),
            );
            if kind == EntryCommand::HostApp {
                if key == "command" && !report.skipped_host_app.contains(&name) {
                    report.skipped_host_app.push(name.clone());
                }
                out.push(line.to_string());
                continue;
            }
            if kind == EntryCommand::OurOwn {
                if key == "command" && !report.skipped_self.contains(&name) {
                    report.skipped_self.push(name.clone());
                }
                out.push(line.to_string());
                continue;
            }
            if has_original || kind == EntryCommand::RoutedByHand {
                if key == "command" && !report.skipped_already_wrapped.contains(&name) {
                    report.skipped_already_wrapped.push(name.clone());
                }
                out.push(line.to_string());
                continue;
            }
            let Some(orig_bin) = entry.get("command").and_then(|c| c.as_str()) else {
                if key == "command" && !report.skipped_unrecognised_shape.contains(&name) {
                    report.skipped_unrecognised_shape.push(name.clone());
                }
                out.push(line.to_string());
                continue;
            };
            let orig_args: Vec<String> = entry
                .get("args")
                .and_then(|a| a.as_array())
                .map(|a| {
                    a.iter()
                        .map(|v| v.as_str().unwrap_or_default().to_string())
                        .collect()
                })
                .unwrap_or_default();
            match key {
                "command" => {
                    let mut new_args = vec![
                        WRAPPER_SUBCOMMAND.to_string(),
                        PROXY_ARGV_SEPARATOR.to_string(),
                        orig_bin.to_string(),
                    ];
                    new_args.extend(orig_args.iter().cloned());
                    out.push(format!("command = {}", toml_str(&wrapper)));
                    out.push(format!("args = {}", toml_array(&new_args)));
                    out.push(format!(
                        "{ORIGINAL_KEY} = {{ command = {}, args = {} }}",
                        toml_str(orig_bin),
                        toml_array(&orig_args)
                    ));
                    changed = true;
                    report.wrapped.push(name.clone());
                }
                "args" => {}
                _ => out.push(line.to_string()),
            }
            continue;
        }

        let Some(original) = entry.get(ORIGINAL_KEY).and_then(|v| v.as_table()) else {
            out.push(line.to_string());
            continue;
        };
        let Some(orig_bin) = original.get("command").and_then(|c| c.as_str()) else {
            out.push(line.to_string());
            continue;
        };
        let orig_args: Vec<String> = original
            .get("args")
            .and_then(|a| a.as_array())
            .map(|a| {
                a.iter()
                    .map(|v| v.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        // Same rule as the JSON path, and the same declarant behind it: what
        // the entry launches TODAY beats the snapshot taken when it was
        // wrapped. Both TOML shapes go through here, so neither can drift.
        let (orig_bin, orig_args) = toml_live_or_snapshot(entry, orig_bin, orig_args);
        let orig_bin = orig_bin.as_str();
        match key {
            "command" => {
                out.push(format!("command = {}", toml_str(orig_bin)));
                out.push(format!("args = {}", toml_array(&orig_args)));
                changed = true;
                report.unwrapped.push(name.clone());
            }
            "args" => {}
            k if k == ORIGINAL_KEY => {}
            _ => out.push(line.to_string()),
        }
    }

    if !changed {
        return Ok(None);
    }
    let mut result = format!("{bom}{}", out.join("\n"));
    if content.ends_with('\n') {
        result.push('\n');
    }
    Ok(Some(result))
}
