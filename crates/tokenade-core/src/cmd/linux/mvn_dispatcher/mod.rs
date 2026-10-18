//! Dispatcher for `mvn`.
//! Extracted from `filter.rs` in the L108 refactor.

use crate::compress::generic;
use crate::compress::dedup_lines;
use crate::argv::has_any_flag;
use crate::compress::truncate_middle;
use crate::ansi::strip_ansi;

pub fn dispatch(argv: &[String], output: &str) -> String {
    if has_any_flag(argv, &["-q", "--quiet"]) {
        let s = strip_ansi(output);
        return dedup_lines(&s);
    }
    // For default and verbose modes, keep only lines that signal real
    // progress: phase headers, errors, warnings, BUILD SUCCESS/FAILURE,
    // dependency resolution problems.
    let s = strip_ansi(output);
    let kept: Vec<&str> = s
        .lines()
        .filter(|l| {
            let t = l.trim();
            if t.is_empty() {
                return false;
            }
            t.contains("BUILD ")
                || t.contains("ERROR")
                || t.contains("WARNING")
                || t.contains("FAILED")
                || t.starts_with("[INFO] --- ")     // phase header
                || t.starts_with("[INFO] Building ")
                || t.starts_with("[INFO] Reactor Summary")
                || t.starts_with("[INFO] BUILD")
                || t.contains("Tests run:")
                || t.starts_with("Tests in error")
                || t.starts_with("Failed tests")
        })
        .collect();
    if kept.is_empty() {
        return generic(output);
    }
    truncate_middle(&kept.join("\n"), 80, 20)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn quiet_mode_strips_ansi_and_dedups() {
        let body = "\x1b[32mok\x1b[0m\n\
                    repeated\nrepeated\nrepeated\n\
                    done\n";
        for flag in &["-q", "--quiet"] {
            let out = dispatch(&argv(&["mvn", flag, "package"]), body);
            assert!(!out.contains("\x1b["), "{flag} must strip ANSI");
            assert_eq!(
                out.matches("repeated").count(),
                1,
                "{flag} must dedup repeated lines; got: {out:?}"
            );
            assert!(out.contains("ok") && out.contains("done"));
        }
    }

    #[test]
    fn default_keeps_phase_and_build_lines_drops_chatter() {
        // Realistic mvn output mixes chatter (Downloading, Downloaded,
        // [INFO] --- generic plugin runs) with signal (BUILD SUCCESS,
        // ERROR, phase headers, test counts).
        let body = "[INFO] Scanning for projects...\n\
                    [INFO] Downloading from central: https://repo.maven.apache.org/x.pom\n\
                    [INFO] Downloaded from central: https://repo.maven.apache.org/x.pom (5 KB)\n\
                    [INFO] -------------------< com.foo:bar >--------------------\n\
                    [INFO] Building bar 1.0.0\n\
                    [INFO] --- maven-compiler-plugin:3.10.1:compile (default-compile) ---\n\
                    [INFO] Compiling 50 source files\n\
                    [WARNING] /src/Main.java:42: deprecated API\n\
                    [ERROR] /src/Other.java:10: cannot find symbol\n\
                    [INFO] Tests run: 12, Failures: 0, Errors: 0, Skipped: 1\n\
                    [INFO] BUILD SUCCESS\n\
                    [INFO] Total time: 5.234 s\n";
        let out = dispatch(&argv(&["mvn", "package"]), body);
        // Signal lines must survive.
        assert!(out.contains("Building bar"));
        assert!(out.contains("maven-compiler-plugin"));
        assert!(out.contains("WARNING"));
        assert!(out.contains("ERROR"));
        assert!(out.contains("Tests run:"));
        assert!(out.contains("BUILD SUCCESS"));
        // Chatter must be dropped.
        assert!(!out.contains("Scanning for projects"));
        assert!(!out.contains("Downloading from"));
        assert!(!out.contains("Downloaded from"));
        assert!(!out.contains("Total time"));
    }

    #[test]
    fn default_keeps_test_failure_signal() {
        // Surefire-style failure output: `Tests in error: …` and
        // `Failed tests: …` are critical to keep visible.
        let body = "[INFO] Building x 2.0\n\
                    [ERROR] Failures:\n\
                    Failed tests:   testA(MyClass): expected X, got Y\n\
                    Tests in error: testB(MyClass): NullPointerException\n\
                    [INFO] BUILD FAILURE\n";
        let out = dispatch(&argv(&["mvn", "test"]), body);
        assert!(out.contains("Failed tests:"));
        assert!(out.contains("Tests in error:"));
        assert!(out.contains("BUILD FAILURE"));
    }

    #[test]
    fn empty_signal_falls_back_to_generic() {
        // Output with NO signal lines (just chatter) → fall through
        // to the generic compactor so we don't drop everything.
        let body = "Downloading dep1\n\
                    Downloading dep2\n\
                    Downloading dep3\n\
                    Resolved dep1\n\
                    Resolved dep2\n\
                    Resolved dep3\n";
        let out = dispatch(&argv(&["mvn", "install"]), body);
        // generic preserves at least some of the input.
        assert!(!out.is_empty(), "generic fallback must not be empty");
        assert!(
            out.contains("Downloading") || out.contains("Resolved"),
            "generic must preserve recognizable content; got: {out}"
        );
    }
}
