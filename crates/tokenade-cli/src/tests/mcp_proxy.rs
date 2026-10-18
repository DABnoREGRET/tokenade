//! Tests for `mcp_proxy`, split out of the flat `tests.rs`.

use super::*;

#[test]
fn derive_label_picks_last_positional_basename() {
    // `npx mcp-server-github` — the canonical wrap shape. Label
    // = the server name, not `npx`.
    let argv = vec!["npx".into(), "mcp-server-github".into()];
    assert_eq!(derive_mcp_target_label(&argv), "mcp-server-github");
}

#[test]
fn derive_label_strips_dir_and_extension() {
    let argv = vec!["/opt/foo/bar/server.js".into()];
    assert_eq!(derive_mcp_target_label(&argv), "server");
}

#[test]
fn derive_label_skips_leading_flags() {
    // `node --inspect mcp-server-x` → label is the server name,
    // not the flag value.
    let argv = vec!["node".into(), "--inspect".into(), "mcp-server-x".into()];
    assert_eq!(derive_mcp_target_label(&argv), "mcp-server-x");
}

#[test]
fn derive_label_fallback_on_empty_argv() {
    let argv: Vec<String> = vec![];
    assert_eq!(derive_mcp_target_label(&argv), "unknown");
}

#[test]
fn derive_label_fallback_when_only_flags() {
    let argv = vec!["--foo".into(), "--bar".into()];
    // No positional → "unknown" rather than crashing.
    assert_eq!(derive_mcp_target_label(&argv), "unknown");
}

#[test]
fn transform_preserves_non_json_lines_verbatim() {
    // MCP servers occasionally emit blank separator lines; the
    // transform MUST be a no-op on anything that doesn't parse.
    assert_eq!(
        transform_mcp_response_line("\n", "github", None, None),
        "\n"
    );
    assert_eq!(
        transform_mcp_response_line("not json\n", "github", None, None),
        "not json\n"
    );
}

#[test]
fn transform_preserves_jsonrpc_errors_verbatim() {
    let err_line = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"oops"}}"#;
    let out = transform_mcp_response_line(err_line, "github", None, None);
    // Must contain the original error shape so the agent can
    // diagnose. The trimmed comparison is for trailing newline
    // tolerance.
    assert!(out.contains("\"code\":-32000"));
    assert!(out.contains("\"oops\""));
}

#[test]
fn transform_compacts_text_items_in_result_content() {
    // Also takes the `ENV_RW_LOCK` writer lock: this test rewrites HOME, which
    // every reader of that lock resolves state from.
    // Env vars are PROCESS-global while cargo runs tests on parallel threads,
    // so this must serialize against every other env-mutating test — exactly
    // like its neighbours. Without the lock a concurrent test restored HOME
    // mid-flight and the shrink assertion failed: measured 2 runs in 3 red.
    let _g = INSTALL_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _rw = ENV_RW_LOCK.write().unwrap_or_else(|p| p.into_inner());
    // A typical tools/call response. The huge HTML triggers
    // disclose::maybe_stash above its threshold → returns a
    // summary that's much smaller than the original.
    //
    // #496: redirect HOME to a tempdir so the gain::record() call
    // inside transform_mcp_response_line (added in #493) writes
    // to a throwaway ledger instead of polluting the user's
    // production ~/.tokenade/gain.jsonl. Without this, every
    // `cargo test` adds a leaked `mcp-proxy:github` row.
    let tmp = std::env::temp_dir().join(format!(
        "tokenade-test-transform-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::create_dir_all(&tmp);
    let prev_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", &tmp);

    let huge = "<html><body>".to_string()
        + &"<p>filler text to push above stash threshold</p>\n".repeat(800)
        + "</body></html>";
    let resp = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "content": [{"type": "text", "text": huge}],
            "isError": false,
        }
    });
    let line = serde_json::to_string(&resp).unwrap() + "\n";
    let out = transform_mcp_response_line(&line, "github", None, None);

    // Restore HOME before any assert, to avoid polluting subsequent tests in
    // the same binary.
    if let Some(prev) = prev_home {
        std::env::set_var("HOME", prev);
    } else {
        std::env::remove_var("HOME");
    }
    let _ = std::fs::remove_dir_all(&tmp);

    // The output must still be valid JSON-RPC.
    let parsed: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(parsed["jsonrpc"], "2.0");
    // The text payload should have shrunk meaningfully.
    let new_text = parsed["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        new_text.len() < line.len() / 2,
        "expected stash to shrink text by >50%; got {} → {}",
        line.len(),
        new_text.len()
    );
}

