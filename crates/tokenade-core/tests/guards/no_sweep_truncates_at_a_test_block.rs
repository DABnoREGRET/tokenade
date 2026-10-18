//! **Cutting a file at the FIRST `#[cfg(test)]` throws away production.**
//!
//! `src.split("#[cfg(test)]").next()` is the obvious shortcut for "read only
//! the half that ships", and it is wrong: everything after the first test
//! module vanishes, production code included. MEASURED on 2026-09-08 on this
//! tree: **1,521,151 bytes of production out of 10,659,031 — 14%, spread over
//! 127 files — were invisible to any guard built that way**.
//!
//! And the repo made the defect SYSTEMATIC by mounting its tests through
//! `#[cfg(test)] include!("x_tests.rs")`: the line now sits in the middle of
//! the trunk, so the cut throws away the second half of almost every split
//! module. Three guards were caught red-handed on 2026-09-13, including one
//! that counted **zero** credit constructors and passed all its silence
//! checks on that empty list.
//!
//! `tk_base::source_guard::truncates_at_a_test_block` recognised the pattern
//! — and had **no caller**. A rule nothing applies is an intention; this test
//! is its caller.
//!
//! The fix is always the same: `srcsweep::production_lines`, which follows
//! braces and is the only place in the repo allowed to carry the pattern.

use std::path::{Path, PathBuf};

fn rs_files(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(root) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// The only module allowed to carry the pattern: it is the one replacing it,
/// and it must be able to NAME it to refuse it.
fn is_the_declarer(p: &Path) -> bool {
    p.file_name()
        .is_some_and(|n| n == "srcsweep.rs" || n == "source_guard.rs")
}

#[test]
fn no_source_cuts_a_file_at_its_first_test_block() {
    let crates = tokenade_core::source_guard::crates_dir();
    let mut files = Vec::new();
    for e in std::fs::read_dir(&crates).into_iter().flatten().flatten() {
        rs_files(&e.path().join("src"), &mut files);
        rs_files(&e.path().join("tests"), &mut files);
    }
    assert!(
        files.len() > 500,
        "only {} file(s) walked: the descent stopped",
        files.len()
    );

    let mut seen_the_pattern_somewhere = false;
    let mut bad = Vec::new();
    for f in &files {
        let Ok(body) = std::fs::read_to_string(f) else {
            continue;
        };
        for (n, line) in body.lines().enumerate() {
            if !tokenade_core::source_guard::truncates_at_a_test_block(line) {
                continue;
            }
            seen_the_pattern_somewhere = true;
            if is_the_declarer(f) {
                continue;
            }
            bad.push(format!("    {}:{}: {}", f.display(), n + 1, line.trim()));
        }
    }
    // Without this floor, a detector gone mute would turn this test green.
    assert!(
        seen_the_pattern_somewhere,
        "the detector no longer recognises its own pattern, even in its \
         declarer: green here would prove nothing"
    );

    bad.sort();
    assert!(
        bad.is_empty(),
        "these lines cut a file at its FIRST `#[cfg(test)]`: everything \
         that follows — production included — becomes invisible, and this repo \
         mounts its tests in the MIDDLE of its trunks. The fix is \
         `srcsweep::production_lines`:\n{}",
        bad.join("\n")
    );
}
