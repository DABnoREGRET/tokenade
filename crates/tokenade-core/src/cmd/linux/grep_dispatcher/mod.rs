//! Dispatcher for `grep`.
//! Extracted from `filter.rs` in the L108 refactor.

use crate::argv::{has_flag, has_any_flag};

/// The short options of `grep` that take a value: in a cluster, every letter
/// after one of these is that value (`-e`, `-f`, `-m`, `-A`, `-B`, `-C`,
/// `-d`, `-D`).
const GREP_VALUE_LETTERS: &str = "efmABCdD";

pub fn dispatch(argv: &[String], output: &str) -> String {
    let short = |c| crate::argv::has_short_flag_letter(argv, c, GREP_VALUE_LETTERS);
    if short('c') || has_flag(argv, "--count") {
        return crate::cmd::linux::grep_count_fold::compact(output);
    }
    if short('l') || short('L') || has_any_flag(argv, &["--files-with-matches", "--files-without-match"]) {
        return crate::cmd::linux::grep_l::compact(output);
    }
    if has_flag(argv, "--json") {
        let (_, compacted, _) = crate::auto::compact(output);
        return compacted;
    }
    // -A / -B / -C with a value modify shape (groups separated by `--`).
    let has_context = argv.iter().any(|a| {
        matches!(a.as_str(), "-A" | "-B" | "-C")
            || a.starts_with("-A=")
            || a.starts_with("-B=")
            || a.starts_with("-C=")
            || a == "--context"
    });
    if has_context {
        return crate::cmd::linux::grep_context_fold::compact(output);
    }
    let needles = searched_pattern(argv)
        .map(crate::cmd::linux::grep::pattern_needles)
        .unwrap_or_default();
    crate::cmd::linux::grep::compact_for(output, &needles)
}

/// The pattern argument: `-e PAT` / `--regexp=PAT`, else the first operand.
fn searched_pattern(argv: &[String]) -> Option<&str> {
    let mut it = argv.iter().skip(1);
    while let Some(a) = it.next() {
        if a == "-e" || a == "--regexp" {
            return it.next().map(String::as_str);
        }
        if let Some(p) = a.strip_prefix("--regexp=") {
            return Some(p);
        }
        if a == "--" {
            return it.next().map(String::as_str);
        }
        if a.starts_with('-') {
            // Flags that take a separate value.
            if matches!(a.as_str(), "-f" | "-m" | "--include" | "--exclude" | "--exclude-dir" | "-A" | "-B" | "-C") {
                it.next();
            }
            continue;
        }
        return Some(a);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn count_mode_routes_to_grep_count() {
        // `grep -c` / `--count`: `file:N` per line. The grep_c
        // compactor preserves every count line and caps at 80 lines
        // (zero-count files are kept too — grep emits them when run
        // with `-c` over multiple files; the agent can see "0 matches"
        // is also signal).
        let body = "a.rs:5\nb.rs:0\nc.rs:12\n";
        for flag in &["-c", "--count"] {
            let out = dispatch(&argv(&["grep", flag, "foo", "."]), body);
            assert!(out.contains("a.rs:5"), "{flag} must preserve a.rs:5");
            assert!(out.contains("c.rs:12"), "{flag} must preserve c.rs:12");
        }
    }

    #[test]
    fn files_with_matches_routes_to_grep_files() {
        // `-l` / `--files-with-matches`: file paths, one per line.
        let body = "src/foo.rs\nsrc/bar.rs\nsrc/baz.rs\ntests/integration.rs\n";
        for flag in &["-l", "-L", "--files-with-matches", "--files-without-match"] {
            let out = dispatch(&argv(&["grep", flag, "TODO", "src"]), body);
            // grep_files preserves the paths (possibly with stripping).
            assert!(out.contains("foo.rs"), "{flag} must preserve filenames");
        }
    }

    #[test]
    fn json_mode_routes_to_auto() {
        // `grep --json` emits ndjson. The dispatcher hands off to auto
        // so the ndjson compactor takes over.
        let body = r#"{"type":"match","data":{"path":{"text":"foo.rs"},"line_number":5,"lines":{"text":"fn foo"}}}
{"type":"match","data":{"path":{"text":"bar.rs"},"line_number":10,"lines":{"text":"fn bar"}}}
{"type":"match","data":{"path":{"text":"baz.rs"},"line_number":15,"lines":{"text":"fn baz"}}}"#;
        let out = dispatch(&argv(&["grep", "--json", "foo", "."]), body);
        assert!(!out.is_empty(), "JSON dispatch must produce output");
        // ndjson compactor preserves at least some content.
        assert!(out.contains("foo") || out.contains("bar") || out.contains("baz"));
    }

    #[test]
    fn context_flag_routes_to_grep_context() {
        // `-A N` / `-B N` / `-C N` / `--context`: matches separated by
        // `--`. grep_context preserves the group structure.
        let body = "src/a.rs-3-prev line\nsrc/a.rs:5:matched\nsrc/a.rs-7-next line\n--\nsrc/b.rs:10:also matched\n";
        for flag in &["-A", "-B", "-C", "--context"] {
            let out = dispatch(
                &argv(&["grep", flag, "2", "matched", "src"]),
                body,
            );
            assert!(out.contains("matched"), "{flag} context must keep matches");
        }
    }

    #[test]
    fn context_eq_form_routes_to_grep_context() {
        // `-A=N` / `-B=N` / `-C=N` (less common but accepted).
        let body = "src/a.rs:5:hit\n--\nsrc/b.rs:10:hit\n";
        for flag in &["-A=2", "-B=2", "-C=2"] {
            let out = dispatch(&argv(&["grep", flag, "hit", "src"]), body);
            assert!(out.contains("hit"), "{flag} must engage grep_context");
        }
    }

    #[test]
    fn default_routes_to_grep_compactor() {
        // No special flag → standard `grep` output (path:lineno:match).
        let body = "src/a.rs:5:fn alpha()\n\
                    src/b.rs:10:fn beta()\n\
                    src/c.rs:15:fn gamma()\n";
        let out = dispatch(&argv(&["grep", "-rn", "fn", "src"]), body);
        // grep compactor preserves identifiers + line numbers.
        assert!(out.contains("alpha") || out.contains("beta") || out.contains("gamma"));
    }

    /// `grep -rci stash crates`: `-c` inside a CLUSTER did not reach the
    /// count fold — « 2011 match(es) in 2011 file(s) », each at 1.
    #[test]
    fn a_count_flag_in_a_cluster_reaches_the_count_fold() {
        let out: String = (0..300).map(|i| format!("src/f{i}.rs:{}\n", if i % 10 == 0 { 3 } else { 0 })).collect();
        let got = dispatch(&argv(&["grep", "-rci", "stash", "src"]), &out);
        assert!(got.contains("270 file(s) counted 0, omitted"), "{got}");
        // `-ec`: the `c` is the PATTERN that `-e` takes, not `--count`.
        assert!(!crate::argv::has_short_flag_letter(&argv(&["grep", "-ec", "f"]), 'c', GREP_VALUE_LETTERS));
        assert!(crate::argv::has_short_flag_letter(&argv(&["grep", "-rlIiE", "x"]), 'l', GREP_VALUE_LETTERS));
    }
}
