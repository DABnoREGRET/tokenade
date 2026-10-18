//! PostToolUse compactor for MCP tool results
//! (`tokenade hookwrap-mcp-post`).
//!
//! **The gap this module closes.** Until now, an MCP tool result —
//! verbose JSON, logs, console output — was folded by NO hook: no
//! `matcher` took `mcp__<server>__<tool>`, and the handler behind the hooks
//! only read Bash-shaped responses. The only coverage was
//! `tokenade mcp-wrap install`, which wraps the LAUNCH of each configured
//! server: set-once, but one more process per server, a configuration to
//! rewrite, and nothing at all for a server added afterwards.
//!
//! What the Claude Code documentation says, and what makes the proxy
//! unnecessary on this agent: a `PostToolUse` hook replaces what the model
//! sees via `hookSpecificOutput.updatedToolOutput`, and — this is the
//! sentence that matters — « **MCP tool output is passed through without
//! schema validation** ». Where a native tool requires the replacement to
//! respect its output schema (otherwise it is silently ignored), an MCP
//! result accepts whatever shape we hand back. MCP tools show up in tool
//! events like any other, under the name `mcp__<server>__<tool>`.
//!
//! **A single folder for both routes.** The transformation is the proxy's,
//! `mcp_proxy::transform_mcp_response_line`, called on a JSON-RPC envelope
//! rebuilt around the received `tool_response`. Writing a second folder
//! would make the two paths diverge at the first fix; here the general
//! mechanism gains a caller rather than losing one.
//!
//! Everything is FAIL-OPEN: the slightest guard, parse or shape problem
//! prints nothing and exits 0, so the native output stands. Per-call bypass
//! with `TOKENADE_NO_MCP_HOOK=1` (or the broader
//! `TOKENADE_HOOK_DISABLED=1`).
use super::*;

/// The prefix under which every compatible agent names an MCP tool.
pub(crate) const MCP_TOOL_PREFIX: &str = "mcp__";

/// `mcp__<server>__<tool>` → `<server>`, for the gains table label. A name
/// without the second pair of underscores returns the rest as-is rather
/// than nothing.
pub(crate) fn server_of(tool_name: &str) -> &str {
    let rest = tool_name.strip_prefix(MCP_TOOL_PREFIX).unwrap_or(tool_name);
    match rest.find("__") {
        Some(at) => &rest[..at],
        None => rest,
    }
}

/// The tool name as the SERVER declares it in `tools/list`, whatever the
/// agent's spelling: `mcp__<server>__<tool>` (Claude Code), `MCP:<tool>`
/// (Cursor), or already bare (the proxy).
///
/// The folder compares this name by EQUALITY (`directory_tree`,
/// `read_query`…) and writes it into the registry op (`mcp:<tool>`). The hook
/// used to pass it the raw name: the Filesystem and Database folds never
/// fired through the hook, and the fleet counted 54 tools twice, under
/// `mcp:X` and `mcp:MCP:X` (measured 2026-10-02).
pub(crate) fn bare_tool_of(tool_name: &str) -> &str {
    if let Some(rest) = tool_name.strip_prefix(MCP_TOOL_PREFIX) {
        return rest.find("__").map_or(rest, |at| &rest[at + 2..]);
    }
    tool_name.strip_prefix("MCP:").unwrap_or(tool_name)
}

/// The decision, kept apart from I/O to stay testable: returns the JSON
/// line to print, or `None` when there is nothing to replace.
///
/// `None` covers THREE cases that must all leave the native output in
/// place: it is not an MCP tool, the response does not parse, or the fold
/// changed nothing. The third is the most common and the most important —
/// replacing with the identical text would cost a rewrite for zero tokens.
pub(crate) fn mcp_post_decision(parsed: &serde_json::Value) -> Option<String> {
    let tool = parsed.get("tool_name").and_then(|v| v.as_str())?;
    if !tool.starts_with(MCP_TOOL_PREFIX) {
        return None;
    }
    let response = parsed.get("tool_response")?;
    let replacement = fold_mcp_result(tool, parsed.get("tool_input"), response)?;
    Some(
        serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "updatedToolOutput": replacement,
            }
        })
        .to_string(),
    )
}

