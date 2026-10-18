//! Tests for `misc`, split out of the flat `tests.rs`.

use super::*;

/// #996 — `released_age_suffix` formats the unix-seconds
/// `manifest.released_at` into the friendly suffix shown by
/// `tokenade upgrade --check`. Guards both the unset-zero case
/// AND a future timestamp (a just-published manifest under clock
/// skew shouldn't print a negative age).
#[test]
fn released_age_suffix_buckets() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert_eq!(super::released_age_suffix(0), "");
    // Future timestamp → silent ("").
    assert_eq!(super::released_age_suffix(now + 3600), "");
    // <1h ago.
    assert_eq!(super::released_age_suffix(now - 60), ", released <1h ago");
    // Mid-day window.
    let s = super::released_age_suffix(now - 5 * 3600);
    assert_eq!(s, ", released 5h ago", "got: {s}");
    // Exactly 1 day → singular.
    let s = super::released_age_suffix(now - 24 * 3600);
    assert_eq!(s, ", released 1 day ago", "got: {s}");
    // Plural days.
    let s = super::released_age_suffix(now - 7 * 24 * 3600);
    assert_eq!(s, ", released 7 days ago", "got: {s}");
}

#[test]
fn cli_version_prints_zero() {
    // `tokenade --version` always exits 0.
    let code = run(&args(&["--version"]));
    assert_eq!(code, 0);
}

#[test]
fn cli_help_exits_zero() {
    assert_eq!(run(&args(&["help"])), 0);
    assert_eq!(run(&args(&["--help"])), 0);
    assert_eq!(run(&args(&["-h"])), 0);
}

#[test]
fn recovery_cli_exit_codes() {
    // Zero-MCP pivot: the recovery tools (expand-ref / expand-fold /
    // search-stash) that previously only existed as MCP endpoints are
    // now first-class CLI commands. Lock in their exit-code contract so
    // a regression in arg parsing or dispatch is caught in-process
    // (a subprocess smoke-test is swallowed by the shell-proxy wrapper).
    with_tmp_cwd(|| {
        // missing required arg → usage, rc=2
        assert_eq!(run(&args(&["expand-ref"])), 2, "expand-ref no hash → usage");
        assert_eq!(
            run(&args(&["expand-fold"])),
            2,
            "expand-fold no template → usage"
        );
        // well-formed but absent → not-found, rc=1
        assert_eq!(
            run(&args(&["expand-ref", "0123456789ab"])),
            1,
            "unknown hash → not found",
        );
        assert_eq!(
            run(&args(&["expand-fold", "no_such_template"])),
            1,
            "unknown template → not found",
        );
        // `search-stash` was removed: `expand-ref --prompt` subsumes it (same
        // RAG, multi-question, small blobs served whole). An unknown command
        // must not silently resolve.
        assert_ne!(
            run(&args(&["search-stash", "deadbeef", "query"])),
            0,
            "search-stash must be gone, not silently succeed",
        );
    });
}

// #506 — `tokenade discover`'s markdown table used a fixed
// 24-char tool-name column. MCP tool names exceeding that
// (e.g. `mcp__plugin_playwright_playwright__browser_navigate`)
// overflowed and pushed the row's `|` divider out of alignment
// with shorter rows, breaking markdown rendering. This pins
// the dynamic-width fix: a name longer than the divider row
// (which counts hyphens between the first two `|`s) is a
// regression.
#[test]
fn discover_table_column_alignment_holds_for_long_names() {
    // Pure-function reproduction of the format code in
    // cmd_discover. If the production code changes, this test
    // catches a divergence between the divider width and the
    // row width.
    let rows: Vec<(String, u64, u64)> = vec![
        ("Bash".into(), 100, 1_000),
        (
            "mcp__plugin_playwright_playwright__browser_navigate".into(),
            50,
            500,
        ),
        ("ScheduleWakeup".into(), 10, 100),
    ];
    let name_w = rows
        .iter()
        .map(|(n, _, _)| n.chars().count())
        .max()
        .unwrap_or(4)
        .max(4);
    let header = format!("| {:<name_w$} | calls | total bytes |", "tool");
    let header_pad = "-".repeat(name_w);
    let divider = format!(
        "|{:-<w$}|-------|-------------|",
        header_pad,
        w = name_w + 2
    );

    // Every row's `|` divider position must match the header's.
    let header_first_pipe = header.find(" | ").unwrap();
    for (name, calls, bytes) in &rows {
        let row = format!("| {:<name_w$} | {:>5} | {:>11} |", name, calls, bytes);
        assert_eq!(
            row.find(" | ").unwrap(),
            header_first_pipe,
            "row `{name}` misaligns the first `|` divider"
        );
        // And the divider line's `|` matches too.
        assert_eq!(
            divider.find("|-------|").unwrap(),
            row.find(" | ").unwrap() + 1,
            "divider line misaligns relative to data row `{name}`"
        );
    }
}

