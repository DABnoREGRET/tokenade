//! `pytest` — pytest.
//!
//! The old implementation was a one-liner `strip_noise(&["====","----"])`,
//! which had two bugs: (1) it barely saved anything on a real run (the bulk of
//! a noisy pytest log is *passing* test lines and the `warnings summary`
//! section, neither of which start with `=`/`-`), and (2) `strip_noise` drops
//! any line *beginning* with `====`, so it silently deleted the single most
//! useful line — the final tally `=== 1 failed, 60 passed in 4.21s ===`.
//!
//! This version keeps every diagnostic (collected count, FAILED/ERROR lines,
//! failure tracebacks, section headers, the final tally) and folds the two real
//! sources of noise: runs of verbose `… PASSED` lines and the `warnings
//! summary` body.

use std::fmt::Write;

pub fn detect(text: &str) -> bool {
    text.contains("PASSED") || text.contains("FAILED") || text.contains("===")
        || text.contains("passed") || text.contains("failed")
        // jest/vitest/mocha/jasmine + TAP markers, so this one line-based engine
        // folds every JS/Ruby/PHP runner routed here (and `npm test` wrappers),
        // not just pytest. mocha prints only `N passing`/`M failing`; vitest and
        // jest emit checkmark rows and `PASS `/`FAIL ` file lines.
        || text.contains("PASS ") || text.contains("FAIL ")
        || text.contains('✓') || text.contains('✗')
        || text.contains("passing") || text.contains("failing")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    Some(compact(text))
}

/// A pytest banner line like `=== short test summary info ===`: `=` bars
/// wrapping real words. Distinct from pure decoration (`==========`), which
/// carries no information and is dropped.
fn is_section_header(trimmed: &str) -> bool {
    trimmed.starts_with('=')
        && trimmed.trim_matches('=').trim().chars().any(|c| c.is_alphabetic())
}

/// A line that is *only* `=`/`-` decoration (and spaces) — pure noise.
fn is_pure_separator(trimmed: &str) -> bool {
    !trimmed.is_empty() && trimmed.chars().all(|c| c == '=' || c == '-' || c == ' ')
}

/// A single *passing* test line from any runner routed through this engine:
/// pytest (`tests/x.py::test_a PASSED [..]`), a jest file pass (`PASS src/x.test.js`),
/// or a checkmark row from vitest/mocha/jasmine (`✓ should …`, `√ …` on Windows).
/// Lines carrying any failure signal (FAILED/ERROR/✗/✕/×) are never folded — that
/// is the information the agent ran the tests for. Summary tallies
/// (`Tests: 2 failed, 200 passed`, `N passing`, `Test Files 1 failed`) match none
/// of the positive cases, so they always survive verbatim regardless of position —
/// which is why this engine is safe where a head-capped listing fold is not.
fn is_passing_test_line(t: &str) -> bool {
    if t.contains("FAILED") || t.contains("ERROR")
        || t.contains('✗') || t.contains('✕') || t.contains('×')
    {
        return false;
    }
    t.contains(" PASSED")                                   // pytest
        || t.starts_with("PASS ")                           // jest file-level pass
        || t.starts_with('✓') || t.starts_with('✔') || t.starts_with('√') // vitest/mocha/jasmine
        || is_all_dots_progress_line(t)                     // pytest DEFAULT output
}

/// pytest's DEFAULT per-file progress line: `tests/test_x.py ........ [ 42%]`.
///
/// `PASSED` only appears with `-v`, so without this the shape most users
/// actually see was never folded: every line is DISTINCT (different path,
/// different percentage), so neither the identical-run fold nor a line cap
/// removes it. Measured on a 400-file run with one failure — 141 of the 243
/// delivered lines were this, against 4 lines of actual failure.
///
/// Folded ONLY when every status character is a pass (`.`). pytest packs the
/// per-test outcome into those characters, so a single `F`, `E` or `x` makes
/// the line the thing the agent is looking for — which file failed — and it
/// survives verbatim.
fn is_all_dots_progress_line(t: &str) -> bool {
    let body = match t.rsplit_once('[') {
        // Trailing `[ 42%]` is optional (absent on the last line of some runs).
        Some((head, pct)) if pct.trim_end().ends_with("%]") => head,
        _ => t,
    };
    let (path, dots) = match body.trim_end().split_once(char::is_whitespace) {
        Some(p) => p,
        None => return false,
    };
    if !path.ends_with(".py") {
        return false;
    }
    let dots = dots.trim();
    !dots.is_empty() && dots.chars().all(|c| c == '.')
}

