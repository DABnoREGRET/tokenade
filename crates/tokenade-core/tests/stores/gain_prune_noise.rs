//! Regression test for #499: `gain::prune_noise()` drops zero-saved
//! non-code-intel rows and KEEPS legitimate code-intel
//! before-equals-after rows (the dashboard's heuristic expands those
//! baselines at read time).
//!
//! Lives in its own integration-test binary because
//! `tokenade_core::gain` uses a `OnceLock` for the root override + a
//! process-wide write lock — separate binary = fresh process = empty
//! OnceLock + no contention. Uses `set_root` (the test-isolation API)
//! rather than HOME redirection so the test is unaffected by any
//! sibling test mutating env vars in parallel.

use std::fs;

#[test]
fn prune_noise_drops_zero_saved_keeps_real_and_code_intel() {
    let _ledger = crate::ledger_lock();
    let tmp = std::env::temp_dir().join(format!(
        "tokenade-gain-prune-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&tmp).unwrap();
    // set_root() lands writes at `<root>/.tokenade/gain.jsonl`. This is
    // the canonical test-isolation API; OnceLock means once set, it
    // stays for the rest of this process's lifetime — fine because
    // this is the binary's only test.
    tokenade_core::gain::set_root(tmp.clone());

    let ledger = tokenade_core::gain::current_ledger_path();
    if let Some(parent) = ledger.parent() {
        let _ = fs::create_dir_all(parent);
    }

    // Pre-populate: rows we'd expect to survive vs rows we'd expect to drop.
    let lines = [
        // KEEP: real savings.
        r#"{"ts":1000,"source":"wrap","op":"grep","before":500,"after":120,"saved":380}"#,
        // KEEP: real savings.
        r#"{"ts":1001,"source":"hook","op":"cargo-build","before":2000,"after":300,"saved":1700}"#,
        // KEEP: code-intel before==after — dashboard heuristic expands baseline.
        r#"{"ts":1002,"source":"mcp","op":"symbol_find","before":42,"after":42,"saved":0}"#,
        // KEEP: code-intel before==after.
        r#"{"ts":1003,"source":"mcp","op":"semantic_search","before":150,"after":150,"saved":0}"#,
        // KEEP: malformed JSON — preserved verbatim (we don't have grounds to judge).
        r#"{not valid json"#,
        // DROP: zero-saved non-code-intel (the #424/#411-style noise).
        r#"{"ts":1004,"source":"wrap","op":"passthrough","before":66,"after":66,"saved":0}"#,
        // DROP: zero-byte row (the #392 pattern).
        r#"{"ts":1005,"source":"mcp","op":"auto:unknown","before":0,"after":0,"saved":0}"#,
        // DROP: tiny exec_script row from a stale MCP process.
        r#"{"ts":1006,"source":"mcp","op":"script:bash","before":7,"after":7,"saved":0}"#,
        // DROP: inflation row (saved=0 via saturating_sub).
        r#"{"ts":1007,"source":"mcp","op":"auto:challenge","before":100,"after":150,"saved":0}"#,
        // #542 — DROP: micro-savings below the writer threshold (saved=1 token,
        // 0.4 % of input). Historically these slipped through the strict-zero
        // gate; bringing prune in line with the writer reclaims them.
        r#"{"ts":1008,"source":"wrap","op":"grep","before":278,"after":277,"saved":1}"#,
        // #542 — DROP: 5 tokens saved is < 10 absolute even if the percentage
        // would look fine (5/100 = 5 %).
        r#"{"ts":1009,"source":"wrap","op":"filter:git-status","before":100,"after":95,"saved":5}"#,
        // #542 — DROP: pct < 5 % even when absolute is comfortably > 10.
        r#"{"ts":1010,"source":"hook","op":"ls","before":1000,"after":985,"saved":15}"#,
        // #542 — KEEP: code-intel ops are exempt from the threshold — a 3-token
        // symbol_find result is signal (dashboard expands the baseline).
        r#"{"ts":1011,"source":"mcp","op":"symbol_find","before":50,"after":47,"saved":3}"#,
    ];
    fs::write(&ledger, lines.join("\n") + "\n").unwrap();

    let dropped = tokenade_core::gain::prune_noise().unwrap();

    assert_eq!(
        dropped, 7,
        "expected 7 noise rows dropped (4 zero-saved + 3 micro-savings), got {dropped}"
    );

    let content = fs::read_to_string(&ledger).unwrap();
    // Positive controls.
    assert!(
        content.contains("\"op\":\"grep\""),
        "real-saving grep row dropped"
    );
    assert!(
        content.contains("\"op\":\"cargo-build\""),
        "real-saving cargo row dropped"
    );
    assert!(
        content.contains("\"op\":\"symbol_find\""),
        "code-intel symbol_find row dropped (heuristic needs it)"
    );
    assert!(
        content.contains("\"op\":\"semantic_search\""),
        "code-intel semantic_search row dropped (heuristic needs it)"
    );
    assert!(
        content.contains("{not valid json"),
        "malformed line should be preserved verbatim, not silently dropped"
    );
    // Negative controls.
    assert!(
        !content.contains("\"op\":\"passthrough\""),
        "passthrough noise leaked"
    );
    assert!(
        !content.contains("\"op\":\"auto:unknown\""),
        "auto:unknown noise leaked"
    );
    assert!(
        !content.contains("\"op\":\"script:bash\""),
        "script:bash noise leaked"
    );
    assert!(
        !content.contains("\"op\":\"auto:challenge\""),
        "inflation row leaked"
    );
    // #542 — micro-savings non-code-intel rows are gone, code-intel survives.
    // Anchor on the unique timestamp of each row so we don't accidentally
    // match a substring of an unrelated number (e.g. "saved":1 inside
    // "saved":1700).
    assert!(
        !content.contains("\"ts\":1008"),
        "1-token grep micro-saving (ts=1008) should have been pruned"
    );
    assert!(
        !content.contains("\"ts\":1009"),
        "filter:git-status 5-token row (ts=1009) should have been pruned"
    );
    assert!(
        !content.contains("\"ts\":1010"),
        "ls 1.5%-pct row (ts=1010) should have been pruned"
    );
    assert!(
        content.contains("\"ts\":1011"),
        "code-intel 3-token symbol_find (ts=1011) survives (heuristic baseline)"
    );

    // No-op on a second pass — idempotent.
    let dropped_again = tokenade_core::gain::prune_noise().unwrap();
    assert_eq!(dropped_again, 0, "prune_noise must be idempotent");

    let _ = fs::remove_dir_all(&tmp);
}