// #505 — `tokenade help <subcmd>` previously discarded the
// subcmd arg. Now it forwards as `tokenade <subcmd> --help`,
// landing the user on the dedicated help text for that command
// (or the general fallback when none exists). Smoke-only — we
// don't capture stdout in this test harness, so we just assert
// the exit codes and absence of recursion / infinite loop.
#[test]
fn help_subcmd_forwards_and_exits_zero() {
    // Dedicated help (install_help_text — #412).
    assert_eq!(run(&args(&["help", "install"])), 0);
    // Dedicated help (scrub-secret — #483).
    assert_eq!(run(&args(&["help", "scrub-secret"])), 0);
    // No dedicated help → general fallback.
    assert_eq!(run(&args(&["help", "dashboard"])), 0);
    // Unknown subcmd → forwarded `<unknown> --help` lands in
    // the unknown-command path which returns 2 (the universal
    // --help interceptor doesn't fire because the subcmd is
    // unknown). Either 0 or 2 is acceptable — what we MUST NOT
    // do is recurse. So just assert it terminates.
    let _ = run(&args(&["help", "definitely-not-a-cmd"]));
    // Recursion guard: `help help` must not loop.
    assert_eq!(run(&args(&["help", "help"])), 0);
    assert_eq!(run(&args(&["help", "--help"])), 0);
    assert_eq!(run(&args(&["help", "-h"])), 0);
}

#[test]
fn cli_unknown_command_exits_with_code_2() {
    let code = run(&args(&["definitely-not-a-cmd"]));
    assert_eq!(code, 2);
}

// #507 — flag-shaped unknown args (anything starting with `-`)
// must NOT be funneled into the command-name fuzzy match.
// `-v` previously yielded "did you mean 'csv'?" (Levenshtein 2);
// now it points at `--version`. Exit code stays 2 for all
// unknown args (flag or command).
#[test]
fn cli_unknown_flag_exits_2_and_does_not_fuzzy_match_a_command() {
    // We can't capture stderr in this harness, but the exit code
    // proves we hit the unknown-flag arm (which also returns 2).
    assert_eq!(run(&args(&["-v"])), 2);
    assert_eq!(run(&args(&["--foo"])), 2);
    assert_eq!(run(&args(&["-x"])), 2);
    // Sanity: known flags still work.
    assert_eq!(run(&args(&["--version"])), 0);
    assert_eq!(run(&args(&["-V"])), 0);
}

#[test]
fn cli_dashboard_runs_on_empty_dir() {
    with_tmp_cwd(|| {
        let code = run(&args(&["dashboard"]));
        assert_eq!(code, 0);
    });
}

#[test]
fn cli_index_first_full_then_incremental() {
    with_tmp_cwd(|| {
        // Plant a small Rust file so the index has something to chew.
        std::fs::write("a.rs", "pub fn alpha() {}").unwrap();
        assert_eq!(run(&args(&["index"])), 0);
        // The index must be PERSISTED where `load` finds it. This asserted
        // `.tokenade/index.json` exists, which was a stand-in for exactly that
        // — and a stand-in that stopped meaning it: the sidecar is a migration
        // artefact `Index::save` now removes once the store holds the index,
        // so the run that persists correctly is the one that leaves no
        // sidecar. Asking `load` asks the real question.
        let cwd = std::env::current_dir().unwrap();
        let loaded = tokenade_core::index::Index::load(&cwd)
            .expect("the first index must be retrievable afterwards");
        assert!(
            loaded.symbols.iter().any(|s| s.name == "alpha"),
            "the persisted index does not hold what was indexed"
        );
        assert!(
            !std::path::Path::new(".tokenade/index.json").exists(),
            "a stale sidecar was left in the project"
        );
        // Second invocation must not crash and reuses the cached index.
        assert_eq!(run(&args(&["index"])), 0);
    });
}

#[test]
fn cli_ask_handles_empty_index_gracefully() {
    with_tmp_cwd(|| {
        // No files indexed yet — `tokenade ask` builds on the fly. With no
        // source files the semantic index is empty and the command
        // returns 0 with a "no match" message.
        let code = run(&args(&["ask", "anything"]));
        assert_eq!(code, 0);
    });
}

#[test]
fn cli_map_runs_on_empty_dir() {
    with_tmp_cwd(|| {
        // Empty repo → structure_map says "0 files, 0 symbols indexed".
        assert_eq!(run(&args(&["map"])), 0);
    });
}

#[test]
fn cli_detect_returns_zero() {
    with_tmp_cwd(|| {
        assert_eq!(run(&args(&["detect"])), 0);
    });
}

#[test]
fn cli_proxy_requires_a_command() {
    // No argv after `proxy` → usage error → exit code 2.
    let code = run(&args(&["proxy"]));
    assert_eq!(code, 2);
}

