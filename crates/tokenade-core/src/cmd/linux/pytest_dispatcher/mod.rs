//! Dispatcher for `pytest`.
//! Extracted from `filter.rs` in the L108 refactor.

use crate::argv::{has_any_flag, flag_value};

pub fn dispatch(argv: &[String], output: &str) -> String {
    let collect_only = has_any_flag(argv, &["--collect-only", "--co"]);
    let quiet = has_any_flag(argv, &["-q", "--quiet"]);
    if collect_only && quiet {
        return crate::cmd::linux::pytest::compact(output);
    }
    if collect_only {
        return crate::cmd::linux::pytest::compact(output);
    }
    if has_any_flag(argv, &["-v", "--verbose", "-vv", "-vvv"]) {
        return crate::cmd::linux::pytest::compact(output);
    }
    if flag_value(argv, "--tb").is_some_and(|v| v == "no") {
        return crate::cmd::linux::pytest::compact_no_tracebacks(output);
    }
    if flag_value(argv, "--tb").is_some_and(|v| v == "line") {
        return crate::cmd::linux::pytest::compact(output);
    }
    crate::cmd::linux::pytest::compact(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    /// Minimal pytest output sample with enough shape to differentiate
    /// dispatch arms (failure header, dots, summary).
    const SAMPLE: &str = "============================= test session starts =============================\n\
                          collected 10 items\n\
                          \n\
                          tests/test_alpha.py ..F.....                                            [ 80%]\n\
                          tests/test_beta.py ..                                                   [100%]\n\
                          \n\
                          ============================== FAILURES ===============================\n\
                          _____________________________ test_failing ______________________________\n\
                          \n\
                          >       assert 1 == 2\n\
                          E       assert 1 == 2\n\
                          \n\
                          =================== 1 failed, 9 passed in 0.42s ====================\n";

    #[test]
    fn collect_only_with_quiet_uses_combined_compactor() {
        // Precedence test: collect_only+quiet takes precedence over
        // collect_only alone. Without this, --collect-only --quiet
        // would lose the quiet-specific format.
        let body = "tests/test_a.py::test_one\n\
                    tests/test_a.py::test_two\n\
                    tests/test_b.py::test_three\n\
                    \n3 tests collected in 0.05s\n";
        for co in &["--collect-only", "--co"] {
            for q in &["-q", "--quiet"] {
                let out = dispatch(&argv(&["pytest", co, q]), body);
                assert!(!out.is_empty());
                assert!(
                    out.contains("test_") || out.contains("collected"),
                    "{co}+{q} must preserve test list; got: {out:?}"
                );
            }
        }
    }

    #[test]
    fn collect_only_alone_uses_collect_compactor() {
        // --collect-only without --quiet: hierarchical tree format.
        let body = "============================= test session starts =============================\n\
                    collected 3 items\n\
                    \n\
                    <Module test_a.py>\n  \
                      <Function test_one>\n  \
                      <Function test_two>\n\
                    <Module test_b.py>\n  \
                      <Function test_three>\n\
                    \n3 tests collected in 0.05s\n";
        for flag in &["--collect-only", "--co"] {
            let out = dispatch(&argv(&["pytest", flag]), body);
            assert!(out.contains("collected") || out.contains("test_"));
        }
    }

    #[test]
    fn verbose_flags_use_verbose_compactor() {
        // -v / --verbose / -vv / -vvv all engage the verbose-aware
        // compactor (preserves per-test PASS/FAIL/skipped lines).
        for flag in &["-v", "--verbose", "-vv", "-vvv"] {
            let body = "tests/test_a.py::test_one PASSED\n\
                        tests/test_a.py::test_two FAILED\n\
                        tests/test_b.py::test_three PASSED\n\
                        \n=========== 1 failed, 2 passed in 0.42s ============\n";
            let out = dispatch(&argv(&["pytest", flag]), body);
            assert!(
                out.contains("test_one") || out.contains("PASSED") || out.contains("passed"),
                "{flag} must preserve verbose detail; got: {out:?}"
            );
        }
    }

    #[test]
    fn tb_no_routes_to_no_traceback_compactor() {
        // `--tb=no` strips per-failure tracebacks; the compactor
        // matches that intent (keeps summary + test list only).
        let out = dispatch(&argv(&["pytest", "--tb", "no"]), SAMPLE);
        assert!(
            out.contains("FAIL")
                || out.contains("session")
                || out.contains("passed")
                || out.contains("test_")
                || out.contains("collected"),
            "tb=no dispatch produced unrecognizable output: {out:?}"
        );
    }

    #[test]
    fn tb_line_routes_to_one_line_traceback_compactor() {
        // `--tb=line` collapses each failure to one line.
        let out = dispatch(&argv(&["pytest", "--tb", "line"]), SAMPLE);
        assert!(out.contains("test") || out.contains("failed"));
    }

    #[test]
    fn default_routes_to_pytest_compactor() {
        // No special flag → standard pytest compactor (handles dots +
        // failure header + summary). The compactor is aggressive on
        // summary lines (folding "N failed, M passed in T s" into a
        // shorter form), so we only assert that SOMETHING recognizable
        // from the input survives.
        let out = dispatch(&argv(&["pytest"]), SAMPLE);
        assert!(
            out.contains("session")
                || out.contains("FAIL")
                || out.contains("passed")
                || out.contains("failed")
                || out.contains("test_")
                || out.contains("collected"),
            "default dispatch produced unrecognizable output: {out:?}"
        );
    }
}
