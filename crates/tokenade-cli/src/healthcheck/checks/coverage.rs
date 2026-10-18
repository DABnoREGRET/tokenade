//! **Is the hook installed, and does it cover what its handler handles?**
//!
//! Two questions, and the second is the one that bit: an INSTALLED hook whose
//! matcher is narrower than the router behind it is a dead hook that reports
//! itself alive. It is the worse of the two failures, because it returns OK.
//!
//! MCP entries fall under the same question asked in another place: declared
//! but pointing to a missing binary, an entry is wiring that conducts nothing.

use super::*;

/// Does the tokenade PreToolUse hook's matcher cover the `Read` tool?
///
/// `Some(true)`  — a tokenade `hookwrap-pretool` entry exists AND its matcher
///                 includes `Read` (Read dedup / staging / post-edit re-read
///                 saver all fire).
/// `Some(false)` — the entry exists but its matcher is Bash-only / omits
///                 `Read` — so every Read runs un-deduped (a real field case:
///                 283 Reads, zero read events, heavy re-reads, no savings).
/// `None`        — no tokenade PreToolUse entry at all (the `hook_installed`
///                 check already reports that).
pub(crate) fn pretool_matcher_covers_read(settings_json: &str) -> Option<bool> {
    let v: serde_json::Value = serde_json::from_str(settings_json).ok()?;
    let groups = v.pointer("/hooks/PreToolUse")?.as_array()?;
    let mut found = false;
    let mut covers = false;
    for g in groups {
        let is_pretool = g
            .get("hooks")
            .and_then(|h| h.as_array())
            .map(|arr| {
                arr.iter().any(|h| {
                    h.get("command")
                        .and_then(|c| c.as_str())
                        .map(|s| s.contains("hookwrap-pretool"))
                        .unwrap_or(false)
                        || h.get("_tag")
                            .and_then(|t| t.as_str())
                            .map(|t| t.contains("pretool"))
                            .unwrap_or(false)
                })
            })
            .unwrap_or(false);
        if !is_pretool {
            continue;
        }
        found = true;
        let matcher = g.get("matcher").and_then(|m| m.as_str()).unwrap_or("");
        // Claude matchers are regex alternations (`Bash|Read|WebFetch|…`); an
        // empty matcher or `*` matches every tool.
        if matcher.is_empty() || matcher == "*" || matcher.split('|').any(|t| t.trim() == "Read") {
            covers = true;
        }
    }
    if found {
        Some(covers)
    } else {
        None
    }
}

