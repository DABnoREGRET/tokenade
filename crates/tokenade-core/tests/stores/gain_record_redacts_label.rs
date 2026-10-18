//! Regression test for #480: `gain::record` redacts the LABEL field
//! before writing to gain.jsonl.
//!
//! Lives in its own integration-test binary because
//! [`tokenade_core::gain`] uses a `OnceLock` for the root override and
//! a process-wide write lock — running this in the same binary as
//! `writes_jsonl_record` (which removes its own tempdir at the end)
//! produced flaky FAILED results in parallel-test runs. Separate
//! binary = fresh process = no contention.

use std::fs;

#[test]
fn record_redacts_aws_key_in_label() {
    let _ledger = crate::ledger_lock();
    // Redirect the ledger into our own tempdir via HOME override.
    // We can't reuse set_root() reliably from a parallel test, but
    // HOME is read on every record() call so it Just Works here.
    let tmp = std::env::temp_dir().join(format!(
        "tokenade-gain-redact-label-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&tmp).unwrap();
    let prev_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", &tmp);

    let observed_path = tokenade_core::gain::current_ledger_path();
    if let Some(parent) = observed_path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let secret = "AKIA1234567890ABCDEF";
    let label = format!("grep -r {secret} /etc");
    // #495: source must NOT start with "test" (defense-in-depth guard
    // drops those when ROOT_OVERRIDE is None). Use "ci-probe" — the
    // test still validates the redaction path, just under a label
    // that production code would never use.
    // Dev builds book nothing (`gain::record_inner`); this test asserts what
    // the WRITE path does to a label, so it opts back in.
    std::env::set_var("TOKENADE_FORCE_GAIN", "1");
    tokenade_core::gain::record(
        "ci-probe",
        "leak_probe_op_480",
        &label,
        tokenade_core::Savings::new(50, 10),
    );

    if let Some(prev) = prev_home {
        std::env::set_var("HOME", prev);
    } else {
        std::env::remove_var("HOME");
    }

    std::env::remove_var("TOKENADE_FORCE_GAIN");
    let content = fs::read_to_string(&observed_path).unwrap_or_default();
    let our_line = content
        .lines()
        .find(|l| l.contains("leak_probe_op_480"))
        .unwrap_or_else(|| {
            panic!(
                "test record not found in {}: {content}",
                observed_path.display()
            )
        });
    assert!(
        !our_line.contains(secret),
        "AWS key leaked into gain.jsonl label: {our_line}"
    );
    assert!(
        our_line.contains("<redacted>"),
        "expected redacted marker in label (catches a future drop-label regression): {our_line}"
    );

    let _ = fs::remove_dir_all(&tmp);
}
