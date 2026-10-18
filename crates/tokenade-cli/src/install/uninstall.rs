#![allow(unused_imports)]
//! Mechanical module split of install.rs (no behaviour change). Wired
//! back into the `install` namespace by mod.rs via `pub(crate) use <name>::*;`.
use super::*;
use crate::hooks::*;
use crate::wrap::*;
use crate::*;
use std::fs;
use std::path::{Path, PathBuf};

/// #612 — counterpart of the four `install_claude_code_*` hook
/// installers. Walks every event array under `settings["hooks"]` (in
/// `~/.claude/settings.json` or `<root>/.claude/settings.json`) and
/// drops every group whose inner `hooks` array contains an entry
/// tagged `_tag: "tokenade*"` — covering today's tags:
///   - `tokenade`              (PostToolUse Bash)
///   - `tokenade-pretool`      (PreToolUse Bash/Read/Grep/WebFetch)
///   - `tokenade-index`        (PostToolUse auto-index)
///   - `tokenade-postread`     (PostToolUse Read — staged-compaction delivery)
///   - `tokenade-lsp`          (legacy)
///   - `tokenade-sessionstart` / `tokenade-precompact` / `tokenade-prompt`
///   - `tokenade-pre-edit`      (PreToolUse Edit/Write/MultiEdit snapshot)
///
/// Without this step, after `tokenade uninstall && brew uninstall
/// tokenade` every Bash/Read/Grep tool call would fire a hook that
/// spawns a missing binary, breaking the agent. Symmetric to the
/// `_tag`-based detection that #604 / #605 use.
pub(crate) fn strip_tokenade_hooks_from_settings(
    home: Option<&Path>,
    dry_run: bool,
) -> StripHooksOutcome {
    let Some(home) = home else {
        return StripHooksOutcome::NoHome;
    };
    strip_tokenade_hooks_from_settings_at(
        &tokenade_core::platform::claude_config_dir_for(home).join("settings.json"),
        dry_run,
    )
}

// Same strip against an explicit settings.json path — used by uninstall to
// also clean PER-PROJECT installs (`--per-project` writes
// `<root>/.claude/settings.json`, which the home-only pass left orphaned).

/// Qwen twin of the Claude statusLine strip: our entry is removed (or a
/// wrapped original restored) from ~/.qwen/settings.json. Audit: the
/// installer wraps Qwen statuslines but no uninstall path unwound them —
/// dead status bar + the user's own command trapped in base64.
pub(crate) fn strip_qwen_statusline(home: Option<&Path>, dry_run: bool) -> StripHooksOutcome {
    let Some(home) = home
        .map(|h| h.to_path_buf())
        .or_else(tokenade_core::platform::home_dir)
    else {
        return StripHooksOutcome::NoHome;
    };
    strip_tokenade_hooks_from_settings_at(&home.join(".qwen").join("settings.json"), dry_run)
}

/// Remove our one named bundle from Antigravity CLI's `hooks.json`.
///
/// Surgical rather than file-deleting: that file is a map of NAMED bundles and
/// the others are the user's. A left-behind entry is a hook that spawns a
/// binary that is no longer there, on a host whose hooks are synchronous and
/// block the agent loop — so it is not a cosmetic leftover, it is every tool
/// call paying for a failed spawn.
///
/// The file is dropped entirely only when our key was the last one in it, so
/// an uninstall leaves no empty document behind either.
pub(crate) fn strip_antigravity_hook(home: Option<&Path>, dry_run: bool) -> Option<PathBuf> {
    let home = home
        .map(|h| h.to_path_buf())
        .or_else(tokenade_core::platform::home_dir)?;
    let path = home.join(".gemini").join("config").join("hooks.json");
    let raw = fs::read_to_string(&path).ok()?;
    let mut v: serde_json::Value = serde_json::from_str(raw.trim_start_matches('\u{feff}')).ok()?;
    let obj = v.as_object_mut()?;
    obj.remove(crate::install::ANTIGRAVITY_HOOK_NAME)?;
    if dry_run {
        return Some(path);
    }
    if obj.is_empty() {
        let _ = fs::remove_file(&path);
    } else {
        let _ =
            crate::write_json_atomic(&path, &serde_json::to_string_pretty(&v).unwrap_or_default());
    }
    Some(path)
}

