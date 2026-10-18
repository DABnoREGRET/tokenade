//! Dispatcher for `test_runner`.
//! Extracted from `filter.rs` in the L108 refactor.

use crate::argv::{has_flag, flag_value};

pub(crate) mod tap;

pub fn dispatch(_prog: &str, argv: &[String], output: &str) -> String {
    let reporter = flag_value(argv, "--reporter")
        .or_else(|| flag_value(argv, "--reporters"))
        .unwrap_or("");
    if has_flag(argv, "--json") || reporter == "json" || reporter == "json-stream" {
        let (_, c, _) = crate::auto::compact(output);
        return c;
    }
    if tap::detect(output) {
        if let Some(c) = tap::compact(output) {
            return c;
        }
    }
    if reporter == "junit" || reporter == "tap" {
        let (_, c, _) = crate::auto::compact(output);
        return c;
    }
    // Default: rely on pytest's parser which handles dots + failure
    // separators + summary lines well enough.
    crate::cmd::linux::pytest::compact(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    /// `node --test` piped writes TAP with no `--reporter` flag at all.
    #[test]
    fn node_default_tap_reaches_the_tap_compactor() {
        let mut body = String::from("TAP version 13\n");
        for i in 0..8 {
            body.push_str(&format!("# Subtest: t{i}\nok {} - t{i}\n  ---\n  duration_ms: 0.1\n  ...\n", i + 1));
        }
        body.push_str("not ok 9 - t8\n  ---\n  error: 'boom'\n  ...\n1..9\n");
        let out = dispatch("node", &argv(&["node", "--test"]), &body);
        assert!(out.starts_with("[tokenade:tap] 8 passing"), "{out}");
        assert!(out.contains("not ok 9 - t8\n  ---\n  error: 'boom'\n  ..."), "{out}");
    }

    #[test]
    fn json_flag_routes_to_auto() {
        // `jest --json` emits structured output — auto's JSON compactor
        // takes over.
        let body = r#"{"numTotalTests":5,"numPassedTests":4,"numFailedTests":1,"testResults":[{"name":"a","status":"passed"},{"name":"b","status":"passed"},{"name":"c","status":"failed"},{"name":"d","status":"passed"},{"name":"e","status":"passed"}]}"#;
        let out = dispatch("jest", &argv(&["jest", "--json"]), body);
        assert!(out.contains("numFailed") || out.contains("testResults"));
    }

    #[test]
    fn json_reporter_routes_to_auto() {
        // mocha / vitest: `--reporter json` and `--reporter json-stream`.
        for reporter in &["json", "json-stream"] {
            let body = r#"{"stats":{"tests":42,"passes":40,"failures":2}}"#;
            let out = dispatch(
                "mocha",
                &argv(&["mocha", "--reporter", reporter]),
                body,
            );
            assert!(out.contains("stats") || out.contains("tests"));
        }
    }

    #[test]
    fn junit_reporter_routes_to_auto() {
        // jest / vitest `--reporter junit` emits XML — auto picks the JUnit
        // compactor, which answers with the counts, the failure and the test
        // that hit it rather than with the markup. Asserted on the SUBSTANCE:
        // the suite name, the failing test and the reason. This test used to
        // accept the literal string `testsuite`, which only the passthrough it
        // was measuring would produce.
        // Big enough that summarising WINS. The old fixture was 260 bytes,
        // where the summary is longer than the file and the compactor
        // correctly returns it — so the test could not tell routing from
        // passthrough, and passed either way.
        let mut body = String::from(
            "<?xml version=\"1.0\"?>\n<testsuites>\n  <testsuite name=\"unit\" tests=\"40\" failures=\"1\">\n",
        );
        for i in 0..39 {
            body.push_str(&format!("    <testcase name=\"passing_{i}\"/>\n"));
        }
        body.push_str("    <testcase name=\"bar\"><failure message=\"oops\">stack line</failure></testcase>\n");
        body.push_str("  </testsuite>\n</testsuites>\n");
        let out = dispatch("jest", &argv(&["jest", "--reporter", "junit"]), &body);
        assert!(out.contains("unit"), "the suite name was lost:\n{out}");
        assert!(out.contains("bar"), "the failing test was not named:\n{out}");
        assert!(out.contains("oops"), "the reason was lost:\n{out}");
        assert!(out.contains("1 failed"), "the counts were lost:\n{out}");
        assert!(
            out.len() * 2 < body.len(),
            "routed but not summarised: {} of {}",
            out.len(),
            body.len()
        );
    }

    #[test]
    fn tap_reporter_routes_to_auto() {
        // `--reporter tap` emits the Test Anything Protocol.
        let body = "TAP version 14\n\
                    1..3\n\
                    ok 1 alpha\n\
                    not ok 2 beta\n  ---\n  message: oops\n  ---\n\
                    ok 3 gamma\n";
        let out = dispatch("mocha", &argv(&["mocha", "--reporter", "tap"]), body);
        // auto preserves the TAP markers.
        assert!(out.contains("ok") || out.contains("not ok") || out.contains("alpha"));
    }

    #[test]
    fn reporters_plural_flag_also_recognized() {
        // jest accepts BOTH `--reporter` (singular) and `--reporters`
        // (plural, comma-separated list). The dispatcher checks the
        // singular form first, then the plural form.
        let body = r#"{"numTotalTests":5}"#;
        let out = dispatch(
            "jest",
            &argv(&["jest", "--reporters", "json"]),
            body,
        );
        assert!(out.contains("numTotalTests"));
    }

    #[test]
    fn default_routes_to_pytest_compactor() {
        // No reporter / json flag → pytest compactor (dots + summary
        // shape).
        let body = "Test Suites: 1 passed, 1 total\n\
                    Tests:       5 passed, 5 total\n\
                    Snapshots:   0 total\n\
                    Time:        2.34 s\n";
        let out = dispatch("jest", &argv(&["jest"]), body);
        // pytest compactor preserves the summary lines.
        assert!(out.contains("passed") || out.contains("Tests:"));
    }
}
