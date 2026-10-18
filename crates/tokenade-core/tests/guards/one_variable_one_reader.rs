//! **One environment variable, ONE reader.**
//!
//! This repo reads **123 `TOKENADE_*` variables** in its production code. The
//! re-architecture plan wanted to gather them behind a facade; the
//! measurement says the risk is not their NUMBER but their DUPLICATION — two
//! places reading the same variable end up no longer saying the same thing,
//! and the second does not inherit the first one's fix.
//!
//! It happened: a public-key variable had two readers. The defect —
//! `std::env::var` returns `Ok("")` for a variable set EMPTY, so the embedded
//! fallback never fired and verification failed announcing "not set" — was
//! fixed in ONE of the two. The other, the one that really checks the
//! signature, still carried the pre-fix shape.
//!
//! Three variables were returned to a single reader on 2026-09-13: that key
//! variable, `TOKENADE_REPLAY_MAX_AGE_SECS` (two word-for-word copies,
//! default included) and `TOKENADE_BENCH_MODEL`. Multi-reader families went
//! from **15 to 7**, and the seven are declared below.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The variables it is right to read in several places, with the reason.
///
/// An entry here is a verifiable claim: they are SWITCHES (the same `is_ok()`
/// tested on two routes) or ENTRY points, never a value with a default — for
/// it is the default that diverges.
const SEVERAL_READERS_ON_PURPOSE: &[(&str, &str)] = &[
    (
        "TOKENADE_BENCH_EXE",
        "the measurement harness: its library and its binaries",
    ),
    (
        "TOKENADE_CRASH_PROBE",
        "two DIFFERENT probes on the same variable: `abort` in `wrap`, `panic` in `filter`",
    ),
    (
        "TOKENADE_DOM_DEDUP",
        "a switch, two modules of the same feature",
    ),
    ("TOKENADE_NO_LOOP_NUDGE", "a switch, tested on two routes"),
    (
        "TOKENADE_NO_REPLAY",
        "a switch: \"same switch as the command path\", says its header",
    ),
    (
        "TOKENADE_READ_MODE",
        "`read_mode` PARSES it, `feature_cli` DISPLAYS it as is",
    ),
];

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .to_path_buf()
}

/// `variable -> module families that read it`, on the production half.
fn readers() -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for krate in std::fs::read_dir(crates_dir())
        .into_iter()
        .flatten()
        .flatten()
    {
        let root = krate.path().join("src");
        if !root.is_dir() {
            continue;
        }
        let kname = krate.file_name().to_string_lossy().to_string();
        let mut stack = vec![root.clone()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                let name = p
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                if p.is_dir() {
                    if name != "tests" {
                        stack.push(p);
                    }
                    continue;
                }
                if !p.extension().is_some_and(|x| x == "rs") || name.ends_with("_tests.rs") {
                    continue;
                }
                let Ok(s) = std::fs::read_to_string(&p) else {
                    continue;
                };
                let production = tokenade_core::srcsweep::production_source(&s);
                let rel = p
                    .strip_prefix(&root)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .to_string();
                let family = rel
                    .split(['/', '\\'])
                    .next()
                    .unwrap_or(&rel)
                    .trim_end_matches(".rs")
                    .to_string();
                let mut rest = production.as_str();
                while let Some(at) = rest.find("(\"TOKENADE_") {
                    rest = &rest[at + 2..];
                    let var: String = rest
                        .chars()
                        .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_')
                        .collect();
                    // Only a READ: `var("X")` / `var_os("X")`.
                    let before = &production[..production.len() - rest.len()];
                    // A READ, not a write: `set_var("X")` also ends with
                    // `var("`, and counting it made two variables nobody reads
                    // twice stand accused.
                    if !before.ends_with("::var(\"") && !before.ends_with("::var_os(\"") {
                        continue;
                    }
                    out.entry(var)
                        .or_default()
                        .insert(format!("{kname}/{family}"));
                }
            }
        }
    }
    out
}

#[test]
fn a_variable_with_a_default_has_one_reader() {
    let r = readers();
    assert!(
        r.len() >= 100,
        "only {} variables read — the sweep got lost",
        r.len()
    );
    let declared: BTreeSet<&str> = SEVERAL_READERS_ON_PURPOSE.iter().map(|(v, _)| *v).collect();

    let mut fresh = Vec::new();
    for (var, fams) in &r {
        if fams.len() < 2 || declared.contains(var.as_str()) {
            continue;
        }
        fresh.push(format!("    {var}: {fams:?}"));
    }
    assert!(
        fresh.is_empty(),
        "these variables have several readers. Two places reading the same \
         variable end up no longer saying the same thing — and the second \
         does not inherit the first one's fix. Write ONE reader, or \
         declare the reason:\n{}",
        fresh.join("\n")
    );

    // And the reverse direction: a paid declaration leaves the list.
    let paid: Vec<&str> = SEVERAL_READERS_ON_PURPOSE
        .iter()
        .map(|(v, _)| *v)
        .filter(|v| r.get(*v).map(|f| f.len()).unwrap_or(0) < 2)
        .collect();
    assert!(
        paid.is_empty(),
        "these variables have only one reader left: remove them from the list, \
         otherwise it becomes decoration again:\n  {}",
        paid.join("\n  ")
    );
}
