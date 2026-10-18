//! Establish Codex hook *trust* so the hooks tokenade installs actually run.
//!
//! Codex (unlike Claude Code) refuses to execute a non-managed lifecycle hook
//! until the user has reviewed and *trusted* it via the `/hooks` TUI — it
//! records trust as a SHA-256 of the hook's normalized identity in
//! `~/.codex/config.toml` under `[hooks.state."<key>"] trusted_hash = "…"`.
//! A hook we write to `hooks.json` but never trust is INERT: no PreToolUse
//! command rewrite, no compaction, zero savings (observed in the field: a
//! heavy Codex user seeing ~0 gain because every tokenade hook was untrusted).
//!
//! There is no official CLI to trust a hook (openai/codex#21615 is open), so
//! we reproduce Codex's own trust hash and write the state directly. The
//! algorithm is verified byte-for-byte against a live `codex exec` run: with
//! the state written below (and NO `--dangerously-bypass-hook-trust`), Codex
//! rewrote `seq 1 777` → `tokenade wrap 'seq 1 777'`, proving the hash matched.
//!
//! Source of truth (codex `main`):
//!   - `config/src/fingerprint.rs::version_for_toml` — canonical-JSON SHA-256.
//!   - `hooks/src/engine/discovery.rs::command_hook_hash` — the hashed identity
//!     is `NormalizedHookIdentity { event_name, #[flatten] MatcherGroup }` with
//!     the handler normalized to `command_windows = None`, `timeout = 600`
//!     (default), `async = false`. `None` fields vanish because the identity is
//!     routed through `toml::Value` first (TOML has no null).
//!   - `hooks/src/lib.rs::hook_key` — state key is
//!     `"{source_path}:{event_label}:{group_index}:{handler_index}"`.
//!
//! We only ever trust hooks WE wrote (tagged `_tag: "tokenade-*"`); the user's
//! other hooks are never touched.

use sha2::{Digest, Sha256};
use std::path::Path;

/// Map a hooks.json event key (`"PostToolUse"`) to codex's snake_case label
/// (`"post_tool_use"`) used in both the hashed identity and the state key.
fn event_label(event: &str) -> Option<&'static str> {
    Some(match event {
        "PreToolUse" => "pre_tool_use",
        "PermissionRequest" => "permission_request",
        "PostToolUse" => "post_tool_use",
        "PreCompact" => "pre_compact",
        "PostCompact" => "post_compact",
        "SessionStart" => "session_start",
        "UserPromptSubmit" => "user_prompt_submit",
        "SubagentStart" => "subagent_start",
        "SubagentStop" => "subagent_stop",
        "Stop" => "stop",
        _ => return None,
    })
}

/// Recursively sort object keys — mirrors codex `fingerprint::canonical_json`
/// so `serde_json::to_vec` yields the exact bytes codex hashes.
fn canonicalize(v: &serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let mut out = serde_json::Map::with_capacity(keys.len());
            for k in keys {
                out.insert(k.clone(), canonicalize(&m[k]));
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(a) => {
            serde_json::Value::Array(a.iter().map(canonicalize).collect())
        }
        other => other.clone(),
    }
}

