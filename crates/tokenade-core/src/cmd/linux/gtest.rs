//! GoogleTest — any test binary built with it, whatever its name.
//!
//! ```text
//! [==========] Running 180 tests from 3 test suites.
//! [ RUN      ] SchedulerTest.Case17
//! mla_tests.cpp:81: Failure
//! Expected equality of these values: …
//! [  FAILED  ] SchedulerTest.Case17 (0 ms)
//! …
//! [==========] 180 tests from 3 test suites ran. (0 ms total)
//! [  PASSED  ] 178 tests.
//! [  FAILED  ] 2 tests, listed below:
//! ```
//!
//! A test binary has its own name (`mla_base_lib_test_multi_thread`: 373 k
//! uncovered tokens over 175 runs across the fleet in 30 days), so only the
//! CONTENT can route it. The generic fold gathered lines by shape: on a real
//! 180-test run with two failures, the second failure's `file:line: Failure`
//! was printed under the FIRST failing suite and its expected values were
//! merged with the other's — which message belongs to which test was lost.
//!
//! Kept: the run header, every failing test's block whole and in order, one
//! count per suite of the tests that passed, and the closing summary verbatim.

use std::fmt::Write;

pub fn detect(text: &str) -> bool {
    text.lines().any(|l| l.starts_with("[==========] Running ")) && text.lines().any(|l| l.starts_with("[ RUN      ] "))
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = String::new();
    let mut passed_in_suite = 0usize;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.starts_with("[ RUN      ] ") {
            // A test's block runs to its verdict line.
            let mut j = i + 1;
            while j < lines.len() && !lines[j].starts_with("[       OK ] ") && !lines[j].starts_with("[  FAILED  ] ") && !lines[j].starts_with("[  SKIPPED ] ") {
                j += 1;
            }
            let verdict = lines.get(j).copied().unwrap_or("");
            if verdict.starts_with("[       OK ] ") && j == i + 1 {
                passed_in_suite += 1;
            } else {
                for l in &lines[i..=j.min(lines.len() - 1)] {
                    let _ = writeln!(out, "{l}");
                }
            }
            i = j + 1;
            continue;
        }
        // `[----------] 60 tests from X (0 ms total)` closes a suite.
        if line.starts_with("[----------] ") && line.ends_with(" total)") && passed_in_suite > 0 {
            let _ = writeln!(out, "  … {passed_in_suite} test(s) passed");
            passed_in_suite = 0;
        }
        let _ = writeln!(out, "{line}");
        i += 1;
    }
    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN: &str = include_str!("../../../tests/fixtures/linux/gtest_two_failures.txt");

    #[test]
    fn each_failure_stays_with_its_test_and_the_summary_is_whole() {
        assert!(detect(RUN));
        let out = compact(RUN);
        // Each failure's block, whole and in order.
        let sched = out.find("[ RUN      ] SchedulerTest.Case17").expect("first failure");
        let codec = out.find("[ RUN      ] CodecTest.Case42").expect("second failure");
        assert!(sched < codec, "{out}");
        assert!(out[sched..codec].contains("mla_tests.cpp:81: Failure"), "{out}");
        assert!(out[sched..codec].contains("sum of Scheduler batch 17"), "{out}");
        assert!(out[codec..].contains("mla_tests.cpp:166: Failure"), "{out}");
        assert!(out[codec..].contains("sum of Codec batch 42"), "{out}");
        // Passing tests are counted, not listed.
        assert!(!out.contains("[       OK ] ParserTest.Case07"), "{out}");
        assert!(out.contains("  … 60 test(s) passed"), "{out}");
        assert!(out.contains("  … 59 test(s) passed"), "{out}");
        // The summary, verbatim.
        for l in ["[==========] 180 tests from 3 test suites ran. (0 ms total)", "[  PASSED  ] 178 tests.", "[  FAILED  ] 2 tests, listed below:", " 2 FAILED TESTS"] {
            assert!(out.contains(l), "{l}: {out}");
        }
        assert!(out.len() * 5 < RUN.len(), "{} vs {}", out.len(), RUN.len());
    }

    #[test]
    fn a_run_without_the_header_is_not_googletest() {
        assert!(!detect("[ RUN      ] A.b\n[       OK ] A.b (0 ms)\n"));
    }
}