/// The fold itself, shared by both hook dialects.
///
/// The envelope the proxy's folder knows how to read: it decides on
/// `/result/content` (the shape of a `tools/call`), so the result is put
/// back exactly where the proxy would have found it. Returns `None` when
/// nothing changed — replacing with the identical text would cost a rewrite
/// for zero tokens.
pub(crate) fn fold_mcp_result(
    tool: &str,
    args: Option<&serde_json::Value>,
    response: &serde_json::Value,
) -> Option<serde_json::Value> {
    let envelope = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": response,
    });
    let line = serde_json::to_string(&envelope).ok()?;
    let folded = crate::mcp_proxy::transform_mcp_response_line(
        &line,
        server_of(tool),
        Some(bare_tool_of(tool)),
        args,
    );
    let mut back: serde_json::Value = serde_json::from_str(&folded).ok()?;
    let replacement = back.get_mut("result")?.take();
    (&replacement != response).then_some(replacement)
}

/// **The Cursor dialect.** Same fold, two differences that matter:
///
/// 1. There `postToolUse` « fires for all tools » and the replacement output
///    is called `updated_mcp_tool_output`, documented « For MCP tools only:
///    replaces the tool output seen by the model ». The installer's comment
///    said « Cursor has no after-hook to substitute output »: that was true,
///    it no longer is.
/// 2. `tool_output` arrives as a STRING (« JSON-stringified result
///    payload »), where Claude Code passes the object.
///
/// The guard is on the SHAPE, not on the tool name: an MCP result is an
/// object carrying a `content` array. Cursor does not promise an `mcp__`
/// prefix on `tool_name`, and a guard that only rests on an undeclared
/// naming convention is a guard lost at the first rename (we read what the
/// data DECLARES).
pub(crate) fn cursor_mcp_post_decision(parsed: &serde_json::Value) -> Option<String> {
    let raw = parsed.get("tool_output")?.as_str()?;
    let response: serde_json::Value = serde_json::from_str(raw).ok()?;
    if !response.get("content").is_some_and(|c| c.is_array()) {
        return None;
    }
    let tool = parsed
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or("mcp");
    let replacement = fold_mcp_result(tool, parsed.get("tool_input"), &response)?;
    Some(serde_json::json!({ "updated_mcp_tool_output": replacement }).to_string())
}

/// **On Cursor, nothing tracked the agent's writes.** The write generation
/// — guard no. 1 of replay (`replay.rs`) and the loop-guard reset — only
/// moved through Claude Code's edit hook. Cursor's `preToolUse` only listens
/// to `Shell|Read`; its `postToolUse`, however, fires for ALL tools and
/// already arrives here. Read from the installed bundle (cursor-agent
/// 2026.09.26): a file write is named `Write` there, a deletion `Delete`.
/// Without this, a `git status` replayed after an agent edit answered the
/// previous state.
pub(crate) fn cursor_post_tracks_writes(parsed: &serde_json::Value) {
    let tool = parsed
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if crate::hooks::tool_is(tool, "Write") || crate::hooks::tool_is(tool, "Delete") {
        crate::hooks::session_lifecycle::bump_write_generation(Some(&parsed.to_string()));
    } else if crate::hooks::tool_is(tool, "Shell") {
        if let Some(cmd) = parsed
            .pointer("/tool_input/command")
            .and_then(|c| c.as_str())
        {
            crate::hooks::session_lifecycle::bump_write_generation_if_bash_writes(parsed, cmd);
        }
    }
}

/// Entry point of `tokenade hookwrap-mcp-post`.
pub(crate) fn cmd_hookwrap_mcp_post() -> i32 {
    let started = std::time::Instant::now();
    if hook_disabled_by_env() || std::env::var_os("TOKENADE_NO_MCP_HOOK").is_some() {
        return 0;
    }
    let Some(input) = crate::hooks::stdin::read_event("hookwrap-mcp-post") else {
        return 0;
    };
    let Some(parsed) = crate::hooks::stdin::parse_event(&input) else {
        return 0;
    };
    if let Some(sid) = crate::hooks::session_id_of(&parsed) {
        let skey: String = sid
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(40)
            .collect();
        tokenade_core::session::set_session_key_authoritative(&skey);
    }
    // Same guard as the other folders: without the retrieval commands,
    // serving a folded view and a reference the agent cannot expand is
    // strictly worse than the native output.
    let before = serde_json::to_string(
        parsed
            .get("tool_response")
            .unwrap_or(&serde_json::Value::Null),
    )
    .map(|s| s.len())
    .unwrap_or(0);
    let Some(line) = mcp_post_decision(&parsed) else {
        tokenade_core::debug_log::log(
            "hookwrap-mcp-post",
            "no_change",
            serde_json::json!({"elapsed_ms": started.elapsed().as_millis() as u64}),
        );
        return 0;
    };
    println!("{line}");
    tokenade_core::debug_log::log(
        "hookwrap-mcp-post",
        "replaced",
        serde_json::json!({
            "before_bytes": before,
            "after_bytes": line.len(),
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }),
    );
    0
}

