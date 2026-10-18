//! **The ledger, the leftovers, and the agent talking to us.**

use super::*;

/// 2. Gain ledger writable.
pub(crate) fn check_gain_ledger(home: &std::path::Path) -> HealthCheckResult {
    let gain_path = home.join(".tokenade").join("gain.jsonl");
    tokenade_core::platform::ensure_parent_dir(&gain_path);
    let probe = home.join(".tokenade").join(".healthcheck.tmp");
    match fs::write(&probe, "test") {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            hc(
                "gain_ledger",
                CheckStatus::Ok,
                format!("gain ledger writable at {}", gain_path.display()),
                vec![],
            )
        }
        Err(e) => hc(
            "gain_ledger",
            CheckStatus::Fail,
            format!("gain ledger NOT writable at {}: {e}", gain_path.display()),
            vec![],
        ),
    }
}

/// 2b. Bypass counter (#856). Surface the 24h bypass count from
///     ~/.tokenade/bypass-counter so agents have a way to see
///     their cumulative bypass volume without grepping the file.
///     Silent when no file exists (= no recorded bypasses yet).
pub(crate) fn check_bypass_counter(home: &std::path::Path) -> Option<HealthCheckResult> {
    let bypass_path = home.join(".tokenade").join("bypass-counter");
    let s = fs::read_to_string(&bypass_path).ok()?;
    let (ws_str, c_str) = {
        let mut it = s.split_whitespace();
        (it.next()?, it.next()?)
    };
    let (ws, count) = (ws_str.parse::<u64>().ok()?, c_str.parse::<u64>().ok()?);
    let now = tokenade_core::platform::now_unix_secs();
    Some(hc(
        "bypass_count",
        CheckStatus::Info,
        format_bypass_count_check(count, now.saturating_sub(ws)),
        vec![],
    ))
}

/// 3. Memory.db — retired vestige. Stay silent unless present.
pub(crate) fn check_memory_db_vestige(home: &std::path::Path) -> Option<HealthCheckResult> {
    let mem_path = home.join(".tokenade").join("memory.db");
    if !mem_path.exists() {
        return None;
    }
    let sz = fs::metadata(&mem_path).map(|m| m.len()).unwrap_or(0);
    Some(hc(
        "memory_db_vestige",
        CheckStatus::Info,
        format!(
            "memory.db present ({sz} bytes) at {} — vestige of the retired memory MCP tools; safe to `rm`",
            mem_path.display()
        ),
        vec![],
    ))
}

/// 4. Detected agent./// 4. Detected agent.
pub(crate) fn check_agent_detect() -> HealthCheckResult {
    let agent = detect::detect(&root());
    if matches!(agent, detect::Agent::Unknown) {
        hc(
            "agent_detect",
            CheckStatus::Info,
            "no AI agent detected in current dir tree".into(),
            vec![],
        )
    } else {
        hc(
            "agent_detect",
            CheckStatus::Ok,
            format!("agent detected: {}", agent.as_str()),
            vec![],
        )
    }
}
