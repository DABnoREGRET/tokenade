//! **Are the hooks there, and what are they doing?**
//!
//! The real state, read from the configuration files rather than from a log,
//! and the tail of the hook log. It is the first thing asked when tokenade
//! "does nothing" — and the answer must tell "not installed" from "installed
//! and declined".

use super::*;

/// `tokenade hooks status|tail` — runtime visibility into the hooks
/// we wired during `install`. `status` enumerates each
/// settings.json hook entry that mentions tokenade + reports the
/// last-fire timestamp by grepping `~/.tokenade/debug.log`. `tail`
/// streams the debug log line-by-line so the user can watch hooks
/// fire live without remembering the filename.
/// The tokenade subcommand a hook command runs — the one interesting token in
/// a shell-quoted line.
///
/// NOT the last token: the installer writes
/// `sh -c '… tokenade hookwrap 2>/dev/null; exit 0'`, whose last token is
/// `0'`, so `tokenade hooks status` rendered `→ 0` on every row. A bare zero
/// beside a hook reads as "fired zero times" — a broken install — on a
/// perfectly wired machine. Match the subcommand by NAME; keep the old
/// last-token guess as a fallback for a shape we have not seen.
pub(crate) fn hook_variant_of(cmd: &str) -> String {
    cmd.split_whitespace()
        .map(|t| t.trim_matches(|c| c == '\'' || c == '"'))
        .find(|t| t.starts_with("hookwrap") || matches!(*t, "pre-edit-snapshot" | "index-touch"))
        .map(str::to_string)
        .unwrap_or_else(|| {
            cmd.split_whitespace()
                .next_back()
                .unwrap_or(cmd)
                .trim_end_matches('\'')
                .to_string()
        })
}

pub(crate) fn cmd_hooks(rest: &[String]) -> i32 {
    // #626 — `--json` flag can appear anywhere in rest (e.g.
    // `tokenade hooks --json` or `tokenade hooks status --json`).
    // Strip ONLY `--json` from the positional list (#627 — earlier
    // filter `starts_with("--")` also ate `--help`, breaking the
    // subcommand-help branch below).
    let json_mode = rest.iter().any(|a| a == "--json");
    let positional: Vec<&str> = rest
        .iter()
        .map(String::as_str)
        .filter(|a| *a != "--json")
        .collect();
    let sub = positional.first().copied().unwrap_or("status");
    match sub {
        "status" => {
            // #786 — pre-fix `hooks status --bogus` and
            // `hooks status extra-pos` silently dispatched to status
            // and dropped the extra arg. Reject anything past the
            // subcommand name (status takes only `--json`, already
            // stripped above).
            if let Some(extra) = positional.get(1) {
                eprintln!(
                    "tokenade hooks status: unexpected argument '{extra}' \
                     (this subcommand takes only `--json`; \
                     try `tokenade hooks --help`)."
                );
                return 2;
            }
            cmd_hooks_status(json_mode)
        }
        // #636 — surface an error on non-integer N instead of silently
        // falling back to the default. `tokenade hooks tail 200x` or
        // `tokenade hooks tail "200 lines"` used to swallow the typo
        // and behave like `tokenade hooks tail` — confusing.
        "tail" => {
            // #786 — also reject extra args past `N` (e.g.
            // `hooks tail 5 extra` used to silently ignore "extra").
            if let Some(extra) = positional.get(2) {
                eprintln!(
                    "tokenade hooks tail: unexpected argument '{extra}' \
                     (this subcommand takes at most one positional `[N]`; \
                     try `tokenade hooks --help`)."
                );
                return 2;
            }
            match positional.get(1) {
                Some(raw) => match raw.parse::<usize>() {
                    Ok(n) => cmd_hooks_tail(Some(n), json_mode),
                    Err(_) => {
                        eprintln!(
                            "tokenade hooks tail: '{raw}' is not a non-negative integer. \
                             Usage: `tokenade hooks tail [N]` (default N=20)."
                        );
                        2
                    }
                },
                None => cmd_hooks_tail(None, json_mode),
            }
        }
        "--help" | "-h" | "help" => {
            println!("usage: tokenade hooks <subcommand> [--json]");
            println!();
            println!("Subcommands:");
            println!(
                "  status         show every installed Claude Code hook with its matcher + command"
            );
            println!(
                "  tail [N]       show the last N hook-fire records from ~/.tokenade/debug.log"
            );
            println!("                 (default N = {HOOKS_TAIL_DEFAULT})");
            println!();
            println!("Default (no subcommand): same as `status`.");
            println!();
            println!("Flags:");
            println!("  --json   emit a structured JSON object:");
            println!("           hooks[] (event, matcher, variant) +");
            println!("           recent_activity[] (component, count, last_ts_*)");
            0
        }
        other => {
            eprintln!(
                "tokenade hooks: unknown subcommand '{other}'. \
                 Try `status`, `tail [N]`, or `--help`."
            );
            2
        }
    }
}

