//! Agents OTHER than Claude Code whose status line is a COMMAND the host
//! spawns, handing it the session JSON on stdin and rendering its stdout.
//!
//! That is byte-for-byte the contract `tokenade statusline` already answers,
//! so the ticker needs no per-agent renderer — only the wiring. Each entry
//! below was read off the vendor's OWN shipped bundle, not
//! inferred from a family resemblance:
//!
//! * **Copilot CLI** (`@github/copilot`, `app.js`) — settings schema carries
//!   `statusLine: { type: "command", command: <string>, padding }`, described
//!   as "Path to the script or shell command to execute for generating the
//!   status line content". The runner spawns it with `stdio: [pipe,pipe,pipe]`
//!   and `stdin.end(JSON.stringify(statusObject))`, keeps trimmed stdout, and
//!   kills at 10 s. The status object is
//!   `{cwd, session_id, session_name, transcript_path, model:{id,display_name},
//!   workspace:{current_dir}, username, remote}` — `session_id` and `model.id`
//!   are exactly the two fields the ticker reads. Config home is
//!   `$COPILOT_HOME` else `~/.copilot`, and `config.json` there says in its own
//!   first line: "User settings belong in settings.json."
//!
//! * **Cursor CLI** (`cursor-agent`, `src/hooks/use-status-line.ts` in the
//!   shipped bundle) — `statusLine: { type: "command", command, padding?,
//!   updateIntervalMs?, timeoutMs? }` in `~/.cursor/cli-config.json`. The
//!   runner splits the command with `string-argv` and spawns it with
//!   **`shell: false`**, writes `JSON.stringify(payload)` to stdin, and
//!   defaults `timeoutMs` to **2000**. Two consequences this wiring depends
//!   on: our command must survive a plain argv split (`<exe> statusline
//!   [--wrap-b64 <b64>]` does — base64's alphabet has no space), and the
//!   render must stay well inside 2 s (measured 4 ms warm on this machine).
//!
//! Qwen Code has the same shape and is wired in `gemini_hooks.rs`, next to the
//! rest of its settings write. Measured NEGATIVES, so nobody re-derives them:
//! Gemini CLI (the only `statusline` strings in its bundle are vim
//! `setlocal statusline=` in the external-editor helper), Codex CLI, Droid,
//! Reasonix and Kilo (no such key anywhere in the shipped code), OpenClaw
//! (`setStatus` is its messaging-channel account sink, not a TUI line) and
//! OpenCode (whose plugin SDK offers `tui.showToast`, a transient toast —
//! not a persistent line).

use super::*;
use std::path::{Path, PathBuf};

/// One host: a display label and the config file, relative to `$HOME`, whose
/// root object carries the `statusLine` key.
pub(crate) struct StatuslineHost {
    pub(crate) label: &'static str,
    pub(crate) rel: &'static [&'static str],
}

/// The hosts wired by [`wire_statusline_hosts`]. Claude Code and Qwen Code are
/// absent on purpose: both are written by their own agent installer, which
/// already owns that file for hooks.
pub(crate) const STATUSLINE_HOSTS: &[StatuslineHost] = &[
    StatuslineHost {
        label: "Copilot CLI",
        rel: &[".copilot", "settings.json"],
    },
    StatuslineHost {
        label: "Cursor CLI",
        rel: &[".cursor", "cli-config.json"],
    },
];

fn host_path(home: &Path, h: &StatuslineHost) -> PathBuf {
    h.rel.iter().fold(home.to_path_buf(), |p, seg| p.join(seg))
}

/// Wire (or refresh) the ticker in one already-existing JSON config.
///
/// Returns `Ok(true)` when the file was actually rewritten.
///
/// The file itself may be absent — a Copilot or Cursor user who has never
/// changed a setting has no `settings.json` at all, and gating on the FILE
/// would mean the ticker appeared only for people who had already customised
/// something. What is NOT created is the agent's DIRECTORY: its presence is
/// the same signal `detect` uses for the agent being installed, so creating it
/// would plant a config for a product the user does not have. Same contract as
/// the hook writers next door, which create `~/.copilot/hooks/tokenade.json`
/// under an already-existing `~/.copilot`.
///
/// A config that is not parsable JSON, or is JSON but not an object, is left
/// untouched with an error — the same hardening as every other writer here
/// (never replace a hand-edited config with a tokenade-only one).
pub(crate) fn wire_statusline_file(
    path: &Path,
    exe: &str,
    home: Option<&Path>,
    dry_run: bool,
) -> Result<bool, String> {
    if !path.parent().map(Path::exists).unwrap_or(false) {
        return Ok(false);
    }
    let mut root = crate::install::json_util::read_json_object_or_bail(path)?;
    let obj = root
        .as_object_mut()
        .ok_or_else(|| format!("{} is not a JSON object", path.display()))?;
    // Same rule as Claude Code's: a ticker the user took out stays out.
    if !wire_statusline_unless_removed(path, obj, exe, home, false, dry_run) {
        return Ok(false);
    }
    if dry_run {
        return Ok(true);
    }
    crate::write_json_atomic(
        path,
        &serde_json::to_string_pretty(&root).unwrap_or_default(),
    )
    .map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(true)
}

