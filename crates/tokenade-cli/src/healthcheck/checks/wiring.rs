//! **Is the wiring CONSISTENT with itself?**
//!
//! A registered hook pointing to a missing binary, two hooks that do not name
//! the same executable, a declared MCP server nobody calls, a configuration
//! knob that has gone dead. Each of these states leaves tokenade INSTALLED and
//! inoperative — and that is the worse of the two ways to fail, because
//! nothing says so.

use super::*;

/// Is each installed plugin bridge actually FIRING?
///
/// Hermes has had this since it shipped — installed / loaded / firing, three
/// distinct answers — and the other five bridges had nothing at all. A user on
/// opencode, pi, openclaw, cline or kilo whose bridge never loads sees exactly
/// what a working install looks like: no error, no warning, and a gain number
/// that stays at zero. That is the failure this whole compat chantier is named
/// after — "it works but we cannot compute the gain, so the client thinks it
/// does not work" — and the answer needs no new marker file, because every
/// bridge already stamps `TOKENADE_AGENT=<label>` on the spawn, so the ledger
/// records which one produced a saving.
pub(crate) fn check_plugin_bridges(home: &std::path::Path) -> Vec<HealthCheckResult> {
    let installed: Vec<&'static str> = crate::install_cli::plugin_bridges(home)
        .iter()
        // Hermes has its own, richer check (it can tell "loaded" from
        // "firing" via markers its Python bridge writes).
        .filter(|(label, marker, dir, _)| *label != "hermes" && dir.join(marker).exists())
        .map(|(label, _, _, _)| *label)
        .collect();
    if installed.is_empty() {
        return Vec::new();
    }
    let ledger = home.join(".tokenade").join("gain.jsonl");
    let content = fs::read_to_string(&ledger).unwrap_or_default();
    // The LAST time each bridge produced a saving, not merely whether it ever
    // did — see `bridge_verdict`.
    let mut seen: std::collections::HashMap<String, u64> = Default::default();
    tokenade_core::gain::scan_ledger_with(&content, 0, 0, |row, _| {
        if let Some(a) = row.get("agent").and_then(|v| v.as_str()) {
            if !a.is_empty() {
                let ts = row.get("ts").and_then(|v| v.as_u64()).unwrap_or(0);
                let e = seen.entry(a.to_string()).or_insert(0);
                if ts > *e {
                    *e = ts;
                }
            }
        }
    });
    installed
        .into_iter()
        .map(|label| bridge_verdict("plugin-bridge", label, seen.get(label).copied()))
        .collect()
}

/// How long a bridge may be silent before "firing" stops being true.
///
/// Thirty days, the window the dead-lever check already uses. One number for
/// "recently enough to be believed", not two.
pub(crate) const BRIDGE_SILENT_DAYS: u64 = 30;

/// Is this bridge working — and how do we know?
///
/// Both bridge checks answered in the PRESENT tense from evidence with no time
/// bound. The ledger scan asked whether an `agent` row had EVER been written
/// (`scan_ledger_with(&content, 0, 0, …)`, no window), and the Hermes check
/// asked whether a `hook-fired` marker file EXISTS — a file created once and
/// never removed. So a bridge that stopped loading months ago still read
/// `plugin active — tool-result compaction firing`. Same shape as a check calling agreement
/// on one stale binary healthy: true about the neighbouring question.
///
/// `last_seen` is a unix timestamp in SECONDS, or `None` when the bridge has
/// never been seen working. Two mechanisms, one rule — a ledger row and a
/// marker file's mtime are the same evidence in different clothes, and letting
/// them drift apart is how one of them ends up saying something the other
/// would not.
pub(crate) fn bridge_verdict(
    id: &'static str,
    label: &str,
    last_seen: Option<u64>,
) -> HealthCheckResult {
    let Some(last) = last_seen.filter(|t| *t > 0) else {
        return HealthCheckResult {
            id,
            status: CheckStatus::Info,
            message: format!(
                "{label} plugin installed but no saving was ever recorded for it \
                 — start {label} once; if it stays silent the bridge is not loading \
                 (shell-proxy compaction still applies)"
            ),
            detail: vec![],
        };
    };
    let now = tokenade_core::platform::now_unix_secs();
    // A timestamp ahead of the clock is a clock that moved, not the future.
    let age = now.saturating_sub(last);
    if age > BRIDGE_SILENT_DAYS * 86_400 {
        return HealthCheckResult {
            id,
            status: CheckStatus::Info,
            message: format!(
                "{label} plugin last compacted {} ago — it may have stopped loading \
                 (shell-proxy compaction still applies)",
                crate::diagnostics_cli::human_age(age)
            ),
            detail: vec![],
        };
    }
    HealthCheckResult {
        id,
        status: CheckStatus::Ok,
        message: format!("{label} plugin active — tool-result compaction firing"),
        detail: vec![],
    }
}

