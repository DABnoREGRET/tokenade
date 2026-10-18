//! Claude Code hook handlers (PreToolUse/PostToolUse/SessionStart/
//! UserPromptSubmit/PreCompact) + Read/Bash/Grep/WebFetch compaction,
//! staging, dedup, nudges. Extracted from main.rs (refactor split).
#![allow(unused_imports)]
use crate::install::*;
use crate::wrap::*;
use crate::*;
use std::fs;
use std::path::{Path, PathBuf};

/// Which session is this? — ONE home for a question asked forty-seven times.
///
/// Every hook read `parsed["session_id"]` by hand, and each of those
/// reads silently degrades on a host that spells the field differently: the
/// anti-spam preamble gate stops recognising the session and re-states the
/// preamble EVERY turn, the read-dedup cache misses and re-serves files, the
/// bash keys scatter. None of that fails loudly — it just costs.
///
/// The spellings are read off what each host actually sends, not guessed:
/// Claude, Codex and Grok send `session_id`; Copilot normalises to `session_id`
/// too (`X$(t,e)` in its shipped `app.js` builds
/// `{hook_event_name, session_id: e.sessionId, …}`), but its OTHER payload
/// builders pass `sessionId` straight through — the subagent-start one does;
/// Antigravity keys its payloads on `conversationId`.
///
/// Returns `None` for a missing or empty id so each caller keeps its own
/// fallback, which is what they already differ on (`""`, `"nosess"`, `None`).
pub(crate) fn session_id_of(parsed: &serde_json::Value) -> Option<&str> {
    for key in ["session_id", "sessionId", "conversationId"] {
        if let Some(v) = parsed.get(key).and_then(|s| s.as_str()) {
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

mod guards;
pub(crate) mod stdin;
use guards::tool_input_str;

// --- module split (mechanical, no behaviour change) ---
mod antigravity_pretool;
pub(crate) mod bash_pretool;
mod cache_health;
mod cursor_pretool;
mod gemini_aftertool;
mod grep_pretool;
pub(crate) mod mcp_posttool;
mod read_diffs;
pub(crate) mod read_pretool;
pub(crate) mod read_staging;
pub(crate) mod replay;
pub(crate) mod session_lifecycle;
pub(crate) mod subagent_pretool;
pub(crate) mod webfetch_pretool;
mod websearch_post;
pub(crate) mod websearch_pretool;
mod write_pretool;
pub(crate) use read_pretool::register_bash_served_files;
pub(crate) use session_lifecycle::bump_session_delivered;
mod loop_guard;
mod nudges;
pub(crate) mod permission_rules;
mod postread;
pub(crate) use loop_guard::*;
pub(crate) use nudges::*;
pub(crate) use postread::*;

// `count:last_ts` state of the sidechain window file. Legacy single-ts
// files parse as count=1 (upgrade path).

/// Replay/context key: skey + transcript hash when available (filename-safe).
pub(crate) fn ctx_replay_key(parsed: &serde_json::Value, skey: &str) -> String {
    match parsed.get("transcript_path").and_then(|t| t.as_str()) {
        Some(tp) if !tp.is_empty() => format!(
            "{skey}-{}",
            &tokenade_core::dedup_ref::hash_content(tp.as_bytes())[..8]
        ),
        _ => skey.to_string(),
    }
}

pub(crate) use tokenade_core::session::read_sidechain_state;

/// SubagentStop hook: the exact end of a sidechain — decrement the window
/// counter so in-context claims resume as soon as no subagent is running
/// (the cause-level fix for the blanket 15-min suspension).
pub(crate) fn cmd_hookwrap_subagentstop() -> i32 {
    let mut buf = String::new();
    use std::io::Read as _;
    let _ = crate::bounded_input::read_stdin_lossy(&mut buf);
    let parsed: serde_json::Value = serde_json::from_str(&buf).unwrap_or_default();
    let session = crate::hooks::session_id_of(&parsed).unwrap_or("");
    if !session.is_empty() {
        let skey: String = session
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(40)
            .collect();
        let p = read_staging::read_pending_dir().join(format!("sidechain-{skey}"));
        let (count, ts) = read_sidechain_state(&p);
        if count <= 1 {
            let _ = std::fs::remove_file(&p);
        } else {
            let _ = tokenade_core::paths::write_atomic(&p, &format!("{}:{ts}", count - 1));
        }
        tokenade_core::debug_log::log(
            "hookwrap-subagentstop",
            "sidechain_window_dec",
            serde_json::json!({"remaining": count.saturating_sub(1)}),
        );
    }
    0
}

pub(crate) use antigravity_pretool::*;
pub(crate) use bash_pretool::*;
pub(crate) use cursor_pretool::*;
pub(crate) use gemini_aftertool::*;
pub(crate) use grep_pretool::*;
pub(crate) use read_diffs::*;
pub(crate) use read_pretool::*;
pub(crate) use read_staging::*;
pub(crate) use session_lifecycle::*;
pub(crate) use subagent_pretool::*;
pub(crate) use webfetch_pretool::*;
pub(crate) use websearch_post::*;
pub(crate) use websearch_pretool::*;

/// The shell tool's name differs per agent: Claude Code / Codex say `Bash`,
/// Grok says lowercase `bash`, Factory Droid says `Execute`. The matchers in
/// each agent's hook config already target the right name — this predicate
/// keeps the HANDLER in sync so a matched event is never silently dropped.
/// (Field bug: Grok's hooks fired but `tool == "Bash"` ate every event.)
pub(crate) fn is_shell_tool_name(tool: &str) -> bool {
    // Case-INSENSITIVE equality, not an enumeration of the spellings we have
    // happened to see. `Bash` was listed twice because Grok lowercases every
    // tool name and the field bug was found on that one; `Execute` — droid's
    // shell tool — was listed once, so the very next lowercasing host would
    // reproduce the same wall on the same function. Same names, no invented
    // aliases: that is exactly the line `tool_is` draws below.
    matches!(tool.to_ascii_lowercase().as_str(), "bash" | "execute")
}

/// The same field bug, every other tool over.
///
/// `is_shell_tool_name` exists because Grok lowercases its tool names and
/// `tool == "Bash"` ate every shell event. Grok lowercases ALL of them, and
/// only the shell predicate was ever taught that: measured 2026-08-19 on the
/// compat bench, a Grok `read` payload came out of `hookwrap-pretool` with an
/// EMPTY response — no compaction, no dedup, no document routing — because the
/// arm below asked for `"Read"` exactly. Nine tools were matched by literal
/// equality and eight of them had the defect the ninth was fixed for.
///
/// Case-insensitive equality only, deliberately: an ALIAS (`read_file`,
/// `view`) belongs to a host whose contract we have measured, and inventing
/// them here would route events the handler has never seen. What is being
/// fixed is spelling, not vocabulary.
pub(crate) fn tool_is(tool: &str, canonical: &str) -> bool {
    tool.eq_ignore_ascii_case(canonical)
}
pub(crate) fn cmd_hookwrap_pretool() -> i32 {
    let _p = tokenade_core::profile::phase("pretool:TOTAL");
    // Same reason as `cmd_wrap`: this hook redacts whatever it is about to
    // deliver, and compiling the redaction rules is ~14 ms that would
    // otherwise sit on the critical path of every Read. Start it now, in
    // parallel with parsing stdin and stat-ing the file.
    std::thread::spawn(|| {
        tokenade_core::redact::warm();
    });
    let started = std::time::Instant::now();
    let Some(input) = crate::hooks::stdin::read_event("hookwrap-pretool") else {
        return 0;
    };
    let Some(parsed) = crate::hooks::stdin::parse_event(&input) else {
        tokenade_core::debug_log::log(
            "hookwrap-pretool",
            "parse_err",
            serde_json::json!({"elapsed_ms": started.elapsed().as_millis() as u64}),
        );
        return 0;
    };
    // Session-scope every row this hook process books (statusline ticker).
    if let Some(sid) = crate::hooks::session_id_of(&parsed) {
        let skey: String = sid
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(40)
            .collect();
        tokenade_core::session::set_session_key_authoritative(&skey);
    }
    record_tool_tick(&parsed);

    // Cursor speaks a different hook envelope: `hook_event_name: "preToolUse"`
    // (lowercase). It can only REWRITE the command — via top-level `permission`
    // + `updated_input.command` — there is no after-hook to substitute output.
    // So we route Shell commands through `tokenade wrap`, which runs them and
    // compacts the result. (Cross-agent hook coverage, stolen wire format: trs.)
    if parsed.get("hook_event_name").and_then(|v| v.as_str()) == Some("preToolUse") {
        return handle_cursor_pretool(&parsed, started);
    }
    // Antigravity CLI (`agy`) speaks protojson: every key is camelCase and the
    // tool call is a `toolCall` object, so a Claude-shaped read finds nothing
    // and this hook would answer nothing — indistinguishable, from outside,
    // from not being wired at all. Its `PreToolUse` can REWRITE the command
    // (`overwrite`), which its `PostToolUse` cannot do for the result.
    if crate::hooks::is_antigravity_pretool(&parsed) {
        return crate::hooks::handle_antigravity_pretool(&parsed, started);
    }
    let tool = parsed
        .get("tool_name")
        .and_then(|s| s.as_str())
        .unwrap_or("");
    // Bash: run the command ourselves, compact, return via deny+reason —
    // PreToolUse SUBSTITUTES the result, so Claude only ever sees the compact
    // version.
    //
    // The justification written here was "PostToolUse can only ADD context
    // (raw output stays), so the model paid for raw + compact". That WAS true
    // and is not any more, and leaving it would keep sending the next reader to
    // the wrong conclusion. Measured on the shipped Claude Code 2.1.245:
    // `hookSpecificOutput.updatedToolOutput` REPLACES a tool result
    // (`if("updatedToolOutput" in X && …) I.data = K`), and the schema's own
    // note on the MCP-only variant says "Prefer updatedToolOutput, which works
    // for all tools". This binary already uses it for WebSearch
    // (`websearch_post.rs`), version-gated because Claude Code < 2.1.121
    // ignores it.
    //
    // So the Bash path is no longer the ONLY way to substitute, and moving it
    // would drop a whole class of problems with rewriting a command: quoting
    // through the outer shell, matching the inner shell on Windows,
    // interactive/TTY detection, the `TOKENADE_SESSION=` prefix, and whatever
    // a host's command guard makes of the result (e.g. a worktree
    // guard). It is NOT moved here, and that is a scoped decision rather than
    // an oversight: tools declare an `outputSchema` and the runtime
    // `safeParse`s the replacement against it, so the swap has to reproduce
    // Bash's result shape and be measured end to end — including on the
    // versions that ignore the field, where a PostToolUse-only path would fold
    // nothing at all. Not attempted on a comment's word.
    if is_shell_tool_name(tool) {
        return handle_bash_pretool(&parsed, started);
    }
    // PowerShell: Claude Code's PowerShell tool (used on Windows when Git Bash
    // is absent — mutually exclusive with the Bash tool). Without this branch
    // tokenade did NOTHING on a no-Git-Bash Windows box: the agent runs every
    // command through PowerShell, which no `Bash`-only matcher intercepted.
    // Same substitution model as Bash — rewrite to `tokenade wrap --shell
    // powershell '<cmd>'` so the output is compacted, but run under PowerShell.
    if tool_is(tool, "PowerShell") {
        return handle_powershell_pretool(&parsed, started);
    }
    // Task / Agent: inject a tokenade preamble into the subagent
    // prompt so spawned agents inherit the "prefer the tokenade CLI
    // over grep/find/Read" preference WITHOUT the parent having to
    // remember to brief them per-call. Without explicit instruction the
    // agent defaults to standard Unix file ops. This hook puts that
    // instruction in front of every call.
    if tool_is(tool, "Task") || tool_is(tool, "Agent") {
        return handle_subagent_pretool(&parsed, started, tool);
    }
    // Read: when the agent re-reads a file unchanged since the last
    // time we registered it (within the 7-day dedup TTL), substitute
    // a §ref:HASH§ marker so the bytes don't get re-shipped. Gated
    // on file size — for tiny files the "Error:" UI cost (Claude
    // Code surfaces deny as Error) would exceed the savings.
    if tool_is(tool, "Read") {
        // Large-file refactor nudge: at most once per (session, file), steer a
        // split when a code file exceeds ~1000 lines. The read still runs
        // natively this turn (nudge rides as additionalContext).
        if let Some(nudge) = large_file_refactor_nudge(&parsed) {
            log_emitted_additional_context("hookwrap-pretool-largefile", "PreToolUse", &nudge);
            tokenade_core::gain::record_nudge(
                "large-file",
                "fire",
                tokenade_core::gain::NudgeMetric::None,
            );
            println!(
                "{}",
                serde_json::json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "additionalContext": nudge,
                    }
                })
            );
            tokenade_core::debug_log::log(
                "hookwrap-pretool",
                "large_file_nudge",
                serde_json::json!({"elapsed_ms": started.elapsed().as_millis() as u64}),
            );
            return 0;
        }
        // #1072 — once-per-session batching nudge on the Nth Read. It runs
        // the read NATIVELY (hook contract = one JSON, so nudge and staging
        // are mutually exclusive) — which was a terrible trade on a BIG
        // file: the nudge fired exactly when reads were hot and skipped a
        // 15k-token compaction to save a ~50-token steer (measured live).
        // Only spend it on a small read; big reads keep the pipeline and
        // the nudge waits for the next small one.
        let small_read = parsed
            .pointer("/tool_input/file_path")
            .and_then(|p| p.as_str())
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| m.len() < 16 * 1024)
            .unwrap_or(true);
        if small_read {
            if let Some(nudge) = batching_read_nudge(&parsed) {
                log_emitted_additional_context("hookwrap-pretool-batchnudge", "PreToolUse", &nudge);
                println!(
                    "{}",
                    serde_json::json!({
                        "hookSpecificOutput": {
                            "hookEventName": "PreToolUse",
                            "additionalContext": nudge,
                        }
                    })
                );
                tokenade_core::debug_log::log(
                    "hookwrap-pretool",
                    "batch_nudge",
                    serde_json::json!({"elapsed_ms": started.elapsed().as_millis() as u64}),
                );
                return 0;
            }
        }
        // Adaptive thresholds: deep sessions stash more aggressively.
        session_lifecycle::export_pressure(&parsed);
        return handle_read_pretool(&parsed, started);
    }
    // WebFetch: fetch the URL ourselves, run web::html_to_markdown,
    // substitute via deny+reason. Mirrors the Bash pattern — the
    // model sees ONLY the compacted markdown, never the raw HTML.
    // Typical savings on a real article page are 75-90%. Bypass with
    // TOKENADE_NO_WEBFETCH=1 (or the broader TOKENADE_HOOK_DISABLED).
    if tool_is(tool, "WebFetch") {
        return handle_webfetch_pretool(&parsed, started);
    }
    // WebSearch: run the search ourselves (Brave, client-side) and
    // substitute via deny+reason — the native tool's cost is dominated by
    // its server-side loop (uncached context re-billing + per-search fee),
    // which no PostToolUse compaction can touch. Fail-open everywhere.
    // Bypass with TOKENADE_WEBSEARCH_GATE=0 / TOKENADE_NO_WEBSEARCH=1.
    if tool_is(tool, "WebSearch") {
        return handle_websearch_pretool(&parsed, started);
    }
    // Write: output-side steer — a Write that re-emits an existing file
    // mostly unchanged gets a once-per-(session,file) "use Edit" nudge.
    // Additive only; the Write itself is never touched.
    if tool_is(tool, "Write") {
        return write_pretool::handle_write_pretool(&parsed, started);
    }
    // Edit: purely additive batching steer for one-edit-per-turn chains
    // (parallel Edits in one message). The Edit itself is never touched.
    if tool_is(tool, "Edit") {
        if let Some(nudge) = batching_edit_nudge(&parsed) {
            log_emitted_additional_context("hookwrap-pretool-editbatch", "PreToolUse", &nudge);
            println!(
                "{}",
                serde_json::json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "additionalContext": nudge,
                    }
                })
            );
            tokenade_core::debug_log::log(
                "hookwrap-pretool",
                "batch_nudge_edit",
                serde_json::json!({}),
            );
        }
        return 0;
    }
    // Grep: re-run the same ripgrep, fold the output, and cap the native
    // result to a head anchor so the raw + compacted view don't BOTH sit in
    // context (the additive double-cost that shelved the first attempt). Same
    // two-step staging as Read. Gated TOKENADE_NO_GREP; passes through on any
    // unsupported param.
    if tool_is(tool, "Grep") {
        return handle_grep_pretool(&parsed, started);
    }
    // Every other tool (Glob, …) is passed through untouched.
    // The deny-redirect path for Grep/Glob was removed: PreToolUse `deny`
    // is the only substitution channel for those tools, and Claude Code
    // wraps the reason in <error>…</error> — making a successful redirect
    // look like a failure and risking a wasted retry. Bash/Read/WebFetch
    // keep their handlers (Bash/WebFetch substitute via the safe
    // allow+updatedInput or fetch-and-replace paths; Read dedup is gated).
    tokenade_core::debug_log::log(
        "hookwrap-pretool",
        "skip_tool",
        serde_json::json!({"tool": tool, "elapsed_ms": started.elapsed().as_millis() as u64}),
    );
    0
}