// #617 — `tokenade dashboard --json` payload shape lock-in. Same
// pattern as the gain-json and healthcheck-json tests: build the
// value hermetically, parse it back, assert the contract CI
// dashboards rely on.
#[test]
fn dashboard_json_shape_is_stable() {
    let per_op = [
        ("auto:json".to_string(), (1000u64, 200u64, 5u64)),
        ("auto:yaml".to_string(), (500u64, 100u64, 2u64)),
    ];
    let payload = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "root": "/x/proj",
        "ledger_path": "/x/proj/.tokenade/gain.jsonl",
        "operations": 7u64,
        "tokens_before": 1500u64,
        "tokens_after": 300u64,
        "tokens_saved": 1200u64,
        "savings_pct": 80.0_f64,
        "cache_hits": 42u64,
        "cache_misses": 8u64,
        "cache_hit_rate": 0.84_f64,
        "per_op": per_op.iter().map(|(k,(b,a,n))| serde_json::json!({
            "op": k,
            "before": b,
            "after": a,
            "n": n,
            "pct": if *b > 0 { 100.0 * (*b as f64 - *a as f64) / *b as f64 } else { 0.0 }
        })).collect::<Vec<_>>(),
    });
    let s = serde_json::to_string(&payload).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert_eq!(parsed["operations"], serde_json::json!(7));
    assert_eq!(parsed["tokens_saved"], serde_json::json!(1200));
    assert_eq!(parsed["cache_hits"], serde_json::json!(42));
    let arr = parsed["per_op"].as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["op"], serde_json::json!("auto:json"));
    // 1000 → 200 is exactly 80% saved.
    let pct = arr[0]["pct"].as_f64().unwrap();
    assert!(
        (pct - 80.0).abs() < 0.01,
        "per_op pct should be ~80, got {pct}"
    );
}

// #617-class regression: dashboard --json must NOT underflow when
// an `op` is inflated (after > before). Mirrors the #492 guard in
// cmd_dashboard_html: the cast-to-f64 happens BEFORE the
// subtraction so the result is a signed negative percent.
#[test]
fn dashboard_json_pct_is_signed_on_inflation() {
    let b: u64 = 100;
    let a: u64 = 150;
    let pct = if b > 0 {
        100.0 * (b as f64 - a as f64) / b as f64
    } else {
        0.0
    };
    assert!(
        pct < 0.0,
        "inflation case should produce negative pct, got {pct}"
    );
    assert!((pct - (-50.0)).abs() < 0.01);
}

// #694 — compose_edit_diff_hint must stay silent on the easy
// negative paths (no fs, no dedup_ref state): file doesn't exist,
// file too small, file is a directory. Positive case (prior cache
// hit) needs a temp dedup_ref store which crosses module
// boundaries — skipped here, deferred to e2e.
#[test]
fn compose_edit_diff_hint_silent_when_file_missing() {
    // Use a path under our session-scoped TOKENADE_STASH_DIR if set,
    // else a clearly-bogus path. Either way, the file doesn't exist.
    let bogus = "/nonexistent/file/that/cannot/exist.rs";
    let result = crate::edit_hint::compose_edit_diff_hint(std::path::Path::new("/tmp"), bogus);
    assert!(
        result.is_none(),
        "missing file must not emit hint, got: {result:?}"
    );
}

#[test]
fn compose_edit_diff_hint_silent_on_tiny_file() {
    // Write a tiny file (< MIN_BYTES_TO_DEDUPE = 1500). The hint
    // overhead (~50 tokens) wouldn't pay back here.
    let dir = std::env::temp_dir().join(format!("tokenade-edit-hint-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("tiny.rs");
    std::fs::write(&f, b"fn x() {}\n").unwrap();
    let result = crate::edit_hint::compose_edit_diff_hint(&dir, f.to_str().unwrap());
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        result.is_none(),
        "tiny file must not emit hint, got: {result:?}"
    );
}

#[test]
fn edited_file_path_extracts_from_post_tool_use_stdin() {
    // #712 — happy path: valid Edit JSON with file_path field.
    let stdin = serde_json::json!({
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "/tmp/foo.rs",
            "old_string": "x",
            "new_string": "y",
        }
    })
    .to_string();
    assert_eq!(
        super::edited_file_path(Some(stdin.as_str())),
        Some("/tmp/foo.rs".to_string())
    );
}

#[test]
fn edited_file_path_silent_on_non_edit_tool() {
    // Read / Bash / Glob — never edits. Must return None so
    // the outside-root check is never applied to non-edits.
    let stdin = serde_json::json!({
        "tool_name": "Read",
        "tool_input": { "file_path": "/tmp/foo.rs" }
    })
    .to_string();
    assert!(super::edited_file_path(Some(stdin.as_str())).is_none());
}

#[test]
fn edited_file_path_silent_on_garbage() {
    assert!(super::edited_file_path(None).is_none());
    assert!(super::edited_file_path(Some("")).is_none());
    assert!(super::edited_file_path(Some("not json")).is_none());
    assert!(super::edited_file_path(Some("{}")).is_none());
}

#[test]
fn path_within_root_true_for_nested_child() {
    // #712 — a file under the root must be detected as inside.
    // Use the workspace's own README.md which is definitely
    // under the cargo CARGO_MANIFEST_DIR root.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let child = format!("{}/Cargo.toml", root.display());
    assert!(super::path_within_root(root, &child));
}