/// 5. Hook installed — preferred marker `"_tag": "tokenade"` (#605),
///    with fallback to the hookwrap-command marker (#774) for installs
///    where an upgrade path stripped the `_tag` (real reproduction on
///    user machine: settings.json had 6 live `tokenade hookwrap*`
///    entries but no `_tag`, so the legacy probe falsely reported FAIL).
pub(crate) fn check_hook_installed(
    home: &std::path::Path,
    claude_settings: &std::path::Path,
    hook_check: Option<bool>,
) -> HealthCheckResult {
    match hook_check {
        Some(true) => hc(
            "hook_installed",
            CheckStatus::Ok,
            format!("hookwrap found in {}", claude_settings.display()),
            vec![],
        ),
        // No tokenade hooks in settings.json. Distinguish two very
        // different causes via the install sentinel: a machine that NEVER
        // installed (→ run install) vs one that DID install but whose hooks
        // were later dropped/overwritten by another tool or an agent upgrade
        // (the silent-bypass failure mode). The latter is drift, not a
        // missing install — point at the self-heal, which also runs
        // automatically from SessionStart / `tokenade wrap` / the shell proxy.
        Some(false) => {
            if tokenade_core::install_state::installed_ever(home) {
                hc(
                    "hook_installed",
                    CheckStatus::Fail,
                    format!(
                        "tokenade WAS installed here but its hooks are GONE from {} — another tool or an agent upgrade overwrote settings.json (everything ran un-compacted). Run `tokenade ensure-hooks` to rebuild them now; SessionStart + the shell proxy also self-heal automatically.",
                        claude_settings.display()
                    ),
                    vec![],
                )
            } else {
                hc(
                    "hook_installed",
                    CheckStatus::Fail,
                    format!(
                        "hookwrap NOT in {} — run `tokenade install`",
                        claude_settings.display()
                    ),
                    vec![],
                )
            }
        }
        // No settings.json AT ALL. On Claude Code that is not a neutral fact:
        // it is the purest form of "hooks are not installed", and it is what a
        // machine that has never run `tokenade install` looks like. It was
        // reported as INFO, three lines under `OK all agent-config hook paths
        // resolve` and `OK every hook runs the same tokenade binary` — both
        // vacuously true over an empty set — so the report read green on the
        // one question the scaffold tells agents to bring here: "`tokenade
        // health` says whether they are INSTALLED, and names the fix when they
        // are not, so you never have to guess."
        //
        // Same two causes as `Some(false)`, and the same two answers: never
        // installed → `tokenade install`; installed once and the file is gone
        // → drift, point at the self-heal. On any other agent a missing
        // ~/.claude/settings.json means nothing, and stays INFO.
        None if matches!(
            tokenade_core::detect::detect(&std::env::current_dir().unwrap_or_default()),
            tokenade_core::detect::Agent::ClaudeCode
        ) =>
        {
            if tokenade_core::install_state::installed_ever(home) {
                hc(
                    "hook_installed",
                    CheckStatus::Fail,
                    format!(
                        "tokenade WAS installed here but {} is GONE — everything is running un-compacted. Run `tokenade ensure-hooks` to rebuild them now; SessionStart + the shell proxy also self-heal automatically.",
                        claude_settings.display()
                    ),
                    vec![],
                )
            } else {
                hc(
                    "hook_installed",
                    CheckStatus::Fail,
                    format!(
                        "no {} — tokenade hooks are NOT installed for Claude Code, so nothing is being compacted. Run `tokenade install`",
                        claude_settings.display()
                    ),
                    vec![],
                )
            }
        }
        None => hc(
            "hook_installed",
            CheckStatus::Info,
            format!(
                "Claude Code settings.json not present at {}",
                claude_settings.display()
            ),
            vec![],
        ),
    }
}

/// 5a2. PreToolUse must cover `Read`, not just `Bash`. Field case: a user
///      re-read the same files 18× with ZERO read-dedup savings and no read
///      events because his installed matcher was Bash-only — so Read dedup,
///      slice staging, and the post-edit re-read saver never ran. Only
///      meaningful once hooks ARE installed.
pub(crate) fn check_pretool_read_coverage(
    claude_settings: &std::path::Path,
    hook_check: Option<bool>,
) -> Option<HealthCheckResult> {
    if !matches!(hook_check, Some(true)) {
        return None;
    }
    let s = fs::read_to_string(claude_settings).ok()?;
    match pretool_matcher_covers_read(&s) {
        Some(true) => Some(hc(
            "pretool_read",
            CheckStatus::Ok,
            // Coverage, not activity: this reads the MATCHER, so it knows the
            // read levers CAN run and nothing about whether they have. Saying
            // "active" here made the same claim already removed from the bridge
            // lines — and on a machine whose read levers have no rows in
            // thirty days, the dead-lever row three lines down contradicted it.
            "PreToolUse hook covers Read — dedup and slice staging can run".to_string(),
            vec![],
        )),
        Some(false) => Some(hc(
            "pretool_read",
            CheckStatus::Fail,
            format!(
                "PreToolUse matcher does NOT include Read in {} — Read dedup/staging \
                 and the post-edit re-read saver never fire (every Read runs \
                 un-compacted). Run `tokenade ensure-hooks` to rebuild the matcher.",
                claude_settings.display()
            ),
            vec![],
        )),
        None => None, // no pretool entry — hook_installed already covers it
    }
}

// 5b. "Hook commands point to a real binary on disk" was a SECOND check over
//     the same fact, scoped to Claude Code alone, and it is gone: it existed
//     because `check_stale_binary_paths` could not see the `sh -c '…'` shape
//     the installer writes, so the working answer had to come from somewhere.
//     With that extraction fixed, keeping both meant one report carrying an OK
//     and a FAIL about the same binary — which is exactly what it did.
//     `check_stale_binary_paths` covers every agent config, not just Claude's.

