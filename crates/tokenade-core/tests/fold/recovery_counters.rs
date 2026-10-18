//! The `recovery:*` pair only answers anything if BOTH halves are recorded at a
//! choke point, and if the ledger row is a COUNTER rather than a credited
//! saving.
//!
//! An integration test, not a unit test: ledger writes are disabled under
//! `debug_assertions` unless `TOKENADE_FORCE_GAIN=1`, and setting that from a
//! unit test would race every other test in the same binary. This is the
//! pattern `gain_record_redacts_label.rs` already established.

use std::path::PathBuf;

#[test]
fn a_fold_that_withholds_content_records_what_it_withheld() {
    let dir = std::env::temp_dir().join(format!("tokenade-recovery-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("TOKENADE_FORCE_GAIN", "1");
    tokenade_core::gain::set_root(dir.clone());

    let big = "a line of perfectly ordinary log output\n".repeat(6000);
    let withheld = tokenade_core::tokenizer::estimate(&big);
    let out = tokenade_core::disclose::maybe_stash_with_threshold(
        big,
        tokenade_core::disclose::SourceHint::Shell,
        100,
    );
    assert!(
        out.len() < 2000,
        "nothing was withheld, so there is nothing to measure: {}",
        &out[..out.len().min(200)]
    );

    let ledger: PathBuf =
        tokenade_core::gain::current_ledger_path().with_file_name("gain_estimated.jsonl");
    let text = std::fs::read_to_string(&ledger)
        .unwrap_or_else(|e| panic!("no ledger at {}: {e}", ledger.display()));
    let rows: Vec<&str> = text
        .lines()
        .filter(|l| l.contains("recovery:offered"))
        .collect();
    assert_eq!(
        rows.len(),
        1,
        "expected exactly one `offered` record — one per fold, written at the \
         single choke point every stash path goes through: {text}"
    );
    let v: serde_json::Value = serde_json::from_str(rows[0]).unwrap();
    assert_eq!(
        v["before"].as_u64(),
        Some(withheld as u64),
        "the record must carry the tokens actually withheld"
    );
    assert_eq!(
        v["saved"].as_u64(),
        Some(0),
        "a counter must never be credited as a saving — that is how a media \
         serve once booked 209 million tokens for one video"
    );

    std::env::remove_var("TOKENADE_FORCE_GAIN");
    let _ = std::fs::remove_dir_all(&dir);
}
