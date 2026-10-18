//! **A crate the map does not name does not exist for its reader.**
//!
//! `architecture_doc_is_true` already checks that the map names no REMOVED
//! crate. The reverse direction was missing, and it is the easiest to miss:
//! two crates were extracted on 2026-09-13, `tk-base` then `tk-fold`, and the
//! repo's map went on announcing a library in a single piece without a test
//! flinching. An omission never turns red on its own.
//!
//! The list is DERIVED from the `crates/` directory, not written here: a
//! hand-enumerated guard lets the next crate through, which is exactly the
//! defect it is meant to catch.

use std::path::{Path, PathBuf};

fn crates_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .to_path_buf()
}

/// Every crate of the workspace is named by `docs/ARCHITECTURE.md`.
#[test]
fn every_crate_of_the_workspace_is_named_by_the_map() {
    let doc = std::fs::read_to_string(
        crates_root()
            .parent()
            .expect("the repo root")
            .join("docs")
            .join("ARCHITECTURE.md"),
    )
    .expect("docs/ARCHITECTURE.md");

    let mut krates: Vec<String> = std::fs::read_dir(crates_root())
        .expect("crates/")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("src").is_dir() && p.join("Cargo.toml").is_file())
        .map(|p| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        })
        .collect();
    krates.sort();
    assert!(
        krates.len() >= 4,
        "only {} crate(s) found: the guard no longer looks at anything",
        krates.len()
    );

    let unnamed: Vec<&String> = krates
        .iter()
        .filter(|k| !doc.contains(k.as_str()))
        .collect();
    assert!(
        unnamed.is_empty(),
        "{} workspace crate(s) that `docs/ARCHITECTURE.md` does not name:\n  {}\n\
         A map that keeps quiet about a crate is wrong by omission, and it is the \
         form of error no other test catches.",
        unnamed.len(),
        unnamed
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}
