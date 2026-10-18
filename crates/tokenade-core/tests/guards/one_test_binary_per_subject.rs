//! A test subject is a FILE in a group, never a target of its own.
//!
//! Each `.rs` placed directly under `tests/` is a crate: cargo compiles and
//! LINKS it separately. This crate carried 87 of them, for 192 executables in
//! `target/debug/deps`, the largest weighing 194 MB, and that is where the
//! suite's hours were — `nextest` measures 101 s of EXECUTION for 9,193
//! tests. Grouped by subject: 8 targets.
//!
//! Nothing stops the proliferation from coming back the way it came, hence
//! this test. It holds THREE rules, and the third is the one that bit during
//! the grouping itself:
//!
//! 1. no `.rs` directly under `tests/`;
//! 2. every group directory has its `main.rs`;
//! 3. every `.rs` of a group is declared by a `mod` of its `main.rs`.
//!
//! The third is not cosmetic. A subject moved into a group whose `main.rs`
//! does not declare it is NOT compiled: it breaks nothing, it says nothing,
//! its tests simply vanish from the count. Two groups were in exactly that
//! state during the grouping, and only a before/after count showed it. One
//! target fewer shows; a mute subject does not.

use std::path::{Path, PathBuf};

fn tests_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")
}

/// The directories under `tests/` that are not subject groups: shared data
/// and helpers, which have neither `main.rs` nor `mod`.
const NOT_GROUPS: &[&str] = &["fixtures", "support", "snapshots"];

fn groups() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(tests_dir())
        .expect("tests/ readable")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .filter(|p| {
            let name = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            !NOT_GROUPS.contains(&name.as_str())
        })
        .collect();
    v.sort();
    v
}

#[test]
fn no_subject_sits_alone_at_the_root_of_tests() {
    let strays: Vec<String> = std::fs::read_dir(tests_dir())
        .expect("tests/ readable")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "rs"))
        .map(|p| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert!(
        strays.is_empty(),
        "these files are each a test TARGET — they compile and link their \
         own binary. Move them into a group of `tests/<group>/` and \
         declare them in its `main.rs`:\n  {}",
        strays.join("\n  ")
    );
}

#[test]
fn every_group_has_its_binary_root() {
    let orphans: Vec<String> = groups()
        .into_iter()
        .filter(|g| !g.join("main.rs").is_file())
        .map(|g| g.to_string_lossy().into_owned())
        .collect();
    assert!(
        orphans.is_empty(),
        "a group without `main.rs` is not a target: cargo compiles NONE of \
         its files, silently.\n  {}",
        orphans.join("\n  ")
    );
}

#[test]
fn every_subject_of_a_group_is_declared_by_its_main() {
    let mut silent: Vec<String> = Vec::new();
    // The FLOOR. A guard that reads nothing is green, and that is the worst
    // of verdicts: `tests/` renamed, `NOT_GROUPS` widened by mistake, and
    // this test would pass without opening a single file.
    let groups = groups();
    assert!(
        groups.len() >= 5,
        "only {} group(s) under `tests/` — the sweep got lost",
        groups.len()
    );
    let mut read = 0usize;
    for g in groups {
        let main = match std::fs::read_to_string(g.join("main.rs")) {
            Ok(s) => s,
            // `every_group_has_its_binary_root` already carries this case;
            // missing it here would make two red tests for a single defect.
            Err(_) => continue,
        };
        read += 1;
        let declared: Vec<String> = main
            .lines()
            .map(str::trim)
            .filter_map(|l| {
                l.strip_prefix("mod ")
                    .or_else(|| l.strip_prefix("pub mod "))
            })
            .filter_map(|l| l.strip_suffix(';'))
            .map(|s| s.trim().to_string())
            .collect();
        for e in std::fs::read_dir(&g).expect("readable group").flatten() {
            let p = e.path();
            if !p.extension().is_some_and(|x| x == "rs") {
                continue;
            }
            let stem = p
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if stem == "main" || declared.contains(&stem) {
                continue;
            }
            silent.push(p.to_string_lossy().into_owned());
        }
    }
    assert!(read >= 5, "only {read} group `main.rs` read");

    assert!(
        silent.is_empty(),
        "these subjects are declared by no `mod` of their `main.rs`: they \
         are NOT compiled and their tests do not run, without a single \
         error.\n  {}",
        silent.join("\n  ")
    );
}