#[test]
fn path_within_root_false_for_unrelated_path() {
    let root = std::env::temp_dir().join(format!(
        "tokenade-prw-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&root).unwrap();
    // `~/.tokenade/...` style — definitely outside this tempdir.
    let other = "/etc/hostname";
    let inside = super::path_within_root(&root, other);
    let _ = std::fs::remove_dir_all(&root);
    assert!(!inside, "unrelated path must not register as inside-root");
}

// #649 — get_flag must return None when the value looks like
// another flag (starts with `--`). Single `-` (stdin marker)
// must still pass through.
#[test]
fn get_flag_rejects_double_dash_value_returns_some_for_single_dash() {
    let rest = vec![
        "--lang".to_string(),
        "--script".to_string(), // Looks like a flag, not a real lang.
        "print(1)".to_string(),
    ];
    assert_eq!(super::get_flag(&rest, "--lang"), None);

    let rest = vec!["--script".to_string(), "-".to_string()];
    // Single dash is the stdin marker — preserved.
    assert_eq!(super::get_flag(&rest, "--script"), Some("-"));

    let rest = vec!["--lang".to_string(), "python".to_string()];
    // Plain string value — passes through.
    assert_eq!(super::get_flag(&rest, "--lang"), Some("python"));

    let rest = vec!["--lang".to_string()];
    // No value at all — returns None.
    assert_eq!(super::get_flag(&rest, "--lang"), None);
}

#[test]
fn bot_wall_status_is_read_from_the_curl_trailer() {
    // curl exits 0 on an HTTP 403 (no `-f`), so the ONLY evidence that a fetch
    // was refused is the status in the `-w` trailer. Losing it is what made the
    // browser-profile retry unreachable on exactly the hosts it exists for.
    use tokenade_core::web_fetch::{split_meta_trailer, META_SENTINEL, META_SEP};
    let wall = format!(
        "<html>Access Denied</html>{META_SENTINEL}403{META_SEP}https://x.test/{META_SEP}text/html"
    );
    assert_eq!(
        split_meta_trailer(wall.into_bytes()).status,
        403,
        "403 must be recoverable from the trailer"
    );
    // Binary body: the status must survive bytes that are not UTF-8.
    let mut pdf: Vec<u8> = vec![0x25, 0x50, 0x44, 0x46, 0xff, 0xfe, 0x00];
    pdf.extend_from_slice(
        format!("{META_SENTINEL}200{META_SEP}{META_SEP}application/pdf").as_bytes(),
    );
    let f = split_meta_trailer(pdf);
    assert_eq!(f.status, 200);
    assert_eq!(f.content_type.as_deref(), Some("application/pdf"));
    // No trailer (curl died before writing it) → no evidence, not a refusal.
    assert_eq!(split_meta_trailer(b"<html>ok</html>".to_vec()).status, 200);
}

#[test]
fn webfetch_native_failure_detector() {
    use super::webfetch_native_result_looks_failed as failed;
    // Failures: empty, short with explicit markers.
    assert!(failed(""));
    assert!(failed("   \n"));
    assert!(failed("Error fetching content: HTTP 403 Forbidden"));
    assert!(failed("Request failed: timed out after 30s"));
    assert!(failed("Please enable JavaScript to view this page."));
    // Successes: a real answer — even one that MENTIONS an error code —
    // and any long summary.
    assert!(!failed("The default keepalive_timeout is 75 seconds."));
    let long = "word ".repeat(200) + "403";
    assert!(!failed(&long));
}

#[test]
fn read_binary_stash_path_appends_txt_extension() {
    // #953 lock-in: when we stash the extracted text from a binary
    // file (PDF/DOCX/etc.), the SourceHint::File path MUST get a
    // `.txt` appended so disclose stashes as `<hash>.txt`, NOT
    // `<hash>.pdf` which would lie about the file's actual content
    // (text, not binary). Tests the path-construction pattern
    // directly since the surrounding flow needs filesystem I/O.
    let original = std::path::PathBuf::from("/tmp/arxiv_test.pdf");
    let mut p = original.clone().into_os_string();
    p.push(".txt");
    let stash_hint = std::path::PathBuf::from(p);
    assert_eq!(
        stash_hint.extension().and_then(|e| e.to_str()),
        Some("txt"),
        "stash hint must end in .txt, not .pdf (got {stash_hint:?})",
    );
    // Path with no extension also works (just gets `.txt`).
    let no_ext = std::path::PathBuf::from("/tmp/foo");
    let mut p2 = no_ext.into_os_string();
    p2.push(".txt");
    let stash_hint_2 = std::path::PathBuf::from(p2);
    assert_eq!(
        stash_hint_2.extension().and_then(|e| e.to_str()),
        Some("txt"),
    );
}

// #804-#808 — silent-accept sweep across all compactor cmd_*.
// validate_stdin_or_single_file is the helper underpinning
// the surviving stdin/file compactor commands (`read` / `web` / `serp` /
// `snapshot`) and
// the read_stdin_or_file_with_tty_hint chain. Lock the 5
// canonical branches so a future regression breaks visibly.
#[test]
fn validate_stdin_or_single_file_accepts_empty() {
    let args: Vec<String> = vec![];
    assert_eq!(
        super::validate_stdin_or_single_file(&args, "dns", &[]),
        Ok(None)
    );
}

#[test]
fn validate_stdin_or_single_file_accepts_dash() {
    let args = vec!["-".to_string()];
    assert_eq!(
        super::validate_stdin_or_single_file(&args, "dns", &[]),
        Ok(None)
    );
}

#[test]
fn validate_stdin_or_single_file_accepts_file_path() {
    let args = vec!["scrape.txt".to_string()];
    assert_eq!(
        super::validate_stdin_or_single_file(&args, "dns", &[]),
        Ok(Some("scrape.txt".to_string()))
    );
}

#[test]
fn validate_stdin_or_single_file_rejects_unknown_flag() {
    let args = vec!["-".to_string(), "--bogus".to_string()];
    assert_eq!(
        super::validate_stdin_or_single_file(&args, "dns", &[]),
        Err(2)
    );
}

#[test]
fn validate_stdin_or_single_file_rejects_extra_positional() {
    let args = vec!["scrape.txt".to_string(), "extra".to_string()];
    assert_eq!(
        super::validate_stdin_or_single_file(&args, "dns", &[]),
        Err(2)
    );
}

// #665 — `tokenade help <typo>` used to silently dump top-level
// usage. Now we surface the typo with a Levenshtein hint using
// the same `nearest_command` helper #414 wired in.
#[test]
fn nearest_command_suggests_install_for_intall_typo() {
    assert_eq!(super::nearest_command("intall", 3), Some("install"));
}

#[test]
fn nearest_command_returns_none_for_far_input() {
    // Distance > 3 — refuse to guess.
    assert_eq!(super::nearest_command("xxxxxxxxxxxxxxx", 3), None);
}

#[test]
fn nearest_command_accepts_exact_command() {
    // Distance 0 — exact match always wins.
    assert_eq!(super::nearest_command("install", 3), Some("install"));
}

/// The global fence is a short "Tokenade is installed" pointer. The
/// 60-95% range lives in the project scaffold / MCP self-description,
/// not here — repeating it stacked a third copy of the lecture.
#[test]
fn global_claude_md_block_uses_canonical_savings_range() {
    assert!(
        GLOBAL_CLAUDE_MD_BLOCK.contains("Tokenade is installed"),
        "GLOBAL_CLAUDE_MD_BLOCK must still name the install: {GLOBAL_CLAUDE_MD_BLOCK}",
    );
    assert!(
        GLOBAL_CLAUDE_MD_BLOCK.contains("this machine's agent config"),
        "GLOBAL_CLAUDE_MD_BLOCK must point at the machine lecture: {GLOBAL_CLAUDE_MD_BLOCK}",
    );
    assert!(
        !GLOBAL_CLAUDE_MD_BLOCK.contains("tokenade map"),
        "GLOBAL_CLAUDE_MD_BLOCK must not dump the CLI lecture: {GLOBAL_CLAUDE_MD_BLOCK}",
    );
    assert!(
        !GLOBAL_CLAUDE_MD_BLOCK.contains("60–99"),
        "GLOBAL_CLAUDE_MD_BLOCK must not use em-dash 60–99% \
         — it's overstated and inconsistent with every other surface",
    );
}

/// `tokenade hooks status` must name the subcommand each hook runs.
///
/// It extracted the LAST whitespace token of the command, but the installer
/// writes `sh -c '… tokenade hookwrap 2>/dev/null; exit 0'` — whose last
/// token is `0'`. Every row rendered `→ 0`, which next to a hook reads as
/// "fired zero times", i.e. as a broken install on a perfectly wired machine.
#[test]
fn hook_variant_is_the_subcommand_not_the_exit_code() {
    for (cmd, want) in [
        (
            "sh -c 'TOKENADE_DEBUG_LOG='\\''/h/.tokenade/debug.log'\\'' '\\''/usr/bin/tokenade'\\'' hookwrap 2>/dev/null; exit 0'",
            "hookwrap",
        ),
        (
            "sh -c '/usr/bin/tokenade hookwrap-postread 2>/dev/null; exit 0'",
            "hookwrap-postread",
        ),
        (
            "sh -c '/usr/bin/tokenade index-touch 2>/dev/null; exit 0'",
            "index-touch",
        ),
        (
            "sh -c '/usr/bin/tokenade pre-edit-snapshot 2>/dev/null; exit 0'",
            "pre-edit-snapshot",
        ),
    ] {
        assert_eq!(
            crate::diagnostics_cli::hook_variant_of(cmd),
            want,
            "command: {cmd}"
        );
    }
}

/// The scaffold now states two things about `wrap` that used to be silent.
/// They are promises the product ships into every project's rules file, so
/// they must stay true — and the second one is surprising enough that an
/// agent reading the file will rely on it.
#[test]
fn scaffold_states_wraps_exit_code_and_stream_behaviour() {
    let block = tokenade_core::project_scaffold::scaffold_block(false);
    assert!(
        block.contains("keeps the exit code exactly"),
        "the exit-code promise must be stated"
    );
    assert!(
        block.contains("MERGES stderr into stdout"),
        "the stream merge must be stated — measured 2026-08-07: \
         `wrap 'echo OUT; echo ERR >&2' 2>/dev/null` still prints ERR"
    );
    assert!(
        block.contains("2>err.log"),
        "and the way around it must be named, not left to be discovered"
    );
}

/// Every process spawn either suppresses the Windows console window or is
/// listed here as deliberately inheriting one.
///
/// Windows allocates a FRESH console for a console-subsystem child whenever
/// the parent has none — the normal case for a hook fired by a GUI agent, or
/// for any detached tokenade child. The window is painted and vanishes as the
/// child exits, so a hook-driven session shows a stream of black rectangles
/// opening and closing. `configure_no_window` suppresses it without touching
/// the inherited handles.
///
/// The helper has existed for a while and that was never the problem: the
/// problem is that a NEW spawn does not have to remember it. Swept 2026-08-25
/// and ten sites were uncovered, the worst being `run_original_statusline` —
/// the host re-runs the status line on every render (Cursor throttles to about
/// three a second while streaming), so a Windows user saw a `cmd.exe` window
/// several times a second. This test exists so the next spawn has to make the
/// choice on purpose. Found by reading `token-optimizer`'s own full-tree sweep
/// of the same class (`fix(windows): eliminate console-window flashes across
/// every spawn surface`).
#[test]
fn every_spawn_either_hides_its_console_or_is_listed_as_wanting_one() {
    use std::path::Path;

    /// `file:line-ish` → why this spawn keeps a console. Matched on the file
    /// and the spawned program so a line-number shift does not break the test.
    const INHERITS_ON_PURPOSE: &[(&str, &str, &str)] = &[
        (
            "bench_cli.rs",
            "\"sh\"",
            "the bench runs commands for a human watching",
        ),
        (
            "cleanup.rs",
            "\"sudo\"",
            "sudo must reach the user's terminal to prompt",
        ),
        (
            "cleanup.rs",
            "&command",
            "the user asked for this command's own output",
        ),
        (
            "legacy_shim_removal.rs",
            "&target",
            "re-exec of the REAL command the shim shadowed — the user's own, interactive",
        ),
        (
            "shell_proxy.rs",
            "prog",
            "passthrough, and it only fires when stdin IS a terminal",
        ),
        (
            "browser.rs",
            "\"xdg-open\"",
            "cfg(unix, not(macos)) — no console concept",
        ),
        ("autostart.rs", "\"stat\"", "BSD `stat -f`, macOS only"),
        ("autostart.rs", "\"sh\"", "Unix only"),
        (
            "autostart.rs",
            "\"cmd\"",
            "the cfg(not(windows)) stub of windows_shell — never called",
        ),
        ("detect.rs", "\"ps\"", "macOS only"),
        (
            "sandbox.rs",
            "&argv[0]",
            "configured 29 lines below, past this scanner's window",
        ),
        ("updater.rs", "args[0]", "cfg(target_os = \"macos\") only"),
    ];

    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .to_path_buf();
    let mut sources = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                // The bench crate is a developer tool, not something a user's
                // agent ever spawns; `tests` dirs are not shipped code.
                if name == "target" || name == "tests" || name == "tokenade-bench" {
                    continue;
                }
                stack.push(p);
            } else if p.extension().and_then(|s| s.to_str()) == Some("rs") {
                let n = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                // `*_tests.rs` and `tests.rs` are `#[cfg(test)]` modules that
                // live in `src/`: they are not shipped, and a test harness that
                // spawns a real agent WANTS its console.
                // `build.rs` runs at COMPILE time on a developer machine, not
                // in anybody's agent session.
                if n != "tests.rs" && n != "build.rs" && !n.ends_with("_tests.rs") {
                    sources.push(p);
                }
            }
        }
    }
    assert!(
        sources.len() > 50,
        "the source walk found almost nothing: {}",
        sources.len()
    );

    let mut gaps: Vec<String> = Vec::new();
    for path in &sources {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let lines: Vec<&str> = text.lines().collect();
        let file = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        for (i, line) in lines.iter().enumerate() {
            if !line.contains("Command::new(") {
                continue;
            }
            let t = line.trim_start();
            if t.starts_with("//") {
                continue;
            }
            let window = lines[i..(i + 26).min(lines.len())].join("\n");
            if window.contains("configure_no_window")
                || window.contains("configure_detached")
                || window.contains("spawn_fully_detached")
                || window.contains("creation_flags")
            {
                continue;
            }
            // A declaration names a MODULE, not a file: `updater.rs`
            // declared, `updater/swap.rs` blamed after a split, for the same
            // call and the same reason. The file's parent therefore counts as
            // its name too.
            let family = path
                .parent()
                .and_then(|d| d.file_name())
                .and_then(|s| s.to_str())
                .map(|d| format!("{d}.rs"));
            if INHERITS_ON_PURPOSE.iter().any(|(f, prog, _)| {
                (*f == file || Some(*f) == family.as_deref()) && line.contains(prog)
            }) {
                continue;
            }
            gaps.push(format!("{file}:{}  {}", i + 1, t.trim_end()));
        }
    }
    assert!(
        gaps.is_empty(),
        "these spawns neither suppress the Windows console nor declare that they want one.\n\
         Add `process_util::configure_no_window(&mut cmd)` when the stdio is piped/nulled or \
         captured with `output()`, or add the site to INHERITS_ON_PURPOSE with the reason:\n  {}",
        gaps.join("\n  ")
    );
}

