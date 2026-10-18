//! Tests for `healthcheck`, split out of the flat `tests.rs`.

use super::*;

#[test]
fn pretool_matcher_covers_read_detects_bash_only() {
    // Healthy: matcher includes Read → Some(true).
    let good = r#"{"hooks":{"PreToolUse":[{"matcher":"Bash|Read|WebFetch|Task|Agent",
        "hooks":[{"type":"command","command":"tokenade hookwrap-pretool","_tag":"tokenade-pretool"}]}]}}"#;
    assert_eq!(pretool_matcher_covers_read(good), Some(true));
    // The field bug: Bash-only matcher → Some(false) (Read never deduped).
    let bash_only = r#"{"hooks":{"PreToolUse":[{"matcher":"Bash",
        "hooks":[{"type":"command","command":"tokenade hookwrap-pretool","_tag":"tokenade-pretool"}]}]}}"#;
    assert_eq!(pretool_matcher_covers_read(bash_only), Some(false));
    // `*` / empty matcher covers everything → Some(true).
    let star = r#"{"hooks":{"PreToolUse":[{"matcher":"*",
        "hooks":[{"type":"command","command":"x hookwrap-pretool"}]}]}}"#;
    assert_eq!(pretool_matcher_covers_read(star), Some(true));
    // No tokenade pretool entry → None (hook_installed reports that instead).
    let none = r#"{"hooks":{"PreToolUse":[{"matcher":"Bash",
        "hooks":[{"type":"command","command":"some-other-tool"}]}]}}"#;
    assert_eq!(pretool_matcher_covers_read(none), None);
    assert_eq!(pretool_matcher_covers_read("{}"), None);
}

#[cfg(target_os = "linux")] // /proc-shaped input; the classifier is Linux-only (D5)
#[test]
fn classify_stale_mcps_flags_deleted_tokenade_mcp_server() {
    // The canonical case: a `tokenade mcp` process whose on-disk
    // binary was replaced via `mv` — the kernel adds " (deleted)"
    // to the readlink output.
    let procs = vec![proc(
        "1234",
        "/usr/local/bin/tokenade (deleted)",
        "tokenade\0mcp\0",
    )];
    let stale = classify_stale_mcps(&procs, "9999");
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0].0, "1234");
    assert_eq!(stale[0].1, "/usr/local/bin/tokenade");
}

#[cfg(target_os = "linux")] // /proc-shaped input; the classifier is Linux-only (D5)
#[test]
fn classify_stale_mcps_excludes_self_pid() {
    // The healthcheck process itself MUST never appear in the
    // result — even if its own binary was just replaced.
    let procs = vec![proc(
        "5555",
        "/usr/local/bin/tokenade (deleted)",
        "tokenade\0mcp\0",
    )];
    let stale = classify_stale_mcps(&procs, "5555");
    assert!(stale.is_empty(), "self PID must be excluded");
}

#[cfg(target_os = "linux")] // /proc-shaped input; the classifier is Linux-only (D5)
#[test]
fn classify_stale_mcps_ignores_non_deleted_processes() {
    // A fresh tokenade-mcp matching the current binary doesn't
    // need a restart — no " (deleted)" suffix means it's still
    // running the current code.
    let procs = vec![proc(
        "100",
        "/home/u/.local/bin/tokenade-mcp",
        "tokenade-mcp\0",
    )];
    let stale = classify_stale_mcps(&procs, "9999");
    assert!(stale.is_empty());
}

#[cfg(target_os = "linux")] // /proc-shaped input; the classifier is Linux-only (D5)
#[test]
fn classify_stale_mcps_ignores_non_tokenade_binaries() {
    // Other deleted binaries (vim, cargo, etc.) are none of our
    // business — only basename "tokenade" or "tokenade-mcp".
    let procs = vec![
        proc("1", "/usr/bin/vim (deleted)", "vim\0"),
        proc("2", "/usr/bin/cargo (deleted)", "cargo\0build\0"),
    ];
    let stale = classify_stale_mcps(&procs, "9999");
    assert!(stale.is_empty());
}

