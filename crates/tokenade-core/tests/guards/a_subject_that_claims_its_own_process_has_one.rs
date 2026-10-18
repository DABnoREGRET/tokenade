//! **A subject that declares it needs its own process must have one.**
//!
//! Three tests of this tree carry in their header the reason why they each
//! lived in their own crate: a `OnceLock` whose first call wins, a process
//! counter, an environment variable set for the whole binary. Grouping the 87
//! targets into 8 put them with neighbours — and the sentence in their header
//! became false without a single line of code changing.
//!
//! It does not show under `cargo nextest`, which gives **one process per
//! test**. It shows under `cargo test --test-threads=1`, which is the release
//! runner: 1.1.12 stopped on one, 1.1.13 on the other two, each time after
//! twenty minutes of compilation on three machines.
//!
//! The rule is therefore: a subject whose HEADER claims its process is ALONE
//! in its target, or listed below with the reason why sharing has become safe
//! again.

use std::path::{Path, PathBuf};

/// The phrases by which a header claims its process (French ones kept for
/// headers not yet translated).
const CLAIMS: &[&str] = &[
    "own test binary",
    "own binary",
    "fresh process",
    "separate test binary",
    "propre binaire",
    "propre processus",
    "processus vierge",
];

/// The subjects that carry the phrase but may share, with the reason.
const SHARES_ON_PURPOSE: &[(&str, &str)] = &[
    (
        "gain_prune_noise.rs",
        "it SETS the root (`set_root`) instead of reading it unset: its neighbour \
         that read it unset moved to `gain_virgin`",
    ),
    (
        "gain_record_redacts_label.rs",
        "its reason was CONTENTION on a temporary directory under parallel \
         execution; nextest gives one process per test and the release runs serially",
    ),
];

fn tests_roots() -> Vec<PathBuf> {
    let crates = tk_base::source_guard::crates_dir();
    std::fs::read_dir(crates)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path().join("tests"))
        .filter(|p| p.is_dir())
        .collect()
}

/// A file's `//!` header: the leading lines, up to the first item.
fn header(src: &str) -> String {
    src.lines()
        .take_while(|l| {
            let t = l.trim_start();
            t.starts_with("//!") || t.is_empty() || t.starts_with("//")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_subject_that_claims_its_own_process_is_alone_in_its_target() {
    let mut claims: Vec<(PathBuf, String)> = Vec::new();
    for root in tests_roots() {
        for group in std::fs::read_dir(&root).into_iter().flatten().flatten() {
            let dir = group.path();
            if !dir.is_dir()
                || matches!(
                    dir.file_name().and_then(|n| n.to_str()),
                    Some("fixtures") | Some("support") | Some("snapshots")
                )
            {
                continue;
            }
            for f in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let p = f.path();
                if p.extension().and_then(|x| x.to_str()) != Some("rs") {
                    continue;
                }
                if p.file_name().and_then(|n| n.to_str()) == Some("main.rs") {
                    continue;
                }
                // This guard SPELLS the phrases it looks for, so it finds
                // itself — and accuses itself of sharing the `guards` binary,
                // which it has every reason to share. The name is written in
                // two pieces so that this skip does not find itself either.
                if p.file_name().and_then(|n| n.to_str())
                    == Some(concat!(
                        "a_subject_that_claims",
                        "_its_own_process_has_one.rs"
                    ))
                {
                    continue;
                }
                let Ok(src) = std::fs::read_to_string(&p) else {
                    continue;
                };
                let head = header(&src);
                if CLAIMS.iter().any(|c| head.contains(c)) {
                    claims.push((p, head));
                }
            }
        }
    }
    // FLOOR: three subjects carry the phrase the day this is written. Reading
    // zero would mean the detection no longer detects.
    assert!(
        claims.len() >= 3,
        "only {} subject(s) claiming their process: the detection no longer reads anything",
        claims.len()
    );
    let mut offenders = Vec::new();
    for (p, _) in &claims {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if SHARES_ON_PURPOSE.iter().any(|(n, _)| *n == name) {
            continue;
        }
        let main = p.with_file_name("main.rs");
        let Ok(m) = std::fs::read_to_string(&main) else {
            continue;
        };
        let mods = m
            .lines()
            .filter(|l| l.trim_start().starts_with("mod "))
            .count();
        if mods != 1 {
            offenders.push(format!("{}: its target declares {mods}", rel(p)));
        }
    }
    assert!(
        offenders.is_empty(),
        "these subjects declare they need their own process and yet share \
         their binary:\n    {}\n\
         Give them a target (`[[test]]` in Cargo.toml), or list them in \
         `SHARES_ON_PURPOSE` with the reason why sharing is safe.",
        offenders.join("\n    ")
    );
}

fn rel(p: &Path) -> String {
    let crates = tk_base::source_guard::crates_dir();
    p.strip_prefix(crates.parent().unwrap_or(&crates))
        .unwrap_or(p)
        .display()
        .to_string()
}