#[cfg(test)]
mod session_id_home {
    /// "Which session is this?" was asked by hand in FORTY-SEVEN places,
    /// and every one of them degrades in SILENCE on a host that spells the
    /// field differently: the anti-spam preamble gate stops recognising the
    /// session and re-states the preamble every turn, the read-dedup cache
    /// misses and re-serves files. Nothing fails loudly — it only costs.
    ///
    /// Read off the SOURCE, because a forty-eighth hand-rolled read is
    /// invisible to behaviour until someone runs tokenade under the host that
    /// spells it differently.
    #[test]
    fn nothing_reads_the_session_id_by_hand() {
        let mut offenders = Vec::new();
        let mut stack = vec![std::path::PathBuf::from("src")];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().and_then(|s| s.to_str()) != Some("rs") {
                    continue;
                }
                let Ok(body) = std::fs::read_to_string(&p) else {
                    continue;
                };
                for (n, line) in body.lines().enumerate() {
                    let t = line.trim_start();
                    if t.starts_with("//") {
                        continue;
                    }
                    // Only READS. A fixture that CONSTRUCTS a payload has to
                    // name the field — that is the host's spelling being
                    // exercised, not a second reader of it.
                    if !t.contains(".get(\"session_id\")") && !t.contains(".get(\"sessionId\")") {
                        continue;
                    }
                    // The declarer itself is the one place allowed to ask.
                    if body.contains("pub(crate) fn session_id_of") {
                        continue;
                    }
                    offenders.push(format!("{}:{}", p.display(), n + 1));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "these read the session id by hand instead of asking `session_id_of`, \
             and will lose the session on a host that spells it differently: {offenders:?}"
        );
    }