#[cfg(target_os = "linux")] // /proc-shaped input; the classifier is Linux-only (D5)
#[test]
fn classify_stale_mcps_ignores_non_mcp_tokenade_invocations() {
    // `tokenade wrap …` and `tokenade auto …` shouldn't be flagged
    // even if their binary is deleted — they're transient.
    let procs = vec![
        proc("1", "/u/tokenade (deleted)", "tokenade\0wrap\0echo foo\0"),
        proc("2", "/u/tokenade (deleted)", "tokenade\0auto\0/tmp/x\0"),
        proc("3", "/u/tokenade (deleted)", "tokenade\0healthcheck\0"),
    ];
    let stale = classify_stale_mcps(&procs, "9999");
    assert!(stale.is_empty(), "non-mcp invocations leaked: {:?}", stale);
}

#[cfg(target_os = "linux")] // /proc-shaped input; the classifier is Linux-only (D5)
#[test]
fn classify_stale_mcps_matches_mcp_at_end_of_cmdline() {
    // cmdline ending with `…\0mcp` (no trailing NUL — some kernels
    // omit it on the final arg) must still match.
    let procs = vec![proc("777", "/u/tokenade (deleted)", "tokenade\0mcp")];
    let stale = classify_stale_mcps(&procs, "9999");
    assert_eq!(stale.len(), 1, "trailing \\0mcp pattern must match");
}

#[cfg(target_os = "linux")] // /proc-shaped input; the classifier is Linux-only (D5)
#[test]
fn classify_stale_mcps_matches_tokenade_mcp_basename_even_without_mcp_arg() {
    // A dedicated `tokenade-mcp` binary (no need for a `mcp`
    // sub-arg) — match by basename alone.
    let procs = vec![proc("42", "/u/tokenade-mcp (deleted)", "tokenade-mcp\0")];
    let stale = classify_stale_mcps(&procs, "9999");
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0].1, "/u/tokenade-mcp");
}

#[cfg(target_os = "linux")] // /proc-shaped input; the classifier is Linux-only (D5)
#[test]
fn classify_stale_mcps_returns_multiple_stale_entries() {
    // Real production case: a user with 2 AI agents (Claude Code
    // + Codex) → 2 stale MCP servers after a `mv` redeploy.
    let procs = vec![
        proc("100", "/u/tokenade (deleted)", "tokenade\0mcp\0"),
        proc("200", "/u/tokenade (deleted)", "tokenade\0mcp\0"),
    ];
    let stale = classify_stale_mcps(&procs, "9999");
    assert_eq!(stale.len(), 2);
}

#[test]
fn format_stale_mcp_nudge_empty_with_when_clean() {
    // Empty stale + Some(msg) → prints the clean-state confirmation
    // (cmd_upgrade uses this to confirm the upgrade is live).
    let s = super::format_stale_mcp_nudge(&[], Some("upgrade is live."));
    assert_eq!(s, "  upgrade is live.\n");
}

#[test]
fn format_stale_mcp_nudge_empty_without_when_clean() {
    // Empty stale + None → empty string (cmd_install: success line
    // already covers "you're set", no extra noise needed).
    let s = super::format_stale_mcp_nudge(&[], None);
    assert_eq!(s, "");
}

#[test]
fn format_stale_mcp_nudge_single_pid_full_block() {
    let stale = vec![("1234".to_string(), "/u/local/bin/tokenade".to_string())];
    let s = super::format_stale_mcp_nudge(&stale, None);
    assert!(s.contains("1 tokenade MCP process(es)"), "got: {s}");
    assert!(s.contains("pid 1234 → /u/local/bin/tokenade"), "got: {s}");
    assert!(s.contains("Restart your AI agent"), "got: {s}");
    assert!(
        !s.contains("... "),
        "1 entry should not show truncation: {s}"
    );
}

#[test]
fn format_stale_mcp_nudge_when_clean_ignored_if_stale_nonempty() {
    // when_clean is for the empty-stale case; if there ARE stale
    // entries, the rich block prints regardless.
    let stale = vec![("1".to_string(), "/u/tokenade".to_string())];
    let s = super::format_stale_mcp_nudge(&stale, Some("upgrade is live."));
    assert!(
        !s.contains("upgrade is live"),
        "clean msg leaked into stale path: {s}"
    );
    assert!(s.contains("Restart your AI agent"), "got: {s}");
}