pub(crate) fn strip_tokenade_hooks_from_settings_at(
    path: &Path,
    dry_run: bool,
) -> StripHooksOutcome {
    let path = path.to_path_buf();
    let Ok(content) = fs::read_to_string(&path) else {
        return StripHooksOutcome::AbsentOrUnreadable;
    };
    let mut settings: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => return StripHooksOutcome::Malformed(format!("parse settings.json: {e}")),
    };
    // statusLine: ours → REMOVE, restoring a wrapped original command if
    // any (leaving our entry behind = a DEAD status bar once the binary is
    // gone — the lab showed exactly that). Foreign entries untouched.
    let mut sl_removed = false;
    if let Some(cmd) = settings
        .get("statusLine")
        .and_then(|v| v.get("command"))
        .and_then(|c| c.as_str())
        .map(str::to_string)
    {
        // The SAME predicate the installer uses. The basename-only test that
        // stood here missed an auto-updater temp name (`.tokenade.new.<pid>`)
        // and a renamed install — exactly the spellings `is_tokenade_statusline`
        // was hardened for — so uninstall left those users a status bar
        // spawning a binary that was about to be deleted.
        let is_ours = crate::install::is_tokenade_statusline(&cmd);
        if is_ours {
            use base64::Engine as _;
            let restored = cmd
                .split_once("--wrap-b64 ")
                .map(|(_, b64)| b64.trim())
                .and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
                .and_then(|raw| String::from_utf8(raw).ok());
            if !dry_run {
                match restored {
                    Some(orig) if !orig.trim().is_empty() => {
                        if let Some(sl) = settings
                            .get_mut("statusLine")
                            .and_then(|v| v.as_object_mut())
                        {
                            sl.insert("command".into(), serde_json::json!(orig));
                        }
                    }
                    _ => {
                        settings.as_object_mut().map(|o| o.remove("statusLine"));
                    }
                }
            }
            sl_removed = true;
        }
    }
    // Windows carries the hook debug-log path in the settings-global `env`
    // block (see ensure_debug_log_env) — the pre-fix uninstall left it
    // behind (field report 2026-07-11: an orphaned `TOKENADE_DEBUG_LOG`
    // after a full uninstall). Ours to remove; drop a now-empty env map.
    if let Some(obj) = settings.as_object_mut() {
        let mut env_removed = false;
        if let Some(env) = obj.get_mut("env").and_then(|e| e.as_object_mut()) {
            env_removed = env.remove("TOKENADE_DEBUG_LOG").is_some();
        }
        if obj
            .get("env")
            .and_then(|e| e.as_object())
            .is_some_and(|m| m.is_empty())
        {
            obj.remove("env");
        }
        sl_removed = sl_removed || env_removed;
    }
    let sl_bump = usize::from(sl_removed);
    let Some(hooks_obj) = settings.get_mut("hooks").and_then(|h| h.as_object_mut()) else {
        // No hooks key but a statusLine to strip still needs the write.
        if sl_removed && !dry_run {
            let _ = crate::write_json_atomic(
                &path,
                &serde_json::to_string_pretty(&settings).unwrap_or_default(),
            );
        }
        return if sl_removed {
            if dry_run {
                StripHooksOutcome::WouldStrip {
                    path,
                    groups_removed: sl_bump,
                }
            } else {
                StripHooksOutcome::Stripped {
                    path,
                    groups_removed: sl_bump,
                }
            }
        } else {
            StripHooksOutcome::NoGroups
        };
    };
    let mut total_removed = 0usize;
    for (_event, arr_val) in hooks_obj.iter_mut() {
        let Some(arr) = arr_val.as_array_mut() else {
            continue;
        };
        let before = arr.len();
        arr.retain(|group| {
            let is_tokenade = group
                .get("hooks")
                .and_then(|h| h.as_array())
                .map(|inner| {
                    inner.iter().any(|h| {
                        let tagged = h
                            .get("_tag")
                            .and_then(|t| t.as_str())
                            .map(|s| s == "tokenade" || s.starts_with("tokenade-"))
                            .unwrap_or(false);
                        // Claude Code's settings editor DROPS unknown fields
                        // like `_tag` — the installers already recognize
                        // their own entries by the hookwrap subcommand for
                        // exactly that reason (#775). Without this fallback
                        // an untagged entry survived uninstall and spawned a
                        // missing binary on every tool call.
                        let by_command = h
                            .get("command")
                            .and_then(|c| c.as_str())
                            .map(|c| c.contains(" hookwrap"))
                            .unwrap_or(false);
                        tagged || by_command
                    })
                })
                .unwrap_or(false);
            !is_tokenade
        });
        total_removed += before - arr.len();
    }
    // Drop the event keys our removal just emptied, and the `hooks` map with
    // them if nothing else lives there. Exactly the treatment `env` gets
    // fifteen lines above, and it was never extended here.
    //
    // Measured 2026-08-18 on a disposable HOME seeded with a one-key
    // settings.json: after a full uninstall the file came back carrying
    // `"hooks":{"PostToolUse":[],"PreCompact":[],"PreToolUse":[],
    // "SessionStart":[],"SubagentStop":[],"UserPromptSubmit":[]}` — six arrays
    // the user never wrote, on a machine tokenade had just been removed from.
    // No behaviour rides on an empty array, so this changes nothing at run
    // time; it changes what the user sees when they diff their own config,
    // which is the whole of Rule 5's claim ("a state equivalent to never
    // having installed"). Gemini and Qwen left `"hooks":{}` by the same path.
    //
    // An array a user had left empty themselves is removed too. That is a
    // mutation, and it is the right trade: an empty hook array carries no
    // configuration, and leaving ours behind to protect a hypothetical empty
    // one of theirs would keep the residue in every real case to spare a case
    // nobody has reported.
    hooks_obj.retain(|_event, arr_val| !arr_val.as_array().is_some_and(|a| a.is_empty()));
    let hooks_now_empty = hooks_obj.is_empty();
    if hooks_now_empty {
        if let Some(obj) = settings.as_object_mut() {
            obj.remove("hooks");
        }
    }
    let total_removed = total_removed + sl_bump;
    if total_removed == 0 {
        return StripHooksOutcome::NoGroups;
    }
    if dry_run {
        return StripHooksOutcome::WouldStrip {
            path,
            groups_removed: total_removed,
        };
    }
    // Backup + write, mirroring the install path (which also snapshots
    // before mutating settings.json).
    crate::diagnostics_cli::backup_file(&path);
    let pretty = match serde_json::to_string_pretty(&settings) {
        Ok(p) => p,
        Err(e) => return StripHooksOutcome::WriteFailed(format!("serialise: {e}")),
    };
    match crate::write_json_atomic(&path, &pretty) {
        Ok(()) => StripHooksOutcome::Stripped {
            path,
            groups_removed: total_removed,
        },
        Err(e) => StripHooksOutcome::WriteFailed(format!("write: {e}")),
    }
}

