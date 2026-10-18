//! Tests for `dashboard_cli`, split out of the flat `tests.rs`.

use super::*;

/// #731 — pin all attribution rules so the next refactor doesn't
/// silently regress the Read-bypass mis-attribution that #731
/// fixed. The 5 cases below mirror the 5 rules in
/// `attribute_agent_tool`; the catch-all `None` cases verify
/// non-agent traffic (cli, test fixtures, mcp/sandbox) stays out
/// of `tokenade_after_by_tool` and therefore out of the dashboard's
/// Functions section.
#[test]
fn attribute_agent_tool_covers_all_writer_paths() {
    // Rule 1 — explicit label prefix (the pre-#731 baseline)
    assert_eq!(
        attribute_agent_tool("hook", "Bash:grep:foo", "lsp_enforce_bash"),
        Some("Bash"),
    );
    assert_eq!(
        attribute_agent_tool("hook", "Read:/path/file.rs", "read_disclose"),
        Some("Read"),
    );

    // Rule 2 — read_* op-prefix fallback (THE #731 fix). The Read
    // pretool labels rows with the absolute file path, NOT
    // "Read:…", so without this rule these 4 ops would be
    // unattributed and inflate Read bypass.
    for op in &[
        "read_disclose",
        "read_auto_compact",
        "read_content_dedup",
        "read_code_strip_lite",
    ] {
        assert_eq!(
            attribute_agent_tool("hook", "/abs/path/to/file.txt", op),
            Some("Read"),
            "op {op} should attribute to Read via op-prefix rule",
        );
    }

    // Rule 3 — bash_* op-prefix + lsp_enforce_bash (defensive
    // future-proofing; currently only lsp_enforce_bash is written).
    assert_eq!(
        attribute_agent_tool("hook", "(no label)", "lsp_enforce_bash"),
        Some("Bash"),
    );
    assert_eq!(
        attribute_agent_tool("hook", "(no label)", "bash_anything"),
        Some("Bash"),
    );

    // #733 — Grep (lsp_enforce label is "Grep:<pattern>"), Glob,
    // WebFetch. Without these the Functions section shows their
    // entire tool_result_tokens as bypass even when tokenade
    // actively shaved the traffic.
    assert_eq!(
        attribute_agent_tool("hook", "Grep:fn foo", "lsp_enforce"),
        Some("Grep"),
    );
    assert_eq!(
        attribute_agent_tool("hook", "(no label)", "lsp_enforce"),
        Some("Grep"),
    );
    assert_eq!(
        attribute_agent_tool("hook", "**/*.rs", "glob_compact"),
        Some("Glob"),
    );
    assert_eq!(
        attribute_agent_tool("hook", "https://x.example/y", "webfetch_compact"),
        Some("WebFetch"),
    );
    // #950 — V1 binary-extraction route also attributes to WebFetch.
    assert_eq!(
        attribute_agent_tool(
            "hook",
            "https://x.example/y.pdf",
            "webfetch_binary_extracted"
        ),
        Some("WebFetch"),
    );
    // #951 — local-file Read of a PDF/DOCX/EPUB also attributes
    // to the Read tool (sibling of read_dedup_hit / read_diff_hit).
    assert_eq!(
        attribute_agent_tool("hook", "/tmp/report.pdf", "read_binary_extracted"),
        Some("Read"),
    );

    // #737 — auto:* PostToolUse Bash compactions (label is the
    // bash command, not prefixed). Mirrors the auto-detector
    // filter names cmd_hookwrap writes.
    assert_eq!(
        attribute_agent_tool("hook", "cd /tmp && ls", "auto:filelist"),
        Some("Bash"),
    );
    assert_eq!(
        attribute_agent_tool("hook", "python3 script.py", "auto:stacktrace"),
        Some("Bash"),
    );
    assert_eq!(
        attribute_agent_tool("hook", "kubectl get pods", "auto:json"),
        Some("Bash"),
    );

    // Rule 4 — source=wrap always Bash (CLI `tokenade wrap` is
    // Bash-invoked from the agent's PreToolUse hook).
    assert_eq!(
        attribute_agent_tool("wrap", "anything", "auto:json"),
        Some("Bash"),
    );

    // Rule 5 — non-agent sources are skipped. cli = user typing
    // `tokenade auto < file` at a shell; mcp = exec_compact MCP
    // call (already accounted via wrapped_mcps section); test:* =
    // test fixtures (#464); empty = malformed row.
    assert_eq!(
        attribute_agent_tool("cli", "Read:foo", "read_disclose"),
        None
    );
    assert_eq!(attribute_agent_tool("mcp", "Bash:foo", "auto:json"), None);
    assert_eq!(attribute_agent_tool("test:proxy", "x", "x"), None);
    assert_eq!(attribute_agent_tool("", "", ""), None);
}

/// Our ticker takes its own line when it is sharing the status line with a
/// command the user already had.
///
/// Reported by a user whose other tool's KPI line and our figures "collapse"
/// into one run-on line. The hosts do support several lines — Copilot's runner
/// splits the output on newlines and pads each, Cursor renders into an
/// unconstrained Ink box. When the left part is our OWN synthesised
/// `model · dir` there is nothing competing for the line, and it stays one.
#[test]
fn the_ticker_takes_its_own_line_only_when_it_wrapped_someone_else() {
    // The FAMILY: the status line has had branches since the split.
    let root = tokenade_core::source_guard::crates_dir()
        .join("tokenade-cli")
        .join("src")
        .join("dashboard_cli");
    let src = tokenade_core::source_guard::module_family(&root, "statusline");
    let src = src.as_str();
    assert!(
        src.contains(r#"(false, true) => format!("{left}\n{ticker}")"#),
        "the wrapped case no longer breaks the line"
    );
    assert!(
        src.contains(r#"(false, false) => format!("{left}  {ticker}")"#),
        "our own synthesised left part should stay on one line"
    );
}