/// The dedup CLAIM window and the compaction GATE must stay the same number.
///
/// A "you already have this in your context" claim is only made for a read
/// younger than W; a compaction inside the last W silences every claim. Those
/// two facts together are what make a pre-compaction read unclaimable — not
/// either one alone. Split them and the hole opens in the gap: the model gets
/// told a file is still in its context after a compaction dropped it.
///
/// `token-optimizer` shipped that exact bug (`fix(#101)`) and had to clear the
/// live session's read rows at compaction to close it. tokenade needs no clear
/// BECAUSE the window is shared, so this test guards the reason rather than the
/// symptom: every age limit handed to a dedup lookup must come from
/// `read_context_recency_secs()`, never a literal or a second knob.
#[test]
fn the_dedup_claim_window_and_the_compaction_gate_are_the_same_number() {
    use std::path::Path;

    /// Lookups whose last argument is the age limit for a context claim.
    const AGE_TAKING_LOOKUPS: &[&str] = &[
        "lookup_by_path_with_age",
        "lookup_session_file_latest",
        "range_is_covered",
    ];

    let hooks = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/hooks");
    let mut sources = Vec::new();
    let mut stack = vec![hooks.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|s| s.to_str()) == Some("rs") {
                let n = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                if n != "tests.rs" && !n.ends_with("_tests.rs") {
                    sources.push(p);
                }
            }
        }
    }
    assert!(
        sources.len() > 5,
        "the hooks walk found almost nothing: {}",
        sources.len()
    );

    // 1. The gate itself still reads the shared knob.
    let gate = std::fs::read_to_string(hooks.join("read_pretool.rs")).expect("read_pretool.rs");
    let body = gate
        .split("fn compaction_recent")
        .nth(1)
        .expect("compaction_recent still exists");
    let body = &body[..body.find("\n}").map(|i| i + 2).unwrap_or(body.len())];
    assert!(
        body.contains("read_context_recency_secs()"),
        "compaction_recent no longer draws its window from read_context_recency_secs(); \
         the dedup claim still does, and the two must agree or a pre-compaction read \
         becomes claimable"
    );

    // 2. Every claim's age limit comes from the same place.
    let mut split: Vec<String> = Vec::new();
    for path in &sources {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let lines: Vec<&str> = text.lines().collect();
        let file = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        for (i, line) in lines.iter().enumerate() {
            let Some(name) = AGE_TAKING_LOOKUPS.iter().find(|n| line.contains(**n)) else {
                continue;
            };
            let t = line.trim_start();
            if t.starts_with("//") || t.starts_with("///") {
                continue;
            }
            // Two shapes to cover, so the window is generous rather than
            // paren-tight: the limit is passed IN (`range_is_covered`), or the
            // age comes BACK and is compared a dozen lines later
            // (`lookup_by_path_with_age`). Both must reach the shared knob.
            let window = lines[i..(i + 20).min(lines.len())].join("\n");
            if window.contains("read_context_recency_secs()") {
                continue;
            }
            split.push(format!("{file}:{} — {name}", i + 1));
        }
    }
    assert!(
        split.is_empty(),
        "these context claims take an age limit that is NOT read_context_recency_secs(), \
         so they can outlive the compaction gate and claim a read the model can no longer \
         see:\n  {}",
        split.join("\n  ")
    );
}