pub(crate) enum StripHooksOutcome {
    NoHome,
    AbsentOrUnreadable,
    NoGroups,
    Malformed(String),
    WouldStrip {
        path: PathBuf,
        groups_removed: usize,
    },
    Stripped {
        path: PathBuf,
        groups_removed: usize,
    },
    WriteFailed(String),
}
mod files;
mod integrations;
mod shell_proxy;
pub(crate) use files::*;
pub(crate) use integrations::*;
pub(crate) use shell_proxy::*;

#[cfg(test)]
mod strip_hook_tests {
    use super::*;

    fn tmpfile(tag: &str) -> std::path::PathBuf {
        // The clock alone does not make this unique: `SystemTime` granularity
        // is coarse on some platforms (~15 ms on Windows), so parallel tests
        // can read the same nanos, and `create_dir_all` adopts an existing
        // directory silently. A process-wide counter makes the name unique by
        // construction; `create_dir` fails loudly on any residual collision.
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "tokenade-test-{tag}-{}-{}-{seq}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir(&dir).expect("test tempdir name must be unique");
        dir.join("settings.json")
    }

    /// A realistic post-install Gemini settings.json: our BeforeTool wrap,
    /// generic AfterTool fold, web AfterTool compactor — plus a user hook
    /// whose PATH contains "gemini-hook" (must survive).
    fn gemini_settings() -> serde_json::Value {
        serde_json::json!({
            "theme": "dark",
            "hooks": {
                "enabled": true,
                "BeforeTool": [
                    {"matcher": "^(run_shell_command|read_file|read_many_files)$",
                     "hooks": [{"name": "tokenade-wrap", "type": "command",
                                "command": "tokenade gemini-hook"}]},
                    {"matcher": "my_tool",
                     "hooks": [{"name": "mine", "type": "command",
                                "command": "/home/u/gemini-hooks/check.sh"}]}
                ],
                "AfterTool": [
                    {"matcher": ".*",
                     "hooks": [{"name": "tokenade-fold", "type": "command",
                                "command": "tokenade gemini-after-tool"}]},
                    {"matcher": "^(google_web_search|web_fetch)$",
                     "hooks": [{"name": "tokenade-web-compact", "type": "command",
                                "command": "tokenade gemini-afterhook"}]}
                ]
            }
        })
    }

