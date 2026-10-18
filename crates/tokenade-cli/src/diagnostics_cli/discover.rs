//! **What tokenade COULD have folded, and did not.**
//!
//! We walk the agent's logs to put a number on missed opportunities. The only
//! honest number is the one that compares against what ACTUALLY went through
//! — hence the time window, the exclusion of side chains, and the census of
//! traffic per tool.

use super::*;

/// Human-readable duration (`3m`, `2h`, `4d`). Used by hooks status.
pub(crate) fn human_age(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

/// Walk the agent log tree and tally, per tool, the bytes and calls it spent.
///
/// 60 lines of directory walking lifted out of `cmd_discover`, which is about
/// REPORTING what the walk found.
/// Traffic tokenade is NOT compacting today, most-used first (at most five).
///
/// The named tools are opportunities only while the hook is ABSENT — the
/// installed matcher covers `Bash|Read|Grep|WebFetch|WebSearch|Task|Agent|
/// Write|Edit`. The text path applied that condition and the JSON did not, so
/// `--json` reported `Bash` (95 326 calls, 202 MB) as an opportunity beside
/// `hook_installed: true`, and a machine reading it would size a saving that
/// had already been taken.
///
/// An `mcp__` tool is never covered: a hook matcher does not see
/// `mcp__<server>__<tool>`, which is what `tokenade mcp-wrap install` exists
/// for. It stays an opportunity whatever the hook's state — except our own
/// server, which compacts itself.
pub(crate) fn discover_opportunities(
    rows: &[(String, ToolTraffic)],
    hook_installed: bool,
) -> Vec<(String, ToolTraffic)> {
    const OURS: [&str; 3] = ["mcp__tokenade__", "exec_compact", "auto_compact"];
    rows.iter()
        .filter(|(name, _)| !OURS.iter().any(|p| name.starts_with(p)))
        .filter(|(name, _)| {
            if name.starts_with("mcp__") {
                return true;
            }
            !hook_installed
                && matches!(
                    name.as_str(),
                    "Bash" | "Read" | "Grep" | "Glob" | "WebFetch"
                )
        })
        .take(5)
        .map(|(n, t)| (n.clone(), *t))
        .collect()
}

/// Is this transcript entry inside the reporting window?
///
/// `true` when it has no parseable timestamp: never drop on missing evidence.
pub(crate) fn entry_in_window(v: &serde_json::Value, cutoff_secs: u64) -> bool {
    let Some(ts) = v.get("timestamp").and_then(|t| t.as_str()) else {
        return true;
    };
    match tokenade_core::claudecode_transcript::iso8601_to_epoch_ms(ts) {
        Some(ms) => ms / 1000 >= cutoff_secs,
        None => true,
    }
}

/// Channels whose bytes come back mostly inside SUBAGENTS.
///
/// Two conditions, and both are needed. More than half the bytes in sidechains
/// is what makes the main-session view unrepresentative of the channel. A floor
/// of 1 MiB is what stops a tool with three subagent calls and nothing else
/// from being reported as a finding: a ratio on a tiny denominator is noise.
///
/// Extracted rather than written inline because a predicate buried in a
/// hundred-line renderer is a predicate no test can reach.
pub(crate) fn sidechain_heavy(rows: &[(String, ToolTraffic)]) -> Vec<(String, ToolTraffic)> {
    const FLOOR_BYTES: u64 = 1024 * 1024;
    rows.iter()
        .filter(|(_, t)| t.side_bytes >= FLOOR_BYTES && t.side_bytes * 2 > t.bytes)
        .map(|(n, t)| (n.clone(), *t))
        .collect()
}

/// Bytes and calls for one tool, split by WHERE the exchange happened.
///
/// A subagent's transcript is a separate JSONL under `<session>/subagents/`,
/// and every one of its entries declares `"isSidechain": true` — the host's own
/// word for it, which is why that is what this reads rather than the path.
///
/// The split is not decoration. Summed, the `Read` row on this machine reads
/// "4 345 calls, 28.7 MB" and looks like one healthy channel; split, it is
/// 1 747 calls returning 1.1 MB in the main session and 2 598 returning
/// **27.6 MB** inside subagents — 616 bytes per read against 10 633, a factor
/// of 17. The command exists to answer "where would tokenade help me most",
/// and merging the two hid the answer inside the row that should have carried
/// it.
#[derive(Default, Clone, Copy)]
pub(crate) struct ToolTraffic {
    pub(crate) bytes: u64,
    pub(crate) calls: u64,
    /// The part of `bytes`/`calls` above that came from a sidechain. A SUBSET,
    /// never a second bucket: a reader who ignores these two still sees the
    /// true total.
    pub(crate) side_bytes: u64,
    pub(crate) side_calls: u64,
}

pub(crate) fn scan_agent_logs(
    walker: &mut Vec<std::path::PathBuf>,
    by_tool: &mut std::collections::BTreeMap<String, ToolTraffic>,
    sessions_scanned: &mut u64,
    cutoff_secs: u64,
) {
    while let Some(dir) = walker.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walker.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(md) = entry.metadata() else {
                continue;
            };
            let mtime = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            if mtime < cutoff_secs {
                continue;
            }
            let Ok(content) = fs::read_to_string(&path) else {
                continue;
            };
            // TWO passes over the same session, because a result carries no
            // tool name — only the `tool_use_id` of the call that produced it.
            //
            // The single pass this replaces matched the first content block
            // carrying a `name`, which only ever exists on a `tool_use`, and
            // charged it `line.len()` — the REQUEST. On the command whose job
            // is "where would tokenade help me most", that is the wrong side
            // of the exchange: tokenade compacts what comes BACK.
            //
            // Measured over this machine's last 30 days, the two answers do
            // not merely differ in size, they reorder the recommendation:
            //   Edit   38.3 MB requested vs  3.0 MB returned (0.08x)
            //   Write  16.2 MB requested vs  0.5 MB returned (0.03x)
            //   Read   13.5 MB requested vs 30.0 MB returned (2.22x)
            // so the old table ranked Edit and Write second and third — the
            // two tools whose bytes the agent WROTE and no compactor can ever
            // touch — while Read, second in reality, sat fourth.
            let mut id_to_name: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();
            let mut lines: Vec<serde_json::Value> = Vec::new();
            // The host stamps every entry of a subagent transcript. Read the
            // declaration, not the file name: a layout change renames files,
            // it does not change what the entries say about themselves.
            // Measured over 547 sessions, the two agree on every file.
            let mut is_sidechain = false;
            // Names are collected, not counted, because the flag above may be
            // read from a line AFTER the first call: attributing as we go would
            // charge a subagent's opening calls to the main session.
            let mut calls_here: Vec<String> = Vec::new();
            // The window bounds the ENTRIES, not just the file.
            //
            // The mtime check above is a cheap skip — a file untouched for the
            // window can hold nothing inside it — but it is not the window.
            // A session opened in June and appended to this morning passes it,
            // and every June entry in it was then counted under a header that
            // says "last 30 days". Measured on this machine: 6 of the 547 files
            // straddle the boundary, and they carried 44 574 tool results worth
            // **25.5 MB — 21 % of everything the report attributed to the
            // window**.
            //
            // A line with no parseable timestamp is KEPT. Every Claude Code
            // entry here carries one (127 245 of 127 245), but another agent's
            // transcript dialect may not, and dropping on missing evidence
            // would silently report zero rather than too much.
            let mut contributed = false;
            for line in content.lines() {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                    continue;
                };
                let in_window = entry_in_window(&v, cutoff_secs);
                if in_window {
                    contributed = true;
                }
                if let Some(items) = v
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                {
                    for item in items {
                        if item.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
                            continue;
                        }
                        let (Some(id), Some(name)) = (
                            item.get("id").and_then(|i| i.as_str()),
                            item.get("name").and_then(|n| n.as_str()),
                        ) else {
                            continue;
                        };
                        // The NAME is learned from every line, in or out of the
                        // window: a call made just before the cutoff whose
                        // result lands just after it would otherwise become an
                        // orphan and lose its bytes.
                        id_to_name.insert(id.to_string(), name.to_string());
                        // One call, counted where it was made — and only if the
                        // window covers it. Counting the call but not its bytes
                        // would put a call count and a byte count from two
                        // different spans in the same row.
                        if in_window {
                            calls_here.push(name.to_string());
                        }
                    }
                }
                if v.get("isSidechain").and_then(|b| b.as_bool()) == Some(true) {
                    is_sidechain = true;
                }
                lines.push(v);
            }
            if contributed {
                *sessions_scanned += 1;
            }
            for name in calls_here {
                let e = by_tool.entry(name).or_default();
                e.calls += 1;
                if is_sidechain {
                    e.side_calls += 1;
                }
            }
            for v in &lines {
                if !entry_in_window(v, cutoff_secs) {
                    continue;
                }
                let Some(items) = v
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                else {
                    continue;
                };
                for item in items {
                    if item.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
                        continue;
                    }
                    let Some(name) = item
                        .get("tool_use_id")
                        .and_then(|i| i.as_str())
                        .and_then(|i| id_to_name.get(i))
                    else {
                        continue;
                    };
                    let bytes = match item.get("content") {
                        Some(serde_json::Value::String(t)) => t.len() as u64,
                        Some(serde_json::Value::Array(a)) => a
                            .iter()
                            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                            .map(|t| t.len() as u64)
                            .sum(),
                        _ => 0,
                    };
                    let e = by_tool.entry(name.clone()).or_default();
                    e.bytes += bytes;
                    if is_sidechain {
                        e.side_bytes += bytes;
                    }
                }
            }
        }
    }
}

