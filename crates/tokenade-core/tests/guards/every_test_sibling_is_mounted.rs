//! **A tests file placed next to its function must be MOUNTED.**
//!
//! The re-architecture work moves tests out of god files into `x_tests.rs`
//! siblings, mounted by `include!("x_tests.rs")` or by
//! `#[path = "x_tests.rs"] mod`. The file stays on disk, git tracks it,
//! `wc -l` counts it, a search finds it — and if it is no longer mounted,
//! **it is not compiled and nobody says so**. The tests it carries vanish
//! from no report: they simply stop existing.
//!
//! It happened TWICE on 2026-09-13, while splitting `index.rs` then
//! `gain.rs`: the cut took the tail of the file, hence the `include!` line,
//! and 4,096 then 2,000 lines of tests left the compilation without a single
//! counter moving. Both times, it was comparing test NAMES before/after that
//! caught it — a manual check this test replaces.
//!
//! It is the twin of `one_test_binary_per_subject`, which holds the same rule
//! one level up: a subject no `mod` declares is not compiled and does not say
//! so.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The source roots of ALL crates, binaries included.
///
/// `source_guard::crate_srcs()` only returns the library, on purpose; here
/// the rule also applies to `tokenade-cli` and `tokenade-bench`, which
/// between them carry twenty-four of these files.
fn src_roots() -> Vec<PathBuf> {
    let crates = tokenade_core::source_guard::crates_dir();
    let mut out: Vec<PathBuf> = std::fs::read_dir(&crates)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path().join("src"))
        .filter(|p| p.is_dir())
        .collect();
    out.sort();
    assert!(
        out.len() >= 4,
        "only {} root(s) under {}: a guard that reads nothing is green",
        out.len(),
        crates.display()
    );
    out
}

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

/// What a file MOUNTS, resolved to an absolute path.
///
/// Both spellings count, and they are not interchangeable: `include!`
/// splices the tokens into the calling file (the mounted file's `super::`
/// then keep their meaning), `#[path] mod` adds a level. This repo uses both
/// — the five files of `llm_proxy/` through `#[path]`, all others through
/// `include!` — and a guard knowing only one would accuse five perfectly
/// mounted files.
fn mounts_of(file: &Path, body: &str) -> Vec<PathBuf> {
    let dir = file.parent().unwrap_or(Path::new("."));
    let mut out = Vec::new();
    for opener in ["include!(\"", "#[path = \""] {
        let mut rest = body;
        while let Some(at) = rest.find(opener) {
            rest = &rest[at + opener.len()..];
            if let Some(end) = rest.find('"') {
                out.push(dir.join(&rest[..end]));
            }
        }
    }
    out
}

#[test]
fn every_test_sibling_is_mounted_exactly_once() {
    let mut files = Vec::new();
    for r in src_roots() {
        rs_files(&r, &mut files);
    }
    assert!(
        files.len() > 500,
        "only {} .rs file(s) walked: the descent stopped",
        files.len()
    );

    let mut mounted: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    let mut siblings = Vec::new();
    for f in &files {
        if f.file_name()
            .is_some_and(|n| n.to_string_lossy().ends_with("_tests.rs"))
        {
            siblings.push(f.clone());
        }
        let Ok(body) = std::fs::read_to_string(f) else {
            continue;
        };
        for m in mounts_of(f, &body) {
            mounted.entry(m).or_default().push(f.clone());
        }
    }
    assert!(
        siblings.len() >= 70,
        "only {} tests sibling(s) found: the census is wrong",
        siblings.len()
    );

    let orphans: Vec<_> = siblings
        .iter()
        .filter(|s| !mounted.contains_key(*s))
        .collect();
    assert!(
        orphans.is_empty(),
        "these tests files exist on disk and are COMPILED by \
         nobody — the tests they carry are missing from no report, \
         they simply stopped existing:\n{}",
        orphans
            .iter()
            .map(|p| format!("    {}", p.display()))
            .collect::<Vec<_>>()
            .join("\n")
    );

    let twice: Vec<_> = siblings
        .iter()
        .filter_map(|s| mounted.get(s).filter(|v| v.len() > 1).map(|v| (s, v)))
        .collect();
    assert!(
        twice.is_empty(),
        "a tests sibling mounted TWICE compiles its tests twice, under \
         two module paths: {twice:?}"
    );
}