pub(crate) fn check_hermes_plugin(home: &std::path::Path) -> Option<HealthCheckResult> {
    // Only speak up when the Hermes plugin is actually installed.
    let plugin_dir = crate::install_cli::hermes_home_dir(home)
        .join("plugins")
        .join("tokenade");
    if !plugin_dir.join("__init__.py").exists() {
        return None;
    }
    let registered = plugin_dir.join("hook-registered").exists();
    let fired = plugin_dir.join("hook-fired").exists();
    Some(if !registered {
        HealthCheckResult {
            id: "hermes-plugin",
            status: CheckStatus::Info,
            message: "Hermes plugin installed but hasn't run yet (start Hermes once)".into(),
            detail: vec![],
        }
    } else if !fired {
        // Registered on a real Hermes start, but the transform hook never
        // fired — this Hermes build likely exposes no tool-result transform
        // point, so tool-result compaction is a silent no-op. The shell
        // proxy still compacts terminal output.
        HealthCheckResult {
            id: "hermes-plugin",
            status: CheckStatus::Info,
            message: "Hermes plugin loaded but its tool-result hook never fired — \
                      this Hermes build may not expose tool-result compaction \
                      (shell-proxy compaction still applies)"
                .into(),
            detail: vec![],
        }
    } else {
        // The marker says the hook fired ONCE; its mtime says when. Same rule
        // as the ledger-backed bridges — see `bridge_verdict`.
        let last = std::fs::metadata(plugin_dir.join("hook-fired"))
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        bridge_verdict("hermes-plugin", "Hermes", last)
    })
}

/// Codex refuses to RUN a hook until its identity is trusted in config.toml.
/// A tokenade hook we installed but that codex still treats as untrusted is a
/// SILENT no-op — no command rewrite, no compaction, zero savings (the field
/// symptom: heavy Codex user, ~0 gain). `tokenade install` now auto-writes the
/// trust state, but a config drift / manual edit / codex key-scheme change
/// could desync it — surface that rather than let it fail silently.
pub(crate) fn check_codex_hook_trust(home: &std::path::Path) -> Option<HealthCheckResult> {
    let codex_home = home.join(".codex");
    if !codex_home.join("hooks.json").exists() {
        return None;
    }
    let states = crate::codex_trust::verify_trust(&codex_home).ok()?;
    if states.is_empty() {
        return None;
    }
    let untrusted = states.iter().filter(|(_, ok)| !ok).count();
    Some(if untrusted == 0 {
        hc(
            "codex-hook-trust",
            CheckStatus::Ok,
            format!(
                "Codex hooks trusted — compaction active ({} hook(s))",
                states.len()
            ),
            vec![],
        )
    } else {
        // FAIL, not Info (2026-08-10). An untrusted hook is not a detail: it is
        // the product not running. The field report that prompted the change —
        // a heavy Codex user with a signed receipt showing 359 520 tokens saved
        // and a dashboard showing nothing — is what an `Info` line buys you.
        // The user does not read it as "tokenade is off"; they read it as
        // "tokenade is broken", and they are nearly right.
        hc(
            "codex-hook-trust",
            CheckStatus::Fail,
            format!(
                "{untrusted}/{} Codex hook(s) NOT trusted — tokenade compacts NOTHING on \
                 Codex until this is fixed, and the dashboard will keep showing 0. \
                 Re-run `tokenade install` to auto-trust (or `/hooks` in Codex)",
                states.len()
            ),
            vec![],
        )
    })
}