/// Compute the `sha256:<hex>` trust hash codex expects for one command hook.
///
/// `command` is the platform command string exactly as written to hooks.json
/// (on Windows that is the bare-exe form; codex resolves `command_windows` at
/// discovery and then re-normalizes it to `None`, so the caller passes whatever
/// landed in the `command` field on THIS platform). None-valued fields are
/// omitted to match codex routing the identity through `toml::Value` first.
pub fn trusted_hash(
    event_label: &str,
    matcher: Option<&str>,
    command: &str,
    status_message: Option<&str>,
) -> String {
    let mut handler = serde_json::Map::new();
    handler.insert("type".into(), "command".into());
    handler.insert("command".into(), command.into());
    handler.insert("timeout".into(), serde_json::Value::from(600));
    handler.insert("async".into(), serde_json::Value::from(false));
    if let Some(s) = status_message {
        handler.insert("statusMessage".into(), s.into());
    }
    // command_windows is None post-normalization → omitted (toml has no null).

    let mut identity = serde_json::Map::new();
    identity.insert("event_name".into(), event_label.into());
    identity.insert(
        "hooks".into(),
        serde_json::Value::Array(vec![serde_json::Value::Object(handler)]),
    );
    if let Some(m) = matcher {
        identity.insert("matcher".into(), m.into());
    }

    let canonical = canonicalize(&serde_json::Value::Object(identity));
    let bytes = serde_json::to_vec(&canonical).unwrap_or_default();
    let digest = Sha256::digest(&bytes);
    let mut hex = String::with_capacity(7 + 64);
    hex.push_str("sha256:");
    for b in digest {
        hex.push_str(&format!("{b:02x}"));
    }
    hex
}

/// A tokenade hook discovered in hooks.json, with its computed state key + hash.
struct Discovered {
    key: String,
    hash: String,
}

/// True iff this handler is one WE installed (`_tag: "tokenade-*"`). We never
/// trust hooks we didn't write.
fn is_tokenade_hook(handler: &serde_json::Value) -> bool {
    handler
        .get("_tag")
        .and_then(|t| t.as_str())
        .is_some_and(|t| t.starts_with("tokenade-"))
}

/// Walk the just-written hooks.json exactly as codex's discovery does
/// (per-event group index, per-group handler index) and compute the
/// `(state_key, trusted_hash)` for every tokenade command hook.
fn discover(hooks_json_path: &Path) -> std::io::Result<Vec<Discovered>> {
    let raw = std::fs::read_to_string(hooks_json_path)?;
    let doc: serde_json::Value = serde_json::from_str(&raw).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, format!("hooks.json: {e}"))
    })?;
    // codex: key_source = source_path.display()
    let key_source = hooks_json_path.display().to_string();

    let mut out = Vec::new();
    let Some(events) = doc.get("hooks").and_then(|h| h.as_object()) else {
        return Ok(out);
    };
    for (event, groups) in events {
        let Some(label) = event_label(event) else {
            continue;
        };
        let Some(groups) = groups.as_array() else {
            continue;
        };
        for (gi, group) in groups.iter().enumerate() {
            let matcher = group.get("matcher").and_then(|m| m.as_str());
            let Some(handlers) = group.get("hooks").and_then(|h| h.as_array()) else {
                continue;
            };
            for (hi, handler) in handlers.iter().enumerate() {
                if handler.get("type").and_then(|t| t.as_str()) != Some("command") {
                    continue;
                }
                if !is_tokenade_hook(handler) {
                    continue;
                }
                let Some(command) = handler.get("command").and_then(|c| c.as_str()) else {
                    continue;
                };
                let status = handler.get("statusMessage").and_then(|s| s.as_str());
                out.push(Discovered {
                    key: format!("{key_source}:{label}:{gi}:{hi}"),
                    hash: trusted_hash(label, matcher, command, status),
                });
            }
        }
    }
    Ok(out)
}

