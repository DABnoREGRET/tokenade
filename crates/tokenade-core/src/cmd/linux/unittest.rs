//! `python -m unittest` — Python stdlib unittest TextTestRunner output.
//!
//! unittest's failure format is **block-structured**, not line-structured
//! like pytest, so the line-based `pytest` engine mangles it: it cannot
//! associate a test with its assertion, and it folds nothing useful (the
//! noise here is the `===`/`---` rules and the `Traceback (most recent call
//! last):` header, none of which `pytest::is_passing_test_line` matches).
//! Routing unittest output to `auto`/`stacktrace` is worse still — the
//! stacktrace compactor regroups `File "…"` frames across blocks and
//! destroys the test↔failure ordering.
//!
//! A single failure block looks like:
//! ```text
//! ======================================================================
//! FAIL: test_convert (tests.test_money.TestMoney.test_convert)
//! ----------------------------------------------------------------------
//! Traceback (most recent call last):
//!   File "/abs/tests/test_money.py", line 6, in test_convert
//!     self.assertEqual(convert(10), 30)
//! AssertionError: 20 != 30
//! ```
//! followed by a blank line. The run ends with `----`, `Ran N tests in Xs`,
//! a blank line, then `OK` / `OK (skipped=2)` / `FAILED (failures=2, errors=1)`.
//!
//! This compactor preserves every diagnostic the agent ran the tests for —
//! the status dots, each failing test's identity, the deepest stack frame
//! (where it actually broke) with its source line, the full exception
//! message, and the final tally — while folding the pure decoration rules,
//! the redundant `Traceback` header, and intermediate stack frames. Paths
//! stay absolute here; the `filter_command` wrapper relativises them in a
//! single post-pass (`relativize_cwd_paths`).

use std::fmt::Write;

/// unittest's `TextTestRunner` footer is the stable fingerprint: a
/// `Ran N test(s) in …` line plus a verdict (`OK` / `OK (…)` / `FAILED …`).
/// Both must be present so we never claim a bare traceback or a pytest run.
pub fn detect(text: &str) -> bool {
    let mut has_ran = false;
    let mut has_verdict = false;
    for line in text.lines() {
        let t = line.trim();
        if !has_ran && t.starts_with("Ran ") && t.contains(" test") && t.contains(" in ") {
            has_ran = true;
        }
        if !has_verdict
            && (t == "OK" || t.starts_with("OK (") || t.starts_with("FAILED"))
        {
            has_verdict = true;
        }
        if has_ran && has_verdict {
            return true;
        }
    }
    false
}

crate::archetypes::detect_gated_try_compact!();

/// A line that is *only* `=`/`-` decoration (and spaces) — pure noise that
/// unittest prints around every failure block. Carries no information.
fn is_pure_separator(t: &str) -> bool {
    !t.is_empty() && t.chars().all(|c| c == '=' || c == '-' || c == ' ')
}

/// A stack frame's location line: `  File "…", line N, in func`.
fn is_frame_file(t: &str) -> bool {
    t.starts_with("File \"") && t.contains(", line ")
}

pub fn compact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut sep_elided = 0usize; // pure `===`/`---` rules dropped
    let mut frame_elided = 0usize; // intermediate stack frames folded

    // The deepest (most recent) frame seen since the last header/exception —
    // unittest prints frames outermost-first, so the last one is where the
    // exception was actually raised. We buffer it and its source line, then
    // flush both right before the exception message so order is preserved.
    let mut pending_file: Option<String> = None;
    let mut pending_code: Option<String> = None;
    let mut prev_was_file = false;

    fn flush_frame(out: &mut String, file: &mut Option<String>, code: &mut Option<String>) {
        if let Some(f) = file.take() {
            out.push_str(&f);
            out.push('\n');
            if let Some(c) = code.take() {
                out.push_str(&c);
                out.push('\n');
            }
        }
    }

    for line in text.lines() {
        let t = line.trim();
        let lead = line.trim_start();

        // Pure decoration rule (`======`, `------`): always drop.
        if is_pure_separator(t) {
            sep_elided += 1;
            prev_was_file = false;
            continue;
        }
        // Redundant traceback header: drop (the frames below carry the info).
        if t == "Traceback (most recent call last):" {
            prev_was_file = false;
            continue;
        }
        // Stack frame location: keep only the deepest, fold the rest.
        if is_frame_file(lead) {
            if pending_file.is_some() {
                // a deeper frame supersedes the buffered one → it was intermediate
                frame_elided += 1;
                pending_code = None;
            }
            pending_file = Some(line.to_string());
            prev_was_file = true;
            continue;
        }
        // Source line immediately under a frame (indented): keep with its frame.
        if prev_was_file && (line.starts_with(' ') || line.starts_with('\t')) {
            pending_code = Some(line.to_string());
            prev_was_file = false;
            continue;
        }
        prev_was_file = false;

        // A `FAIL:` / `ERROR:` header opens a new block. Any buffered frame
        // belongs to the *previous* block's exception, which we'd already
        // have flushed; flush defensively, then echo the header.
        if t.starts_with("FAIL:") || t.starts_with("ERROR:") {
            flush_frame(&mut out, &mut pending_file, &mut pending_code);
            out.push_str(line);
            out.push('\n');
            continue;
        }

        // Anything else — the status dots, the exception message line(s), the
        // `Ran N tests` line, the `OK`/`FAILED(…)` verdict, blank lines. Flush
        // the deepest frame first so it precedes the exception it raised.
        flush_frame(&mut out, &mut pending_file, &mut pending_code);
        out.push_str(line);
        out.push('\n');
    }
    flush_frame(&mut out, &mut pending_file, &mut pending_code);

    if frame_elided > 0 {
        let _ = writeln!(out, "  [elided: {frame_elided} intermediate stack frame(s)]");
    }
    if sep_elided > 0 {
        let _ = writeln!(out, "  [elided: {sep_elided} separator line(s)]");
    }
    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