/// Every op this product books that is a COMPACTION LEVER, with the name the
/// user sees. Substring match, so a family counts as one lever.
///
/// This list was hand-kept and had fallen a long way behind. Measured
/// 2026-09-01: the ledger writers book **26** distinct op names and this list
/// watched **7** of them. The read hook alone books fourteen, of which four
/// were watched — so `health` reported "content dedup: never fired, possible
/// dead routing" on a machine whose read dedup works fine, because the one that
/// actually fires there books `read_identity_dedup` and nothing was looking for
/// it. A check that exists to find dead routing was blind to four fifths of the
/// product, and it can only ever have been WRONG in the reassuring direction:
/// a genuinely dead lever among the unwatched ones is never reported at all.
///
/// [`LEVERS`] and [`NOT_LEVERS`] together must cover every op literal in the
/// source. `the_lever_table_covers_every_op_the_product_books` enforces it, so
/// adding a lever without adding it here fails the build rather than quietly
/// widening the blind spot again.
pub(crate) const LEVERS: &[(&str, &str)] = &[
    ("read_slice_diff", "re-read diff"),
    ("read_touch_dedup", "touch dedup"),
    ("read_identity_dedup", "identical re-read dedup"),
    // Booked through `DedupSite { … }` rather than a `record(…)` call, which
    // is how both of these escaped the sweep and this table for so long. The
    // note under `check_dead_levers` used to call `read_dedup` "a spelling no
    // current code path books" — it is booked by `tokenade read` and by the
    // Gemini after-tool hook, and it had fired eight times in thirty days on
    // the machine where that note was written.
    ("read_dedup", "`tokenade read` re-read dedup"),
    ("cmd_dedup", "`tokenade wrap` repeated-command dedup"),
    ("read_content_dedup", "content dedup"),
    ("read_range_dedup", "range dedup"),
    ("read_doc_dedup", "document dedup"),
    ("read_image_dedup", "image dedup"),
    ("read_paginated_diff", "paginated re-read diff"),
    ("read_auto_compact", "read auto-compaction"),
    ("read_code_skeleton", "code skeleton view"),
    ("read_sheet_skeleton", "spreadsheet skeleton"),
    ("read_media_summary", "media summary"),
    ("read_binary_extracted", "binary text extraction"),
    ("read_disclose", "read stash"),
    ("exec_dedup", "exec/poll dedup"),
    ("grep_compact", "grep compaction"),
    ("webfetch_compact", "webfetch compaction"),
    ("webfetch_binary_extracted", "webfetch binary extraction"),
    ("web-replay", "web replay"),
    ("websearch_compact", "web-search compaction"),
    ("websearch_gate", "web-search gate"),
    ("gemini_web_compact", "Gemini web compaction"),
    ("session_resume_saved", "session resume"),
    ("disclose:stash", "command-output stash"),
    ("compounding", "compounding estimate"),
    // The LLM proxy and Pi's request bridge fold tool results in the request
    // itself, for agents whose hooks cannot (1.2.0: they booked nothing before).
    ("llm-proxy:fold", "LLM proxy fold"),
    ("fold-payload:fold", "request-bridge fold"),
    // Booked through a variable (`log_code_intel(label, op, …)`), so the
    // source sweep below cannot see these three. Listed by hand, and named
    // here so the next reader knows why they are not in its output.
    ("symbol_find", "code-intel query"),
    ("structure_map", "code-intel map"),
    ("semantic_search", "code-intel semantic search"),
    ("impact_of", "code-intel impact"),
];

// Ops that are NOT levers, and why — so the coverage guard can tell "we
// decided this one does not belong" from "nobody has looked at this one yet".
// A bare deny-list would lose that difference on the first reader.