pub fn compact(text: &str) -> String {
    // A green vitest run: the tally is the answer, the app's logs are not.
    if let Some(green) = super::vitest_pass::try_compact(text) {
        return green;
    }
    compact_inner(text, false)
}

/// `--tb=no` variant: the user explicitly asked pytest to print *no*
/// tracebacks, so in addition to the normal folding we defensively elide any
/// `FAILURES`/`ERRORS` traceback block that is still present (e.g. a saved full
/// log, or differing pytest config). Test identity and counts are preserved by
/// the `short test summary info` `FAILED …` lines and the final tally, so
/// nothing answerable is lost.
pub fn compact_no_tracebacks(text: &str) -> String {
    compact_inner(text, true)
}

/// Does this section header open a failure-traceback block (`=== FAILURES ===`,
/// `=== ERRORS ===`)? Used only by the `--tb=no` path.
fn is_traceback_header(trimmed: &str) -> bool {
    let inner = trimmed.trim_matches('=').trim().to_lowercase();
    inner == "failures" || inner == "errors"
}

/// **The warnings summary says WHICH warning, and where.** Its body is test ids
/// (`test_a.py::test_x`, `test_a.py: 40 warnings`), the source line echoed, and
/// the line that matters: `path:LINE: DeprecationWarning: old api used`. The
/// whole body used to fold to `[elided: 4 warning line(s)]` — the tally still
/// said `40 warnings`, and nothing said what they were. Each DISTINCT message
/// is kept once, at its first site; its other sites are counted.
#[derive(Default)]
struct WarningMessages {
    kept: Vec<(String, String, usize)>, // (message, first line, other sites)
}

impl WarningMessages {
    const MAX: usize = 10;

    /// Takes `t` if it is a warning message line; `false` for anything else.
    fn take(&mut self, t: &str) -> bool {
        let Some(message) = warning_message(t) else { return false };
        if let Some(k) = self.kept.iter_mut().find(|k| k.0 == message) {
            k.2 += 1;
            return true;
        }
        if self.kept.len() >= Self::MAX {
            return false;
        }
        self.kept.push((message.to_string(), t.to_string(), 0));
        true
    }

    fn flush(&mut self, out: &mut String, elided: &mut usize) {
        for (_, line, more) in self.kept.drain(..) {
            let _ = match more {
                0 => writeln!(out, "  {line}"),
                n => writeln!(out, "  {line}  (+{n} more site(s) with this message)"),
            };
        }
        if *elided > 0 {
            let _ = writeln!(out, "  [elided: {elided} warning line(s)]");
            *elided = 0;
        }
    }
}

/// `path:LINE: SomeWarning: text` → `SomeWarning: text`.
fn warning_message(t: &str) -> Option<&str> {
    let mut parts = t.splitn(3, ": ");
    let (site, message) = (parts.next()?, t.get(t.find(": ")? + 2..)?);
    let (_, line) = site.rsplit_once(':')?;
    let kind = message.split(": ").next()?;
    (line.bytes().all(|b| b.is_ascii_digit()) && !line.is_empty() && kind.ends_with("Warning") && !kind.contains(' '))
        .then_some(message)
}