/// Wire every host in [`STATUSLINE_HOSTS`] that has a config on disk.
/// Returns one line per host that changed, for the installer's report.
/// Honours the same opt-out as Claude Code's ticker.
pub(crate) fn wire_statusline_hosts(
    home: Option<&Path>,
    exe: &str,
    dry_run: bool,
) -> Vec<Result<String, String>> {
    let mut out = Vec::new();
    if statusline_opted_out(home) {
        return out;
    }
    let Some(home) = home
        .map(|h| h.to_path_buf())
        .or_else(tokenade_core::platform::home_dir)
    else {
        return out;
    };
    for h in STATUSLINE_HOSTS {
        let p = host_path(&home, h);
        match wire_statusline_file(&p, exe, Some(&home), dry_run) {
            Ok(true) => out.push(Ok(format!(
                "savings ticker wired into {} ({})",
                p.display(),
                h.label
            ))),
            Ok(false) => {}
            Err(e) => out.push(Err(format!("{}: {e}", h.label))),
        }
    }
    out
}

/// Symmetric removal, used by `uninstall` and by `tokenade statusline off`.
/// Restores a wrapped user command when there was one; foreign statuslines are
/// never touched.
pub(crate) fn unwire_statusline_hosts(home: Option<&Path>, dry_run: bool) -> Vec<PathBuf> {
    let mut touched = Vec::new();
    let Some(home) = home
        .map(|h| h.to_path_buf())
        .or_else(tokenade_core::platform::home_dir)
    else {
        return touched;
    };
    for h in STATUSLINE_HOSTS {
        let p = host_path(&home, h);
        let Ok(raw) = std::fs::read_to_string(&p) else {
            continue;
        };
        let Ok(mut v) =
            serde_json::from_str::<serde_json::Value>(raw.trim_start_matches('\u{feff}'))
        else {
            continue;
        };
        let Some(obj) = v.as_object_mut() else {
            continue;
        };
        if !unwire_statusline(obj) {
            continue;
        }
        if !dry_run {
            let _ =
                crate::write_json_atomic(&p, &serde_json::to_string_pretty(&v).unwrap_or_default());
        }
        touched.push(p);
    }
    touched
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = "/opt/tokenade";

    /// Std-only tempdir (no tempfile dev-dep here), unique by construction:
    /// a process-wide counter beside pid+nanos, because the clock alone is too
    /// coarse on some platforms for parallel tests.
    fn tmpdir(tag: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!(
            "tokenade-test-{tag}-{}-{}-{seq}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir(&p).expect("test tempdir name must be unique");
        p
    }

    fn write(p: &Path, s: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    #[test]
    fn an_agent_that_is_not_installed_gains_nothing() {
        let d = tmpdir("sl-copilot");
        let p = d.join(".copilot").join("settings.json");
        assert_eq!(
            wire_statusline_file(&p, EXE, Some(d.as_path()), false),
            Ok(false)
        );
        assert!(
            !p.parent().unwrap().exists(),
            "an absent agent must not gain a directory"
        );
    }

    #[test]
    fn a_detected_agent_with_no_settings_file_yet_gets_one() {
        // `~/.copilot` exists (the agent has run) but the user never changed a
        // setting, so there is no settings.json. Gating on the FILE would leave
        // the ticker off for exactly the untouched default install.
        let d = tmpdir("sl-fresh");
        let p = d.join(".copilot").join("settings.json");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        assert_eq!(
            wire_statusline_file(&p, EXE, Some(d.as_path()), false),
            Ok(true)
        );
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(v["statusLine"]["command"], "/opt/tokenade statusline");
    }

    #[test]
    fn wires_then_is_idempotent_then_unwires() {
        let d = tmpdir("sl-cursor");
        let p = d.join(".cursor").join("cli-config.json");
        write(&p, r#"{"version":1,"notifications":true}"#);

        assert_eq!(
            wire_statusline_file(&p, EXE, Some(d.as_path()), false),
            Ok(true)
        );
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(v["statusLine"]["command"], "/opt/tokenade statusline");
        assert_eq!(v["statusLine"]["type"], "command");
        // sibling keys survive
        assert_eq!(v["version"], 1);

        // second pass writes nothing
        assert_eq!(
            wire_statusline_file(&p, EXE, Some(d.as_path()), false),
            Ok(false)
        );

        assert_eq!(unwire_statusline_hosts(Some(&d), false), vec![p.clone()]);
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert!(v.get("statusLine").is_none());
        assert_eq!(v["version"], 1);
    }

    #[test]
    fn a_users_own_statusline_is_wrapped_and_given_back() {
        let d = tmpdir("sl-copilot");
        let p = d.join(".copilot").join("settings.json");
        write(
            &p,
            r#"{"statusLine":{"type":"command","command":"my-prompt --fancy","padding":2}}"#,
        );
        assert_eq!(
            wire_statusline_file(&p, EXE, Some(d.as_path()), false),
            Ok(true)
        );
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        let cmd = v["statusLine"]["command"].as_str().unwrap();
        assert!(
            cmd.starts_with("/opt/tokenade statusline --wrap-b64 "),
            "{cmd}"
        );
        // Cursor splits the command with string-argv and spawns shell:false —
        // an embedded space would silently truncate the wrapped payload.
        assert_eq!(cmd.split_whitespace().count(), 4, "{cmd}");
        // `padding` is the host's, not ours: it must survive the wrap.
        assert_eq!(v["statusLine"]["padding"], 2);

        unwire_statusline_hosts(Some(&d), false);
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(v["statusLine"]["command"], "my-prompt --fancy");
        assert_eq!(v["statusLine"]["padding"], 2);
    }

    #[test]
    fn a_ticker_the_user_took_out_is_not_put_back() {
        // Reported: every daily update put back a status line the user had
        // removed. Once wired, a file without our ticker is the user's choice.
        let d = tmpdir("sl-removed");
        let p = d.join(".copilot").join("settings.json");
        write(&p, "{}");
        assert_eq!(
            wire_statusline_file(&p, EXE, Some(d.as_path()), false),
            Ok(true)
        );
        write(&p, "{}");
        assert_eq!(
            wire_statusline_file(&p, EXE, Some(d.as_path()), false),
            Ok(false)
        );
        write(&p, r#"{"statusLine":{"command":"mine.sh"}}"#);
        assert_eq!(
            wire_statusline_file(&p, EXE, Some(d.as_path()), false),
            Ok(false)
        );
        assert!(std::fs::read_to_string(&p).unwrap().contains("\"mine.sh\""));
        // A file we never wired is still wired the first time.
        let other = d.join(".cursor").join("cli-config.json");
        write(&other, "{}");
        assert_eq!(
            wire_statusline_file(&other, EXE, Some(d.as_path()), false),
            Ok(true)
        );
    }

    #[test]
    fn a_broken_config_is_reported_not_replaced() {
        let d = tmpdir("sl-cursor");
        let p = d.join(".cursor").join("cli-config.json");
        write(&p, "{ this is not json");
        assert!(wire_statusline_file(&p, EXE, Some(d.as_path()), false).is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{ this is not json");
    }

    #[test]
    fn dry_run_reports_without_writing() {
        let d = tmpdir("sl-copilot");
        let p = d.join(".copilot").join("settings.json");
        write(&p, "{}");
        assert_eq!(
            wire_statusline_file(&p, EXE, Some(d.as_path()), true),
            Ok(true)
        );
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{}");
    }

    #[test]
    fn wire_statusline_hosts_fills_cursor_cli_config_without_statusline() {
        // Same hole `ensure_hooks` now heals: Cursor CLI present, ticker absent.
        let d = tmpdir("sl-ensure");
        let p = d.join(".cursor").join("cli-config.json");
        write(&p, r#"{"version":1,"notifications":true}"#);
        let msgs: Vec<String> = wire_statusline_hosts(Some(&d), EXE, false)
            .into_iter()
            .filter_map(Result::ok)
            .collect();
        assert!(msgs.iter().any(|m| m.contains("Cursor CLI")), "{msgs:?}");
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(v["statusLine"]["command"], "/opt/tokenade statusline");
        assert_eq!(v["version"], 1);
    }
}