/// #626 — one installed tokenade hook entry; populated from
/// `~/.claude/settings.json` and used by both the text and JSON
/// renderers below.
pub(crate) struct InstalledHookRow {
    event: String,
    matcher: String,
    variant: String,
}

pub(crate) fn cmd_hooks_status(json_mode: bool) -> i32 {
    use std::io::IsTerminal;
    let color =
        !json_mode && std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    let green = if color { "\x1b[32m" } else { "" };
    let dim = if color { "\x1b[2m" } else { "" };
    let rst = if color { "\x1b[0m" } else { "" };
    let Some(home) = tokenade_core::platform::home_dir() else {
        eprintln!("tokenade hooks: HOME unresolvable");
        return 1;
    };
    let settings_path = tokenade_core::platform::claude_config_dir_for(&home).join("settings.json");
    if !settings_path.exists() {
        if json_mode {
            let payload = serde_json::json!({
                "version": env!("CARGO_PKG_VERSION"),
                "settings_path": settings_path.display().to_string(),
                "settings_present": false,
                "hooks": [],
                "recent_activity": [],
            });
            println!("{}", serde_json::to_string_pretty(&payload).unwrap());
        } else {
            println!(
                "no Claude Code settings at {} — run `tokenade install` first.",
                settings_path.display()
            );
        }
        return 0;
    }
    let Ok(content) = fs::read_to_string(&settings_path) else {
        eprintln!("tokenade hooks: cannot read {}", settings_path.display());
        return 1;
    };
    let Ok(v): serde_json::Result<serde_json::Value> = serde_json::from_str(&content) else {
        eprintln!(
            "tokenade hooks: {} isn't valid JSON",
            settings_path.display()
        );
        return 1;
    };

    // Collect once; render twice. Same data flows into both the
    // ASCII tree and the JSON payload so they stay in lockstep.
    let mut rows: Vec<InstalledHookRow> = Vec::new();
    if let Some(hooks_obj) = v.get("hooks").and_then(|h| h.as_object()) {
        for (event, arr) in hooks_obj {
            let Some(arr) = arr.as_array() else { continue };
            for entry in arr {
                let matcher = entry.get("matcher").and_then(|m| m.as_str()).unwrap_or("*");
                let Some(hs) = entry.get("hooks").and_then(|h| h.as_array()) else {
                    continue;
                };
                for h in hs {
                    let cmd = h.get("command").and_then(|c| c.as_str()).unwrap_or("");
                    if !cmd.contains("tokenade") {
                        continue;
                    }
                    let variant = hook_variant_of(cmd);
                    rows.push(InstalledHookRow {
                        event: event.clone(),
                        matcher: matcher.to_string(),
                        variant,
                    });
                }
            }
        }
    }

    // Recent fire activity from the debug log — same collection for
    // both renderers.
    let log_path = home.join(".tokenade").join("debug.log");
    let mut by_event: std::collections::BTreeMap<String, (u64, u64)> =
        std::collections::BTreeMap::new();
    if log_path.exists() {
        if let Ok(log) = fs::read_to_string(&log_path) {
            let recent: Vec<&str> = log.lines().rev().take(2000).collect();
            for line in &recent {
                let Ok(v): serde_json::Result<serde_json::Value> = serde_json::from_str(line)
                else {
                    continue;
                };
                let component = v
                    .get("component")
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .to_string();
                if !component.starts_with("hook") {
                    continue;
                }
                let ts = v.get("ts").and_then(|t| t.as_u64()).unwrap_or(0);
                let entry = by_event.entry(component).or_insert((0, 0));
                entry.0 += 1;
                if ts > entry.1 {
                    entry.1 = ts;
                }
            }
        }
    }

    if json_mode {
        let now = tokenade_core::platform::now_unix_secs();
        let hooks_arr: Vec<serde_json::Value> = rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "event": r.event,
                    "matcher": r.matcher,
                    "variant": r.variant,
                })
            })
            .collect();
        let recent_arr: Vec<serde_json::Value> = by_event
            .iter()
            .map(|(component, (count, last_ts))| {
                serde_json::json!({
                    "component": component,
                    "count": count,
                    "last_ts_unix": last_ts,
                    "last_ts_iso": unix_to_iso(*last_ts),
                    "age_secs": now.saturating_sub(*last_ts),
                })
            })
            .collect();
        let payload = serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "settings_path": settings_path.display().to_string(),
            "settings_present": true,
            "hooks": hooks_arr,
            "recent_activity": recent_arr,
        });
        println!("{}", serde_json::to_string_pretty(&payload).unwrap());
        return 0;
    }

    println!("─── installed hooks ───");
    println!();
    if rows.is_empty() {
        println!(
            "  {dim}--{rst}  no tokenade-tagged hooks found in {}",
            settings_path.display()
        );
    } else {
        for r in &rows {
            println!("  {green}OK{rst}  {:<14} matcher={}", r.event, r.matcher);
            println!("       {dim}→ {}{rst}", r.variant);
        }
    }
    if !by_event.is_empty() {
        println!();
        println!("─── recent activity (last ~2000 log lines) ───");
        println!();
        for (component, (count, last_ts)) in &by_event {
            let age = if *last_ts > 0 {
                let now = tokenade_core::platform::now_unix_secs();
                human_age(now.saturating_sub(*last_ts))
            } else {
                "?".into()
            };
            println!("  {green}{count:>4}×{rst}  {component:<30}  last fired {age} ago");
        }
    }
    0
}

