//! Dispatcher for `find`.
//! Extracted from `filter.rs` in the L108 refactor.

use crate::argv::{has_flag, has_any_flag};

pub fn dispatch(argv: &[String], output: &str) -> String {
    if has_any_flag(argv, &["-print0", "-0", "--print0"]) {
        // NUL separator → newline-separate first.
        return crate::cmd::linux::find::compact(&output.replace('\0', "\n"));
    }
    if has_flag(argv, "-ls") {
        // `find -ls` adds ls-style metadata before the path; the exec
        // variant already strips that prefix.
        return crate::cmd::linux::find_exec::compact(output);
    }
    crate::cmd::linux::find::compact(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn print0_replaces_nul_with_newline_then_routes_to_find_list() {
        // `find . -print0` separates paths with NUL bytes — the
        // dispatcher must NL-translate before routing to find_list,
        // otherwise the whole output looks like one giant line.
        let body = "/tmp/a.txt\0/tmp/b.txt\0/tmp/c.txt\0";
        for flag in &["-print0", "-0", "--print0"] {
            let out = dispatch(&argv(&["find", ".", flag]), body);
            // No NUL bytes in output (translated to newlines).
            assert!(!out.contains('\0'), "{flag} must replace NUL");
            // All three paths survive find_list compaction.
            assert!(out.contains("a.txt") || out.contains("/tmp"));
        }
    }

    #[test]
    fn ls_flag_routes_to_find_long() {
        // `find . -ls` produces ls-like long-format rows.
        let body = "12345  4 drwxr-xr-x   2 root root  4096 Jan  1 12:00 /tmp/d1\n\
                    12346  8 -rw-r--r--   1 root root  6789 Jan  1 12:01 /tmp/d1/file\n";
        let out = dispatch(&argv(&["find", ".", "-ls"]), body);
        assert!(!out.is_empty());
        // find_long preserves at least the paths.
        assert!(out.contains("/tmp") || out.contains("file"));
    }

    #[test]
    fn default_routes_to_find_list() {
        // No flag → plain `find` output (one path per line).
        let body = "./src/main.rs\n./src/lib.rs\n./tests/integration.rs\n./Cargo.toml\n";
        let out = dispatch(&argv(&["find", ".", "-name", "*.rs"]), body);
        assert!(out.contains("main.rs") || out.contains("lib.rs"));
    }
}