/// Entry point of `tokenade hookwrap-cursor-mcp-post`.
pub(crate) fn cmd_hookwrap_cursor_mcp_post() -> i32 {
    if hook_disabled_by_env() {
        return 0;
    }
    let Some(input) = crate::hooks::stdin::read_event("hookwrap-cursor-mcp-post") else {
        return 0;
    };
    let Some(parsed) = crate::hooks::stdin::parse_event(&input) else {
        return 0;
    };
    cursor_post_tracks_writes(&parsed);
    if std::env::var_os("TOKENADE_NO_MCP_HOOK").is_some() {
        return 0;
    }
    if let Some(line) = cursor_mcp_post_decision(&parsed) {
        println!("{line}");
    }
    0
}

#[cfg(test)]
mod an_mcp_result_is_folded_by_the_hook {
    use super::*;

    #[test]
    fn a_cursor_write_moves_the_generation() {
        let _home = tokenade_core::platform::ScratchHome::new("cursor-writegen");
        let gen = || crate::hooks::session_lifecycle::current_write_generation("curswg1");
        let ev = |tool: &str, input: serde_json::Value| serde_json::json!({"session_id": "curswg1", "tool_name": tool, "tool_input": input});
        let mut g = gen();
        for (tool, input, moves) in [
            ("Read", serde_json::json!({"file_path": "a.rs"}), false),
            (
                "Shell",
                serde_json::json!({"command": "git status --short"}),
                false,
            ),
            (
                "Write",
                serde_json::json!({"file_path": "a.rs", "content": "x"}),
                true,
            ),
            ("Delete", serde_json::json!({"file_path": "b.rs"}), true),
            (
                "Shell",
                serde_json::json!({"command": "sed -i 's/a/b/' src/a.rs"}),
                true,
            ),
        ] {
            cursor_post_tracks_writes(&ev(tool, input));
            assert_eq!(gen() != g, moves, "{tool}");
            g = gen();
        }
    }

    #[test]
    fn the_three_spellings_of_a_tool_yield_its_bare_name() {
        for name in [
            "mcp__playwright__browser_navigate",
            "MCP:browser_navigate",
            "browser_navigate",
        ] {
            assert_eq!(bare_tool_of(name), "browser_navigate", "{name}");
        }
    }

