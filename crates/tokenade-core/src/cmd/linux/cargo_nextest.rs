//! `cargo_nextest` — cargo nextest.
//!
//! Form-aware: keeps this command's specific detector, but folds its
//! output by detected shape — columnar table (surfaces problem rows),
//! key/value records (never cut mid-record), else head+tail (the final
//! outcome line is never decapitated). Replaces the blind head-slice.
const HEAD: usize = 80;

pub fn detect(text: &str) -> bool {
    text.contains("PASS") || text.contains("FAIL") || text.contains("test[")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    passing_counted(text).or_else(|| crate::archetypes::form_aware(text, HEAD, 5, "cargo-nextest", "line(s)"))
}

/// `        PASS [   0.012s] (17/400) crate::bin mod::test` — one line of
/// answer per passing test.
fn is_pass_line(line: &str) -> bool {
    line.trim_start().strip_prefix("PASS [").is_some_and(|r| r.contains("s]"))
}

/// Passing tests needed before counting them pays for the banner.
const MIN_PASSING: usize = 5;

/// Every PASS line counted, everything else as printed: a failure's block, a
/// SLOW or retry note, the `Summary` line. The head+tail fold this replaces
/// hands a run that fits back WHOLE, the generic pass then won and its cut
/// took the `Summary [ 12.4s] 400 tests run: 400 passed` line with it
/// (2026-10-03).
fn passing_counted(text: &str) -> Option<String> {
    let passing = text.lines().filter(|l| is_pass_line(l)).count();
    if passing < MIN_PASSING {
        return None;
    }
    let rest: Vec<&str> = text.lines().filter(|l| !is_pass_line(l)).collect();
    Some(format!(
        "[tokenade:cargo-nextest] {passing} passing test(s) folded — every other line as printed\n{}",
        rest.join("\n").trim_end()
    ))
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;

    /// A green run: the passing lines are a count, the summary is kept.
    #[test]
    fn passing_tests_are_counted_and_the_summary_kept() {
        let mut t = String::from("    Starting 400 tests across 12 binaries\n");
        for i in 1..=400 {
            t.push_str(&format!("        PASS [   0.0{}s] ({i}/400) cli::e2e mod{}::case_{i}\n", i % 9, i % 12));
        }
        t.push_str("        FAIL [   0.31s] (401/401) cli::e2e misc::the_one_that_broke\n");
        t.push_str("     Summary [  12.4s] 401 tests run: 400 passed, 1 failed, 0 skipped\n");
        let out = try_compact(&t).unwrap();
        assert_eq!(
            out,
            "[tokenade:cargo-nextest] 400 passing test(s) folded — every other line as printed\n    \
             Starting 400 tests across 12 binaries\n        FAIL [   0.31s] (401/401) cli::e2e misc::the_one_that_broke\n     \
             Summary [  12.4s] 401 tests run: 400 passed, 1 failed, 0 skipped"
        );
    }

    #[test]
    fn detector_rejects_empty() { assert!(!detect("")); }

    #[test]
    fn detector_accepts_specific_format() { assert!(detect("PASS test output")); }

    #[test]
    fn short_output_passes_through() {
        let s: String = std::iter::repeat_n("PASS test output", 3).map(|l| format!("{l}\n")).collect();
        assert!(try_compact(&s).is_none());
    }

    #[test]
    fn long_output_folds_non_decapitating() {
        let s: String = std::iter::repeat_n("PASS test output", HEAD + 20).map(|l| format!("{l}\n")).collect();
        let out = try_compact(&s).unwrap();
        assert!(out.contains("[tokenade:cargo-nextest"));
    }

    fn run_of(n: usize, failing: Option<usize>) -> String {
        let names = [
            "keeps_a_failed_target",
            "expands_a_panic",
            "names_a_timeout",
            "folds_the_budget",
        ];
        let mut out = format!("    Starting {n} tests across 12 binaries\n");
        for i in 1..=n {
            let v = if Some(i) == failing { "FAIL" } else { "PASS" };
            out.push_str(&format!(
                "        {v} [   0.0{}s] ({i}/{n}) tokenade-cli::e2e mod{}::{}_{i}\n",
                i % 9,
                i % 12,
                names[i % names.len()]
            ));
        }
        let failed = usize::from(failing.is_some());
        out.push_str(&format!(
            "     Summary [  12.4s] {n} tests run: {} passed, {failed} failed, 0 skipped\n",
            n - failed
        ));
        out
    }

    /// A green run does not announce problems, and a red one hoists the real
    /// failure. MEASURED 2026-09-04: on 400 tests with exactly ONE failure the
    /// banner read `98 problem(s) first` — the other 97 were passing tests
    /// whose NAMES carry the words `failed`, `panic`, `timeout`. With no
    /// failure at all it still read `1 problem(s) first`, promoting the summary
    /// line `400 passed, 0 failed, 0 skipped` for the word `failed` standing
    /// next to a zero.
    #[test]
    fn only_a_real_failure_counts_as_a_problem() {
        let green = try_compact(&run_of(400, None)).unwrap();
        assert!(
            !green.contains("problem(s) first"),
            "a run where nothing failed announces problems:\n{}",
            green.lines().next().unwrap_or_default()
        );

        // Rewritten 2026-10-03: passing lines are now COUNTED, so nothing is
        // hoisted above them — the failure is simply one of the few lines left.
        let red = try_compact(&run_of(400, Some(200))).unwrap();
        assert!(red.starts_with("[tokenade:cargo-nextest] 399 passing test(s) folded"), "{red}");
        assert!(red.lines().any(|l| l.trim_start().starts_with("FAIL [")), "{red}");
        assert!(red.contains("399 passed, 1 failed"), "{red}");
        assert!(!red.lines().any(|l| l.trim_start().starts_with("PASS [")), "{red}");
    }
}
