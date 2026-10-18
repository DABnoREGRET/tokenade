//! **Do this agent's hooks run on this machine now?**
//!
//! Design rule: know, per agent, whether hooks need validating,
//! and behave differently whether they are or not — validated, the hooks carry
//! the features and the proxy does not redo them; not validated, the proxy
//! carries them. Every check below reads the agent's OWN state, where the
//! agent reads it (audited 2026-10-06 in each bundle). Folder trust (Claude Code, Gemini) is
//! per directory and is reported by `health`, never granted on the user's
//! behalf; it is not a machine-wide answer and is not read here.

use std::path::Path;
use tokenade_core::detect::Agent;

fn json(p: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
}

fn is_true(v: Option<&serde_json::Value>) -> bool {
    v.and_then(|b| b.as_bool()) == Some(true)
}

/// The answer for `agent`, under `home`.
pub fn hooks_effective(agent: Agent, home: &Path) -> bool {
    match agent {
        // Per-hook validation: a `trusted_hash` in `~/.codex/config.toml`
        // for every hook; an entry without one does not run.
        Agent::Codex => crate::codex_trust::verify_trust(&home.join(".codex"))
            .is_ok_and(|s| !s.is_empty() && s.iter().all(|(_, ok)| *ok)),
        // Global switches only (user hooks need no validation).
        Agent::ClaudeCode => !is_true(
            json(&home.join(".claude").join("settings.json"))
                .as_ref()
                .and_then(|v| v.get("disableAllHooks")),
        ),
        // Qwen also needs `hooks.enabled: true` — MEASURED on 0.25.0
        // (2026-10-06): without it the tokenade hook never fires.
        Agent::Qwen => json(&home.join(".qwen").join("settings.json")).is_some_and(|v| {
            !is_true(v.get("disableAllHooks")) && is_true(v.pointer("/hooks/enabled"))
        }),
        Agent::Copilot => !is_true(
            json(&home.join(".copilot").join("config.json"))
                .as_ref()
                .and_then(|v| v.get("disableAllHooks")),
        ),
        Agent::Droid => !is_true(
            json(&home.join(".factory").join("settings.json"))
                .as_ref()
                .and_then(|v| v.pointer("/hooks/hooksDisabled")),
        ),
        // `~/.grok/disabled-hooks` lists hooks switched off by name.
        Agent::Grok => !std::fs::read_to_string(home.join(".grok").join("disabled-hooks"))
            .is_ok_and(|t| t.contains("tokenade")),
        // Plugin allow-lists: the agent's own enable switch.
        Agent::OpenClaw => is_true(
            json(&home.join(".openclaw").join("openclaw.json"))
                .as_ref()
                .and_then(|v| v.pointer("/plugins/entries/tokenade/enabled")),
        ),
        Agent::Hermes => std::fs::read_to_string(home.join(".hermes").join("config.yaml"))
            .is_ok_and(|t| hermes_enables_tokenade(&t)),
        // No validation step found in their bundles: installed means running.
        _ => true,
    }
}

/// `plugins: { enabled: [ …, tokenade, … ] }` in Hermes' YAML, either form.
fn hermes_enables_tokenade(yaml: &str) -> bool {
    let mut in_plugins = false;
    let mut in_enabled = false;
    for line in yaml.lines() {
        let indent = line.len() - line.trim_start().len();
        let t = line.trim();
        if indent == 0 {
            in_plugins = t.starts_with("plugins:");
            in_enabled = false;
            continue;
        }
        if !in_plugins {
            continue;
        }
        if let Some(rest) = t.strip_prefix("enabled:") {
            if rest.contains("tokenade") {
                return true;
            }
            in_enabled = true;
            continue;
        }
        if in_enabled
            && t.starts_with('-')
            && t.trim_start_matches('-')
                .trim()
                .trim_matches(|c| c == '"' || c == '\'')
                == "tokenade"
        {
            return true;
        }
        if in_enabled && !t.starts_with('-') {
            in_enabled = false;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home(tag: &str) -> std::path::PathBuf {
        let h = std::env::temp_dir().join(format!("tk-hookstate-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&h);
        std::fs::create_dir_all(&h).unwrap();
        h
    }

    fn put(h: &Path, rel: &str, body: &str) {
        let p = h.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    /// Each agent's own switch flips the answer; absent, hooks run.
    #[test]
    fn each_agent_is_read_where_it_keeps_its_switch() {
        let cases: &[(Agent, &str, &str)] = &[
            (
                Agent::ClaudeCode,
                ".claude/settings.json",
                r#"{"disableAllHooks":true}"#,
            ),
            (
                Agent::Qwen,
                ".qwen/settings.json",
                r#"{"hooks":{"enabled":true},"disableAllHooks":true}"#,
            ),
            (
                Agent::Copilot,
                ".copilot/config.json",
                r#"{"disableAllHooks":true}"#,
            ),
            (
                Agent::Droid,
                ".factory/settings.json",
                r#"{"hooks":{"hooksDisabled":true}}"#,
            ),
            (Agent::Grok, ".grok/disabled-hooks", "tokenade-pretool\n"),
        ];
        for (a, rel, off) in cases {
            let h = home(a.as_str());
            if *a == Agent::Qwen {
                assert!(!hooks_effective(*a, &h), "qwen without `hooks.enabled`");
                put(&h, rel, r#"{"hooks":{"enabled":true}}"#);
            }
            assert!(hooks_effective(*a, &h), "{} with nothing set", a.as_str());
            put(&h, rel, off);
            assert!(!hooks_effective(*a, &h), "{} switched off", a.as_str());
            let _ = std::fs::remove_dir_all(&h);
        }
    }

    /// Codex: no trust recorded, no hooks running.
    #[test]
    fn codex_without_a_trust_record_does_not_run_its_hooks() {
        let h = home("codex");
        put(&h, ".codex/hooks.json", r#"{"hooks":{}}"#);
        assert!(!hooks_effective(Agent::Codex, &h));
        let _ = std::fs::remove_dir_all(&h);
    }

    /// Plugin allow-lists: enabled or not.
    #[test]
    fn a_plugin_runs_only_where_its_host_enables_it() {
        let h = home("plugins");
        assert!(!hooks_effective(Agent::OpenClaw, &h));
        put(
            &h,
            ".openclaw/openclaw.json",
            r#"{"plugins":{"entries":{"tokenade":{"enabled":true}}}}"#,
        );
        assert!(hooks_effective(Agent::OpenClaw, &h));
        put(
            &h,
            ".hermes/config.yaml",
            "model: x\nplugins:\n  enabled:\n    - other\n    - tokenade\n",
        );
        assert!(hooks_effective(Agent::Hermes, &h));
        put(&h, ".hermes/config.yaml", "plugins:\n  enabled: [other]\n");
        assert!(!hooks_effective(Agent::Hermes, &h));
        let _ = std::fs::remove_dir_all(&h);
    }
}