    /// Real-world `~/.profile` breakage: the user wrapped our eval line in
    /// their OWN existence guard, so the per-line strip left `if …; then`
    /// directly followed by `fi` — a syntax error that broke every login
    /// shell sourcing the file (and can login-loop a display manager).
    #[test]
    fn strip_never_orphans_a_user_written_tokenade_guard() {
        let rc = "\
. \"$HOME/.cargo/env\"
# tokenade shell proxy
if command -v tokenade >/dev/null 2>&1; then
  eval \"$(tokenade shell-init --shell sh)\" || true
fi
export EDITOR=vim
";
        let kept: Vec<&str> = rc
            .lines()
            .filter(|l| !shell_proxy_line_is_tokenade(l))
            .collect();
        let (out, orphaned) = drop_emptied_guards(&kept.join("\n"));
        assert_eq!(orphaned, 2, "if/fi pair not dropped:\n{out}");
        assert!(
            !out.contains("if command -v tokenade"),
            "orphan guard:\n{out}"
        );
        assert!(!out.contains("fi"), "orphan closer:\n{out}");
        // User content on BOTH sides of the block survives verbatim.
        assert!(
            out.contains(". \"$HOME/.cargo/env\""),
            "lost user line:\n{out}"
        );
        assert!(out.contains("export EDITOR=vim"), "lost user line:\n{out}");
    }

    /// The guard-dropper never touches a guard that still runs a command —
    /// that is load-bearing user content.
    #[test]
    fn guard_dropper_leaves_load_bearing_guards_alone() {
        for live in [
            "if command -v tokenade >/dev/null 2>&1; then\n  echo hi\nfi\n",
            "if [ -n \"$PS1\" ]; then\n  export EDITOR=vim\nfi\n",
            // Self-contained one-liners close themselves: nothing to orphan.
            "if command -v tokenade >/dev/null 2>&1; then eval \"$(x)\"; fi\n",
            "if not contains $HOME/.tokenade/shim $PATH; set -gx PATH x $PATH; end\n",
        ] {
            assert_eq!(
                drop_emptied_guards(live),
                (live.to_string(), 0),
                "touched: {live}"
            );
        }
    }

    /// The guard the user most often wraps our line in never mentions tokenade
    /// (an interactivity test). Once our line is stripped the shell carries NO
    /// evidence of tokenade, so the dropper must key on "empty body", not on
    /// the condition text — otherwise this `.profile` stays a syntax error.
    #[test]
    fn guard_dropper_repairs_guards_that_never_mention_tokenade() {
        let rc = "if [ -n \"$PS1\" ]; then\nfi\nexport EDITOR=vim\n";
        let (out, removed) = drop_emptied_guards(rc);
        assert_eq!(removed, 2, "PS1 guard left orphaned:\n{out}");
        assert_eq!(out, "export EDITOR=vim\n");
        // fish twin.
        assert_eq!(drop_emptied_guards("if type -q tokenade\nend\n").1, 2);
    }

    /// Nested guards collapse outward (fixed point), and a body of comments is
    /// still a syntax error — but the user's prose survives the repair.
    #[test]
    fn guard_dropper_collapses_nested_guards_and_keeps_comments() {
        let rc = "\
if [ -n \"$PS1\" ]; then
  if command -v tokenade >/dev/null 2>&1; then
    # loads tokenade
  fi
fi
export EDITOR=vim
";
        let (out, removed) = drop_emptied_guards(rc);
        assert!(removed >= 4, "nested guards not collapsed:\n{out}");
        assert!(!out.contains("if "), "guard survived:\n{out}");
        assert!(!out.contains("fi"), "orphan closer:\n{out}");
        assert!(out.contains("# loads tokenade"), "ate user comment:\n{out}");
        assert!(out.contains("export EDITOR=vim"), "lost user line:\n{out}");
    }

