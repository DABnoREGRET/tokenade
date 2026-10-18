//! `tokenade healthcheck` — the ORDER of the checks, and their rendering.
//!
//! One check per question lives in `checks/`; this trunk only calls them in
//! an order that goes from the most structural to the most incidental, then
//! renders them as text or JSON. A report that starts with the disk footprint
//! and ends with "the hook is not installed" reads backwards.
#![allow(unused_imports)]
mod audit;
mod checks;
mod stale_mcp;
use crate::*;
pub(crate) use audit::*;
pub(crate) use checks::*;
pub(crate) use stale_mcp::*;

/// `tokenade healthcheck` — diagnose a tokenade install. Inspired by
/// cocoindex's `doctor` command. Prints a status row per check:
///   - Binary path + version
///   - Global gain ledger writable
///   - Memory.db reachable
///   - Detected agent's settings.json reachable
///   - Hook installed (looking for `_tag: "tokenade*"` entries)
///   - MCP server entry present
///   - Embedder + reranker models cached locally
///
/// Exits 0 when every check passes, 1 if any check is degraded. The
/// goal is "is the install fully working?" diagnostics that a user
/// can paste into a bug report.
/// Status of one [`HealthCheckResult`]. Maps to the human glyph
/// (`OK`/`!!`/`--`) AND to the `"status"` field in JSON mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckStatus {
    Ok,
    Fail,
    Info,
}

impl CheckStatus {
    pub(crate) fn as_json(self) -> &'static str {
        match self {
            CheckStatus::Ok => "ok",
            CheckStatus::Fail => "fail",
            CheckStatus::Info => "info",
        }
    }
}

/// One observable healthcheck result. Collected into a vector and
/// rendered at the end so the same check pipeline can serve both
/// `tokenade healthcheck` (human) and `tokenade healthcheck --json`
/// (CI / scripts).
#[derive(Debug, Clone)]
pub(crate) struct HealthCheckResult {
    pub(crate) id: &'static str,
    pub(crate) status: CheckStatus,
    pub(crate) message: String,
    pub(crate) detail: Vec<String>,
}

pub(crate) fn cmd_healthcheck(rest: &[String]) -> i32 {
    // Silent-accept sweep (#813-#815 follow-up): reject unknown flags.
    if let Err(code) = validate_simple_bool_flags(rest, "healthcheck", &["--json"]) {
        return code;
    }
    let json_mode = rest.iter().any(|a| a == "--json");
    let results = collect_health_checks();
    let failures = results
        .iter()
        .filter(|r| r.status == CheckStatus::Fail)
        .count();
    if json_mode {
        render_health_checks_json(&results, failures);
    } else {
        render_health_checks_text(&results, failures);
    }
    if failures == 0 {
        0
    } else {
        1
    }
}

