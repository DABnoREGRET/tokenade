#![allow(unused_imports)]
//! PATH shim — REMOVED from the product (2026-07). This module is
//! now the DISMANTLER and nothing else.
//!
//! Earlier releases dropped a directory of tiny per-command shims at the
//! FRONT of `PATH` as the fallback for agents without a native rewrite hook.
//! No code path installs one: `tokenade install` never creates a shim, and an
//! upgrade RIPS OUT whatever an older version left.
//!
//! What remains here, and why:
//!   * [`uninstall_path_shim`] / [`report_path_shim_removal`] — the removal
//!     + upgrade migration: strip the rc `PATH` block from every rc file and
//!     delete `~/.tokenade/shim`. This is the only thing that gets a machine
//!     off the shim, so it must outlive the installer.
//!   * [`SHIM_PATH_MARKER`], [`shim_dir`], [`shim_path_line_is_tokenade`] —
//!     how the dismantler recognises what an old install wrote.
//!   * [`cmd_shim_exec`] — the invocation a surviving shim makes. It
//!     dismantles the shim that called it and then runs the wrapped command
//!     ONCE, so the machine is repaired without anyone having to type
//!     anything. See its doc.
//!
//! The general runtime passthrough that used to serve stale shims is still
//! gone: this is not "resolve and run whatever a shim asks for" but a one-shot
//! recovery that fires only after the shim is provably deleted, resolves the
//! target on a PATH stripped of every tokenade directory, and marks its child
//! so a second `shim-exec` in the same tree refuses (the fork-bomb depth cap
//! the removed feature needed, reduced to the single case that needs it).

use super::*;
use crate::wrap::*;
use crate::*;
use std::fs;
use std::path::{Path, PathBuf};

/// Marker comment that heads the rc-file PATH block.
pub(crate) const SHIM_PATH_MARKER: &str = "# tokenade path shim";

/// The managed shim directory: `~/.tokenade/shim`.
pub(crate) fn shim_dir(home: &Path) -> PathBuf {
    home.join(".tokenade").join("shim")
}

/// True for every line the shim's rc PATH block wrote. Per-line so surrounding
/// user content is untouched. Distinct from `shell_proxy_line_is_tokenade`
/// (which matches the legacy ALIAS lines) — the two never overlap because the
/// shim block references `.tokenade/shim` and the alias block
/// `.tokenade/shellproxy`.
pub(crate) fn shim_path_line_is_tokenade(line: &str) -> bool {
    let t = line.trim();
    t == SHIM_PATH_MARKER || t.contains(".tokenade/shim") || t.contains(".tokenade\\shim")
}

/// Rip out the PATH shim an older release installed: strip the rc PATH block
/// from every rc file and delete the `~/.tokenade/shim` directory. Files
/// without the block are left byte-identical. Reuses [`ProxyStrip`] for a
/// uniform report shape.
///
/// The shim is no longer installed by anything, so this is a pure migration:
/// it runs on upgrade to get a machine off a shim it never asked to keep.
pub(crate) fn uninstall_path_shim(
    home: Option<&Path>,
    dry_run: bool,
) -> Vec<(PathBuf, ProxyStrip)> {
    let home = match home
        .map(PathBuf::from)
        .or_else(tokenade_core::platform::home_dir)
    {
        Some(h) => h,
        None => return vec![],
    };
    let rc_files: &[&str] = &[
        ".profile",
        ".bashrc",
        ".bash_profile",
        ".zshenv",
        ".zshrc",
        ".config/fish/config.fish",
        ".config/powershell/Microsoft.PowerShell_profile.ps1",
        "Documents/PowerShell/Microsoft.PowerShell_profile.ps1",
        "Documents/WindowsPowerShell/Microsoft.PowerShell_profile.ps1",
    ];
    let mut results = Vec::new();
    for rel in rc_files {
        let path = home.join(rel);
        if !path.exists() {
            continue;
        }
        let original = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                results.push((path, ProxyStrip::Failed(e.to_string())));
                continue;
            }
        };
        let mut kept: Vec<&str> = Vec::new();
        let mut removed = 0usize;
        for l in original.lines() {
            if shim_path_line_is_tokenade(l) {
                removed += 1;
                if kept.last().is_some_and(|k| k.trim().is_empty()) {
                    kept.pop();
                    removed += 1;
                }
            } else {
                kept.push(l);
            }
        }
        let mut updated = kept.join("\n");
        if original.ends_with('\n') && !updated.is_empty() {
            updated.push('\n');
        }
        // Runs BEFORE the `removed == 0` bail so an rc file already emptied by
        // an older release (≤0.9.4) still gets repaired — it has no tokenade
        // line left to strip, only the broken shell of a guard.
        let (updated, orphaned) = super::uninstall::drop_emptied_guards(&updated);
        removed += orphaned;
        if removed == 0 {
            continue;
        }
        if dry_run {
            results.push((path, ProxyStrip::WouldStrip(removed)));
            continue;
        }
        // Atomic + backup, to match the installer: an interrupted truncating
        // write is itself a "shell no longer starts" bug.
        crate::backup_file(&path);
        match crate::write_text_atomic(&path, &updated) {
            Ok(()) => results.push((path, ProxyStrip::Stripped(removed))),
            Err(e) => results.push((path, ProxyStrip::Failed(e.to_string()))),
        }
    }
    let dir = shim_dir(&home);
    if dir.exists() {
        if dry_run {
            results.push((dir, ProxyStrip::WouldDelete));
        } else {
            match fs::remove_dir_all(&dir) {
                Ok(()) => results.push((dir, ProxyStrip::Deleted)),
                Err(e) => results.push((dir, ProxyStrip::Failed(e.to_string()))),
            }
        }
    }
    results
}