/// Drop every trust entry we wrote — the inverse of [`ensure_trust`].
///
/// Matches by KEY PREFIX (`<codex_home>/hooks.json:`) rather than by
/// re-discovering the hooks: by the time uninstall runs, `hooks.json` has
/// already been emptied, so `discover` returns nothing and a discovery-based
/// revoke would be a silent no-op.
///
/// MEASURED 2026-08-07 on a fixture home: `install` then `uninstall` left
/// `config.toml` carrying five `trusted_hash` entries pointing at hooks that
/// no longer existed — the only file in the whole tree still naming tokenade
/// after the user asked it to leave. Every other agent config came back at
/// `{"hooks":{}}` with no mention of us.
///
/// Removes the now-empty `state` / `hooks` tables too, so an uninstall on a
/// config we never otherwise touched leaves no trace at all. Returns the
/// number of entries dropped.
pub fn revoke_trust(codex_home: &Path) -> std::io::Result<usize> {
    let cfg_path = codex_home.join("config.toml");
    let Ok(cfg_raw) = std::fs::read_to_string(&cfg_path) else {
        return Ok(0);
    };
    let mut doc: toml_edit::DocumentMut = cfg_raw.parse().map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, format!("config.toml: {e}"))
    })?;
    let prefix = format!("{}:", codex_home.join("hooks.json").display());

    let Some(state) = doc
        .get_mut("hooks")
        .and_then(|h| h.get_mut("state"))
        .and_then(|s| s.as_table_like_mut())
    else {
        return Ok(0);
    };
    let doomed: Vec<String> = state
        .iter()
        .map(|(k, _)| k.to_string())
        .filter(|k| k.starts_with(&prefix))
        .collect();
    if doomed.is_empty() {
        return Ok(0);
    }
    for k in &doomed {
        state.remove(k);
    }
    let state_empty = state.is_empty();
    if state_empty {
        if let Some(hooks) = doc.get_mut("hooks").and_then(|h| h.as_table_like_mut()) {
            hooks.remove("state");
            if hooks.is_empty() {
                doc.as_table_mut().remove("hooks");
            }
        }
    }
    crate::backup_file(&cfg_path);
    crate::write_text_atomic(&cfg_path, &doc.to_string())?;
    Ok(doomed.len())
}

/// Trust every tokenade hook in `<codex_home>/hooks.json` by writing its
/// `[hooks.state."<key>"] enabled = true, trusted_hash = "…"` into
/// `<codex_home>/config.toml`. Format-preserving (toml_edit): existing config,
/// comments, and unrelated tables are left intact. Idempotent — a command-path
/// change just rewrites the same key with the new hash. Returns the number of
/// hooks trusted.
pub fn ensure_trust(codex_home: &Path) -> std::io::Result<usize> {
    let hooks_json = codex_home.join("hooks.json");
    if !hooks_json.exists() {
        return Ok(0);
    }
    let entries = discover(&hooks_json)?;
    if entries.is_empty() {
        return Ok(0);
    }

    let cfg_path = codex_home.join("config.toml");
    let cfg_raw = std::fs::read_to_string(&cfg_path).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = cfg_raw.parse().map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, format!("config.toml: {e}"))
    })?;

    for d in &entries {
        // `doc["hooks"]["state"][key]` auto-vivifies hooks + state as tables;
        // `key` (containing `/`, `.`, `:`) is one literal key — toml_edit
        // quotes it on render → `[hooks.state."<path>:event:0:0"]`.
        let slot = &mut doc["hooks"]["state"][d.key.as_str()];
        slot["enabled"] = toml_edit::value(true);
        slot["trusted_hash"] = toml_edit::value(d.hash.as_str());
    }

    // Backup + atomic: this rewrites the user's WHOLE config.toml (model,
    // providers, MCP servers…). A truncating write interrupted here leaves
    // Codex with no config — collateral damage from us merely trusting a hook.
    crate::backup_file(&cfg_path);
    crate::write_text_atomic(&cfg_path, &doc.to_string())?;
    Ok(entries.len())
}

/// Trust tokenade's Codex hooks at the DEFAULT codex home (`~/.codex`),
/// resolved the same way the installer writes hooks.json — from `home_dir()`,
/// as an ABSOLUTE path. This matters: codex derives its own `key_source` from
/// an absolute config-dir path, so the state key we persist must be absolute
/// too (a relative `.codex/hooks.json:…` key never matches → hook stays
/// untrusted). Callers on the install path use this rather than threading a
/// Where [`ensure_trust_default`] would write. Exposed so `--dry-run` can
/// NAME the file instead of leaving the user to discover it afterwards.
pub fn config_path_for_default() -> Option<std::path::PathBuf> {
    tokenade_core::platform::home_dir().map(|h| h.join(".codex").join("config.toml"))
}

