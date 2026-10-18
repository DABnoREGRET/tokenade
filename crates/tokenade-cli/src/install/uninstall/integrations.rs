//! **Remove our entries from every agent.**
//!
//! Recognising what is OURS is the whole subject: an entry left behind breaks
//! the agent (the wrapped command no longer exists), one entry too many breaks
//! a third party's configuration. Recognition is based on the command, not on
//! an entry name the user may have renamed.

use super::*;

/// Symmetric uninstall for every NON-Claude agent integration the installer
/// writes. Before this existed, `tokenade uninstall` cleaned Claude Code +
/// shell proxy + MCP entries but left ORPHANS everywhere else: hook entries
/// in cursor/codex/grok/gemini/qwen configs (each spawning a missing binary
/// after package removal) and plugin files for opencode/pi/kilo/openclaw/
/// hermes. Returns human-readable lines describing what was (or would be)
/// removed; empty = nothing found.
pub(crate) fn uninstall_agent_integrations(home: &Path, dry_run: bool) -> Vec<String> {
    let mut out = Vec::new();

    // --- JSON hook configs: drop any entry whose serialized form mentions
    // tokenade. Covers all four shapes (cursor preToolUse groups, codex/grok
    // lifecycle `_tag` groups, gemini/qwen BeforeTool entries) in one pass —
    // an entry that references the tokenade binary is ours by construction;
    // agents never write "tokenade" into their own hook entries.
    let json_targets: [(&str, PathBuf); 8] = [
        ("cursor", crate::install_cli::cursor_hooks_path(home)),
        ("codex", home.join(".codex").join("hooks.json")),
        ("droid", home.join(".factory").join("hooks.json")),
        (
            "grok",
            home.join(".grok").join("hooks").join("tokenade.json"),
        ),
        // grok's hooks moved out of `user-settings.json`, where every
        // install before then had written them. An uninstall that only knows
        // the new address leaves the old, inert copy behind for ever, on
        // exactly the machines that have been running longest.
        ("grok", home.join(".grok").join("user-settings.json")),
        ("gemini-cli", home.join(".gemini").join("settings.json")),
        ("qwen", home.join(".qwen").join("settings.json")),
        // Reasonix: flat `match`/`command` entries — the strip pass is
        // shape-agnostic (it drops any entry whose command mentions hookwrap).
        ("reasonix", crate::install::reasonix_settings_path(home)),
    ];
    for (agent, path) in json_targets {
        match strip_tokenade_entries_from_json_hooks(&path, dry_run) {
            Ok(0) => {}
            Ok(n) => out.push(format!(
                "{agent}: {} {n} tokenade hook entr{} from {}",
                if dry_run { "would remove" } else { "removed" },
                if n == 1 { "y" } else { "ies" },
                path.display()
            )),
            Err(e) => out.push(format!("{agent}: skipped ({e})")),
        }
    }

    // --- Codex trust state. `ensure_trust` writes `hooks.state."<hooks.json
    // path>:<event>:i:j"` entries into `~/.codex/config.toml` so Codex loads
    // our hooks without a `/hooks` prompt. Stripping hooks.json above leaves
    // those entries pointing at nothing — MEASURED on a fixture home, the one
    // file in the whole tree still naming tokenade after an uninstall.
    if !dry_run {
        match crate::codex_trust::revoke_trust(&home.join(".codex")) {
            Ok(0) => {}
            Ok(n) => out.push(format!(
                "codex: removed {n} trust entr{} from {}",
                if n == 1 { "y" } else { "ies" },
                home.join(".codex").join("config.toml").display()
            )),
            Err(e) => out.push(format!("codex trust: skipped ({e})")),
        }
    } else if let Ok(raw) = std::fs::read_to_string(home.join(".codex").join("config.toml")) {
        let n = raw
            .matches(&format!(
                "{}:",
                home.join(".codex").join("hooks.json").display()
            ))
            .count();
        if n > 0 {
            // Same path rendering as the real branch: a preview a user
            // compares against the run must not name the file differently.
            out.push(format!(
                "codex: would remove {n} trust entr{} from {}",
                if n == 1 { "y" } else { "ies" },
                home.join(".codex").join("config.toml").display()
            ));
        }
    }

    // --- Plugin/extension FILES the installer dropped on disk.
    let files: [(&str, PathBuf); 4] = [
        (
            "pi",
            home.join(".pi")
                .join("agent")
                .join("extensions")
                .join("tokenade.ts"),
        ),
        (
            "opencode",
            crate::install_cli::opencode_plugin_dir(home).join("tokenade.ts"),
        ),
        (
            "kilo-code",
            crate::install_cli::kilo_plugin_dir(home).join("tokenade.ts"),
        ),
        (
            "copilot",
            home.join(".copilot").join("hooks").join("tokenade.json"),
        ),
    ];
    for (agent, path) in files {
        if path.is_file() {
            if dry_run {
                out.push(format!("{agent}: would delete {}", path.display()));
            } else if let Err(e) = fs::remove_file(&path) {
                out.push(format!("{agent}: couldn't delete {}: {e}", path.display()));
            } else {
                out.push(format!("{agent}: deleted {}", path.display()));
            }
        }
    }

    // --- OpenClaw: unregister then delete the plugin package dir.
    let openclaw_dir = home.join(".tokenade").join("openclaw-plugin");
    if openclaw_dir.is_dir() {
        if dry_run {
            out.push(format!(
                "openclaw: would run `openclaw plugins uninstall tokenade` and delete {}",
                openclaw_dir.display()
            ));
        } else {
            // Best-effort: openclaw may already be gone; the dir removal is
            // what guarantees the plugin can't load.
            let mut oc = tokenade_core::process_util::internal_command("openclaw");
            oc.args(["plugins", "uninstall", "tokenade"]);
            tokenade_core::process_util::configure_no_window(&mut oc);
            let _ = oc.output();
            match fs::remove_dir_all(&openclaw_dir) {
                Ok(()) => out.push(format!("openclaw: deleted {}", openclaw_dir.display())),
                Err(e) => out.push(format!(
                    "openclaw: couldn't delete {}: {e}",
                    openclaw_dir.display()
                )),
            }
        }
    }

    // --- aider: remove the startup launcher.
    //
    // `backup_once` knows nothing of a file we CREATED: it copies the original
    // when there is one, so for a `.aider.conf.yml` written from scratch there
    // is nothing to restore and the generic restore left it in place. That is
    // the "install writes a file uninstall does not know" class, and it is
    // paid for on the `reversible install` column.
    //
    // The removal is textual and bounded: our two comment lines and the
    // `load:` line naming OUR file. A `load:` pointing elsewhere is the user's
    // and does not move.
    let aider_startup = home.join(".tokenade").join("aider-startup.txt");
    let aider_cfg = home.join(".aider.conf.yml");
    if let Ok(cfg) = fs::read_to_string(&aider_cfg) {
        let ours = aider_startup.display().to_string();
        let kept: Vec<&str> = cfg
            .lines()
            .filter(|l| {
                let t = l.trim();
                !(t.starts_with("# tokenade: run the repo map")
                    || t.starts_with("# line (and the file it names) to undo")
                    || (t.starts_with("load:") && t.contains(&ours)))
            })
            .collect();
        if kept.len() != cfg.lines().count() {
            let rest: String = kept.join("\n");
            if dry_run {
                out.push(format!(
                    "aider: would drop the tokenade startup command from {}",
                    aider_cfg.display()
                ));
            } else if rest.trim().is_empty() {
                // The file did not exist before us: leaving it empty would
                // leave a trace the user never wrote.
                match fs::remove_file(&aider_cfg) {
                    Ok(()) => out.push(format!("aider: deleted {}", aider_cfg.display())),
                    Err(e) => out.push(format!(
                        "aider: couldn't delete {}: {e}",
                        aider_cfg.display()
                    )),
                }
            } else {
                let mut updated = rest;
                if cfg.ends_with('\n') {
                    updated.push('\n');
                }
                match tokenade_core::paths::write_atomic(&aider_cfg, &updated) {
                    Ok(()) => out.push(format!(
                        "aider: dropped the tokenade startup command from {}",
                        aider_cfg.display()
                    )),
                    Err(e) => out.push(format!(
                        "aider: couldn't rewrite {}: {e}",
                        aider_cfg.display()
                    )),
                }
            }
        }
    }
    if aider_startup.is_file() {
        if dry_run {
            out.push(format!("aider: would delete {}", aider_startup.display()));
        } else {
            match fs::remove_file(&aider_startup) {
                Ok(()) => out.push(format!("aider: deleted {}", aider_startup.display())),
                Err(e) => out.push(format!(
                    "aider: couldn't delete {}: {e}",
                    aider_startup.display()
                )),
            }
        }
    }

    // --- Hermes: delete the plugin dir AND drop `- tokenade` from
    // `plugins.enabled` in config.yaml (Hermes errors on enabled-but-missing
    // plugins, so the yaml edit is not optional).
    let hh = crate::install_cli::hermes_home_dir(home);
    let hermes_plugin = hh.join("plugins").join("tokenade");
    if hermes_plugin.is_dir() {
        if dry_run {
            out.push(format!("hermes: would delete {}", hermes_plugin.display()));
        } else {
            match fs::remove_dir_all(&hermes_plugin) {
                Ok(()) => out.push(format!("hermes: deleted {}", hermes_plugin.display())),
                Err(e) => out.push(format!(
                    "hermes: couldn't delete {}: {e}",
                    hermes_plugin.display()
                )),
            }
        }
    }
    let hermes_cfg = hh.join("config.yaml");
    if let Ok(cfg) = fs::read_to_string(&hermes_cfg) {
        let filtered: Vec<String> = cfg
            .lines()
            .filter(|l| l.trim() != "- tokenade")
            .map(|l| {
                // Inline-array form: `enabled: [foo, tokenade, bar]`.
                if l.contains("enabled:") && l.contains('[') && l.contains("tokenade") {
                    l.replace(", tokenade", "")
                        .replace("tokenade, ", "")
                        .replace("[tokenade]", "[]")
                } else {
                    l.to_string()
                }
            })
            .collect();
        let changed = filtered.len() != cfg.lines().count()
            || filtered.iter().zip(cfg.lines()).any(|(a, b)| a != b);
        if changed {
            if dry_run {
                out.push(format!(
                    "hermes: would drop `- tokenade` from {}",
                    hermes_cfg.display()
                ));
            } else {
                let mut updated = filtered.join("\n");
                if cfg.ends_with('\n') {
                    updated.push('\n');
                }
                match crate::write_text_atomic(&hermes_cfg, updated.as_ref()) {
                    Ok(()) => out.push(format!(
                        "hermes: dropped `- tokenade` from {}",
                        hermes_cfg.display()
                    )),
                    Err(e) => out.push(format!(
                        "hermes: couldn't rewrite {}: {e}",
                        hermes_cfg.display()
                    )),
                }
            }
        }
    }

    out
}

