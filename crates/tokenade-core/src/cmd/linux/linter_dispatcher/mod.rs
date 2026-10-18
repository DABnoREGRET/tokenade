//! Dispatcher for `linter`.
//! Extracted from `filter.rs` in the L108 refactor.

use crate::compress::dedup_lines;
use crate::argv::{has_flag, has_any_flag, flag_value};
use crate::compress::truncate_middle;
use crate::ansi::strip_ansi;

pub fn dispatch(argv: &[String], output: &str) -> String {
    // JSON / NDJSON shape → auto compactor handles it.
    let fmt = flag_value(argv, "--format")
        .or_else(|| flag_value(argv, "--output-format"))
        .or_else(|| flag_value(argv, "--reporter"))
        .or_else(|| flag_value(argv, "-f"))
        .unwrap_or("");
    if has_flag(argv, "--json") || fmt == "json" || fmt.starts_with("json") {
        let (_, compacted, _) = crate::auto::compact(output);
        return compacted;
    }
    if fmt == "junit" || fmt == "junit-xml" {
        // JUnit XML — auto detects it.
        let (_, compacted, _) = crate::auto::compact(output);
        return compacted;
    }
    if fmt == "github" || fmt == "github-actions" {
        // GitHub Actions annotation lines (`::error file=…::msg`).
        // Each is already a single line — just pass through with cap.
        let s = strip_ansi(output);
        let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
        return truncate_middle(&lines.join("\n"), 60, 10);
    }
    // black --diff / ruff format --diff → unified diff output.
    if has_any_flag(argv, &["--diff"]) {
        return crate::cmd::linux::git::diff::dispatch(&[], output);
    }
    // --quiet / --silent → already filtered by linter, just dedup.
    if has_any_flag(argv, &["-q", "--quiet", "--silent"]) {
        let s = strip_ansi(output);
        return dedup_lines(&s);
    }
    // Default file:line:col format.
    //
    // BUG-FIX (2026-05-31): the previous body called `filter::linter`
    // which dispatches BACK to this function with empty argv —
    // infinite recursion → stack overflow. The recursion was latent
    // because production agents typically pass `--json` / `--format=
    // …` / `--quiet`, hitting the early-return arms. Linters added
    // without those flags (e.g. `actionlint .` defaults to file:line
    // format) hit the default and crashed.
    //
    // Fix: do the actual work inline — ANSI-strip and dedup, capped.
    // This matches what a generic compactor would do but stays in
    // the linter-aware code path (so the dashboard sees the right
    // filter label).
    let s = strip_ansi(output);
    let deduped = dedup_lines(&s);
    truncate_middle(&deduped, 80, 20)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn json_flag_routes_to_auto() {
        // `eslint --json` / `ruff --format=json` etc.
        let body = r#"[{"filePath":"src/a.js","messages":[{"ruleId":"no-unused-vars","line":5,"message":"unused"}]}]"#;
        let out = dispatch(&argv(&["eslint", "--json"]), body);
        assert!(out.contains("no-unused-vars") || out.contains("filePath"));
    }

    #[test]
    fn format_json_value_routes_to_auto() {
        // `--format=json`, `--output-format=json`, `--reporter=json`, `-f=json`
        // all hit the auto path. flag_value tries each in turn.
        let body = r#"{"errors":[{"file":"x.py","line":1,"msg":"E501"}]}"#;
        for flag in &["--format", "--output-format", "--reporter", "-f"] {
            let out = dispatch(
                &argv(&["ruff", "check", flag, "json", "src/"]),
                body,
            );
            assert!(out.contains("errors") || out.contains("E501"));
        }
    }

    #[test]
    fn format_json_pretty_starts_with_json_also_routes() {
        // `--format=json-stream` / `json-render-diagnostics` → starts_with("json") true.
        let body = r#"{"file":"x.py","violations":[]}"#;
        let out = dispatch(
            &argv(&["ruff", "check", "--format", "json-stream"]),
            body,
        );
        assert!(!out.is_empty());
    }

    #[test]
    fn format_junit_routes_to_auto() {
        // JUnit XML output.
        let body = "<?xml version=\"1.0\"?>\n\
                    <testsuites><testsuite name=\"lint\">\n  \
                    <testcase name=\"file.py\"><failure>E501</failure></testcase>\n\
                    </testsuite></testsuites>\n";
        for fmt in &["junit", "junit-xml"] {
            let out = dispatch(
                &argv(&["ruff", "check", "--format", fmt]),
                body,
            );
            assert!(out.contains("testsuite") || out.contains("E501"));
        }
    }

    #[test]
    fn format_github_actions_strips_ansi_and_passes_through() {
        // GitHub Actions annotation format: `::error file=…::msg`.
        // Dispatcher strips ANSI, drops blank lines, caps. Each
        // annotation is already a single line — DO NOT auto-detect.
        let body = "\x1b[31m::error file=src/a.js,line=5::Unused variable `x`\x1b[0m\n\
                    \n\
                    ::warning file=src/b.js,line=10::TODO\n\
                    \n\
                    ::error file=src/c.js,line=15::Missing semicolon\n";
        for fmt in &["github", "github-actions"] {
            let out = dispatch(
                &argv(&["eslint", "--format", fmt]),
                body,
            );
            assert!(!out.contains("\x1b["), "{fmt} must strip ANSI");
            // Annotations preserved.
            assert!(out.contains("::error"));
            assert!(out.contains("Unused variable"));
            // Blank lines dropped.
            assert!(!out.contains("\n\n\n"));
        }
    }

    #[test]
    fn diff_flag_routes_to_git_diff() {
        // `black --diff src/` / `ruff format --diff` emit unified diffs.
        let body = "--- /tmp/before.py\n+++ /tmp/after.py\n@@ -1,3 +1,3 @@\n \
                    def foo():\n-    return  1\n+    return 1\n";
        let out = dispatch(&argv(&["black", "--diff", "src/"]), body);
        // git_diff compactor preserves diff structure.
        assert!(out.contains("return") || out.contains("@@"));
    }

    #[test]
    fn quiet_flag_strips_and_dedups() {
        // `--quiet` / `-q` / `--silent`: ANSI strip + dedup.
        let body = "\x1b[31merror\x1b[0m\n\
                    repeated\nrepeated\nrepeated\n\
                    done\n";
        for flag in &["-q", "--quiet", "--silent"] {
            let out = dispatch(&argv(&["eslint", flag, "src/"]), body);
            assert!(!out.contains("\x1b["), "{flag} must strip ANSI");
            assert_eq!(
                out.matches("repeated").count(),
                1,
                "{flag} must dedup; got: {out:?}"
            );
        }
    }

    #[test]
    fn default_does_not_recurse() {
        // #385 — the previous default arm called filter::linter
        // which routed back HERE → infinite recursion → stack
        // overflow. This test would catch a regression that
        // reintroduced the recursive call. Fixture is a typical
        // file:line:col linter output that hits the default arm
        // (no --json, no --format, no --diff, no --quiet).
        let body = "src/a.js:5:3: warning: unused variable `x`\n\
                    src/a.js:5:3: warning: unused variable `x`\n\
                    src/a.js:5:3: warning: unused variable `x`\n\
                    src/b.js:10:1: error: missing semicolon\n\
                    \x1b[33mprose\x1b[0m\n";
        // If this call ever hangs / overflows again, the test would
        // never reach the assertion.
        let out = dispatch(&argv(&["actionlint"]), body);
        // ANSI gone.
        assert!(!out.contains("\x1b["));
        // Repeated line deduped.
        assert_eq!(
            out.matches("src/a.js:5:3").count(),
            1,
            "default must dedup; got: {out:?}"
        );
        // Other content preserved.
        assert!(out.contains("missing semicolon"));
    }
}
