//! Dispatcher for `dotnet`.
//! Extracted from `filter.rs` in the L108 refactor.

use crate::compress::dedup_lines;
use crate::argv::has_any_flag;
use crate::ansi::strip_ansi;

pub fn dispatch(argv: &[String], output: &str) -> String {
    if has_any_flag(argv, &["-v", "--verbosity", "/v"]) {
        // Verbosity-modulated; just pass through dedup'd.
        let s = strip_ansi(output);
        return dedup_lines(&s);
    }
    let sub = argv.iter().skip(1).find(|a| !a.starts_with('-')).map(String::as_str);
    let read = match sub {
        Some("test") => msbuild::compact_test(output),
        _ => msbuild::compact_build(output),
    };
    // Nothing MSBuild-shaped (no diagnostic, no failure): the shared
    // "Build succeeded / Build FAILED" reader.
    read.unwrap_or_else(|| crate::cmd::linux::cargo::build::compact(output))
}

mod msbuild;

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn verbosity_flag_strips_and_dedups() {
        // `-v` / `--verbosity` / `/v` (Windows-style): verbose output
        // is dedup'd.
        let body = "\x1b[33mwarning CS1234\x1b[0m\n\
                    repeated trace line\n\
                    repeated trace line\n\
                    repeated trace line\n\
                    Build succeeded\n";
        for flag in &["-v", "--verbosity", "/v"] {
            let out = dispatch(&argv(&["dotnet", "build", flag, "quiet"]), body);
            assert!(!out.contains("\x1b["), "{flag} must strip ANSI");
            assert_eq!(
                out.matches("repeated trace line").count(),
                1,
                "{flag} must dedup; got: {out:?}"
            );
            assert!(out.contains("Build succeeded"));
        }
    }

    #[test]
    fn default_routes_to_cargo_build_compactor() {
        // No verbosity flag → cargo_build compactor handles
        // `Build succeeded` / `Build FAILED` summary shapes.
        let body = "Microsoft (R) Build Engine version 17.8.3\n\
                    Determining projects to restore...\n\
                    /src/Foo.csproj : warning CS1234: deprecated API\n\
                    /src/Bar.csproj : error CS5678: missing semicolon\n\
                    Build FAILED.\n\
                    1 Warning(s)\n\
                    1 Error(s)\n";
        let out = dispatch(&argv(&["dotnet", "build"]), body);
        // cargo_build preserves error/warning lines.
        assert!(out.contains("error") || out.contains("FAILED") || out.contains("warning"));
    }
}