#[test]
fn transform_passes_through_image_and_non_text_content() {
    // image / audio / resource items are already encoded — never
    // try to stash or rewrite them.
    let img_b64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAA";
    let resp = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "content": [{"type": "image", "data": img_b64, "mimeType": "image/png"}],
        }
    });
    let line = serde_json::to_string(&resp).unwrap() + "\n";
    let out = transform_mcp_response_line(&line, "github", None, None);
    // The base64 payload must survive verbatim.
    assert!(out.contains(img_b64), "image data dropped: {out}");
}

#[test]
fn transform_leaves_screenshot_capture_inside_computer_text() {
    // Claude-in-Chrome `computer` / `browser_batch`: the tool name is not
    // screenshot, the body is. Generic auto::compact is the hole; the
    // leave_uncompacted gate on the proxy path is the fix.
    let notice = "Successfully captured screenshot (1098x538, jpeg) - ID: ss_8834s8965 — 0.7-scale view; coordinate frame: 1568x769.";
    let text = format!(
        "{notice}\n\nTab Context:\n- Executed on tabId: 994507971\n- Available tabs:\n{}",
        "  • tabId 1: filler title that makes a long chrome blob\n".repeat(40)
    );
    let line = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": { "content": [{ "type": "text", "text": text }] }
    })
    .to_string();
    let out = transform_mcp_response_line(&line, "claude-in-chrome", Some("computer"), None);
    assert!(
        out.contains(notice),
        "computer screenshot notice was folded: {out}"
    );
    let batch = transform_mcp_response_line(
        &line,
        "claude-in-chrome",
        Some("mcp__claude-in-chrome__browser_batch"),
        None,
    );
    assert!(
        batch.contains(notice),
        "browser_batch screenshot notice was folded: {batch}"
    );
}

#[test]
fn transform_does_not_skip_fold_when_the_word_screenshot_is_only_prose() {
    // False-positive bar: mentioning "screenshot" in a log must not trip
    // leave_uncompacted. Whether auto::compact then shrinks is its job.
    let text = "[info] see the screenshot guide at 1920x1080\n".repeat(80);
    assert!(
        !tokenade_core::mcp_compact::leave_uncompacted("computer", &text),
        "gate must not fire on the word screenshot in a log line"
    );
    let line = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": { "content": [{ "type": "text", "text": text }] }
    })
    .to_string();
    let out = transform_mcp_response_line(&line, "claude-in-chrome", Some("computer"), None);
    assert!(
        out.contains("screenshot guide"),
        "prose payload must still reach the model: {out}"
    );
}

// Widening `already_folded` to recognise fold BANNERS (2026-08-08) gave this
// path a new way to return the server's bytes without folding them. In
// `cmd_auto` the equivalent early return printed them RAW and leaked a live
// key. Here it cannot: `transform_mcp_response_line` wraps the whole branch
// tree in `redact_mcp_response` precisely so that every "return verbatim" exit
// is still scrubbed. This pins that, because the guard above it is one edit
// away from being moved outside the wrapper.
#[test]
fn an_already_folded_result_still_leaves_through_the_redactor() {
    let secret = format!("sk-ant-api03-{}", "A".repeat(80));
    let text = format!(
        "[tokenade:git] 82 line(s)\nkey: {secret}\n{}",
        "filler line to clear the size floor\n".repeat(60)
    );
    let line = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": { "content": [{ "type": "text", "text": text }] }
    })
    .to_string();
    let out = transform_mcp_response_line(&line, "github", Some("get_file"), None);
    assert!(
        !out.contains(&secret),
        "a result the fold guard skipped left the proxy unredacted: {out}"
    );
    // And the skip itself held: the banner text was not folded a second time.
    assert!(out.contains("[tokenade:git] 82 line(s)"));
}