    /// The events tokenade adds must leave with it, and the guard cannot be a
    /// list of tag names — that list is what failed. Two agents gained
    /// `BeforeAgent` and `SessionStart`, the enumerated stripper did not know
    /// their tags, and `reversible` went from twenty-four green rows to
    /// twenty-two with a settings file left carrying tokenade's own hooks
    /// after an uninstall.
    ///
    /// The negative half matters as much: a user hook whose COMMAND PATH
    /// contains one of our subcommand names is not ours.
    #[test]
    fn strip_drops_every_tokenade_event_and_keeps_look_alike_user_hooks() {
        let p = tmpfile("strip-lifecycle");
        let doc = serde_json::json!({
            "hooks": {
                "enabled": true,
                "BeforeAgent": [
                    {"hooks": [{"name": "tokenade-style", "type": "command",
                                "command": "/opt/tokenade gemini-lifecycle --agent gemini-cli"}]},
                    {"hooks": [{"name": "mine", "type": "command",
                                "command": "/home/u/gemini-lifecycle-notes/run.sh"}]}
                ],
                "SessionStart": [
                    {"hooks": [{"name": "tokenade-sessionstart", "type": "command",
                                "command": "/opt/tokenade gemini-lifecycle --agent gemini-cli"}]}
                ],
                "UserPromptSubmit": [
                    {"type": "command", "bash": "/opt/tokenade hookwrap-userprompt",
                     "_tag": "tokenade-copilot-userprompt"}
                ]
            }
        });
        std::fs::write(&p, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
        let removed = strip_tokenade_entries_from_json_hooks(&p, false).unwrap();
        assert_eq!(removed, 3, "one per tokenade entry, across three events");
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert!(
            v["hooks"]["SessionStart"].is_null(),
            "emptied event survived:\n{v}"
        );
        assert!(
            v["hooks"]["UserPromptSubmit"].is_null(),
            "emptied event survived:\n{v}"
        );
        let ba = v["hooks"]["BeforeAgent"]
            .as_array()
            .expect("user hook kept");
        assert_eq!(ba.len(), 1);
        assert!(
            ba[0].to_string().contains("gemini-lifecycle-notes"),
            "a user hook whose PATH looks like one of ours was deleted:\n{v}"
        );
    }

    #[test]
    fn uninstall_strips_gemini_before_and_after_tool_entries() {
        let p = tmpfile("strip-gem");
        std::fs::write(
            &p,
            serde_json::to_string_pretty(&gemini_settings()).unwrap(),
        )
        .unwrap();
        let removed = strip_tokenade_entries_from_json_hooks(&p, false).unwrap();
        assert_eq!(removed, 3, "wrap + fold + web-compact entries dropped");
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        // AfterTool emptied → the event key itself is gone.
        assert!(v["hooks"]["AfterTool"].is_null(), "{v}");
        // The user's hook survives even though its path says "gemini-hooks".
        let bt = v["hooks"]["BeforeTool"].as_array().unwrap();
        assert_eq!(bt.len(), 1);
        assert!(bt[0].to_string().contains("/home/u/gemini-hooks/check.sh"));
        // Non-hook settings untouched.
        assert_eq!(v["theme"], "dark");
    }

    #[test]
    fn uninstall_removes_orphan_enabled_flag_when_only_ours_installed() {
        let p = tmpfile("strip-gem-all");
        let mut s = gemini_settings();
        // Drop the user hook: everything under hooks is ours.
        s["hooks"]["BeforeTool"].as_array_mut().unwrap().remove(1);
        std::fs::write(&p, serde_json::to_string_pretty(&s).unwrap()).unwrap();
        let removed = strip_tokenade_entries_from_json_hooks(&p, false).unwrap();
        assert_eq!(removed, 3);
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(
            v["hooks"],
            serde_json::json!({}),
            "orphan enabled:true cleaned up: {v}"
        );
    }
}
