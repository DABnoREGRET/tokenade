//! `npm_test` — `npm test` / `npm t` (and the pnpm/yarn/bun equivalents).
//!
//! `npm test` is a wrapper: it shells out to whatever runner the project uses
//! (jest, vitest, mocha, …). Those runners print their failures *and the final
//! tally* AT THE END of the output. The old implementation folded with a
//! head-capped listing engine (`git_listing::fold`, first 80 lines kept), which
//! on any run longer than 80 lines decapitated exactly that signal — the agent
//! got 80 lines of `PASS` and lost which tests failed. We now route through the
//! shared line-based test compactor (`pytest::compact`), which folds runs of
//! passing lines while preserving every failure, traceback and tally regardless
//! of position. Same engine the direct `jest`/`vitest`/`mocha` paths use.

pub fn detect(text: &str) -> bool {
    crate::cmd::linux::pytest::detect(text)
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    Some(crate::cmd::linux::pytest::compact(text))
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
    }

    #[test]
    fn detector_accepts_runner_output() {
        assert!(detect("PASS src/a.test.js\nTests: 1 passed"));
        assert!(detect("  ✓ should work\n  50 passing")); // mocha
    }

    /// The regression this module exists to prevent: jest prints all passing
    /// files first and the failure + tally last. A head-capped fold would drop
    /// the tail; the line-based engine keeps the failure and the summary while
    /// folding the long run of passes.
    fn jest_run_with_trailing_failure() -> String {
        let mut s = String::new();
        for i in 0..120 {
            s.push_str(&format!("PASS src/unit_{i:03}.test.js\n"));
        }
        s.push_str("FAIL src/billing.test.js\n");
        s.push_str("  ● applies proration\n");
        s.push_str("    expect(received).toBe(expected)\n");
        s.push_str("Tests:       1 failed, 120 passed, 121 total\n");
        s.push_str("Test Suites: 1 failed, 120 passed, 121 total\n");
        s
    }

    #[test]
    fn jest_failure_and_tally_survive_past_head_cap() {
        let out = try_compact(&jest_run_with_trailing_failure()).unwrap();
        // The failing file, its detail, and the final tally are all preserved…
        assert!(out.contains("FAIL src/billing.test.js"), "failure line dropped:\n{out}");
        assert!(out.contains("applies proration"));
        assert!(out.contains("1 failed, 120 passed, 121 total"));
        assert!(out.contains("Test Suites:"));
        // …and the 120 passing files are folded, not echoed verbatim.
        assert!(out.contains("passing test line"), "passes not folded:\n{out}");
        assert!(!out.contains("PASS src/unit_050.test.js"));
    }

    #[test]
    fn vitest_checkmarks_fold_failures_kept() {
        let mut s = String::new();
        for i in 0..90 {
            s.push_str(&format!("✓ src/comp_{i:02}.test.ts (3)\n"));
        }
        s.push_str("✗ src/auth.test.ts (1)\n");
        s.push_str("Test Files  1 failed | 90 passed (91)\n");
        s.push_str("Tests  1 failed | 270 passed (271)\n");
        let out = try_compact(&s).unwrap();
        assert!(out.contains("✗ src/auth.test.ts"), "vitest failure dropped:\n{out}");
        assert!(out.contains("1 failed | 270 passed"));
        assert!(out.contains("passing test line"));
        assert!(!out.contains("comp_45"));
    }

    #[test]
    fn short_clean_output_is_preserved() {
        // A handful of lines with no foldable run: nothing is lost.
        let s = "PASS src/a.test.js\nTests: 1 passed, 1 total\n";
        let out = compact(s);
        assert!(out.contains("Tests: 1 passed, 1 total"));
    }
}
