//! Every 0600 write-then-rename goes through one declarer, and that declarer
//! picks a temp name no other process shares.
//!
//! Six callers had each written the dance out and every one chose a shared
//! name — `<path>.tmp`, `gain.jsonl.rotating`, `gain.jsonl.pruning`,
//! `<name>.merging`. The ledger's write lock is process-LOCAL (its own comment
//! says cross-process safety rests on the OS making small appends atomic,
//! which says nothing about the rewrite rotation performs), so two `tokenade`
//! runs crossing the rotation threshold together wrote into one temp file and
//! renamed a half-written one over the accounting the product reports.

use std::path::PathBuf;

fn scratch(label: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "tokenade-atomic600-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Deterministic stand-in for the race: plant a DIRECTORY at the shared name
/// the old code used. An implementation that writes there cannot, and saves
/// nothing; one that picks a per-process name is unaffected. So this fails
/// exactly on the implementation that would corrupt under concurrency — every
/// run, instead of one run in a hundred.
#[test]
fn the_shared_temp_name_is_no_longer_used() {
    let d = scratch("squat");
    let target = d.join("state.jsonl");
    std::fs::write(&target, "old\n").unwrap();
    for squat in [
        target.with_extension("tmp"),
        target.with_extension("jsonl.rotating"),
        target.with_extension("jsonl.pruning"),
    ] {
        std::fs::create_dir_all(&squat).unwrap();
    }

    tokenade_core::platform::write_atomic_600(&target, b"new\n").expect("the write must land");
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "new\n",
        "the write did not reach the destination"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// A failed rename must not leave the temp file behind, or a busy machine
/// accumulates one per attempt.
#[test]
fn a_failed_rename_removes_its_temp_file() {
    let d = scratch("failed");
    // Renaming a file ONTO a non-empty directory fails on every platform we
    // support, which is a rename failure we can cause on purpose.
    let target = d.join("occupied");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("inside"), "x").unwrap();

    let before = std::fs::read_dir(&d).unwrap().count();
    let r = tokenade_core::platform::write_atomic_600(&target, b"payload");
    assert!(
        r.is_err(),
        "renaming onto a non-empty directory should fail"
    );
    let after = std::fs::read_dir(&d).unwrap().count();
    assert_eq!(
        after, before,
        "a temp file was left behind after a failed rename"
    );
    let _ = std::fs::remove_dir_all(&d);
}
