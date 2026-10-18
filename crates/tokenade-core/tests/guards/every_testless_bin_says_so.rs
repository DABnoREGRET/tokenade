//! **A binary without tests does not build a test harness.**
//!
//! `cargo test --workspace` produces a test binary for EVERY target of the
//! workspace, `[[bin]]`s included — even those that do not contain a single
//! `#[test]`. Measured on 2026-09-14: ten of the bench's eighteen instruments
//! were in that case, i.e. **ten links and 95 MB** produced on every pass to
//! run zero tests. Linking is this repo's memory peak: it is the step that got
//! four releases in a row killed.
//!
//! `test = false` in the `[[bin]]` removes them from the harness without
//! removing them from the build. The rule is derived from the DIRECTORY, not
//! from a list: an instrument that gains tests must remove its line, and a
//! new instrument without tests must add it.
//!
//! Count after the fix: **41 test binaries → 29, 1.80 GB → 1.70 GB**.

use std::path::{Path, PathBuf};

/// A binary's tests: its file, plus the module directory carrying its name —
/// `page_loss.rs` has twelve, `accuracy/` has none but `accuracy.rs` has
/// four, and both count for the same target.
fn test_count(file: &Path) -> usize {
    let mut n = std::fs::read_to_string(file)
        .map(|s| s.matches("#[test]").count())
        .unwrap_or(0);
    let dir = file.with_extension("");
    if dir.is_dir() {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            if e.path().extension().and_then(|x| x.to_str()) == Some("rs") {
                n += std::fs::read_to_string(e.path())
                    .map(|s| s.matches("#[test]").count())
                    .unwrap_or(0);
            }
        }
    }
    n
}

/// Is `test = false` declared for this binary name in this manifest?
fn declares_no_harness(manifest: &str, name: &str) -> bool {
    manifest.split("[[bin]]").skip(1).any(|block| {
        let head = block.split("\n[[").next().unwrap_or(block);
        head.contains(&format!("name = \"{name}\"")) && head.contains("test = false")
    })
}

fn bins_of(crate_dir: &Path) -> Vec<PathBuf> {
    let dir = crate_dir.join("src/bin");
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("rs"))
        .collect()
}

#[test]
fn every_testless_bin_says_so() {
    let crates = tk_base::source_guard::crates_dir();
    let mut seen = 0usize;
    let mut mute = Vec::new();
    let mut liars = Vec::new();
    for e in std::fs::read_dir(&crates).into_iter().flatten().flatten() {
        let dir = e.path();
        let Ok(manifest) = std::fs::read_to_string(dir.join("Cargo.toml")) else {
            continue;
        };
        for bin in bins_of(&dir) {
            let Some(name) = bin.file_stem().and_then(|n| n.to_str()) else {
                continue;
            };
            seen += 1;
            let tests = test_count(&bin);
            let declared = declares_no_harness(&manifest, name);
            if tests == 0 && !declared {
                mute.push(format!("{name} (0 tests, harness built for nothing)"));
            }
            if tests > 0 && declared {
                liars.push(format!(
                    "{name} ({tests} test(s) silenced by `test = false`)"
                ));
            }
        }
    }
    // FLOOR: eighteen binaries exist the day this is written.
    assert!(
        seen >= 15,
        "only {seen} binar(y/ies) read: the walk no longer walks"
    );
    assert!(
        mute.is_empty(),
        "these binaries carry no tests and yet build a harness \
         — one link and ~9 MB each, on every `cargo test`:\n    {}\n\
         Add `[[bin]] name = … path = … test = false` to their Cargo.toml.",
        mute.join("\n    ")
    );
    assert!(
        liars.is_empty(),
        "these binaries carry tests that `test = false` prevents from running:\n    {}\n\
         Remove the line — a test that does not run is an ornament.",
        liars.join("\n    ")
    );
}