/// 6. MCP entry — ZERO-MCP PIVOT (2026-06): tokenade no longer
///    registers an MCP server; everything routes through the hooks
///    (verified in step 5) and the universal shell proxy. So the
///    DESIRED state is now the inverse of what this check used to
///    assert: NO `tokenade` MCP entry anywhere. We still walk the
///    three Claude Code MCP lookup locations (#662) — a global
///    ~/.claude.json `.mcpServers`, a per-project
///    `.projects[cwd].mcpServers`, and a project-local <cwd>/.mcp.json —
///    but only to flag a STALE pre-pivot entry that should be purged
///    (it would spawn the now-removed `tokenade mcp` command). Its
///    absence is OK; its presence is an Info nudge to re-run install.
pub(crate) fn check_mcp_entry(home: &std::path::Path) -> HealthCheckResult {
    let claude_json_path = home.join(".claude.json");
    let cwd = env::current_dir().ok();
    let cwd_str = cwd.as_ref().and_then(|p| p.to_str()).unwrap_or("");
    let project_mcp_json = cwd.as_ref().map(|p| p.join(".mcp.json"));
    let has_tokenade = |servers: Option<&serde_json::Value>| -> bool {
        servers
            .and_then(|m| m.as_object())
            .is_some_and(|m| m.contains_key("tokenade"))
    };
    let claude_json = fs::read_to_string(&claude_json_path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
    let global = claude_json
        .as_ref()
        .map(|v| has_tokenade(v.get("mcpServers")))
        .unwrap_or(false);
    let per_project = !cwd_str.is_empty()
        && claude_json
            .as_ref()
            .and_then(|v| v.get("projects"))
            .and_then(|p| p.get(cwd_str))
            .map(|p| has_tokenade(p.get("mcpServers")))
            .unwrap_or(false);
    let project_local = project_mcp_json
        .as_ref()
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .map(|v| has_tokenade(v.get("mcpServers")))
        .unwrap_or(false);
    let stale_mcp_present = global || per_project || project_local;
    if stale_mcp_present {
        let where_at = if global {
            format!("{} (global)", claude_json_path.display())
        } else if per_project {
            format!("{} (project: {cwd_str})", claude_json_path.display())
        } else {
            project_mcp_json
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "<cwd>/.mcp.json".into())
        };
        hc(
            "mcp_entry",
            CheckStatus::Info,
            format!(
                "stale tokenade MCP entry in {where_at} (spawns the removed `tokenade mcp` \
                 command) — re-run `tokenade install` to purge it; tokenade is hook-first now"
            ),
            vec![],
        )
    } else {
        hc(
            "mcp_entry",
            CheckStatus::Ok,
            "hook-first: no stale tokenade MCP entry (expected — wired via hooks + shell proxy)"
                .into(),
            vec![],
        )
    }
}

/// 6b. Wrapped third-party MCP entries (stale wrapper paths).
pub(crate) fn check_wrapped_mcp_entries() -> HealthCheckResult {
    let entries = tokenade_core::mcp_wrap::inspect_wrapped_entries();
    let current = env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let stale: Vec<_> = entries
        .iter()
        .filter(|e| !current.is_empty() && e.wrapper_command != current)
        .collect();
    if entries.is_empty() {
        hc(
            "wrapped_mcps",
            CheckStatus::Info,
            "no wrapped third-party MCP entries detected".into(),
            vec![],
        )
    } else if stale.is_empty() {
        hc(
            "wrapped_mcps",
            CheckStatus::Ok,
            format!(
                "{} wrapped MCP entry(ies) all point to current binary",
                entries.len()
            ),
            vec![],
        )
    } else {
        let mut detail: Vec<String> = stale
            .iter()
            .take(3)
            .map(|e| format!("- {}::{} → {}", e.config_path, e.name, e.wrapper_command))
            .collect();
        if stale.len() > 3 {
            detail.push(format!("... {} more", stale.len() - 3));
        }
        detail.push("fix: tokenade unwrap-mcps && tokenade install".into());
        hc(
            "wrapped_mcps",
            CheckStatus::Fail,
            format!(
                "{}/{} wrapped MCP entry(ies) point to a stale tokenade path",
                stale.len(),
                entries.len()
            ),
            detail,
        )
    }
}