// #613 — collect_health_checks must produce parseable JSON that
// round-trips through serde and exposes the per-check `id`,
// `status`, `message`, `detail` shape callers depend on.
#[test]
fn healthcheck_json_render_round_trips_via_serde() {
    let sample = [
        super::HealthCheckResult {
            id: "binary",
            status: super::CheckStatus::Ok,
            message: "binary at /x/tokenade".into(),
            detail: vec![],
        },
        super::HealthCheckResult {
            id: "stale_mcp_procs",
            status: super::CheckStatus::Fail,
            message: "1 tokenade MCP process(es) running an OLD binary".into(),
            detail: vec!["- pid 1234 → /x/tokenade".into()],
        },
    ];
    // Render to a String by capturing what `render_health_checks_json`
    // emits. The function `println!`s; rebuild the same JSON here so
    // the test stays hermetic and doesn't shell out.
    let entries: Vec<serde_json::Value> = sample
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
        "failures": 1usize,
        "ok": false,
        "checks": entries,
    });
    let s = serde_json::to_string(&root).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert_eq!(parsed["ok"], serde_json::json!(false));
    assert_eq!(parsed["failures"], serde_json::json!(1));
    assert_eq!(parsed["checks"].as_array().unwrap().len(), 2);
    assert_eq!(parsed["checks"][0]["id"], serde_json::json!("binary"));
    assert_eq!(parsed["checks"][0]["status"], serde_json::json!("ok"));
    assert_eq!(parsed["checks"][1]["status"], serde_json::json!("fail"));
    assert_eq!(
        parsed["checks"][1]["detail"][0],
        serde_json::json!("- pid 1234 → /x/tokenade")
    );
}

/// Doctor: a config pointing at a DEAD absolute tokenade path must FAIL
/// loudly (field case: hooks pinned to a removed /opt/homebrew binary =
/// total silent fail-open), and healthy paths pass.
#[test]
fn doctor_flags_stale_binary_paths() {
    let home = tmp("docstale");
    fs::create_dir_all(home.join(".claude")).unwrap();
    // THE SHAPE THE INSTALLER ACTUALLY WRITES.
    //
    // This fixture used to be `"/nonexistent/vendor/tokenade hookwrap-pretool"`
    // — the exe as the first token — and the check read the first whitespace
    // token as the exe path. Every command tokenade installs begins
    // `sh -c '…'`, so in production that token was `sh`, the check found
    // nothing, and it reported `all agent-config hook paths resolve` in the
    // same report as the FAIL that had found the missing binary. A fixture in
    // a dialect the product never emits is a guard that cannot fail.
    let real_shape = r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"sh -c 'TOKENADE_DEBUG_LOG='\''/tmp/d.log'\'' '\''/nonexistent/vendor/tokenade'\'' hookwrap-pretool 2>/dev/null; exit 0'"}]}]}}"#;
    fs::write(home.join(".claude/settings.json"), real_shape).unwrap();
    let r = crate::healthcheck::check_stale_binary_paths(&home);
    assert!(
        matches!(r.status, crate::healthcheck::CheckStatus::Fail),
        "{:?}",
        r.message
    );
    assert!(r
        .detail
        .iter()
        .any(|d| d.contains("/nonexistent/vendor/tokenade")));

    // The bare-exe shape still works: fixing the extraction must not trade one
    // dialect for another.
    fs::write(
        home.join(".claude/settings.json"),
        r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"/nonexistent/vendor/tokenade hookwrap-pretool"}]}]}}"#,
    )
    .unwrap();
    let r = crate::healthcheck::check_stale_binary_paths(&home);
    assert!(
        matches!(r.status, crate::healthcheck::CheckStatus::Fail),
        "the plain form must still be caught: {:?}",
        r.message
    );

    // PRECISION, not just recall. The extractor walks every token of the JSON,
    // so what it must NOT report matters as much as what it must: a missing
    // path that is not a tokenade binary (the debug-log target, which is
    // created on first write and legitimately absent before it), and a
    // relative `tokenade` that the shell resolves through PATH.
    fs::write(
        home.join(".claude/settings.json"),
        r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"sh -c 'TOKENADE_DEBUG_LOG='\''/tmp/tokenade-not-created-yet.log'\'' tokenade hookwrap-pretool'"}]}]}}"#,
    )
    .unwrap();
    let r = crate::healthcheck::check_stale_binary_paths(&home);
    // The invariant is that nothing is REPORTED dead, not which colour the row
    // gets. There is no path here to resolve — the exe is a bare name the shell
    // resolves through PATH — so the row is now Info ("none to resolve") rather
    // than a green "all agent-config hook paths resolve", which claimed a
    // verdict over an empty set. What must never happen is a Fail, or a detail
    // line naming one of these two as a dead binary.
    assert!(
        !matches!(r.status, crate::healthcheck::CheckStatus::Fail),
        "neither a non-binary path nor a PATH-resolved name is a dead exe: {:?}",
        r.detail
    );
    assert!(
        r.detail.is_empty(),
        "nothing here is a dead exe path: {:?}",
        r.detail
    );

    // Existing path → Ok.
    //
    // The path has to be one the extractor COLLECTS, and its rule is that the
    // basename is exactly `tokenade` or `tokenade-mcp`. This fixture used
    // `current_exe()`, which under the test runner is `…/deps/tokenade-<hash>`
    // — skipped by that rule — so the check examined ZERO paths and the `Ok`
    // asserted here was the green row it returns over an empty set. The
    // assertion could not fail for the reason it names. Measured 2026-09-03,
    // when that empty case stopped being `Ok` and this line went red.
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let exe = bin_dir.join("tokenade");
    fs::write(&exe, b"#!/bin/sh\nexit 0\n").unwrap();
    let exe = exe.display().to_string();
    fs::write(
        home.join(".claude/settings.json"),
        format!(r#"{{"hooks":{{"PreToolUse":[{{"matcher":"Bash","hooks":[{{"type":"command","command":"{exe} hookwrap-pretool"}}]}}]}}}}"#),
    )
    .unwrap();
    let r = crate::healthcheck::check_stale_binary_paths(&home);
    assert!(
        matches!(r.status, crate::healthcheck::CheckStatus::Ok),
        "{:?}",
        r.detail
    );
    assert!(
        r.message.contains("resolve"),
        "and the OK must be the one about paths that resolve: {:?}",
        r.message
    );
}