/// Remove every element mentioning "tokenade" from every array under the
/// top-level `hooks` object of a JSON config. Returns how many elements were
/// dropped. Leaves the file untouched (and errors) when it isn't valid JSON —
/// never risk destroying a user's hand-edited config on uninstall.
/// Is this hook entry one tokenade wrote?
///
/// Two tests, and neither is a list that has to be kept up to date by hand.
/// The enumerated version named `tokenade-wrap`, `tokenade-fold` and
/// `tokenade-web-compact`, and the day two events were added (`BeforeAgent`,
/// `SessionStart`, tags `tokenade-style` and `tokenade-sessionstart`) the
/// uninstall walked straight past them — caught by the compat bench as
/// `reversible` falling from twenty-four green rows to twenty-two, on the two
/// agents that had just gained the feature.
///
/// So: any quoted tag under our namespace, or a command that RUNS one of our
/// hook subcommands.
pub(crate) fn entry_is_ours(entry: &serde_json::Value) -> bool {
    if entry.to_string().contains("\"tokenade-") {
        return true;
    }
    let field = |v: &serde_json::Value, k: &str| -> Option<String> {
        v.get(k).and_then(|x| x.as_str()).map(|s| s.to_string())
    };
    let mut cmds: Vec<String> = Vec::new();
    cmds.extend(field(entry, "command"));
    cmds.extend(field(entry, "bash"));
    if let Some(hs) = entry.get("hooks").and_then(|h| h.as_array()) {
        for h in hs {
            cmds.extend(field(h, "command"));
            cmds.extend(field(h, "bash"));
        }
    }
    cmds.iter().any(|c| command_is_ours(c))
}

