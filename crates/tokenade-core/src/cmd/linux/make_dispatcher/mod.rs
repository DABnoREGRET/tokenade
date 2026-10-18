//! Dispatcher for `make`.
//! Extracted from `filter.rs` in the L108 refactor.

use crate::compress::dedup_lines;
use crate::argv::has_any_flag;
use crate::compress::truncate_middle;
use crate::ansi::strip_ansi;

pub fn dispatch(argv: &[String], output: &str) -> String {
    if has_any_flag(argv, &["-s", "--silent", "--quiet"]) {
        let s = strip_ansi(output);
        return truncate_middle(&dedup_lines(&s), 60, 20);
    }
    if has_any_flag(argv, &["-n", "--just-print", "--dry-run", "--recon"]) {
        let s = strip_ansi(output);
        return truncate_middle(&s, 80, 10);
    }
    // -d / --debug emits many noise lines (`Considering target file …`).
    // Drop those.
    if has_any_flag(argv, &["-d", "--debug"]) || crate::argv::has_flag_prefix(argv, "--debug=") {
        let s = strip_ansi(output);
        let kept: Vec<&str> = s
            .lines()
            .filter(|l| {
                let t = l.trim();
                !t.starts_with("Considering target")
                    && !t.starts_with("Looking for")
                    && !t.starts_with("Reading makefile")
                    && !t.starts_with("Pruning file")
            })
            .collect();
        return truncate_middle(&kept.join("\n"), 100, 30);
    }
    crate::cmd::linux::cargo::build::compact(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn silent_mode_strips_ansi_and_dedups() {
        // -s/--silent/--quiet: ANSI-strip + dedup repeated lines +
        // head/tail truncate (60/20).
        let body = "\x1b[32mbuilt foo\x1b[0m\n\
                    built bar\n\
                    built bar\n\
                    built bar\n\
                    built baz\n";
        for flag in &["-s", "--silent", "--quiet"] {
            let out = dispatch(&argv(&["make", flag, "all"]), body);
            assert!(!out.contains("\x1b["), "{flag} must strip ANSI");
            // Dedup collapses the 3 "built bar" runs into one + count.
            assert!(out.contains("built bar"), "{flag} must preserve content");
            assert!(
                out.matches("built bar").count() == 1,
                "{flag} must dedup repeated lines; got: {out:?}"
            );
        }
    }

    #[test]
    fn dry_run_strips_ansi_but_preserves_repeats() {
        // -n/--just-print/--dry-run: print what WOULD happen — do NOT
        // dedup, because consecutive identical lines may be intentional
        // (running the same rule for multiple targets).
        let body = "\x1b[33mecho a\x1b[0m\n\
                    echo b\n\
                    echo b\n\
                    echo c\n";
        for flag in &["-n", "--just-print", "--dry-run", "--recon"] {
            let out = dispatch(&argv(&["make", flag, "build"]), body);
            assert!(!out.contains("\x1b["), "{flag} must strip ANSI");
            assert_eq!(
                out.matches("echo b").count(),
                2,
                "{flag} must NOT dedup dry-run output; got: {out:?}"
            );
        }
    }

    #[test]
    fn debug_mode_filters_noise_lines() {
        // -d/--debug/--debug=…: drop the chatter that make emits
        // when tracing target consideration. Real signal lines pass.
        let body = "Considering target file 'foo.o'.\n\
                    Looking for an implicit rule for 'foo.o'.\n\
                    Reading makefile 'common.mk'.\n\
                    Pruning file 'bar.h'.\n\
                    gcc -c foo.c -o foo.o\n\
                    foo.c:5: warning: unused variable\n";
        for flag in &["-d", "--debug"] {
            let out = dispatch(&argv(&["make", flag, "all"]), body);
            assert!(
                !out.contains("Considering target")
                    && !out.contains("Looking for")
                    && !out.contains("Reading makefile")
                    && !out.contains("Pruning file"),
                "{flag} must filter make-internal chatter; got: {out:?}"
            );
            assert!(
                out.contains("gcc -c foo.c") && out.contains("warning:"),
                "{flag} must keep real signal; got: {out:?}"
            );
        }
        // `--debug=` prefix form also fires the same filter.
        let out = dispatch(&argv(&["make", "--debug=j", "all"]), body);
        assert!(!out.contains("Considering target"));
        assert!(out.contains("warning:"));
    }

    #[test]
    fn default_routes_to_cargo_build_compactor() {
        // No special flags → falls through to the cargo_build compactor
        // (gcc/clang/cc errors line up close enough to cargo's `error:`
        // shape that the same compactor handles both well).
        let body = "gcc -c foo.c -o foo.o\n\
                    foo.c: In function 'main':\n\
                    foo.c:5:5: error: 'x' undeclared\n\
                       5 |     x = 1;\n\
                         |     ^\n";
        let out = dispatch(&argv(&["make", "all"]), body);
        // The default compactor preserves the error content.
        assert!(out.contains("error") && out.contains("undeclared"));
    }
}