pub(crate) fn check_dead_levers(home: &std::path::Path) -> HealthCheckResult {
    let mut seen: std::collections::HashSet<&str> = Default::default();
    let cutoff = tokenade_core::platform::now_unix_secs().saturating_sub(30 * 86_400); // ledger ts is unix SECONDS (C8!)
    for f in ["gain.jsonl", "gain_estimated.jsonl"] {
        let Ok(txt) = std::fs::read_to_string(home.join(".tokenade").join(f)) else {
            continue;
        };
        for line in txt.lines() {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if v.get("ts").and_then(|t| t.as_u64()).unwrap_or(u64::MAX) < cutoff {
                continue;
            }
            if let Some(op) = v.get("op").and_then(|o| o.as_str()) {
                for (needle, name) in LEVERS {
                    if op.contains(needle) {
                        seen.insert(name);
                    }
                }
            }
        }
    }
    let dead: Vec<String> = LEVERS
        .iter()
        .filter(|(_, name)| !seen.contains(name))
        .map(|(_, name)| format!("never fired in 30d: {name}"))
        .collect();
    if dead.len() >= LEVERS.len() {
        // Fresh install / idle machine — everything at zero is expected, and
        // "not assessable" is not a pass: the message says so itself, and a
        // green row is read as one.
        return HealthCheckResult {
            id: "dead-levers",
            status: CheckStatus::Info,
            message: "no recent ledger activity — lever liveness not assessable".into(),
            detail: vec![],
        };
    }
    if dead.is_empty() {
        return HealthCheckResult {
            id: "dead-levers",
            status: CheckStatus::Ok,
            message: "all key levers fired within 30 days".into(),
            detail: vec![],
        };
    }
    // Twenty-two silent levers is not twenty-two findings. When most of the
    // table is silent on a ledger that HAS rows, the cause is upstream of any
    // one lever — the hooks are running a different tokenade, the agent's
    // config was replaced, the ledger was rotated. Saying it once, with the
    // check that can name the usual cause, beats printing a wall the reader
    // scrolls past. Measured here 2026-09-01: 22 of 24 silent, and the only
    // read op in the ledger was `read_dedup` — which this table did not watch,
    // because it is booked through a `DedupSite` struct while the completeness
    // sweep knew only the `record(…)` call. The claim that went with that
    // number ("a spelling no current code path books") was wrong: it is the
    // sentence a gap in a completeness guard writes for you.
    let mostly_silent = dead.len() * 3 >= LEVERS.len() * 2;
    // Name a few and count the rest: the list is evidence, not the report.
    const SHOWN: usize = 6;
    let hidden = dead.len().saturating_sub(SHOWN);
    let mut detail: Vec<String> = dead.iter().take(SHOWN).cloned().collect();
    if hidden > 0 {
        detail.push(format!("… and {hidden} more"));
    }
    let message = if mostly_silent {
        detail.push(
            "most of the table at zero points upstream of any one lever — check that every \
             hook runs the same tokenade (the `binary` and `hook_binaries_agree` rows above)"
                .into(),
        );
        format!(
            "{} of {} levers have no rows in 30 days — one cause, not {} findings",
            dead.len(),
            LEVERS.len(),
            dead.len()
        )
    } else {
        format!(
            "{} lever(s) with zero fires in 30 days — possible dead routing \
             (C13): worth checking if your workload should have triggered them",
            dead.len()
        )
    };
    HealthCheckResult {
        id: "dead-levers",
        status: CheckStatus::Info,
        message,
        detail,
    }
}