/// A config we intend to REWRITE must never be read with `unwrap_or_default()`.
///
/// Every installer here builds `original + our block` and writes the result. A
/// file that EXISTS but cannot be decoded — non-UTF-8 prose, a tightened mode,
/// a directory in the way — then reads as empty, and the write truncates the
/// user's config down to our block alone while returning success. Five sites
/// did exactly that (three MCP writers, the global CLAUDE.md append, the aider
/// conf); `json_util::read_text_or_bail` is the replacement, and it keeps the
/// ONE failure that legitimately means "nothing there yet": NotFound.
///
/// `token-optimizer` shipped the JSON half of this bug (`fix(#106 F3 P1)`:
/// a malformed read returned `{}` and cleanup wrote it straight back, "every
/// user key destroyed"). Ours was the text half.
#[test]
fn no_installer_reads_a_config_it_will_rewrite_with_unwrap_or_default() {
    use std::path::Path;

    /// Matched on the LINE, not the file: an allowlist keyed by filename would
    /// wave through every other read in the same file — including, when this
    /// guard was first written, the very `.aider.conf.yml` clobber it exists to
    /// catch. Only reads of paths this product OWNS qualify: re-deriving our
    /// own content after a failed read loses nothing.
    const OURS_TO_LOSE: &[(&str, &str, &str)] = &[(
        "agent_plugins.rs",
        "&startup",
        "`~/.tokenade/aider-startup.txt` is ours; the read only asks whether it \
         is already current, and a failed read just rewrites it",
    )];

    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources = Vec::new();
    for sub in ["install", "install_cli"] {
        let mut stack = vec![base.join(sub)];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().and_then(|s| s.to_str()) == Some("rs") {
                    let n = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                    if n != "tests.rs" && !n.ends_with("_tests.rs") {
                        sources.push(p);
                    }
                }
            }
        }
    }
    assert!(
        sources.len() > 5,
        "the installer walk found almost nothing: {}",
        sources.len()
    );

    let mut clobbers: Vec<String> = Vec::new();
    for path in &sources {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let file = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        for (i, line) in text.lines().enumerate() {
            if !line.contains("read_to_string") || !line.contains("unwrap_or_default") {
                continue;
            }
            if line.trim_start().starts_with("//") {
                continue;
            }
            if OURS_TO_LOSE
                .iter()
                .any(|(f, needle, _)| *f == file && line.contains(needle))
            {
                continue;
            }
            clobbers.push(format!("{file}:{} — {}", i + 1, line.trim()));
        }
    }
    assert!(
        clobbers.is_empty(),
        "these read a user config as EMPTY when it merely could not be decoded, \
         then write the result back — use json_util::read_text_or_bail:\n  {}",
        clobbers.join("\n  ")
    );
}

