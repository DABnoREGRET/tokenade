//! **A hand-written source path must still exist.**
//!
//! This project's memory names the class: "a guard that names its subject by
//! a PATH accuses the product". Sixteen guards bit the day the god files were
//! split, and five more the day modules changed crate — none said it had got
//! lost, all announced a disappearance ("only 2 call site(s) reached").
//!
//! This one judges nothing about the product: it checks that **every source
//! path of this repo, written out in full in a Rust file, designates a file
//! that exists**. When a module moves, it is this one that turns red, with
//! the right message and at the right line — instead of a product guard
//! accusing a function of having vanished.
//!
//! Only paths naming a REAL workspace crate are looked at
//! (`crates/tk-base/src/…`): `crates/a/src/x.rs` is a synthetic fixture in a
//! temporary directory, and it has no reason to exist.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The paths that designate NOTHING on purpose: they illustrate an error
/// message or serve as a namesake neighbour in a fixture. An entry here is a
/// verifiable claim, not an exemption.
const NOT_A_REAL_FILE: &[(&str, &str)] = &[
    (
        "somewhere.rs",
        "the imaginary file of an error message: \"declared here, found there\"",
    ),
    (
        "yourformat.rs",
        "the example the map gives to whoever adds a format",
    ),
    (
        "updater_extra.rs",
        "a fixture's namesake NEIGHBOUR: the guard must refuse to exempt it",
    ),
    ("foo5.rs", "a fabricated `grep` output, not a file"),
];

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .to_path_buf()
}

fn crate_names() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(crates_dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("src").is_dir())
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .collect();
    v.sort();
    v
}

fn rust_files() -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let mut stack = vec![crates_dir()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            let name = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if name == "target" || name.starts_with('.') {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                if let Ok(s) = std::fs::read_to_string(&p) {
                    out.push((p, s));
                }
            }
        }
    }
    out
}

#[test]
fn every_hand_written_source_path_still_exists() {
    let crates = crate_names();
    assert!(crates.len() >= 4, "only {} crates read", crates.len());
    let exempt: BTreeSet<&str> = NOT_A_REAL_FILE.iter().map(|(f, _)| *f).collect();

    let files = rust_files();
    assert!(files.len() > 1_000, "only {} files read", files.len());

    let mut checked = 0usize;
    let mut stale = Vec::new();
    for (file, body) in &files {
        for piece in body.split('"').skip(1).step_by(2) {
            // `<crate>/src/<…>.rs`, possibly prefixed with `crates/`.
            if !piece.ends_with(".rs") {
                continue;
            }
            // A TEMPLATE (`module_{i}.rs`, `src/file_{i:04}.rs`) makes up
            // names in a temporary directory: it has no reason to exist.
            if piece.contains('{') {
                continue;
            }
            let rel = piece.strip_prefix("crates/").unwrap_or(piece);
            let Some((krate, tail)) = rel.split_once("/src/") else {
                continue;
            };
            if !crates.iter().any(|c| c == krate) {
                continue; // a synthetic fixture, not this repo
            }
            checked += 1;
            let stem = tail.rsplit('/').next().unwrap_or(tail);
            if exempt.contains(stem) {
                continue;
            }
            if !crates_dir().join(krate).join("src").join(tail).is_file() {
                let owner = file
                    .strip_prefix(crates_dir())
                    .unwrap_or(file)
                    .to_string_lossy()
                    .to_string();
                stale.push(format!("    {owner} → `{piece}`"));
            }
        }
    }
    stale.sort();
    stale.dedup();
    assert!(
        checked >= 50,
        "only {checked} source paths examined — the sweep got lost"
    );
    assert!(
        stale.is_empty(),
        "these source paths no longer exist. The module MOVED: derive \
         the address (`source_guard::workspace_module_root`, `module_family`) or \
         fix the path — do not let it rot, that is how a \
         guard ends up accusing the product:\n{}",
        stale.join("\n")
    );
}

#[test]
fn every_declared_non_file_is_still_absent() {
    // An exemption that becomes TRUE is an exemption to remove: the day
    // `somewhere.rs` exists, the list lies.
    let crates = crate_names();
    let mut wrong = Vec::new();
    for (stem, _) in NOT_A_REAL_FILE {
        for c in &crates {
            let root = crates_dir().join(c).join("src");
            let mut stack = vec![root];
            while let Some(d) = stack.pop() {
                for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        stack.push(p);
                    } else if p.file_name().is_some_and(|n| n == *stem) {
                        wrong.push(format!("    `{stem}` exists: {}", p.display()));
                    }
                }
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "these names are declared imaginary and no longer are:\n{}",
        wrong.join("\n")
    );
}