/// The hook subcommands an installed entry can name, besides the `hookwrap*`
/// family matched by prefix.
///
/// Module-level, and not a `const` inside the function, so the test that keeps
/// it aligned with the dispatcher can TRAVERSE it instead of restating it by
/// hand. It carried `cursor-pretool` until 2026-09-13 — a name no installer
/// has ever written and the binary has never answered: what `install_cursor_hooks`
/// writes is `hookwrap-pretool`, caught by the prefix.
pub(crate) const HOOK_SUBCOMMANDS: &[&str] = &[
    "wrap",
    "gemini-hook",
    "gemini-after-tool",
    "gemini-afterhook",
    "gemini-lifecycle",
];

/// Does this command line RUN a tokenade hook subcommand?
///
/// Token-wise, not by substring, and that distinction is the whole point: a
/// user hook at `/home/u/gemini-hooks/check.sh` contains `gemini-hook` and is
/// not ours. The guard test that names that exact path caught a substring
/// version of this function deleting it.
pub(crate) fn command_is_ours(cmd: &str) -> bool {
    let trim = |t: &str| {
        t.trim_matches(|c| c == '"' || c == '\'' || c == '`')
            .to_string()
    };
    let toks: Vec<String> = cmd.split_whitespace().map(trim).collect();
    for (i, t) in toks.iter().enumerate() {
        let prog = t.rsplit(['/', '\\']).next().unwrap_or(t);
        if prog != "tokenade" && prog != "tokenade.exe" {
            continue;
        }
        let Some(sub) = toks.get(i + 1) else { continue };
        if sub.starts_with("hookwrap") || HOOK_SUBCOMMANDS.contains(&sub.as_str()) {
            return true;
        }
    }
    false
}