/// Cheap probe — "is there still a shim DIRECTORY on this machine?". One
/// `exists()`, so it is safe to call on the hot path (every hook, every
/// `tokenade wrap`) ahead of the full rc-file sweep.
///
/// Deliberately narrower than "is there anything to dismantle?": it answers
/// only the case that actively HIJACKS `PATH` and therefore must be repaired
/// without waiting for a throttle window or an install sentinel. The rc-line
/// residue and the ≤0.9.4 emptied-guard repair are cold-path concerns and stay
/// on the unconditional sweep in the interactive entry point.
pub(crate) fn shim_dir_present(home: &Path) -> bool {
    shim_dir(home).exists()
}

/// Run [`uninstall_path_shim`] against the real home dir and print one line
/// per touched file/dir. Returns how many entries were (or would be) changed.
/// Shared by `tokenade uninstall`.
pub(crate) fn report_path_shim_removal(dry_run: bool) -> usize {
    let home = tokenade_core::platform::home_dir();
    let mut touched = 0usize;
    for (path, outcome) in uninstall_path_shim(home.as_deref(), dry_run) {
        match outcome {
            ProxyStrip::Stripped(n) => {
                println!("  ✓ removed {n} PATH shim line(s) from {}", path.display());
                touched += 1;
            }
            ProxyStrip::WouldStrip(n) => {
                println!(
                    "  ◐ would remove {n} PATH shim line(s) from {}",
                    path.display()
                );
                touched += 1;
            }
            ProxyStrip::Deleted => {
                println!("  ✓ deleted shim dir {}", path.display());
                touched += 1;
            }
            ProxyStrip::WouldDelete => {
                println!("  ◐ would delete shim dir {}", path.display());
                touched += 1;
            }
            ProxyStrip::Failed(e) => {
                eprintln!("  ✗ PATH shim removal in {}: {e}", path.display());
            }
        }
    }
    touched
}

// ---- `tokenade shim-exec` — REMOVED ------------------------------------

/// Env marker set on the child of [`run_once_after_repair`]. Its ONLY job is
/// to make a second `shim-exec` in the same process tree refuse to run
/// anything: if a shim we failed to delete is still resolvable, the child
/// bounces straight back here and the pair would spin forever. The removed
/// passthrough's fork-bomb depth cap in one flag — this path runs at most one
/// command, exactly once, on the invocation that repaired the machine.
pub(crate) const SHIM_EXEC_REENTRY_ENV: &str = "TOKENADE_SHIM_EXEC_RECOVERY";