pub(crate) fn check_hook_integrity(home: &std::path::Path) -> HealthCheckResult {
    let settings = tokenade_core::platform::claude_config_dir_for(home).join("settings.json");
    let Ok(txt) = std::fs::read_to_string(&settings) else {
        return nothing_to_check(
            "hook-integrity",
            "no Claude settings.json — nothing to verify",
        );
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) else {
        return HealthCheckResult {
            id: "hook-integrity",
            status: CheckStatus::Info,
            message: "settings.json unparsable — cannot verify hook integrity".into(),
            detail: vec![],
        };
    };
    let mut bad: Vec<String> = Vec::new();
    let mut examined = 0usize;
    if let Some(hooks) = v.get("hooks").and_then(|h| h.as_object()) {
        for (_ev, groups) in hooks {
            let Some(arr) = groups.as_array() else {
                continue;
            };
            for g in arr {
                let Some(hs) = g.get("hooks").and_then(|h| h.as_array()) else {
                    continue;
                };
                for h in hs {
                    let tagged = h
                        .get("_tag")
                        .and_then(|t| t.as_str())
                        .map(|t| t.starts_with("tokenade"))
                        .unwrap_or(false);
                    let cmd = h.get("command").and_then(|c| c.as_str()).unwrap_or("");
                    if !tagged && !cmd.contains("tokenade") {
                        continue;
                    }
                    examined += 1;
                    // Expected shape: optional `sh -c '…TOKENADE_DEBUG_LOG=… exec <bin> <verb>…'`
                    // or a bare `<bin-or-tokenade> <verb>`. Anything with
                    // extra chained shell (;, &&, |, backticks, $() around
                    // OTHER programs) is drift.
                    let suspicious = cmd.contains('`')
                        || cmd.contains("$(")
                        || cmd.contains("&&")
                        || cmd.contains('|')
                        || cmd.matches(';').count() > 0 && !cmd.starts_with("sh -c");
                    if suspicious {
                        bad.push(format!(
                            "suspicious hook command: {}",
                            &cmd[..cmd.len().min(120)]
                        ));
                        continue;
                    }
                    // Binary existence is check_stale_binary_paths' job
                    // (its tokenizer handles the sh -c quoting correctly);
                    // this check only owns SHAPE drift.
                }
            }
        }
    }
    if examined == 0 {
        nothing_to_check(
            "hook-integrity",
            "no tokenade hook commands installed — no shape to verify",
        )
    } else if bad.is_empty() {
        HealthCheckResult {
            id: "hook-integrity",
            status: CheckStatus::Ok,
            message: "installed hook commands match the expected tokenade shape".into(),
            detail: vec![],
        }
    } else {
        bad.sort();
        bad.dedup();
        HealthCheckResult {
            id: "hook-integrity",
            status: CheckStatus::Fail,
            message: format!(
                "{} tokenade hook command(s) drifted from the installed shape — \
                 re-run `tokenade install` (drift can indicate tampering)",
                bad.len()
            ),
            detail: bad,
        }
    }
}

pub(crate) fn check_unused_mcp_servers(home: &std::path::Path) -> HealthCheckResult {
    let mut servers: Vec<String> = Vec::new();
    if let Ok(txt) = std::fs::read_to_string(home.join(".claude.json")) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) {
            if let Some(m) = v.get("mcpServers").and_then(|m| m.as_object()) {
                servers.extend(m.keys().cloned());
            }
            // per-project servers too
            if let Some(projects) = v.get("projects").and_then(|p| p.as_object()) {
                for (_k, pv) in projects {
                    if let Some(m) = pv.get("mcpServers").and_then(|m| m.as_object()) {
                        servers.extend(m.keys().cloned());
                    }
                }
            }
        }
    }
    servers.sort();
    servers.dedup();
    if servers.is_empty() {
        return HealthCheckResult {
            id: "unused-mcp-servers",
            status: CheckStatus::Ok,
            // Say WHAT was inspected. This check reads `~/.claude.json`
            // only — its job is "you pay for tool definitions you never
            // call", which is a Claude-scope question. Unqualified, the
            // line reads as a statement about the whole machine, and on a
            // host with Codex or Gemini servers it is simply false: the
            // same run reported two of them wrapped, three lines below.
            message: "no MCP servers in ~/.claude.json".into(),
            detail: vec![],
        };
    }
    // Count usage in recent transcripts (cheap scan: substring per line).
    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86_400);
    let mut used: std::collections::HashSet<String> = Default::default();
    let proj_dir = tokenade_core::platform::claude_config_dir_for(home).join("projects");
    if let Ok(rd) = std::fs::read_dir(&proj_dir) {
        for proj in rd.flatten() {
            let Ok(sessions) = std::fs::read_dir(proj.path()) else {
                continue;
            };
            for f in sessions.flatten() {
                let Ok(md) = f.metadata() else { continue };
                if md.modified().map(|m| m < cutoff).unwrap_or(true) {
                    continue;
                }
                let Ok(txt) = std::fs::read_to_string(f.path()) else {
                    continue;
                };
                for srv in &servers {
                    if used.contains(srv) {
                        continue;
                    }
                    if txt.contains(&format!("mcp__{srv}__")) {
                        used.insert(srv.clone());
                    }
                }
            }
        }
    }
    let dead: Vec<&String> = servers.iter().filter(|s| !used.contains(*s)).collect();
    if dead.is_empty() {
        HealthCheckResult {
            id: "unused-mcp-servers",
            status: CheckStatus::Ok,
            message: format!(
                "{} MCP server(s) configured, all used within 30d",
                servers.len()
            ),
            detail: vec![],
        }
    } else {
        HealthCheckResult {
            id: "unused-mcp-servers",
            status: CheckStatus::Info,
            message: format!(
                "{} of {} MCP server(s) had ZERO tool calls in 30 days — each still ships \
                 its full tool manifest into every session's context",
                dead.len(),
                servers.len(),
            ),
            detail: vec![format!(
                "unused: {} — consider removing them, or wrapping via `tokenade mcp-wrap` \
                 for lean schemas",
                dead.iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )],
        }
    }
}