    /// The Filesystem fold compares `directory_tree` by equality: with the
    /// hook's raw name, the tree went back out as JSON.
    #[test]
    fn a_directory_tree_folds_into_a_tree_through_the_hook() {
        let tree = r#"[
  {"name":"docs","type":"directory","children":[
    {"name":"guide.md","type":"file"},
    {"name":"readme.md","type":"file"}]},
  {"name":"src","type":"directory","children":[
    {"name":"api","type":"directory","children":[
      {"name":"auth.ts","type":"file"}]}]}
]"#;
        let line = mcp_post_decision(&event("mcp__filesystem__directory_tree", tree))
            .expect("the tree folds");
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        let text = v
            .pointer("/hookSpecificOutput/updatedToolOutput/content/0/text")
            .and_then(|t| t.as_str())
            .unwrap();
        assert!(
            text.contains("docs/\n  guide.md") && text.contains("    auth.ts"),
            "{text}"
        );
    }

    fn event(tool: &str, text: &str) -> serde_json::Value {
        serde_json::json!({
            "hook_event_name": "PostToolUse",
            "tool_name": tool,
            "tool_input": {"path": "/tmp/x"},
            "tool_response": {"content": [{"type": "text", "text": text}], "isError": false},
        })
    }

    /// **The server name is read from the tool name**, and a name without
    /// the second pair of underscores must not yield an empty label.
    #[test]
    fn the_server_is_read_from_the_tool_name() {
        assert_eq!(server_of("mcp__playwright__screenshot"), "playwright");
        assert_eq!(server_of("mcp__fs__read_file"), "fs");
        assert_eq!(server_of("mcp__solo"), "solo");
        assert_eq!(server_of("Bash"), "Bash");
    }

    /// A non-MCP tool is not touched: the `matcher` is supposed to exclude
    /// it, and a guard that rests ONLY on configuration is a guard the next
    /// configuration will lose.
    #[test]
    fn a_native_tool_is_not_touched() {
        assert!(mcp_post_decision(&event("Bash", "x")).is_none());
        assert!(mcp_post_decision(&serde_json::json!({"tool_name": "mcp__a__b"})).is_none());
    }

    /// **Nothing to fold ⇒ nothing to print.** Replacing with the identical
    /// text would cost a rewrite for zero tokens, and would also hide the
    /// native output behind one more path.
    #[test]
    fn an_already_short_result_is_not_replaced() {
        assert!(mcp_post_decision(&event("mcp__fs__read", "ok")).is_none());
    }

    /// **Cursor passes the result as a STRING**, and its replacement output
    /// has another name. The same fold, two envelopes.
    #[test]
    fn the_cursor_dialect_folds_the_same_thing_under_another_name() {
        let noisy = (0..400)
            .map(|i| format!("2026-09-11T10:00:{i:02}Z INFO worker heartbeat ok"))
            .collect::<Vec<_>>()
            .join("\n");
        let payload = serde_json::json!({
            "content": [{"type": "text", "text": noisy}],
            "isError": false,
        });
        let ev = serde_json::json!({
            "tool_name": "tail_logs",
            "tool_input": {"service": "ingest"},
            "tool_output": serde_json::to_string(&payload).unwrap(),
        });
        let line = cursor_mcp_post_decision(&ev).expect("no fold on 400 repetitive lines");
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        let served = v["updated_mcp_tool_output"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default();
        assert!(!served.is_empty(), "empty replacement: {line}");
        assert!(served.len() < noisy.len(), "not shorter");
        // The field is Cursor's, not Claude Code's.
        assert!(
            v.get("hookSpecificOutput").is_none(),
            "wrong envelope: {line}"
        );
    }

    /// The guard is on the SHAPE: a native tool output (Cursor sends one for
    /// ALL tools) has no `content` array and must trigger nothing. Without
    /// this guard, the hook would rewrite outputs that
    /// `updated_mcp_tool_output` cannot replace anyway.
    #[test]
    fn an_output_that_is_not_an_mcp_result_is_not_touched() {
        let ev = serde_json::json!({
            "tool_name": "Shell",
            "tool_output": "{\"exitCode\":0,\"stdout\":\"All tests passed\"}",
        });
        assert!(cursor_mcp_post_decision(&ev).is_none());
        // Nor an output that is not JSON at all.
        let ev = serde_json::json!({"tool_name": "Shell", "tool_output": "plain text"});
        assert!(cursor_mcp_post_decision(&ev).is_none());
        // Nor a missing field.
        assert!(cursor_mcp_post_decision(&serde_json::json!({"tool_name": "x"})).is_none());
    }

    /// The useful case: a repetitive result comes back folded, in the
    /// envelope Claude Code expects, and the served text is no longer the
    /// received text.
    #[test]
    fn a_repetitive_result_comes_back_folded_in_the_expected_envelope() {
        let noisy = (0..400)
            .map(|i| format!("2026-09-11T10:00:{i:02}Z INFO worker heartbeat ok"))
            .collect::<Vec<_>>()
            .join("\n");
        let Some(line) = mcp_post_decision(&event("mcp__logs__tail", &noisy)) else {
            // Fail-open is right in production, not in this test: if the
            // proxy's folder does not bite on this text, the rule is worthless
            // and we need to know.
            panic!("no fold on 400 repetitive lines");
        };
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PostToolUse");
        let served = v["hookSpecificOutput"]["updatedToolOutput"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default();
        assert!(!served.is_empty(), "empty replacement: {line}");
        assert!(
            served.len() < noisy.len(),
            "not shorter: {} vs {}",
            served.len(),
            noisy.len()
        );
    }
}