// The failure this chantier is named after: a bridge that never loads looks
// exactly like a working one — no error, no warning, a gain that stays at zero.
// Hermes could already tell installed / loaded / firing apart; the other five
// said nothing. Both answers pinned, because "installed but silent" is the one
// that matters and it is the one that used to be invisible.
#[test]
fn a_plugin_bridge_that_never_fired_is_reported_as_such() {
    let home = std::env::temp_dir().join(format!(
        "tok-bridge-health-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join(".tokenade")).unwrap();

    // No bridge installed → the check says nothing at all.
    assert!(check_plugin_bridges(&home).is_empty());

    install_opencode_plugin(&home, false).unwrap();
    let silent = check_plugin_bridges(&home);
    assert_eq!(silent.len(), 1, "one installed bridge, one verdict");
    assert!(
        silent[0].message.contains("opencode") && silent[0].message.contains("installed but"),
        "an installed-but-silent bridge must say so: {}",
        silent[0].message
    );

    // One RECENT saving attributed to opencode is the proof the bridge ran.
    // The timestamp is no longer a placeholder: a saving is only evidence that
    // the bridge works *now* while it is recent, so `"ts":1` — the epoch — now
    // reads as a bridge that stopped loading in 1970.
    std::fs::write(
        home.join(".tokenade").join("gain.jsonl"),
        format!(
            "{{\"ts\":{},\"before\":100,\"after\":10,\"source\":\"read\",\"agent\":\"opencode\"}}\n",
            tokenade_core::platform::now_unix_secs()
        ),
    )
    .unwrap();
    let firing = check_plugin_bridges(&home);
    assert!(
        firing[0].message.contains("firing"),
        "a bridge with a recorded saving is firing: {}",
        firing[0].message
    );

    let _ = std::fs::remove_dir_all(&home);
}