/// `with_tmp_cwd` must hand its closure a scratch HOME, not the developer's.
///
/// Without it `cli_dashboard_runs_on_empty_dir` scanned the real ledger and
/// took 181 of the suite's 194 seconds. The cost is the visible half; the
/// other half is that a test which reads the machine it runs on is not
/// reproducible and is not testing what its name says.
#[test]
fn with_tmp_cwd_gives_a_scratch_home_and_restores_it() {
    // `real` is read UNDER the lock, not before the call: read outside, it is
    // a neighbouring test's scratch that gets taken for the real `HOME`.
    let (real, after, inside) = super::with_tmp_cwd_probe(|| {
        (
            tokenade_core::platform::home_dir(),
            std::env::current_dir().ok(),
        )
    });
    let (home, cwd) = inside;
    let home = home.expect("HOME is set inside the scratch environment");
    assert_ne!(
        Some(home.as_os_str()),
        real.as_deref(),
        "the closure ran against the real HOME"
    );
    assert_eq!(
        Some(&home),
        cwd.as_ref(),
        "HOME and the working directory must be the same scratch dir"
    );
    // And it is put back: a test that leaks HOME breaks whatever runs next.
    assert_eq!(after, real, "HOME was not restored afterwards");
}

/// Every text tokenade ships INTO an agent tells the same story about an
/// absolute path.
///
/// Three of them are generated by the product and land where an agent reads
/// them as fact: the block written into `~/.claude/CLAUDE.md`, the brief
/// injected into every subagent prompt, and the project scaffold. Two said an
/// absolute path "bypasses the tokenade hook, so that command's output isn't
/// compacted" while the third said it "is intercepted exactly like `git` when
/// hooks are installed". Measured 2026-09-03: the PreToolUse hook rewrites
/// `/usr/bin/git status` exactly as it rewrites `git status`, and the
/// compactor picks the same spec for either spelling on 15 commands, so the
/// third is right and the other two were telling agents to avoid a form that
/// works — and, worse, that output they were about to receive was raw.
///
/// The test asks what the sentence CLAIMS rather than matching its wording:
/// a shipped text that mentions an absolute path must say it is intercepted,
/// and must not say it is not compacted.
#[test]
fn every_shipped_text_agrees_on_what_an_absolute_path_does() {
    let texts: Vec<(&str, String)> = vec![
        (
            "global CLAUDE.md block",
            crate::install::claude_md::GLOBAL_CLAUDE_MD_BLOCK.to_string(),
        ),
        (
            // `Explore` and `Plan` are the subagents that skip the CLAUDE.md
            // hierarchy, so this brief is their ONLY channel and carries the
            // full text. Every other subagent inherits the project scaffold,
            // which is the third entry below.
            "subagent brief",
            crate::hooks::subagent_pretool::subagent_brief("Explore"),
        ),
        (
            "project scaffold",
            tokenade_core::project_scaffold::scaffold_block(true),
        ),
    ];
    let mut checked = 0usize;
    for (name, body) in &texts {
        if *name == "global CLAUDE.md block" {
            // Short fence: the absolute-path claim lives in the project
            // scaffold, not in ~/.claude/CLAUDE.md.
            assert!(
                !body.contains("absolute path"),
                "the short global fence reintroduced the absolute-path lecture"
            );
            continue;
        }
        let Some(at) = body.find("absolute path") else {
            continue;
        };
        checked += 1;
        // The claim lives in the sentence, not in the file.
        let start = body[..at].rfind(". ").map(|i| i + 2).unwrap_or(0);
        let end = at
            + body[at..]
                .find(". ")
                .map(|i| i + 1)
                .unwrap_or(body.len() - at);
        let sentence = body[start..end].replace('\n', " ");
        assert!(
            sentence.contains("intercepted"),
            "{name} mentions an absolute path without saying it is intercepted: {sentence:?}"
        );
        assert!(
            !sentence.contains("bypass") && !sentence.contains("isn't compacted"),
            "{name} still tells agents an absolute path is not compacted, which the hook \
             disproves: {sentence:?}"
        );
    }
    assert_eq!(
        checked,
        texts.len() - 1,
        "a shipped text that still carries the lecture stopped mentioning absolute paths \
         — the guidance is now silent where it used to be wrong, which is not the same as correct"
    );
}
