//! Regression test for #464: ledger drops `test:*` ops when no
//! ROOT_OVERRIDE is set.
//!
//! Lives in its own integration-test binary because [`tokenade_core::gain`]
//! uses a `OnceLock` for the root override — within a single process the
//! "first set wins" rule means another test that calls `set_root()` first
//! would mask the guard. A separate test binary = fresh process = empty
//! OnceLock.

use std::fs;

#[test]
fn record_drops_test_prefix_ops_when_root_not_overridden() {
    // CRITICAL: this test MUST NOT call gain::set_root() — that would
    // populate the OnceLock and the guard would never trigger.
    //
    // We point HOME at a tempdir so the production ledger path
    // (~/.tokenade/gain.jsonl) lands inside our tempdir instead of the
    // real one. Then we call record() with a `test:*` op and assert
    // NOTHING was written.
    let tmp = std::env::temp_dir().join(format!(
        "tokenade-gain-guard-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&tmp).unwrap();

    // Redirect HOME so jsonl_path() lands in the tempdir.
    let prev_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", &tmp);

    let observed_ledger = tokenade_core::gain::current_ledger_path();

    // Dev builds book nothing (`gain::record_inner`). This test asserts what
    // the WRITE path does — including a positive control that must land — so
    // it opts back in.
    std::env::set_var("TOKENADE_FORCE_GAIN", "1");

    // Negative case: test:* op MUST be dropped.
    tokenade_core::gain::record(
        "ci-probe",
        "test:guard-fixture",
        "label",
        tokenade_core::Savings::new(100, 50),
    );
    // Positive control INSIDE the override window (so we see whether
    // ANY record reaches disk while HOME is redirected).
    tokenade_core::gain::record(
        "ci-probe",
        "real-op-control",
        "label",
        tokenade_core::Savings::new(100, 50),
    );

    std::env::remove_var("TOKENADE_FORCE_GAIN");
    // Restore HOME before any panic so we don't pollute subsequent tests
    // running in the same binary (none in this file, but defensive).
    if let Some(prev) = prev_home {
        std::env::set_var("HOME", prev);
    } else {
        std::env::remove_var("HOME");
    }

    let content = fs::read_to_string(&observed_ledger).unwrap_or_default();
    assert!(
        !content.contains("test:guard-fixture"),
        "guard failed: test:* op leaked into production ledger.\ncontent: {content}"
    );

    // Positive control already issued above (inside the HOME override
    // window). Use the path the gain module actually computed at write
    // time, not just our guess — if the override didn't take effect
    // we want the failure message to say so.
    let content = fs::read_to_string(&observed_ledger).unwrap_or_default();
    assert!(
        content.contains("real-op-control"),
        "positive control failed: non-test op was also dropped — guard is too aggressive.\nobserved_ledger={}\ncontent: {content}",
        observed_ledger.display()
    );

    let _ = fs::remove_dir_all(&tmp);
}