/// `tokenade discover` — scan `~/.claude/projects/*/*.jsonl` for tool
/// calls and recommend MCP servers / commands that would benefit most
/// from tokenade compaction. Ported from mcp-rtk's `discover`
/// subcommand pattern.
///
/// The heuristic: count distinct tool names, sum their byte volume
/// (approximated via the JSONL line bytes), and surface the top
/// ten that AREN'T already routed through tokenade. Output is a
/// short markdown report on stdout — useful as the first thing a new
/// user runs to see "where would tokenade help me most".
/// The epoch second before which a session entry is out of the `--days` window.
///
/// Both operations saturate, and `--days` being unbounded user input is why.
/// `days * 86_400` overflowed u64 above ~2.1e14 days: the debug build PANICKED
/// (and the process still exited 0 with an empty stdout, so a `--json` caller
/// got a parse error and a success code in the same breath), while release
/// WRAPPED — a window meaning "all of history" silently became an arbitrary
/// short one, and the command answered with a different set of opportunities
/// than its own default. A window wider than history is all of history, which
/// is cutoff 0.
pub(crate) fn cutoff_for_days(now_secs: u64, days: u64) -> u64 {
    now_secs.saturating_sub(days.saturating_mul(86_400))
}

pub(crate) fn cmd_discover(rest: &[String]) -> i32 {
    // Silent-accept sweep (#813 extension): reject unknown flags +
    // extra positionals up front. Same class as the cmd_execute fix.
    const DISCOVER_BOOL_FLAGS: &[&str] = &["--json"];
    const DISCOVER_VALUE_FLAGS: &[&str] = &["--days"];
    {
        let mut i = 0;
        while i < rest.len() {
            let a = &rest[i];
            if a.starts_with("--") || (a.starts_with('-') && a.len() > 1 && a != "-") {
                if DISCOVER_VALUE_FLAGS.contains(&a.as_str()) {
                    i += 2;
                    continue;
                }
                if !DISCOVER_BOOL_FLAGS.contains(&a.as_str()) {
                    eprintln!(
                        "tokenade discover: unknown flag '{a}'. Try `tokenade discover --help`."
                    );
                    return 2;
                }
            } else {
                eprintln!(
                    "tokenade discover: unexpected positional '{a}' (this command takes no positionals)."
                );
                return 2;
            }
            i += 1;
        }
    }

    // #643 — surface invalid --days values instead of silently using
    // the default 30. Same class as #636 (hooks tail) and #642
    // (execute --timeout / --cap).
    let days = match rest
        .iter()
        .position(|a| a == "--days")
        .and_then(|i| rest.get(i + 1))
    {
        Some(raw) => match raw.parse::<u64>() {
            Ok(n) => n,
            Err(_) => {
                eprintln!("tokenade discover: --days '{raw}' is not a non-negative integer.");
                return 2;
            }
        },
        None => 30,
    };
    // #619 — `--json` mirrors the pattern from #613 (healthcheck),
    // #616 (gain) and #617 (dashboard). CI consumers want a stable
    // shape to track tool-traffic over time; the markdown table is
    // a dead-end for them.
    let json_mode = rest.iter().any(|a| a == "--json");
    let Some(home) = tokenade_core::platform::home_dir() else {
        eprintln!("tokenade discover: HOME not resolvable");
        return 1;
    };
    let logs_dir = tokenade_core::platform::claude_config_dir_for(&home).join("projects");
    if !logs_dir.exists() {
        // `--json` is a CONTRACT: the caller is a program, and a program that
        // asked for JSON and got an empty stdout gets a parse error, not an
        // empty result. The human line stays on stderr for the human case.
        if json_mode {
            println!(
                "{}",
                serde_json::json!({
                    "scanned": false,
                    "logs_dir": logs_dir.display().to_string(),
                    "tools": [],
                    "note": "no Claude project logs found — nothing to scan",
                })
            );
        } else {
            eprintln!(
                "tokenade discover: no Claude project logs at {} — nothing to scan.",
                logs_dir.display()
            );
        }
        return 0;
    }
    let cutoff_secs = cutoff_for_days(tokenade_core::platform::now_unix_secs(), days);

    // For each session JSONL, count `(tool_name, total_bytes,
    // total_calls)`. Only counts entries whose mtime is within the
    // window — keeps the scan fast on long-lived `~/.claude/` dirs.
    use std::collections::BTreeMap;
    let mut by_tool: BTreeMap<String, ToolTraffic> = BTreeMap::new();
    let mut sessions_scanned = 0u64;
    let mut walker = vec![logs_dir.clone()];
    scan_agent_logs(
        &mut walker,
        &mut by_tool,
        &mut sessions_scanned,
        cutoff_secs,
    );
    // Sort by total bytes desc — needed by both renderers.
    let mut rows: Vec<(String, ToolTraffic)> = by_tool.into_iter().collect();
    rows.sort_by_key(|(_, t)| std::cmp::Reverse(t.bytes));

    // Hook-installed probe used by both renderers (text uses it to
    // pick which suggestion block to print; JSON exposes it raw so
    // a CI script can derive the same recommendation).
    // Preferred marker `_tag: "tokenade"` (#604) with hookwrap-command
    // fallback (#774) for installs where the tag was dropped.
    let hook_already_installed = crate::install_cli::any_agent_has_tokenade().unwrap_or(false);
    // #974 — refine the "hook installed" branch's next-step suggestions
    // by checking whether the global CLAUDE.md scaffold is also already
    // installed, so we don't re-suggest what's done. `pretool_hook_installed`
    // below is now just a `--json` signal (the hook is installed by default
    // since the 2026-06 pivot; the old LSP-enforcement suggestion was dropped).
    let lsp_hook_already_installed =
        tokenade_core::platform::claude_settings_has_lsp_enforcement_hook().unwrap_or(false);
    let claude_md_already_installed =
        tokenade_core::platform::global_claude_md_has_tokenade_block().unwrap_or(false);

    let opportunities = discover_opportunities(&rows, hook_already_installed);

    if json_mode {
        let top_tools: Vec<serde_json::Value> = rows
            .iter()
            .take(15)
            .map(|(name, t)| {
                serde_json::json!({
                    "tool": name,
                    "calls": t.calls,
                    "bytes": t.bytes,
                    // A SUBSET of the two above, never a second bucket — a
                    // reader that ignores these still sees the true total.
                    "sidechain_calls": t.side_calls,
                    "sidechain_bytes": t.side_bytes,
                })
            })
            .collect();
        let opps: Vec<serde_json::Value> = opportunities
            .iter()
            .map(|(name, t)| {
                serde_json::json!({
                    "tool": name,
                    "calls": t.calls,
                    "bytes": t.bytes,
                    "sidechain_calls": t.side_calls,
                    "sidechain_bytes": t.side_bytes,
                })
            })
            .collect();
        let root = serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "sessions_scanned": sessions_scanned,
            "days": days,
            "logs_dir": logs_dir.display().to_string(),
            "hook_installed": hook_already_installed,
            "lsp_enforcement_installed": lsp_hook_already_installed,
            "claude_md_installed": claude_md_already_installed,
            "top_tools": top_tools,
            "opportunities": opps,
        });
        println!("{}", serde_json::to_string_pretty(&root).unwrap());
        return 0;
    }

    if sessions_scanned == 0 {
        println!(
            "no sessions found under {} within the last {} days.",
            logs_dir.display(),
            days
        );
        return 0;
    }
    println!(
        "# tokenade discover — {} session(s) scanned, last {} days\n",
        sessions_scanned, days
    );
    // Name the side of the exchange. "traffic" read as either one, and the
    // column that filled it was the request; a reader sizing where to save
    // needs to know these are the bytes that came BACK, because those are the
    // ones a compactor stands between.
    println!("## Top tools by bytes returned to the agent\n");
    // Name the column before the table, not after: "in subagents" is a SHARE of
    // the row's own bytes, not a separate total to be added to it.
    println!(
        "`in subagents` is the share of that row's bytes returned inside a \
         sidechain — a delegated agent with its own context.\n"
    );
    // #506 — compute the tool-name column width dynamically from the
    // actual rows. The previous fixed `{:<24}` overflowed on long
    // MCP names (e.g. `mcp__plugin_playwright_playwright__browser_
    // navigate` = 52 chars), pushing the `|` divider out of alignment
    // with shorter rows and breaking markdown table rendering. Min
    // 4 to keep "tool" header readable; max bounded only by data.
    let name_w = rows
        .iter()
        .take(15)
        .map(|(n, _)| n.chars().count())
        .max()
        .unwrap_or(4)
        .max(4);
    let header_pad = "-".repeat(name_w);
    println!(
        "| {:<name_w$} | calls | bytes returned | in subagents |",
        "tool"
    );
    println!(
        "|{:-<w$}|-------|----------------|--------------|",
        header_pad,
        w = name_w + 2
    );
    for (name, t) in rows.iter().take(15) {
        // The subagent column is a SHARE of the row, not an extra row: a
        // percentage says that without arithmetic, and 0 % prints as `-` so the
        // eye lands on the channels where it is not.
        // The percentage alone: the exact split is in `--json`, and a cell
        // wide enough for both numbers pushes the divider out of alignment on
        // the long MCP names this table already had to widen for.
        let share = match t.bytes {
            0 => String::from("-"),
            b => match t.side_bytes.saturating_mul(100) / b {
                0 => String::from("-"),
                pct => format!("{pct}%"),
            },
        };
        println!(
            "| {:<name_w$} | {:>5} | {:>14} | {:>12} |",
            name, t.calls, t.bytes, share
        );
    }
    println!();
    // The section still prints for a fully wired install: tightening the
    // opportunity list must not take away the advice that does not depend on
    // it.
    if hook_already_installed || !opportunities.is_empty() {
        // Hook-installed probe + opportunities computed above so the
        // JSON path (#619) and the text path agree. The check uses the
        // robust 2-way marker from #774 (preferred `_tag`, fallback to
        // hookwrap-command substring) so installs that lost their `_tag`
        // are still recognised.
        println!("## Suggested next step\n");
        // A channel whose bytes are mostly returned inside subagents is not
        // addressed by anything in the two branches below: the hook state is
        // the same for both, and what differs is the agent doing the reading.
        // Printed FIRST because it is the largest single finding this command
        // can make and it used to be invisible — the two halves were summed
        // into one row.
        let heavy = sidechain_heavy(&rows);
        for (name, t) in &heavy {
            let pct = t.side_bytes.saturating_mul(100) / t.bytes.max(1);
            let per_side = t.side_bytes / t.side_calls.max(1);
            let main_calls = t.calls.saturating_sub(t.side_calls);
            let per_main = t.bytes.saturating_sub(t.side_bytes) / main_calls.max(1);
            println!(
                "- **{name}: {pct}% of the bytes came back inside subagents** \
                 — {per_side} B per subagent call against {per_main} B in the main session."
            );
        }
        if !heavy.is_empty() {
            // What the split does NOT mean, said before a reader acts on it.
            //
            // A deliberate `Read` under `READ_DEFAULT_THRESHOLD_TOKENS`
            // (~25 000) is served WHOLE on purpose: the agent asked for that
            // file, and re-fetching it costs more than carrying it. The doubt
            // was whether that reasoning survives in a sidechain, where the
            // context is thrown away at the end of the task — so it was
            // measured rather than assumed. Over 478 subagent reads of 20 KB or
            // more on this machine: a median of 27 assistant turns FOLLOW the
            // read inside the same subagent, none has 2 or fewer, and 16 % read
            // the same file again. The file is carried through cached turns
            // exactly as the threshold's note describes.
            //
            // So this is a place to LOOK, not a leak to plug, and the line says
            // so. A finding that reads as waste sends a reader to tune a
            // threshold that a benchmark already set.
            println!(
                "  A deliberate Read under ~25 000 tokens is served whole on purpose — \
                 measured here, a big subagent read is followed by a median of 27 more \
                 turns that re-reference it from cache, so this is where to look rather \
                 than a leak to plug. It matters when a subagent reads and exits: the \
                 levers there are the brief tokenade prepends to every delegation \
                 (`tokenade subagent-brief`) and the commands it names — \
                 `tokenade skeleton <file>`, `tokenade read <file> --prompt \"…\"`, \
                 `tokenade web <url> --prompt \"…\"`.\n"
            );
        }
        if hook_already_installed {
            // #974 — gate each opt-in suggestion on whether it's
            // already installed. Without this the same two recs fire
            // even when the user has both wired — confusing dead end
            // for someone reading discover to learn "what's left".
            println!(
                "Your Claude Code Bash hook is already wired — these byte counts \
                 already include tokenade's compaction. To get more savings:\n"
            );
            println!("- `tokenade dashboard` — see how much you've actually saved so far");
            // (No `--with-lsp-enforcement-hook` suggestion: that flag is a
            // deprecated no-op — the PreToolUse hook is installed by default.)
            if !claude_md_already_installed {
                println!(
                    "- `tokenade install --with-claude-md` — scaffold per-project rules \
                     that nudge agents toward MCP tools first"
                );
            }
            // What the hook genuinely cannot reach. A matcher never sees
            // `mcp__<server>__<tool>`, so this traffic is uncompacted however
            // well the Bash hook is wired — and saying "everything else is
            // already wired" over the top of it is how the list computed just
            // above went unread.
            if !opportunities.is_empty() {
                println!(
                    "- `tokenade mcp-wrap install` — hook matchers never see \
                     `mcp__<server>__<tool>`, so these are still uncompacted:"
                );
                for (name, t) in &opportunities {
                    println!(
                        "    - **{name}**: {} calls, ~{} bytes total",
                        t.calls, t.bytes
                    );
                }
            }
            if lsp_hook_already_installed && claude_md_already_installed && opportunities.is_empty()
            {
                println!(
                    "- everything else is already wired — `tokenade healthcheck` confirms \
                     the install state"
                );
            }
        } else {
            println!(
                "These tools are heavy-traffic but not routed through tokenade — \
                 installing the tokenade Bash hook + MCP server would compress \
                 most of their output:\n"
            );
            for (name, t) in &opportunities {
                println!("- **{name}**: {} calls, ~{} bytes total", t.calls, t.bytes);
            }
            println!("\nRun `tokenade install` to wire the hooks + MCP server in one shot.");
        }
    }
    0
}