pub(crate) fn collect_health_checks() -> Vec<HealthCheckResult> {
    let mut out: Vec<HealthCheckResult> = Vec::new();

    // 1. Binary
    out.push(check_binary());

    // 1a. The web transport. A missing curl used to be INVISIBLE: every fetch
    //     returned None, which each caller read as "the site did not answer",
    //     so an agent on a minimal container saw a web that was simply broken
    //     with no cause to point at.
    out.push(check_web_transport());

    // 1a-bis. Project-local compactors awaiting consent. A repository's
    //     filters stay inert until `tokenade trust-filters` approves them —
    //     correct, but SILENT, and the trust store's own docs point at doctor
    //     for discovery. Without a line here a filter a team committed on
    //     purpose simply never fires and nothing anywhere says why.
    out.push(check_untrusted_project_filters());
    out.push(check_invalid_filters());

    // 1b. Windows: POSIX shell for Bash-command compaction.
    #[cfg(windows)]
    out.push(check_posix_shell_windows());

    // 2. Gain ledger — needs HOME. Unresolvable HOME aborts every
    //    downstream check (all paths derive from it).
    let Some(home) = tokenade_core::platform::home_dir() else {
        out.push(hc(
            "home",
            CheckStatus::Fail,
            "HOME directory unresolvable — many paths will fail".into(),
            vec![],
        ));
        return out;
    };

    // 2a. What we are holding on the user's disk. Placed right after HOME
    //     resolves, because every path below derives from it and this is the
    //     one check whose answer the user cannot get any other way.
    out.push(check_disk_footprint(&home));
    out.push(check_gain_ledger(&home));
    out.push(check_local_log_integrity(&home));
    out.push(check_stale_binary_paths(&home));
    out.push(check_hook_binaries_agree(&home));
    out.push(check_unused_mcp_servers(&home));
    out.push(check_hook_integrity(&home));
    out.push(check_dead_levers(&home));
    out.push(check_ledger_integrity(&home));
    out.extend(check_hermes_plugin(&home));
    out.extend(check_plugin_bridges(&home));
    out.extend(check_codex_hook_trust(&home));
    if let Ok(cwd) = std::env::current_dir() {
        out.extend(check_claude_workspace_trust(&home, &cwd));
    }

    // 2b. Bypass counter (silent when absent).
    out.extend(check_bypass_counter(&home));

    // 3. Memory.db vestige (silent unless present).
    out.extend(check_memory_db_vestige(&home));

    // 4. Detected agent.
    out.push(check_agent_detect());

    // 5. Hook installed. `claude_settings` + `hook_check` are computed
    //    once here and shared with the PreToolUse-coverage check (5a2).
    let claude_settings = home.join(".claude").join("settings.json");
    let hook_check = if claude_settings.exists() {
        fs::read_to_string(&claude_settings)
            .ok()
            .map(|s| tokenade_core::platform::claude_settings_has_tokenade_hooks_in(&s))
    } else {
        None
    };
    out.push(check_hook_installed(&home, &claude_settings, hook_check));

    // 5a2. PreToolUse covers Read (only when hooks installed).
    out.extend(check_pretool_read_coverage(&claude_settings, hook_check));

    // 5b. Hook commands point to a real binary on disk.

    // 6. MCP entry (stale pre-pivot entry detection).
    out.push(check_mcp_entry(&home));

    // 6b. Wrapped third-party MCP entries.
    out.push(check_wrapped_mcp_entries());

    // 7. Embedder + reranker model cache.
    out.extend(check_embedder_cache());

    // 8. Stale MCP processes.
    out.push(check_stale_mcp_processes());

    out
}

pub(crate) fn render_health_checks_text(results: &[HealthCheckResult], failures: usize) {
    use std::io::IsTerminal;
    let color = std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    let green = if color { "\x1b[32m" } else { "" };
    let red = if color { "\x1b[31m" } else { "" };
    let dim = if color { "\x1b[2m" } else { "" };
    let rst = if color { "\x1b[0m" } else { "" };
    println!("─── tokenade healthcheck ───");
    println!();
    for r in results {
        let (col, glyph) = match r.status {
            CheckStatus::Ok => (green, "OK"),
            CheckStatus::Fail => (red, "!!"),
            CheckStatus::Info => (dim, "--"),
        };
        println!("  {col}{glyph}{rst}  {}", r.message);
        for d in &r.detail {
            println!("       {dim}{}{rst}", d);
        }
    }
    println!();
    if failures == 0 {
        println!("  {green}all checks passed{rst}");
    } else {
        println!("  {red}{failures} check(s) failed{rst} — see lines marked `!!` above");
    }
}

pub(crate) fn render_health_checks_json(results: &[HealthCheckResult], failures: usize) {
    // Hand-build the JSON so we don't introduce a dependency on the
    // `serde` derive macro just for this. Each entry is compact; the
    // top-level object also exposes a roll-up so a CI script can
    // branch on a single field.
    let entries: Vec<serde_json::Value> = results
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": r.id,
                "status": r.status.as_json(),
                "message": r.message,
                "detail": r.detail,
            })
        })
        .collect();
    let root = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "failures": failures,
        "ok": failures == 0,
        "checks": entries,
    });
    println!("{}", serde_json::to_string_pretty(&root).unwrap());
}

#[cfg(test)]
mod healthcheck_tests {
    use super::*;