/// possibly-relative home through.
pub fn ensure_trust_default() -> std::io::Result<usize> {
    match tokenade_core::platform::home_dir() {
        Some(h) => ensure_trust(&h.join(".codex")),
        None => Ok(0),
    }
}

/// For diagnostics (`tokenade doctor`): report each tokenade hook and whether
/// the trust state currently persisted in config.toml matches the hook we'd
/// compute now (`true` = trusted+current, `false` = untrusted/modified).
pub fn verify_trust(codex_home: &Path) -> std::io::Result<Vec<(String, bool)>> {
    let hooks_json = codex_home.join("hooks.json");
    if !hooks_json.exists() {
        return Ok(Vec::new());
    }
    let entries = discover(&hooks_json)?;
    let cfg_raw = std::fs::read_to_string(codex_home.join("config.toml")).unwrap_or_default();
    let doc: toml_edit::DocumentMut = cfg_raw.parse().unwrap_or_default();
    Ok(entries
        .into_iter()
        .map(|d| {
            let ok = doc
                .get("hooks")
                .and_then(|h| h.get("state"))
                .and_then(|s| s.get(d.key.as_str()))
                .and_then(|e| e.get("trusted_hash"))
                .and_then(|v| v.as_str())
                == Some(d.hash.as_str());
            (d.key, ok)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Locked against the codex algorithm (verified live: a config.toml written
    // with these hashes made codex run the hook without --bypass-hook-trust).
    #[test]
    fn trusted_hash_matches_codex_reference_vector() {
        let h = trusted_hash(
            "post_tool_use",
            Some("Bash"),
            "tokenade hookwrap",
            Some("tokenade compaction"),
        );
        assert_eq!(
            h,
            "sha256:6af321dd700a967773a938ee0728ff6085cecabecf59dee65e7ee5c638d45bd3"
        );
    }

    #[test]
    fn none_fields_are_omitted_not_null() {
        // No matcher, no statusMessage → those keys must be absent (toml drops
        // None), which changes the hash vs. the full-field vector.
        let a = trusted_hash(
            "session_start",
            None,
            "tokenade hookwrap-sessionstart",
            None,
        );
        let b = trusted_hash(
            "session_start",
            Some("*"),
            "tokenade hookwrap-sessionstart",
            None,
        );
        assert!(a.starts_with("sha256:") && a.len() == 71);
        assert_ne!(a, b);
    }

    #[test]
    fn ensure_trust_writes_state_and_is_idempotent() {
        let dir = std::env::temp_dir().join(format!("tok-codex-trust-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let hooks = r#"{"hooks":{"PostToolUse":[{"matcher":"Bash","hooks":[{"_tag":"tokenade-posttool","type":"command","command":"tokenade hookwrap","statusMessage":"tokenade compaction"}]}]}}"#;
        std::fs::write(dir.join("hooks.json"), hooks).unwrap();
        std::fs::write(dir.join("config.toml"), "model = \"gpt-5.5\"\n").unwrap();

        let n = ensure_trust(&dir).unwrap();
        assert_eq!(n, 1);
        let cfg = std::fs::read_to_string(dir.join("config.toml")).unwrap();
        assert!(cfg.contains("model = \"gpt-5.5\"")); // preserved
        assert!(cfg.contains("6af321dd700a967773a938ee0728ff6085cecabecf59dee65e7ee5c638d45bd3"));
        let verified = verify_trust(&dir).unwrap();
        assert_eq!(verified.len(), 1);
        assert!(verified[0].1, "hook should read back as trusted");

        // Idempotent: second run doesn't error or duplicate.
        assert_eq!(ensure_trust(&dir).unwrap(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