/// Do the hooks all run the SAME tokenade?
///
/// The neighbouring question — "does each path resolve?" — is
/// [`check_stale_binary_paths`], and it was the only one being asked. A machine
/// can have two installs (`cargo install` beside an npm or curl install), both
/// present, both spelled correctly in the config, fifteen months apart in
/// behaviour. Nothing failed; the hooks pointed at the old one just kept doing
/// less. The install path rewrites every hook it owns, so a split like this is
/// the fingerprint of a SECOND install that ran `install` once and then stopped
/// being the one on PATH.
///
/// INFO, not FAIL: a deliberate split is imaginable (a developer pointing one
/// hook at a build), and the fix — re-run `tokenade install` — is one line.
/// What matters is that the state stops being invisible.
pub(crate) fn check_hook_binaries_agree(home: &std::path::Path) -> HealthCheckResult {
    let configs = crate::install_cli::hook_agent_configs(home);
    let mut per_config: Vec<(String, Vec<String>)> = Vec::new();
    for (cfg, _) in &configs {
        let Ok(raw) = fs::read_to_string(cfg) else {
            continue;
        };
        let paths = hook_binary_paths(&raw);
        if !paths.is_empty() {
            per_config.push((cfg.display().to_string(), paths));
        }
    }
    hook_binaries_verdict(
        &per_config,
        tokenade_core::platform::tokenade_on_path()
            .first()
            .map(|p| p.as_path()),
    )
}

/// The verdict, separated from the reading of the files — the same split as
/// [`binary_check_from`], and for the same reason: the inputs are a machine's
/// real config files, which a test cannot conjure without becoming a test of
/// the filesystem instead of a test of the rule.
pub(crate) fn hook_binaries_verdict(
    per_config: &[(String, Vec<String>)],
    path_winner: Option<&std::path::Path>,
) -> HealthCheckResult {
    use std::collections::BTreeMap;
    // Group by IDENTITY, not spelling: `~/.local/bin/tokenade` symlinked to
    // `/opt/tokenade/bin/tokenade` is one install written two ways, and calling
    // that a split would send the user rewriting a working config.
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (cfg, paths) in per_config {
        for p in paths {
            let id = std::path::Path::new(p)
                .canonicalize()
                .map(|c| c.display().to_string())
                .unwrap_or_else(|_| p.clone());
            groups.entry(id).or_default().push(cfg.clone());
        }
    }
    // Nothing to agree ABOUT. Zero hooks fell into the branch below and came
    // back `every hook runs the same tokenade binary` — true of an empty set,
    // and printed on a machine where tokenade was compacting nothing at all.
    // The comment three lines down already tells this story once, about a
    // single stale binary; the same trap has an emptier bottom. An answer
    // about no hooks belongs with the check that says whether any are
    // INSTALLED, not here.
    if groups.is_empty() {
        return nothing_to_check(
            "hook_binaries_agree",
            "no tokenade hooks in any agent config — nothing to compare",
        );
    }
    if groups.len() <= 1 {
        // Agreeing is only half the question. The first version of this check
        // answered OK for ANY single binary — so a machine whose every hook
        // pointed at one STALE tokenade, which is the split-binary state
        // before it happened to be a split, got `every hook runs the same
        // tokenade binary`: true, and silent in exactly the case the check
        // exists for. Ask the second half too: is the one they agree on the one
        // a bare `tokenade` runs?
        let agreed = groups.keys().next().cloned();
        if let (Some(agreed), Some(winner)) = (agreed.as_deref(), path_winner) {
            let same = std::path::Path::new(agreed)
                .canonicalize()
                .map(|c| tokenade_core::platform::same_file(&c, winner))
                .unwrap_or(false);
            if !same {
                return hc(
                    "hook_binaries_agree",
                    CheckStatus::Info,
                    "every hook runs the same tokenade — and it is not the one on your PATH".into(),
                    vec![
                        format!("hooks run: {agreed}"),
                        format!("a bare `tokenade` runs: {}", winner.display()),
                        "an install that is never the one you type keeps compacting with \
                         whatever it was when the hooks were written; `tokenade install` \
                         rewrites them to this binary"
                            .into(),
                    ],
                );
            }
        }
        return hc(
            "hook_binaries_agree",
            CheckStatus::Ok,
            "every hook runs the same tokenade binary".into(),
            vec![],
        );
    }
    // The paths are de-duplicated per config, so counting them would report
    // spellings and read as a count of hooks. Name the config files instead:
    // that is the file the user has to open.
    let mut detail: Vec<String> = Vec::new();
    for (id, uses) in &groups {
        let mut files = uses.clone();
        files.sort();
        files.dedup();
        detail.push(format!("{} — used by {}", id, files.join(", ")));
    }
    detail.push(
        "re-run `tokenade install` to point every hook at one binary; hooks on an older \
         install do not fail, they silently compact less"
            .into(),
    );
    hc(
        "hook_binaries_agree",
        CheckStatus::Info,
        format!(
            "hooks are split across {} different tokenade binaries",
            groups.len()
        ),
        detail,
    )
}

