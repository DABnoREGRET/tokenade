//! Dispatcher for `ls`.
//! Extracted from `filter.rs` in the L108 refactor.

use crate::argv::has_any_flag;

/// Is `letter` present as a short flag, alone or inside a cluster?
///
/// One helper for every letter this dispatcher reads, because it used to be
/// two rules: `l` was looked for INSIDE a cluster (`-la`, `-lah`) while `R`
/// was matched exactly, so `ls -lR` never reached the recursive compactor and
/// fell through to the long-listing one instead. Measured 2026-09-04 on
/// `ls -lR crates`, 138 623 bytes: **74 817 delivered as `-lR`, 11 590 as
/// `-l -R`** — the same command, the same bytes, folded 6.5× worse because
/// the letters were written together.
///
/// Case-sensitive on purpose: `ls -r` is `--reverse` and `ls -R` is
/// `--recursive`, and a case-blind cluster scan would route one as the other.
/// `--` prefixes are excluded so a long flag's spelling (`--color`) never
/// donates a letter.
fn has_short_flag_letter(argv: &[String], letter: char) -> bool {
    crate::argv::has_short_flag_letter(argv, letter, "")
}

pub fn dispatch(argv: &[String], output: &str) -> String {
    let long = has_short_flag_letter(argv, 'l');
    let recursive = has_short_flag_letter(argv, 'R') || has_any_flag(argv, &["--recursive"]);
    if recursive {
        return crate::cmd::linux::ls_R::compact(output);
    }
    if long {
        return crate::cmd::linux::ls_long::compact(output);
    }
    crate::cmd::linux::ls::compact(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `ls -R` body with enough repetition that the recursive fold has
    /// something to do. Shared by the routing tests: a fixture too small to
    /// fold makes every route look identical, so the equality they assert
    /// holds with the bug in place.
    fn recursive_listing() -> String {
        let mut body = String::from(".:\n");
        for i in 0..60 {
            body.push_str(&format!("dir{i:03}\n"));
        }
        for i in 0..60 {
            body.push_str(&format!("\n./dir{i:03}:\n"));
            for j in 0..12 {
                body.push_str(&format!("file{j:03}.rs\n"));
            }
        }
        body
    }

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn recursive_flag_routes_to_ls_recursive() {
        // `ls -R` / `ls --recursive`: per-directory headers + entries.
        let body = ".:\n\
                    src  tests  README.md\n\
                    \n\
                    ./src:\n\
                    main.rs  lib.rs\n\
                    \n\
                    ./tests:\n\
                    integration.rs\n";
        for flag in &["-R", "--recursive"] {
            let out = dispatch(&argv(&["ls", flag]), body);
            assert!(!out.is_empty());
            assert!(out.contains("src") || out.contains("main.rs"));
        }
    }

    #[test]
    fn long_flag_routes_to_ls_long() {
        // `ls -l` / `ls -la` / `ls -lah`: long format rows.
        let body = "total 32\n\
                    drwxr-xr-x 2 user user 4096 Jan  1 12:00 src\n\
                    drwxr-xr-x 2 user user 4096 Jan  1 12:00 tests\n\
                    -rw-r--r-- 1 user user 1234 Jan  1 12:00 README.md\n";
        for flag in &["-l", "-la", "-lah", "-Al"] {
            let out = dispatch(&argv(&["ls", flag]), body);
            assert!(!out.is_empty());
            assert!(out.contains("README") || out.contains("src"));
        }
    }

    /// Every spelling of the same command reaches the same compactor.
    ///
    /// `l` was read inside a cluster and `R` was not, so `ls -lR` — one of the
    /// commonest listings an agent runs — took the long-listing path and the
    /// per-directory fold never ran on it.
    #[test]
    fn a_bundled_R_is_still_recursive() {
        let body = &recursive_listing();
        let reference = dispatch(&argv(&["ls", "-R"]), body);
        assert!(
            reference.len() < body.len(),
            "the fixture must fold on the recursive route"
        );
        for spelling in [
            vec!["ls", "-lR"],
            vec!["ls", "-Rl"],
            vec!["ls", "-aR"],
            vec!["ls", "-Rv"],
            vec!["ls", "-l", "-R"],
            vec!["ls", "--recursive"],
        ] {
            let a: Vec<String> = spelling.iter().map(|s| s.to_string()).collect();
            assert_eq!(
                dispatch(&a, body),
                reference,
                "{spelling:?} must fold like `ls -R`"
            );
        }
    }

    /// The letter is read case-sensitively, and a long flag donates none.
    ///
    /// `-r` is `--reverse` and `-R` is `--recursive`: a case-blind scan routes
    /// one as the other. `--color=auto` carries an `l` and must not make the
    /// listing long. The body is large on purpose — every compactor here
    /// passes a short listing through unchanged, so a small fixture makes all
    /// four routes look identical and asserts nothing.
    #[test]
    fn a_lowercase_r_is_not_recursive_and_a_long_flag_donates_no_letter() {
        let body = recursive_listing();
        let recursive = dispatch(&argv(&["ls", "-R"]), &body);
        assert!(
            recursive.len() < body.len(),
            "the fixture must actually fold on the recursive route"
        );
        assert_ne!(
            dispatch(&argv(&["ls", "-r"]), &body),
            recursive,
            "`ls -r` is --reverse"
        );

        // The long-flag half needs a long-listing body: on the recursive shape
        // above, `ls` and `ls -l` fold the same and the comparison asserts
        // nothing.
        let mut rows = String::from("total 480\n");
        for i in 0..80 {
            rows.push_str(&format!(
                "-rw-r--r-- 1 u u {:5} Jan  1 12:00 file{i:03}.rs\n",
                1000 + i
            ));
        }
        let long = dispatch(&argv(&["ls", "-l"]), &rows);
        let plain = dispatch(&argv(&["ls"]), &rows);
        assert_ne!(long, plain, "anti-vacuity: -l must change the route");
        assert_eq!(
            dispatch(&argv(&["ls", "--color=auto"]), &rows),
            plain,
            "`--color` carries an l and must not select the long listing"
        );
    }

    #[test]
    fn long_flag_combined_short_flags_detect_l() {
        // `-lh` should route to ls_long even though `l` is not alone.
        // The detection: `-lh` is short flag containing `l` (#594 test).
        let body = "total 8\n\
                    -rw-r--r-- 1 u u 4.0K Jan  1 file\n";
        let out = dispatch(&argv(&["ls", "-lh"]), body);
        assert!(out.contains("file"));
    }

    #[test]
    fn default_routes_to_ls() {
        // No flag → plain `ls`: column-formatted names.
        let body = "Cargo.toml  README.md  src  tests\n";
        let out = dispatch(&argv(&["ls"]), body);
        assert!(out.contains("Cargo.toml") || out.contains("README"));
    }

    #[test]
    fn long_does_not_match_double_dash_flags() {
        // Guard: `--reverse` contains `l` (in `reverse`) but starts
        // with `--` — must NOT trigger the long-format path.
        let body = "Cargo.toml  README.md  src  tests\n";
        let out = dispatch(&argv(&["ls", "--reverse"]), body);
        // Routed to the plain `ls` compactor, not ls_long.
        assert!(out.contains("Cargo.toml") || out.contains("README"));
    }
}