/// Resolve `cmd` on `PATH` while IGNORING every tokenade-owned directory, so
/// the answer can never be a shim (deleted or not). Returns None when nothing
/// executable by that name is left — the honest outcome is then the message,
/// not a guess.
///
/// Absolute / relative paths are refused outright: a shim always passes a bare
/// command name, so anything else is a hand-written `shim-exec` invocation we
/// have no business executing.
pub(crate) fn resolve_outside_shim(
    cmd: &str,
    path_var: &str,
    home: Option<&Path>,
) -> Option<PathBuf> {
    if cmd.contains('/') || cmd.contains('\\') {
        return None;
    }
    let tokenade_root = home.map(|h| h.join(".tokenade"));
    for dir in std::env::split_paths(path_var) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        if tokenade_root
            .as_ref()
            .is_some_and(|root| dir.starts_with(root))
        {
            continue;
        }
        let candidate = dir.join(cmd);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
        // Windows carries the extension in PATHEXT; a shim victim there is
        // `node.exe`/`npm.cmd`, never a bare `node`.
        #[cfg(windows)]
        for ext in ["exe", "cmd", "bat", "com"] {
            let candidate = dir.join(format!("{cmd}.{ext}"));
            if is_executable_file(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Run the wrapped command once, after the dismantler has been through, and
/// return its exit code — or None when we must NOT run it.
///
/// The four conditions, all required:
///   1. not a re-entry (see [`SHIM_EXEC_REENTRY_ENV`]);
///   2. `~/.tokenade/shim` no longer exists — we ran because it did, so a
///      surviving directory means the repair failed (read-only home, races)
///      and the next lookup could land on a shim again;
///   3. the target resolves on a PATH with every tokenade directory removed;
///   4. that target is not itself inside `~/.tokenade` (belt and braces —
///      (3) already excludes it).
/// The child also inherits the FILTERED PATH, so anything it spawns in turn
/// resolves the same way this invocation did.
fn run_once_after_repair(
    cmd: &str,
    args: &[String],
    home: Option<&Path>,
    repaired: usize,
) -> Option<i32> {
    if std::env::var_os(SHIM_EXEC_REENTRY_ENV).is_some() {
        return None;
    }
    let home = home?;
    if shim_dir_present(home) {
        return None;
    }
    let path_var = std::env::var("PATH").ok()?;
    let target = resolve_outside_shim(cmd, &path_var, Some(home))?;
    let filtered: Vec<PathBuf> = std::env::split_paths(&path_var)
        .filter(|d| !d.starts_with(home.join(".tokenade")))
        .collect();
    let filtered = std::env::join_paths(filtered).ok()?;
    // One line, on stderr, so a human reading a hook transcript learns what
    // was repaired — while stdout stays exactly what the command produced.
    if repaired > 0 {
        eprintln!(
            "tokenade: removed a stale PATH shim left by an old version ({repaired} item(s): \
             ~/.tokenade/shim + the PATH block in your shell rc) and ran `{cmd}` for you. \
             Restart this app when convenient so its own PATH stops pointing at the shim."
        );
    }
    // This runs the USER's real command, so it owes the same guarantee as every
    // other launcher: cancelling it must cancel the work. A one-shot recovery
    // path is still a `cargo build` that would otherwise outlive its wrapper.
    let mut cmd = std::process::Command::new(&target);
    cmd.args(args)
        .env("PATH", filtered)
        .env(SHIM_EXEC_REENTRY_ENV, "1");
    tokenade_core::process_util::configure_own_group(&mut cmd);
    tokenade_core::process_util::configure_die_with_parent(&mut cmd);
    let mut child = cmd.spawn().ok()?;
    let reaper = if !tokenade_core::process_util::terminal_owns_the_group() {
        tokenade_core::process_util::forward_termination_to(child.id());
        tokenade_core::process_util::guard_group_against_our_death(child.id())
    } else {
        None
    };
    let status = child.wait().ok()?;
    tokenade_core::process_util::stop_forwarding_termination();
    if let Some(r) = reaper {
        r.stand_down();
    }
    // 126/127 are the shell's "found but not executable" / "not found" —
    // reusing 126 for a signal death keeps us inside the same convention.
    Some(status.code().unwrap_or(126))
}

/// The PATH shim's old runtime target. The shim is gone from the product and
/// the passthrough that kept stale shims working went with it
/// (2026-07): only the DISMANTLER above remains.
///
/// The name stays a KNOWN command on purpose. A stale shim still on a user's
/// PATH re-execs `tokenade shim-exec <cmd>`, and letting that fall through to
/// `unknown command` would fail with nothing to act on.
///
/// This is also the one code path that has PROOF a stale shim exists — it was
/// literally invoked by one — so it SELF-HEALS rather than only complaining
/// (field report, 2026-07): the message it used to print lands in an agent's
/// hook transcript, not in front of a human, so "run `tokenade install`" was
/// advice nobody was in a position to follow and the machine stayed broken.
/// The dismantler runs first; only then do we report, and the report tells the
/// user what was repaired and what they still have to do (restart the app that
/// captured the hijacked `PATH` — reopening a shell does nothing for a desktop
/// agent launched before the strip).
///
/// Since 1.1.2 it also RUNS the wrapped command once the shim is provably
/// gone — see [`run_once_after_repair`]. Reporting a repair the caller can't
/// act on was still a dead end for the class of victim that motivated the
/// self-heal: an npm-installed `tokenade` is `bin/tokenade.js`
/// (`#!/usr/bin/env node`), so a stale `node` shim makes `tokenade upgrade`
/// and `tokenade install` — the two commands the message tells you to run —
/// fail exactly like the hook that got you here. Field report 2026-07-29: a
/// user on ≤1.1.0 could not reach any tokenade command at all, npm included.
///
/// When the command cannot be run (still shimmed, not on `PATH`, re-entry) it
/// exits non-zero with the old message: the wrapped command genuinely did not
/// run, and the caller must not mistake a repaired PATH for a successful
/// invocation.
pub(crate) fn cmd_shim_exec(rest: &[String]) -> i32 {
    // A shim always passes the wrapped command first. Anything starting with
    // `-` is someone running `shim-exec --help` by hand, and claiming their
    // flag "was not executed" would be nonsense.
    let wrapped = rest
        .first()
        .map(String::as_str)
        .filter(|c| !c.starts_with('-'));
    // Self-heal BEFORE printing, so the message can state what actually
    // happened. Best-effort: a failure (read-only home, permissions) just
    // means the counts come back empty and we fall back to the manual
    // instruction below.
    let home = tokenade_core::platform::home_dir();
    let repaired = if wrapped.is_some() {
        let n = uninstall_path_shim(home.as_deref(), false)
            .iter()
            .filter(|(_, o)| matches!(o, ProxyStrip::Stripped(_) | ProxyStrip::Deleted))
            .count();
        // The legacy alias proxy is the same removed feature and the same
        // class of breakage; a machine carrying one usually carries both.
        let n = n + super::uninstall_shell_proxy(home.as_deref(), false)
            .iter()
            .filter(|(_, o)| matches!(o, ProxyStrip::Stripped(_) | ProxyStrip::Deleted))
            .count();
        n
    } else {
        0
    };
    // The shim is gone: run what the caller actually asked for, once, so the
    // invocation that proved the machine was broken is also the one that
    // un-breaks it. Returns None when that isn't provably safe.
    if let Some(cmd) = wrapped {
        // Shims call `tokenade shim-exec <cmd> -- "$@"`: the separator is the
        // shim's, not the user's, and must not reach the command.
        let args = match rest[1..].split_first() {
            Some((first, tail)) if first == "--" => tail,
            _ => &rest[1..],
        };
        if let Some(code) = run_once_after_repair(cmd, args, home.as_deref(), repaired) {
            return code;
        }
    }
    eprintln!(
        "tokenade: the PATH shim was removed in 0.9.5 and `shim-exec` no longer runs commands."
    );
    if let Some(cmd) = wrapped {
        eprintln!("`{cmd}` was NOT executed.");
    }
    if repaired > 0 {
        eprintln!(
            "The stale shim has now been REMOVED automatically ({repaired} item(s): \
             ~/.tokenade/shim and the PATH block in your shell rc).\n\
             Restart the app that ran this command — Claude Code / Claude Desktop, your\n\
             editor, or your shell — so it picks up a PATH without the shim. Nothing else\n\
             to do; re-running the same command after the restart will work."
        );
    } else if wrapped.is_some() {
        eprintln!(
            "A stale shim is still on your PATH and could not be removed automatically.\n\
             Run `tokenade install` to strip it (it removes ~/.tokenade/shim and the PATH\n\
             block from your shell rc), then restart the app that ran this command."
        );
    }
    2
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The resolver is the whole safety argument for running a command from
    /// `shim-exec`: whatever it returns is what gets executed, so it must be
    /// incapable of returning a shim — deleted or not.
    #[cfg(unix)]
    #[test]
    fn resolve_outside_shim_never_returns_a_tokenade_owned_path() {
        use std::os::unix::fs::PermissionsExt;
        let home = std::env::temp_dir().join(format!("tokenade-resolve-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        let shim = shim_dir(&home);
        let real = home.join("usr-bin");
        fs::create_dir_all(&shim).unwrap();
        fs::create_dir_all(&real).unwrap();
        for dir in [&shim, &real] {
            let f = dir.join("node");
            fs::write(&f, "#!/bin/sh\n").unwrap();
            fs::set_permissions(&f, fs::Permissions::from_mode(0o755)).unwrap();
        }
        // Shim FIRST, exactly as the hijacked PATH had it.
        let path_var = format!("{}:{}", shim.display(), real.display());

        assert_eq!(
            resolve_outside_shim("node", &path_var, Some(&home)),
            Some(real.join("node")),
            "the shim directory must be skipped even when it still exists"
        );
        // Nothing left outside the shim ⇒ None, so the caller reports instead
        // of guessing.
        let shim_only = format!("{}", shim.display());
        assert_eq!(resolve_outside_shim("node", &shim_only, Some(&home)), None);
        // A bare name is all a shim can pass; anything else is hand-written.
        assert_eq!(
            resolve_outside_shim("../node", &path_var, Some(&home)),
            None
        );
        // Non-executable file with the right name is not a hit.
        let plain = real.join("notexec");
        fs::write(&plain, "x").unwrap();
        fs::set_permissions(&plain, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            resolve_outside_shim("notexec", &path_var, Some(&home)),
            None
        );

        let _ = fs::remove_dir_all(&home);
    }
}