pub(crate) fn check_stale_binary_paths(home: &std::path::Path) -> HealthCheckResult {
    let current = std::env::current_exe()
        .ok()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    // The agent list lives in ONE place (`install_cli::hook_agent_configs`);
    // this was a private fourth copy of it and it had already lost Reasonix, so
    // a Reasonix user whose binary moved got no warning about the exact failure
    // this check exists to name.
    //
    // The EXTRACTION was a private second copy too, and that one was worse than
    // stale: it took the first whitespace token of the command as the exe path.
    // Every command tokenade installs begins `sh -c '…'`, so the token it
    // examined was `sh`, which contains no "tokenade" and was skipped — the
    // check that exists to catch "hooks silently dead" was silently dead
    // itself, and said `all agent-config hook paths resolve` three lines above
    // the check that had found the missing binary. `stale_hook_binary_paths`
    // (top of this file) reads the shape the installer actually writes.
    let configs = crate::install_cli::hook_agent_configs(home);
    let mut stale: Vec<String> = Vec::new();
    // `hook_agent_configs` names where each agent WOULD keep its config, so
    // the list is never empty and its length says nothing about this machine.
    // Count the ones that exist and were read: that is the set this check has
    // an opinion about.
    let mut examined = 0usize;
    for (cfg, _) in &configs {
        let Ok(raw) = fs::read_to_string(cfg) else {
            continue;
        };
        examined += hook_binary_paths(&raw).len();
        for missing in stale_hook_binary_paths(&raw) {
            stale.push(format!("{}: dead exe path {missing}", cfg.display()));
        }
    }
    stale.sort();
    stale.dedup();
    if examined == 0 {
        nothing_to_check(
            "binary_paths",
            "no tokenade hook paths in any agent config — none to resolve",
        )
    } else if stale.is_empty() {
        hc(
            "binary_paths",
            CheckStatus::Ok,
            "all agent-config hook paths resolve".into(),
            vec![],
        )
    } else {
        let n = stale.len();
        let mut detail = stale;
        detail.push(format!(
            "fix: re-run `tokenade install` to rewrite paths (current exe: {current}), \
             or symlink the dead path -> $(which tokenade)"
        ));
        hc(
            "binary_paths",
            CheckStatus::Fail,
            format!("{n} config(s) point at a MISSING tokenade binary — hooks silently dead"),
            detail,
        )
    }
}