pub(crate) fn strip_tokenade_entries_from_json_hooks(
    path: &Path,
    dry_run: bool,
) -> Result<usize, String> {
    let Ok(content) = fs::read_to_string(path) else {
        return Ok(0); // absent = nothing to do
    };
    let mut doc: serde_json::Value =
        serde_json::from_str(&content).map_err(|e| format!("unparsable JSON, left as-is: {e}"))?;
    let Some(hooks) = doc.get_mut("hooks").and_then(|h| h.as_object_mut()) else {
        return Ok(0);
    };
    let mut removed = 0usize;
    let mut empty_events = Vec::new();
    for (event, arr_val) in hooks.iter_mut() {
        let Some(arr) = arr_val.as_array_mut() else {
            continue;
        };
        let before = arr.len();
        // Match OUR entries precisely: every tokenade hook command invokes a
        // `hookwrap*` subcommand (or `tokenade wrap`). A bare "tokenade"
        // substring also deleted user-authored hooks whose path merely
        // contained the word (e.g. ~/tokenade-notes/check.sh).
        arr.retain(|entry| !entry_is_ours(entry));
        removed += before - arr.len();
        if arr.is_empty() && before > 0 {
            empty_events.push(event.clone());
        }
    }
    for ev in empty_events {
        hooks.remove(&ev);
    }
    // Gemini/Qwen: install sets `hooks.enabled = true`. If stripping left
    // NOTHING but that flag, remove it too — an orphan `enabled:true` with
    // no hooks is confusing config litter.
    if removed > 0
        && hooks.len() == 1
        && hooks.get("enabled").and_then(|v| v.as_bool()) == Some(true)
    {
        hooks.remove("enabled");
        removed += 0; // count unchanged: this is cleanup, not an entry
    }
    if removed > 0 && !dry_run {
        let pretty =
            serde_json::to_string_pretty(&doc).map_err(|e| format!("re-serialize: {e}"))?;
        // Through the shared writer, not `fs::write`. This is the LAST hand to
        // touch a Gemini-family settings file on the way out, and it was the
        // fourth writer found for the same file in one afternoon — the three
        // others already carried the "give the user their own bytes back" rule
        // and this one silently undid it, leaving a reformatted config behind
        // an uninstall that had changed nothing.
        tokenade_core::paths::write_atomic(path, &(pretty + "\n"))
            .map_err(|e| format!("write: {e}"))?;
    }
    Ok(removed)
}
