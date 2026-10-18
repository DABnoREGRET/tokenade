//! **Two command compactors carrying the SAME body get fixed twice.**
//!
//! It is the original complaint of this work — "the same code in fifteen
//! places" — and `cmd/**` is the place in the repo where it was true: 990
//! files, one per command and per platform, hand-written by families. The
//! census of 2026-09-13 found **125 files carrying a body identical to
//! another's**, up to their module name.
//!
//! One file per command stays MANDATORY: `build.rs` generates the `pub mod`
//! of `cmd/<os>/` from the directory's content, and the registry relies on
//! it. What this test refuses is therefore not the file, it is the COPIED
//! body — a family is written once, in an `archetypes` macro, and each file
//! invokes it. A file invoking a family macro is short and does not count as
//! a duplicate: its body is ONE line, shared on purpose.
//!
//! The precedent this test guards: three modules drifted from their twin and
//! shipped to two platforms the behaviour from BEFORE a fix (2026-08-04).
//! Delegation, then the macro, close that door by construction.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The declared pairs: a shared body no macro covers, with the reason. The
/// list must SHRINK.
const DECLARED: &[(&str, &str)] = &[(
    "linux/ssh_cmd.rs + windows/ssh_win.rs",
    "`ssh_win` is the dead arm of an ordered `or_else`: `ssh_cmd`'s detector \
     already answers for both. Measured, zero cost, and removing it would \
     require touching routing — not the body.",
)];

fn cmd_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .join("tokenade-core")
        .join("src")
        .join("cmd")
}

/// A file's body, without its module documentation, without blank lines, and
/// with its own module name neutralised — two twins differ only there.
fn body(path: &Path) -> String {
    let stem = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim_start().starts_with("//!") && !l.trim().is_empty())
        .map(|l| l.replace(&stem, "<NAME>"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A file whose whole body is the invocation of a family macro.
fn is_a_family_call(b: &str) -> bool {
    let lines: Vec<&str> = b.lines().collect();
    lines.len() == 1 && lines[0].contains('!') && lines[0].ends_with(");")
}

fn files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![cmd_root()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|x| x.to_str()) == Some("rs") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

#[test]
fn no_two_command_compactors_share_a_body() {
    let all = files();
    assert!(
        all.len() >= 900,
        "only {} file(s) under `cmd/`: the guard no longer looks at anything",
        all.len()
    );

    let mut by_body: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for p in &all {
        let b = body(p);
        if b.trim().is_empty() || is_a_family_call(&b) {
            continue;
        }
        let rel = p
            .strip_prefix(cmd_root())
            .unwrap_or(p)
            .to_string_lossy()
            .replace('\\', "/");
        by_body.entry(b).or_default().push(rel);
    }

    let mut faults = Vec::new();
    for (_, group) in by_body.iter().filter(|(_, g)| g.len() > 1) {
        let key = group.join(" + ");
        if DECLARED.iter().any(|(k, _)| *k == key) {
            continue;
        }
        faults.push(format!("  {key}"));
    }
    assert!(
        faults.is_empty(),
        "{} compactor famil(y/ies) carry the same copied body. A \
         family is written ONCE, in a `cmd::families` macro — see \
         `cmd::families` — and each file invokes it. \
         Otherwise, list the pair in `DECLARED` with its reason:\n{}",
        faults.len(),
        faults.join("\n")
    );

    // The ratchet in the other direction: a declared pair that no longer
    // exists is decoration, and it would suggest a debt is due.
    let stale: Vec<&str> = DECLARED
        .iter()
        .map(|(k, _)| *k)
        .filter(|k| !by_body.values().any(|g| g.join(" + ") == *k))
        .collect();
    assert!(
        stale.is_empty(),
        "declared pair(s) that no longer share a body — remove them:\n  {}",
        stale.join("\n  ")
    );
}
