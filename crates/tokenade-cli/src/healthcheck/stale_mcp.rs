//! **MCP servers left alive after their agent has gone.**
//!
//! An MCP server is launched by the agent and dies with it — except when it
//! does not. Each one then keeps its memory and its descriptors, and nothing
//! says so: the user sees a machine slowing down for no reason.
//!
//! The classification is conservative by construction: a process is declared
//! ORPHANED only if its parent has disappeared. Killing a live server would
//! cost the agent using it, and this diagnostic kills nothing — it NAMES.

use super::*;

/// #503 — Harvest /proc/* and feed it through [`classify_stale_mcps`].
/// Linux-only; returns empty on other platforms (the kernel `(deleted)`
/// suffix signal we depend on is Linux-specific).
///
/// Extracted so both `cmd_healthcheck` and `cmd_install` can warn
/// about MCP processes still running an unlinked binary after a
/// `tokenade install` upgrade — without the user having to remember
/// to run `tokenade healthcheck` themselves.
#[cfg(target_os = "linux")]
pub(crate) fn detect_stale_mcp_pids() -> Vec<(String, String)> {
    let self_pid = std::process::id().to_string();
    let mut procs: Vec<(String, String, String)> = Vec::new();
    if let Ok(rd) = fs::read_dir("/proc") {
        for entry in rd.flatten() {
            let pid_str = entry.file_name();
            let pid_str = pid_str.to_string_lossy();
            if !pid_str.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let Ok(target) = std::fs::read_link(entry.path().join("exe")) else {
                continue;
            };
            let cmdline = std::fs::read_to_string(entry.path().join("cmdline")).unwrap_or_default();
            procs.push((
                pid_str.to_string(),
                target.to_string_lossy().to_string(),
                cmdline,
            ));
        }
    }
    classify_stale_mcps(&procs, &self_pid)
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn detect_stale_mcp_pids() -> Vec<(String, String)> {
    Vec::new()
}

/// Shared stale-MCP nudge for `cmd_install` and `cmd_upgrade`. After a
/// successful binary swap (#503 / tick 21), users had two divergent
/// messages: install printed a 1-line warn with the count, upgrade
/// printed a "No stale … upgrade is live" confirmation OR a count +
/// up-to-3 PIDs + restart hint. The split made the two callers drift
/// (upgrade had richer detail, install just had a count). Centralising
/// it here gives both callers the richer message and makes future edits
/// land in one place.
///
/// `when_clean` is an optional confirmation line to print when nothing
/// is stale — `cmd_upgrade` wants "the upgrade is live", `cmd_install`
/// prefers silence (the success line just before already conveys "you're
/// set"). Pass None to print nothing in the clean case.
pub(crate) fn print_stale_mcp_nudge(when_clean: Option<&str>) {
    let s = format_stale_mcp_nudge(&detect_stale_mcp_pids(), when_clean);
    if !s.is_empty() {
        tokenade_core::gain::record_nudge(
            "stale-mcp",
            "fire",
            tokenade_core::gain::NudgeMetric::None,
        );
        print!("{s}");
    }
}

/// Pure formatter for the stale-MCP nudge block. Split out from
/// `print_stale_mcp_nudge` so the wording / PID-truncation / empty-state
/// behaviours are testable with synthetic input — `/proc` itself can't
/// be mocked but the output shape can. Returns a single string ending
/// in a newline; empty string means "print nothing".
pub(crate) fn format_stale_mcp_nudge(
    stale: &[(String, String)],
    when_clean: Option<&str>,
) -> String {
    if stale.is_empty() {
        return match when_clean {
            Some(msg) => format!("  {msg}\n"),
            None => String::new(),
        };
    }
    let mut out = format!(
        "  note: {} tokenade MCP process(es) still running the previous binary in memory.\n",
        stale.len()
    );
    for (pid, path) in stale.iter().take(3) {
        out.push_str(&format!("        - pid {pid} → {path}\n"));
    }
    if stale.len() > 3 {
        out.push_str(&format!("        ... {} more\n", stale.len() - 3));
    }
    out.push_str(
        "        Restart your AI agent (Claude Code / Codex / Cline) to pick up this build.\n",
    );
    out
}

/// Pure-function half of the #466 stale-MCP-process check.
///
/// `procs` is the harvest of `/proc/<pid>/`: `(pid, exe_target, cmdline)`.
/// `exe_target` is the raw readlink output (still containing the kernel
/// `" (deleted)"` suffix when present). `cmdline` is the NUL-separated
/// /proc/<pid>/cmdline contents.
///
/// Returns the subset that represents a `tokenade-mcp` (or
/// `tokenade … mcp`) server whose executable inode has been unlinked
/// from disk — i.e. the user `mv`'d in a new binary and this PID is
/// still running the OLD code in memory. The self PID is excluded so
/// we don't flag the healthcheck process itself.
///
/// Tuple shape: `(pid, exe_path_without_deleted_suffix)`.
///
/// Linux-only: its input comes from a `/proc` walk, and the `" (deleted)"`
/// suffix it keys on is a Linux kernel detail. This said "unix-only" and was
/// gated `#[cfg(unix)]`, while its only caller `detect_stale_mcp_pids` is
/// gated `target_os = "linux"` — so on macOS the function compiled with
/// nothing calling it and the release build carried a `dead_code` warning
/// that never appeared on Linux. Measured 2026-08-18 in the macOS VM build.
#[cfg(target_os = "linux")]
pub(crate) fn classify_stale_mcps(
    procs: &[(String, String, String)],
    self_pid: &str,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (pid, exe_target, cmdline) in procs {
        if pid == self_pid {
            continue;
        }
        // The kernel-injected " (deleted)" suffix only appears in
        // readlink output; canonicalize() would error on the deleted
        // path, so look for it directly.
        let deleted = exe_target.ends_with(" (deleted)");
        if !deleted {
            continue;
        }
        let path_part = exe_target.trim_end_matches(" (deleted)");
        let basename = std::path::Path::new(path_part)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        if basename != "tokenade-mcp" && basename != "tokenade" {
            continue;
        }
        // Only flag actual MCP servers — short-lived `tokenade wrap …`
        // invocations aren't worth restarting. cmdline format is
        // NUL-separated argv; an `argv` of `["tokenade","mcp"]` becomes
        // "tokenade\0mcp\0".
        let is_mcp_server =
            basename == "tokenade-mcp" || cmdline.contains("\0mcp\0") || cmdline.ends_with("\0mcp");
        if !is_mcp_server {
            continue;
        }
        out.push((pid.clone(), path_part.to_string()));
    }
    out
}