FE.F
======================================================================
ERROR: test_error (tests.test_money.TestMoney.test_error)
----------------------------------------------------------------------
Traceback (most recent call last):
  File \"/tmp/utdemo/tests/test_money.py\", line 10, in test_error
    raise ValueError(\"boom from nested call\")
ValueError: boom from nested call

======================================================================
FAIL: test_convert (tests.test_money.TestMoney.test_convert)
----------------------------------------------------------------------
Traceback (most recent call last):
  File \"/tmp/utdemo/tests/test_money.py\", line 6, in test_convert
    self.assertEqual(convert(10), 30)
AssertionError: 20 != 30

======================================================================
FAIL: test_other (tests.test_money.TestMoney.test_other)
----------------------------------------------------------------------
Traceback (most recent call last):
  File \"/tmp/utdemo/tests/test_money.py\", line 8, in test_other
    self.assertEqual(add(2, 3), 6)
AssertionError: 5 != 6

----------------------------------------------------------------------
Ran 4 tests in 0.001s

FAILED (failures=2, errors=1)
";

    #[test]
    fn detector_needs_ran_and_verdict() {
        assert!(detect(SAMPLE));
        assert!(!detect(""));
        assert!(!detect("Traceback (most recent call last):\n  File \"x\", line 1, in f\n"));
        assert!(!detect("Ran 3 tests in 0.0s\n")); // no verdict
    }

    #[test]
    fn preserves_identity_location_and_tally() {
        let out = compact(SAMPLE);
        // Status dots survive.
        assert!(out.contains("FE.F"), "status dots dropped: {out}");
        // Every failing test's identity survives, in order.
        assert!(out.contains("ERROR: test_error"));
        assert!(out.contains("FAIL: test_convert"));
        assert!(out.contains("FAIL: test_other"));
        let i_err = out.find("test_error").unwrap();
        let i_conv = out.find("test_convert").unwrap();
        let i_other = out.find("test_other").unwrap();
        assert!(i_err < i_conv && i_conv < i_other, "block order broken: {out}");
        // The frame location + source line + exception message survive together.
        assert!(out.contains("line 6, in test_convert"));
        assert!(out.contains("self.assertEqual(convert(10), 30)"));
        assert!(out.contains("AssertionError: 20 != 30"));
        assert!(out.contains("ValueError: boom from nested call"));
        // The tally survives.
        assert!(out.contains("Ran 4 tests in"));
        assert!(out.contains("FAILED (failures=2, errors=1)"));
        // The decoration + traceback headers are gone.
        assert!(!out.contains("===================="), "rules kept: {out}");
        assert!(!out.contains("Traceback (most recent call last):"), "tb header kept: {out}");
        // And it actually compacts.
        assert!(out.len() < SAMPLE.len(), "did not shrink");
    }

    #[test]
    fn folds_intermediate_frames_keeps_deepest() {
        let input = "\
======================================================================
ERROR: test_chain (m.C.test_chain)
----------------------------------------------------------------------
Traceback (most recent call last):
  File \"/abs/test_x.py\", line 3, in test_chain
    helper()
  File \"/abs/lib.py\", line 9, in helper
    deeper()
  File \"/abs/lib.py\", line 14, in deeper
    raise RuntimeError(\"nope\")
RuntimeError: nope

----------------------------------------------------------------------
Ran 1 test in 0.000s

FAILED (errors=1)
";
        let out = compact(input);
        // Deepest frame (where it raised) + its source line survive.
        assert!(out.contains("line 14, in deeper"), "deepest frame lost: {out}");
        assert!(out.contains("raise RuntimeError(\"nope\")"));
        assert!(out.contains("RuntimeError: nope"));
        // Intermediate frames are folded.
        assert!(!out.contains("in test_chain\n") || !out.contains("helper()"), "intermediate kept");
        assert!(out.contains("intermediate stack frame(s)"), "no fold note: {out}");
        assert!(out.contains("FAILED (errors=1)"));
    }

    #[test]
    fn ok_run_passes_through_cleanly() {
        let input = "...\n----------------------------------------------------------------------\nRan 3 tests in 0.002s\n\nOK\n";
        let out = compact(input);
        assert!(out.contains("..."));
        assert!(out.contains("Ran 3 tests in"));
        assert!(out.contains("OK"));
    }
}
