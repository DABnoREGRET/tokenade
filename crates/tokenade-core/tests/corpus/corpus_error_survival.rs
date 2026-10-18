//! A fold may drop anything EXCEPT the line that says something went wrong.
//!
//! `failure_survives.rs` and `error_survival.rs` already assert this, but both
//! are driven by hand-written cases: someone had to think of the compactor
//! first. This file drives the same property from the CAPTURED corpus instead —
//! every `tests/fixtures/{macos,windows}/*.txt`, real output from real tools —
//! so a compactor nobody wrote a case for is still covered.
//!
//! It reports how many fixtures actually reached a compactor. That number is
//! the test's own honesty check: a corpus test whose inputs all route to
//! `None` passes forever while asserting nothing.
//!
//! Measured when written: 368 fixtures, **121 reached a compactor, 87 were
//! actually folded, 13 carried a decisive line, 0 losses**. The other 247 are
//! NOT unrecognised programs — `dispatch_named` returns `None` both for an
//! unknown name and for a compactor that looked at the content and declined, and
//! most of these are the second (`ps`, `ls`, `du`, `netstat` fixtures below the
//! fold threshold). Asserting on those would mean asserting on passthrough,
//! which proves nothing, so they are left out on purpose.

// `corpus` is mounted by `main.rs` for the whole binary: a single copy
// compiled instead of one per subject.
use crate::corpus;

use std::path::{Path, PathBuf};

/// Substrings that make a line decisive. Deliberately narrow — every one of
/// these means "the command did not do what was asked", and losing it turns a
/// failed run into a clean-looking one.
const DECISIVE: &[&str] = &[
    "error:",
    "Error:",
    "ERROR:",
    "[ERROR]",
    "fatal:",
    "FATAL",
    "panic:",
    "Traceback (most recent call last)",
    "Permission denied",
    "No such file or directory",
    "command not found",
    "Segmentation fault",
    "cannot open",
    "Cannot open",
];

/// Fixtures whose decisive line was RESOLVED inside the output itself, so the
/// compactor drops it on purpose — each with why. Not a way to silence a loss:
/// the run's own outcome must say the failure was dealt with.
///
/// `codex_exec_fix.txt`: a whole `codex exec` session in which the sub-agent
/// ran a failing test, fixed `calc.py`, re-ran it green and answered. The
/// `Traceback` is a step it took, not the outcome; the `codex` compactor keeps
/// the session's end (its own test asserts the step is gone). A session that
/// ENDS on an error keeps it — `codex_exec_error.txt` is not listed.
const RESOLVED_IN_THE_OUTPUT: &[&str] = &["codex_exec_fix.txt"];

/// Bare `ERROR` is NOT in the list, and the corpus is why: `sc query` prints
/// `TYPE : f0 ERROR` for a service's error-control flag, and `sc_query.txt`
/// made this test fail three times over a word that means the opposite of a
/// failure. A detector that cries wolf on real output is worse than none — the
/// next person deletes the test instead of reading it.
fn decisive_lines(text: &str) -> Vec<&str> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter(|l| DECISIVE.iter().any(|d| l.contains(d)))
        .collect()
}

/// `sysctl_vm_full.txt` → `sysctl`; `get_process_full.txt` → `get-process`.
///
/// The windows corpus is named after PowerShell cmdlets with `_` where the
/// canonical name has `-`, so both spellings are tried.
fn prog_candidates(file: &Path) -> Vec<String> {
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let first = stem.split('_').next().unwrap_or(stem).to_string();
    let mut out = vec![first.clone()];
    if let Some((a, b)) = stem.split_once('_') {
        let cmdlet = format!("{a}-{}", b.split('_').next().unwrap_or(b));
        if cmdlet != first {
            out.push(cmdlet);
        }
    }
    out
}

fn fixture_files() -> Vec<PathBuf> {
    // A single enumerator for both corpus-driven guards. Each one used to
    // have its own, with `["macos", "windows"]` and `.txt` hardcoded — so
    // widening the corpus served only one of them, and that is what happened:
    // the sixteen most-used `auto:*` compactors were in neither.
    corpus::fixtures().into_iter().map(|f| f.path).collect()
}

#[test]
fn no_compactor_drops_a_decisive_line_from_the_captured_corpus() {
    let files = fixture_files();
    assert!(
        files.len() > 100,
        "corpus went missing: {} files",
        files.len()
    );

    let mut routed = 0usize;
    let mut folded = 0usize;
    let mut with_decisive = 0usize;
    let mut losses: Vec<String> = Vec::new();

    for f in &files {
        let Ok(text) = std::fs::read_to_string(f) else {
            continue;
        };
        let Some(out) = prog_candidates(f)
            .iter()
            .find_map(|p| tokenade_core::cmd::dispatch_named(p, &text))
        else {
            continue;
        };
        routed += 1;
        if out.len() < text.len() {
            folded += 1;
        }
        let resolved = f
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|n| RESOLVED_IN_THE_OUTPUT.contains(&n));
        let want = if resolved {
            Vec::new()
        } else {
            decisive_lines(&text)
        };
        if want.is_empty() {
            continue;
        }
        with_decisive += 1;
        for line in want {
            if !out.contains(line) {
                losses.push(format!(
                    "{}: folded away `{}`",
                    f.file_name().and_then(|s| s.to_str()).unwrap_or("?"),
                    &line[..line.len().min(90)]
                ));
            }
        }
    }

    // Printed on success too (`--nocapture`): the day this reads
    // "0 routed" the assertions below have stopped meaning anything.
    println!(
        "corpus: {} files, {routed} routed to a compactor, {folded} actually folded, \
         {with_decisive} carried a decisive line",
        files.len()
    );
    assert!(
        routed >= 50,
        "only {routed} of {} fixtures reached a compactor — the filename→program \
         mapping has drifted and this test is asserting almost nothing",
        files.len()
    );
    assert!(
        losses.is_empty(),
        "a fold dropped the reason something failed:\n  {}",
        losses.join("\n  ")
    );
}