fn compact_inner(text: &str, drop_tracebacks: bool) -> String {
    // A JavaScript failure's `at …` stack: the program's frames, the
    // runtime's counted (Node's spec reporter, jest and mocha route here).
    let (folded, _) = crate::stacktrace::fold_js_frame_runs(text);
    let text = folded.as_str();
    let mut out = String::with_capacity(text.len());
    let mut passed_run = 0usize; // consecutive verbose `… PASSED` lines folded
    let mut sep_elided = 0usize; // pure-decoration separator lines dropped
    let mut warn_elided = 0usize; // lines folded inside the warnings summary
    let mut warnings = WarningMessages::default();
    let mut tb_elided = 0usize; // lines folded inside a FAILURES/ERRORS block
    let mut in_warnings = false;
    let mut in_traceback = false; // only ever true when drop_tracebacks

    let flush_passed = |out: &mut String, run: &mut usize| {
        if *run > 0 {
            let _ = writeln!(out, "  [elided: {run} passing test line(s)]");
            *run = 0;
        }
    };

    for line in text.lines() {
        let t = line.trim_start();

        // Inside the `warnings summary` block: fold the body until the next
        // section header (the tally / short-summary banner) ends it.
        if in_warnings {
            if is_section_header(t) {
                warnings.flush(&mut out, &mut warn_elided);
                in_warnings = false;
                // fall through to handle this header normally
            } else {
                if !t.is_empty() && !warnings.take(t) {
                    warn_elided += 1;
                }
                continue;
            }
        }

        // Inside a `FAILURES`/`ERRORS` block under `--tb=no`: fold the entire
        // body (per-test headers + tracebacks) until the next section header.
        if in_traceback {
            if is_section_header(t) {
                if tb_elided > 0 {
                    let _ = writeln!(out, "  [elided: {tb_elided} traceback line(s)]");
                    tb_elided = 0;
                }
                in_traceback = false;
                // fall through to handle this header normally
            } else {
                if !t.is_empty() {
                    tb_elided += 1;
                }
                continue;
            }
        }

        if is_section_header(t) {
            flush_passed(&mut out, &mut passed_run);
            // `--tb=no`: open a foldable traceback block instead of echoing it.
            if drop_tracebacks && is_traceback_header(t) {
                in_traceback = true;
                continue;
            }
            out.push_str(line);
            out.push('\n');
            // `warnings summary` headers open a foldable block.
            if t.to_lowercase().contains("warnings summary") {
                in_warnings = true;
            }
            continue;
        }

        // Fold a passing-test line from any runner we route here. Never fold a
        // line that also carries failure signal — that's what the agent wants.
        if is_passing_test_line(t) {
            passed_run += 1;
            continue;
        }

        // Drop pure decoration (`==========`, `----------`).
        if is_pure_separator(t) {
            flush_passed(&mut out, &mut passed_run);
            sep_elided += 1;
            continue;
        }

        flush_passed(&mut out, &mut passed_run);
        out.push_str(line);
        out.push('\n');
    }
    flush_passed(&mut out, &mut passed_run);
    warnings.flush(&mut out, &mut warn_elided);
    if tb_elided > 0 {
        let _ = writeln!(out, "  [elided: {tb_elided} traceback line(s)]");
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

    #[test]
    fn default_progress_lines_fold_but_a_failing_file_survives() {
        let mut input = String::new();
        for i in 0..200 {
            input.push_str(&format!("tests/test_mod{i}.py .................. [{i}%]\n"));
        }
        input.push_str("tests/test_broken.py ....F..... [99%]\n");
        input.push_str("FAILED tests/test_broken.py::test_zqx - assert 41 == 42\n");
        let out = super::compact(&input);
        assert!(
            out.contains("tests/test_broken.py ....F..... [99%]"),
            "the line naming the failing FILE survives:\n{out}"
        );
        assert!(out.contains("FAILED tests/test_broken.py::test_zqx"));
        assert!(
            !out.contains("tests/test_mod150.py"),
            "the passing progress lines are folded:\n{out}"
        );
        assert!(out.contains("passing test line(s)"), "the fold is declared:\n{out}");
    }

    #[test]
    fn a_line_that_is_not_pytest_progress_is_left_alone() {
        for line in [
            "src/main.rs 12 warnings",
            "tests/test_a.py",
            "tests/test_a.py ..F.. [10%]",
            "something.txt ..... [10%]",
        ] {
            assert!(
                !super::is_all_dots_progress_line(line),
                "must not fold: {line}"
            );
        }
        assert!(super::is_all_dots_progress_line("tests/test_a.py ..... [10%]"));
        assert!(super::is_all_dots_progress_line("tests/test_a.py ....."));
    }
    use super::*;

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
        assert!(!detect("   \n"));
    }

    #[test]
    fn try_compact_some_or_none() {
        let s = "sample output line\nsecond line\n";
        let _ = try_compact(s);
        let _ = compact(s);
        let _ = compact_with_savings(s);
    }

    #[test]
    fn keeps_tally_and_failure_folds_passes() {
        let mut input = String::from(
            "============================= test session starts =============================\n\
             collected 61 items\n\n",
        );
        for i in 0..60 {
            input.push_str(&format!("tests/test_api.py::test_case_{i:02} PASSED        [ {i}%]\n"));
        }
        input.push_str(
            "tests/test_billing.py::test_proration FAILED                    [100%]\n\
             ==================================== FAILURES ==================================\n\
             ____________________________ test_proration ____________________________________\n\
             >       assert invoice.total == 1999\n\
             E       assert 2099 == 1999\n\
             tests/test_billing.py:142: AssertionError\n\
             =========================== 1 failed, 60 passed in 4.21s =======================\n",
        );
        let out = compact(&input);

        // The single most useful line — the tally — MUST survive (the old
        // strip_noise dropped it because it begins with `=`).
        assert!(out.contains("1 failed, 60 passed"), "tally dropped: {out}");
        // Failure signal is never folded.
        assert!(out.contains("test_proration FAILED"));
        assert!(out.contains("2099 == 1999"));
        assert!(out.contains("collected 61 items"));
        // The 60 passing lines are folded, not echoed verbatim.
        assert!(out.contains("passing test line(s)"), "passes not folded: {out}");
        assert!(!out.contains("test_case_30 PASSED"));
        // And it actually saves.
        assert!(out.len() * 2 < input.len(), "did not compact enough");
    }

    #[test]
    fn folds_warnings_summary_body() {
        let mut input = String::from(
            "tests/test_a.py .                                                        [100%]\n\
             =============================== warnings summary ===============================\n",
        );
        for i in 0..30 {
            input.push_str(&format!(
                "tests/test_a.py::test_a\n  /usr/lib/python3/foo.py:{i}: DeprecationWarning: x is deprecated\n",
            ));
        }
        input.push_str(
            "-- Docs: https://docs.pytest.org/en/stable/how-to/capture-warnings.html\n\
             ========================= 1 passed, 0 failed in 0.10s ==========================\n",
        );
        let out = compact(&input);
        assert!(out.contains("warnings summary"), "header kept");
        assert!(out.contains("warning line(s)"), "warnings folded: {out}");
        // The message is the one thing the body says; it stays, once.
        assert_eq!(out.matches("DeprecationWarning: x is deprecated").count(), 1, "{out}");
        assert!(out.contains("/usr/lib/python3/foo.py:0: DeprecationWarning: x is deprecated  (+29 more site(s) with this message)"), "{out}");
        assert!(!out.contains("tests/test_a.py::test_a"), "{out}");
        assert!(out.contains("1 passed, 0 failed"), "tally kept");
    }

    #[test]
    fn tb_no_elides_traceback_block_keeps_identity() {
        // `--tb=no` defensively drops any FAILURES traceback that's present,
        // while the failing test's identity + counts survive via the short
        // summary and the tally.
        let input = "============================= test session starts =============================\n\
                     collected 61 items\n\n\
                     tests/test_billing.py ...F                                              [100%]\n\
                     ==================================== FAILURES ==================================\n\
                     ____________________________ test_proration ____________________________________\n\
                     >       assert invoice.total == 1999\n\
                     E       assert 2099 == 1999\n\
                     tests/test_billing.py:142: AssertionError\n\
                     =========================== short test summary info ============================\n\
                     FAILED tests/test_billing.py::test_proration - assert 2099 == 1999\n\
                     =========================== 1 failed, 60 passed in 4.21s =======================\n";
        let out = compact_no_tracebacks(input);
        // The traceback body is gone …
        assert!(!out.contains("AssertionError"), "traceback body kept: {out}");
        assert!(out.contains("traceback line(s)"), "traceback not folded: {out}");
        // … but identity (FAILED line) and counts (tally) survive.
        assert!(out.contains("FAILED tests/test_billing.py::test_proration"), "identity lost: {out}");
        assert!(out.contains("1 failed, 60 passed"), "tally lost: {out}");
        // The default compactor, by contrast, keeps the traceback.
        assert!(compact(input).contains("AssertionError"), "default must keep tracebacks");
    }

    #[test]
    fn preserves_collect_only_tree() {
        // `--collect-only` output routes here too; its `<Module>`/`<Function>`
        // markers and `collected` count must survive untouched.
        let input = "============================= test session starts =============================\n\
                     collected 3 items\n\n\
                     <Module test_a.py>\n  <Function test_one>\n  <Function test_two>\n\
                     <Module test_b.py>\n  <Function test_three>\n\n\
                     ========================== 3 tests collected in 0.05s ==========================\n";
        let out = compact(input);
        assert!(out.contains("collected 3 items"));
        assert!(out.contains("<Function test_one>"));
        assert!(out.contains("<Module test_b.py>"));
    }

    /// A real `node --test --test-reporter=spec` failure: the test's frame
    /// stays, the runner's six are counted, and the brace that opens the
    /// error's properties survives the frame it was printed on.
    #[test]
    fn node_spec_failure_keeps_its_frame_and_its_brace() {
        let raw = "✖ failing tests:\n\n✖ case 150 (0.4ms)\n  AssertionError [ERR_ASSERTION]: budget total\n\n      at TestContext.<anonymous> (x.test.js:154:33)\n      at Test.runInAsyncScope (node:async_hooks:214:14)\n      at Test.run (node:internal/test_runner/test:1047:25)\n      at async Test.processPendingSubtests (node:internal/test_runner/test:744:7) {\n    code: 'ERR_ASSERTION',\n    actual: 41,\n  }\n";
        let out = compact(raw);
        assert!(
            out.contains("      at TestContext.<anonymous> (x.test.js:154:33)\n      [3 runtime frame(s) not shown] {\n    code: 'ERR_ASSERTION',"),
            "{out}"
        );
        assert!(!out.contains("node:internal"), "{out}");
    }
}