    /// The spellings, each read off what a host actually sends. Claude, Codex
    /// and Grok say `session_id`; Copilot's subagent payload says `sessionId`;
    /// Antigravity keys on `conversationId`. An EMPTY value must not shadow a
    /// later spelling, and a missing id must stay `None` so each caller keeps
    /// its own fallback.
    #[test]
    fn each_spelling_is_found_and_an_empty_one_does_not_shadow() {
        use super::session_id_of;
        for key in ["session_id", "sessionId", "conversationId"] {
            let v = serde_json::json!({ key: "abc123" });
            assert_eq!(session_id_of(&v), Some("abc123"), "spelling {key}");
        }
        let shadowed = serde_json::json!({ "session_id": "", "sessionId": "real" });
        assert_eq!(session_id_of(&shadowed), Some("real"));
        assert_eq!(session_id_of(&serde_json::json!({})), None);
        assert_eq!(
            session_id_of(&serde_json::json!({ "session_id": "" })),
            None
        );
    }

    /// No hook dispatch decides on a tool name with `==`.
    ///
    /// Grok lowercases EVERY tool name. That is a measured field bug, not a
    /// hypothesis: `tool == "Bash"` once ate every shell event here, and
    /// literal equality dropped eight of the nine routed tools on that host.
    /// The Bash half was fixed by making `is_shell_tool_name` case-tolerant and
    /// the rest moved to `tool_is` — but two literal comparisons survived, and
    /// each broke a different promise.
    ///
    /// `cmd_hookwrap_postread` was the severe one: the PreToolUse half selects
    /// with `tool_is`, so on `read` it still STAGED a compacted view and shrank
    /// the caller's Read to a head slice under a reason saying "compacted
    /// content follows as additionalContext" — and this half, comparing the raw
    /// string, delivered nothing. Measured against the published 1.1.8 on a
    /// 4 000-line file: `Read` receives 1 050 bytes of compacted content,
    /// `read` receives twenty lines and silence.
    ///
    /// `cmd_hookwrap` was the quiet one: a lowercased Read fell past its
    /// branch, failed the shell test below it, and logged `skip_non_bash`, so
    /// the read nudge never fired on that host.
    ///
    /// A source guard rather than a behavioural one because the branch itself
    /// is not reachable from an isolated HOME — a first large read there is
    /// served by `deny` + `permissionDecisionReason` on every call, so a test
    /// driving the hooks would assert nothing. This asks the only question that
    /// distinguishes the two spellings, at the layer where the choice is made.
    #[test]
    fn no_hook_dispatch_compares_a_tool_name_with_equality() {
        // The FAMILIES, not the trunks. `hooks/` now holds twenty-five files
        // and `hookwrap` has children: naming three paths by hand amounted to
        // watching only the part of the routing left in the original file, and
        // that has a cost — the guard reports a disappearance where it is
        // simply looking in the wrong place.
        let cli_src = tokenade_core::source_guard::crates_dir()
            .join("tokenade-cli")
            .join("src");
        let hooks = tokenade_core::source_guard::module_family(&cli_src, "hooks");
        let hookwrap = tokenade_core::source_guard::module_family(&cli_src, "hookwrap");
        let sources = [("hooks/", hooks.as_str()), ("hookwrap", hookwrap.as_str())];
        let mut offenders = Vec::new();
        let mut per_source: Vec<(&str, usize)> = Vec::new();
        for (name, src) in sources {
            let mut scanned = 0usize;
            for (n, line) in src.lines().enumerate() {
                let code = line.split("//").next().unwrap_or("");
                scanned += 1;
                if code.contains("tool == \"") || code.contains("tool_name == \"") {
                    offenders.push(format!("{name}:{}: {}", n + 1, line.trim()));
                }
            }
            per_source.push((name, scanned));
        }
        // Anti-vacuity, PER SOURCE. A single total let a whole file be dropped
        // and still clear the bar: the two remaining ones are thousands of
        // lines. Every named source must have been read, and be substantial.
        assert_eq!(per_source.len(), 2, "a source was dropped: {per_source:?}");
        for (name, n) in &per_source {
            assert!(
                *n > 1_000,
                "`{name}` contributed only {n} lines — the guard lost it"
            );
        }
        assert!(
            offenders.is_empty(),
            "a hook decided on a tool name with `==`. Grok lowercases every \
             tool name, so literal equality silently skips the branch — and \
             where the PreToolUse half already spent the caller's content, the \
             promise it made goes unkept. Use `tool_is`:\n  {}",
            offenders.join("\n  ")
        );
    }
}