    #[test]
    fn check_binary_names_itself_and_its_version() {
        let r = check_binary();
        assert_eq!(r.id, "binary");
        // Message always embeds the crate version in parens.
        assert!(r.message.contains(env!("CARGO_PKG_VERSION")));
        // Status and details are NOT asserted here on purpose. They now depend
        // on what else is on the machine's PATH — under `cargo test` the test
        // binary is not on PATH at all, which is a perfectly normal state the
        // check reports as INFO. The rule itself is pinned on the pure verdict
        // instead: `checks::which_tokenade_runs`.
    }

    #[test]
    fn check_agent_detect_emits_single_agent_row() {
        let r = check_agent_detect();
        assert_eq!(r.id, "agent_detect");
        // Either "no AI agent detected…" (Info) or "agent detected: …" (Ok).
        assert!(matches!(r.status, CheckStatus::Ok | CheckStatus::Info));
    }
}

#[cfg(test)]
mod footprint_tests {
    use super::{check_disk_footprint, CheckStatus};

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "tokenade-fp-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join(".tokenade").join("models")).unwrap();
        d
    }

    /// A small install says its size and stops there — an actionable line on
    /// every run would train the reader to skip the block.
    #[test]
    fn a_small_install_is_reported_without_advice() {
        let home = tmp("small");
        std::fs::write(home.join(".tokenade").join("gain.jsonl"), "x".repeat(1024)).unwrap();
        let r = check_disk_footprint(&home);
        assert_eq!(r.status, CheckStatus::Ok);
        assert!(r.message.contains("holding"), "{}", r.message);
        assert!(
            r.detail.iter().all(|d| !d.contains("evict-stale")),
            "advice given for a tiny install: {:?}",
            r.detail
        );
    }

    /// Past the point where a disk notices, name the biggest thing and how to
    /// get rid of it.
    #[test]
    fn a_large_install_names_what_is_big_and_how_to_reclaim() {
        let home = tmp("large");
        let models = home.join(".tokenade").join("models");
        // Sparse file: 6 GB of apparent size, no bytes written.
        let f = std::fs::File::create(models.join("weights.gguf")).unwrap();
        f.set_len(6 * 1_073_741_824).unwrap();
        drop(f);
        let r = check_disk_footprint(&home);
        assert_eq!(r.status, CheckStatus::Info);
        assert!(r.message.contains("GB"), "{}", r.message);
        assert!(
            r.detail.iter().any(|d| d.starts_with("models:")),
            "the biggest directory is not named: {:?}",
            r.detail
        );
        assert!(
            r.detail.iter().any(|d| d.contains("evict-stale")),
            "no way out offered: {:?}",
            r.detail
        );
    }
}

#[cfg(test)]
mod telemetry_integrity_tests {
    use super::*;

    fn home_with_ledger(label: &str, body: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tokenade-health-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".tokenade")).unwrap();
        std::fs::write(dir.join(".tokenade").join("gain.jsonl"), body).unwrap();
        dir
    }

    /// A row that says it produced MORE than it was given is the one shape
    /// this check exists to surface. It read field names the ledger has never
    /// used, so it reported a clean install no matter what the ledger held.
    #[test]
    fn a_row_claiming_more_output_than_input_is_reported() {
        let home = home_with_ledger(
            "impossible",
            "{\"op\":\"git_status\",\"before\":100,\"after\":900}\n\
             {\"op\":\"git_status\",\"before\":900,\"after\":100}\n",
        );
        let r = check_local_log_integrity(&home);
        let said = format!("{r:?}");
        assert!(
            said.contains("implausible"),
            "an after>before row went unreported: {said}"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// And a ledger whose rows are all coherent stays quiet — otherwise the
    /// check is just noise.
    #[test]
    fn a_coherent_ledger_reports_nothing_implausible() {
        let home = home_with_ledger(
            "coherent",
            "{\"op\":\"git_status\",\"before\":900,\"after\":100}\n",
        );
        let r = check_local_log_integrity(&home);
        let said = format!("{r:?}");
        assert!(!said.contains("implausible"), "false alarm: {said}");
        let _ = std::fs::remove_dir_all(&home);
    }
}