/// Records `hooks tail` returns when no N is given. Named once, because the
/// last time it was written twice the two spellings disagreed by a factor of
/// two and the help was the one that was wrong.
pub(crate) const HOOKS_TAIL_DEFAULT: usize = 20;

pub(crate) fn cmd_hooks_tail(n: Option<usize>, json_mode: bool) -> i32 {
    use std::io::{BufRead, Seek};
    let Some(home) = tokenade_core::platform::home_dir() else {
        eprintln!("tokenade hooks tail: HOME unresolvable");
        return 1;
    };
    let log_path = home.join(".tokenade").join("debug.log");
    if !log_path.exists() {
        eprintln!(
            "tokenade hooks tail: no debug log at {} — run `tokenade install` first.",
            log_path.display()
        );
        return 0;
    }
    // The number `--help` promises, which this did not honour: it said
    // "(default N = 20)" and handed back 40. Both spellings date from the
    // first commit, so there is no intent to recover — decided on merit: the
    // help is the contract a reader acts on, and on a product that sells
    // fewer tokens the default answer should be the smaller one. Ask for more
    // with `hooks tail N`.
    let n = n.unwrap_or(HOOKS_TAIL_DEFAULT);
    let Ok(content) = fs::read_to_string(&log_path) else {
        return 1;
    };
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(n);
    // `--json` was accepted here and ignored, while the sibling `hooks status
    // --nope` refuses an argument it does not know. The output happened to be
    // one JSON object per line, so the flag looked like it worked — a caller
    // asking for a document got a stream and no way to tell the difference
    // from a truncated one. It answers with a document now: the same shape
    // `status --json` uses, an object with named arrays.
    if json_mode {
        let mut records: Vec<serde_json::Value> = Vec::new();
        let mut unparsable: Vec<&str> = Vec::new();
        for line in &lines[start..] {
            match serde_json::from_str::<serde_json::Value>(line) {
                Ok(v) => records.push(v),
                // A torn line is a fact about the log, not a reason to drop it
                // silently: the ledger has been found half-written before.
                Err(_) => unparsable.push(line),
            }
        }
        let payload = serde_json::json!({
            "path": log_path.display().to_string(),
            "requested": n,
            "total_lines": lines.len(),
            "records": records,
            "unparsable": unparsable,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).unwrap_or_default()
        );
        // No live tail in JSON mode: a document that never ends is not one.
        return 0;
    }
    for line in &lines[start..] {
        println!("{line}");
    }
    // If stdout is a TTY, keep tailing.
    use std::io::IsTerminal;
    if !std::io::stdout().is_terminal() {
        return 0;
    }
    eprintln!();
    eprintln!("─── tailing live — Ctrl-C to stop ───");
    let mut file = match fs::File::open(&log_path) {
        Ok(f) => f,
        Err(_) => return 0,
    };
    let _ = file.seek(std::io::SeekFrom::End(0));
    let mut reader = std::io::BufReader::new(file);
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => std::thread::sleep(std::time::Duration::from_millis(300)),
            Ok(_) => print!("{line}"),
            Err(_) => break,
        }
    }
    0
}
