//! Dispatcher for `gradle`.
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
    if has_any_flag(argv, &["-d", "--debug", "-i", "--info"]) {
        let s = strip_ansi(output);
        let kept: Vec<&str> = s
            .lines()
            .filter(|l| {
                let t = l.trim();
                // Drop "no-op" status suffixes — UP-TO-DATE / NO-SOURCE
                // / SKIPPED / FROM-CACHE all mean "no work happened on
                // this task" and add no signal.
                let no_op = t.ends_with("UP-TO-DATE")
                    || t.ends_with("NO-SOURCE")
                    || t.ends_with("SKIPPED")
                    || t.ends_with("FROM-CACHE")
                    || t.ends_with("UP-TO-DATE.");
                if no_op {
                    return false;
                }
                !t.starts_with("Task")
                    && !t.starts_with("Caching disabled")
                    && !t.starts_with("> Configure project")
                    && (t.contains("FAILED")
                        || t.contains("error")
                        || t.contains("warning")
                        || t.starts_with("BUILD ")
                        || t.starts_with("> Task ")
                        || t.starts_with("FAILURE"))
            })
            .collect();
        if kept.is_empty() {
            return generic(output);
        }
        return truncate_middle(&kept.join("\n"), 80, 20);
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
    fn quiet_mode_strips_ansi_and_dedups() {
        let body = "\x1b[32mok\x1b[0m\n\
                    repeated\nrepeated\nrepeated\n\
                    done\n";
        for flag in &["-q", "--quiet"] {
            let out = dispatch(&argv(&["gradle", flag, "build"]), body);
            assert!(!out.contains("\x1b["), "{flag} must strip ANSI");
            assert_eq!(
                out.matches("repeated").count(),
                1,
                "{flag} must dedup; got: {out:?}"
            );
        }
    }

    #[test]
    fn debug_keeps_signal_drops_no_ops_and_chatter() {
        // Realistic gradle output mixing no-op task suffixes (UP-TO-DATE,
        // NO-SOURCE, SKIPPED, FROM-CACHE) with chatter (`Task`, `Caching
        // disabled`, `> Configure project`) and signal (`> Task `,
        // `BUILD `, `FAILED`, `error`, `warning`, `FAILURE`).
        let body = "> Task :compileJava UP-TO-DATE\n\
                    > Task :test NO-SOURCE\n\
                    > Task :assemble SKIPPED\n\
                    > Task :build FROM-CACHE\n\
                    > Task :api:compile\n\
                    Task :foo took 5s\n\
                    Caching disabled for task ':bar'\n\
                    > Configure project :app\n\
                    > Task :app:compile FAILED\n\
                    error: cannot find symbol\n\
                    warning: deprecated\n\
                    BUILD SUCCESSFUL in 30s\n\
                    FAILURE: Build failed\n";
        for flag in &["-d", "--debug", "-i", "--info"] {
            let out = dispatch(&argv(&["gradle", flag, "build"]), body);
            // No-op suffixes dropped.
            assert!(!out.contains("UP-TO-DATE"), "{flag} must drop UP-TO-DATE");
            assert!(!out.contains("NO-SOURCE"), "{flag} must drop NO-SOURCE");
            assert!(!out.contains("SKIPPED"), "{flag} must drop SKIPPED");
            assert!(!out.contains("FROM-CACHE"), "{flag} must drop FROM-CACHE");
            // Chatter dropped.
            assert!(!out.contains("Task :foo"), "{flag} must drop `Task `-prefixed chatter");
            assert!(!out.contains("Caching disabled"), "{flag} must drop Caching disabled");
            assert!(!out.contains("Configure project"), "{flag} must drop Configure project");
            // Signal kept.
            assert!(out.contains("FAILED"), "{flag} must keep FAILED");
            assert!(out.contains("error"), "{flag} must keep error");
            assert!(out.contains("warning"), "{flag} must keep warning");
            assert!(out.contains("BUILD SUCCESSFUL"), "{flag} must keep BUILD");
            assert!(out.contains("FAILURE:"), "{flag} must keep FAILURE");
        }
    }

    #[test]
    fn debug_empty_signal_falls_back_to_generic() {
        // -d engaged but no signal markers → generic fallback so the
        // agent still sees something.
        let body = "> Task :compileJava UP-TO-DATE\n\
                    > Task :test NO-SOURCE\n\
                    > Task :assemble SKIPPED\n\
                    Caching disabled for ':bar'\n";
        let out = dispatch(&argv(&["gradle", "-d", "build"]), body);
        assert!(!out.is_empty(), "generic fallback must not be empty");
    }

    #[test]
    fn default_routes_to_cargo_build() {
        // No flag → cargo_build compactor handles gcc-like errors well.
        let body = "> Task :compileJava\n\
                    /src/Main.java:42: error: cannot find symbol\n\
                       String x = bar();\n\
                                 ^\n";
        let out = dispatch(&argv(&["gradle", "build"]), body);
        assert!(out.contains("error") && out.contains("cannot find symbol"));
    }
}
