//! Tests for `hookwrap`, split out of the flat `tests.rs`.

use super::*;

#[test]
fn unwrap_tokenade_wrap_handles_bare_prog() {
    assert_eq!(
        unwrap_tokenade_wrap("tokenade wrap 'grep -r foo .'"),
        Some("grep -r foo .")
    );
}

#[test]
fn unwrap_tokenade_wrap_handles_full_path() {
    assert_eq!(
        unwrap_tokenade_wrap("/home/u/.local/bin/tokenade wrap 'cargo build'"),
        Some("cargo build")
    );
}

#[test]
fn unwrap_tokenade_wrap_rejects_non_wrap_subcommand() {
    assert_eq!(unwrap_tokenade_wrap("tokenade dashboard"), None);
    assert_eq!(unwrap_tokenade_wrap("tokenade raw cat foo"), None);
}

#[test]
fn unwrap_tokenade_wrap_rejects_non_tokenade_prog() {
    assert_eq!(unwrap_tokenade_wrap("cargo wrap 'foo'"), None);
    assert_eq!(unwrap_tokenade_wrap("rg 'tokenade wrap ' ."), None);
}

#[test]
fn unwrap_tokenade_wrap_rejects_double_quoted_inner() {
    // posix_quote always emits single-quoted form; if we see
    // double quotes the cmd was hand-typed — don't try to parse.
    assert_eq!(unwrap_tokenade_wrap("tokenade wrap \"foo bar\""), None);
}

#[test]
fn search_hint_fires_on_zero_matches() {
    // Each of grep/rg/ug/ag/ack/find/fd/locate with empty output
    // triggers the nudge (path-qualified forms also work).
    for cmd in [
        "grep foo bar.txt",
        "egrep '^x' f",
        "fgrep -i foo .",
        "zgrep pat *.gz",
        "rg pattern src/",
        "ripgrep pattern src/",
        "ugrep 'x' .",
        "ug 'x' .",
        "ag pattern .",
        "ack pattern .",
        "ack-grep pattern .",
        "find . -name '*.foo'",
        "fd '*.rs'",
        "fdfind '*.rs'",
        "locate 'libfoo'",
        "mlocate 'libfoo'",
        "plocate 'libfoo'",
        "git grep nothing",
        "/usr/bin/rg pat .",
    ] {
        let h = super::search_suggestion_hint(cmd, "");
        assert!(h.is_some(), "expected hint for: {cmd}");
        assert!(h.unwrap().contains("0 matches"));
    }
}

#[test]
fn search_hint_pgrep_does_not_nudge_to_semantic() {
    // pgrep searches processes — semantic_search makes no sense.
    // Hint should be None whether matches=0 or matches=many.
    assert!(super::search_suggestion_hint("pgrep firefox", "").is_none());
    assert!(super::search_suggestion_hint(
        "pgrep -a chrome",
        &(1..=200).map(|i| format!("{i} proc\n")).collect::<String>()
    )
    .is_none());
}

#[test]
fn search_hint_fires_when_too_many_matches() {
    let many = (0..120)
        .map(|i| format!("src/f{i}.rs:1:hit\n"))
        .collect::<String>();
    let h = super::search_suggestion_hint("rg pattern src/", &many);
    assert!(h.is_some());
    let s = h.unwrap();
    assert!(s.contains("120 matches"));
    assert!(s.contains("too broad"));
}

#[test]
fn search_hint_silent_in_sweet_spot() {
    // 1-50 matches → no nudge.
    let some = "src/a.rs:1:foo\nsrc/b.rs:2:foo\n";
    assert!(super::search_suggestion_hint("grep foo src/", some).is_none());
}

#[test]
fn search_hint_skips_counter_and_list_flags() {
    // `-c`/`-l` outputs are file counts / paths, not match lines.
    assert!(super::search_suggestion_hint("grep -c foo src/a.rs", "").is_none());
    assert!(super::search_suggestion_hint("grep -l foo src/", "").is_none());
    assert!(super::search_suggestion_hint("rg --files-with-matches foo .", "").is_none());
    assert!(super::search_suggestion_hint("grep -q foo .", "").is_none());
}

#[test]
fn search_hint_skips_non_search_commands() {
    // Only grep/rg/ug/find/git-grep qualify.
    assert!(super::search_suggestion_hint("cargo build", "").is_none());
    assert!(super::search_suggestion_hint("ls -la", "").is_none());
    // `git status` ≠ `git grep`.
    assert!(super::search_suggestion_hint("git status", "").is_none());
}

#[test]
fn search_hint_tolerates_env_prefix() {
    // `LC_ALL=C grep pattern .` should still be recognised.
    let h = super::search_suggestion_hint("LC_ALL=C grep foo .", "");
    assert!(h.is_some());
}

// P2b — bypass argv extraction for the lost-savings ledger.
#[test]
fn bypass_inner_argv_extracts_env_and_raw_forms() {
    assert_eq!(
        super::bypass_inner_argv("TOKENADE_HOOK_DISABLED=1 git log --stat"),
        Some(vec!["git".into(), "log".into(), "--stat".into()])
    );
    assert_eq!(
        super::bypass_inner_argv("STS_HOOK_DISABLED=true cargo test"),
        Some(vec!["cargo".into(), "test".into()])
    );
    assert_eq!(
        super::bypass_inner_argv("tokenade raw git diff"),
        Some(vec!["git".into(), "diff".into()])
    );
    assert_eq!(
        super::bypass_inner_argv("/usr/local/bin/tokenade noproxy kubectl get pods"),
        Some(vec!["kubectl".into(), "get".into(), "pods".into()])
    );
    // combined: env prefix + tokenade raw → inner argv, not "tokenade"
    assert_eq!(
        super::bypass_inner_argv("TOKENADE_HOOK_DISABLED=1 tokenade raw git diff"),
        Some(vec!["git".into(), "diff".into()])
    );
    // non-bypass forms
    assert_eq!(super::bypass_inner_argv("git log --stat"), None);
    assert_eq!(
        super::bypass_inner_argv("TOKENADE_HOOK_DISABLED=0 git log"),
        None
    );
    assert_eq!(
        super::bypass_inner_argv("tokenade raw"),
        None,
        "no inner command"
    );
    assert_eq!(super::bypass_inner_argv("tokenade wrap 'git log'"), None);
}

// #691 — read_hint_category_for routes the PostToolUse Read
// hint. Code → skeleton suggestion; TextData → auto_compact /
// semantic_search suggestion; None → no hint at all.
#[test]
fn read_hint_category_classifies_code() {
    use super::ReadHintCategory;
    for path in [
        "/abs/foo.rs",
        "src/lib.rs",
        "main.py",
        "app.ts",
        "app.tsx",
        "x.go",
        "Main.java",
        "x.c",
        "x.cpp",
        "x.h",
        "x.hpp",
        "x.rb",
        "x.php",
        "x.swift",
        "x.kt",
        "x.scala",
        "x.zig",
        // #707 — shell / SQL / Terraform / proto were silently
        // dropping out of the hint with no nudge to skeleton.
        "install.sh",
        "setup.bash",
        "x.zsh",
        "x.fish",
        "deploy.ps1",
        "build.bat",
        "migrations/001.sql",
        "schema.proto",
        "infra/main.tf",
        "vars.tfvars",
        "providers.hcl",
    ] {
        assert_eq!(
            super::read_hint_category_for(path),
            ReadHintCategory::Code,
            "{path} should be classified Code"
        );
    }
}

#[test]
fn read_hint_category_classifies_text_data() {
    use super::ReadHintCategory;
    // Plain text and prose-shaped files: no AST → no skeleton,
    // but auto_compact handles repetition / log runs / markdown.
    for path in [
        "/abs/notes.txt",
        "logs/server.log",
        "README.md",
        "doc.markdown",
        "page.mdx",
        "spec.rst",
        "notes.org",
        "manual.adoc",
        "paper.tex",
        // #707 — html_to_markdown via auto_compact + diff/patch
        // line-oriented compaction were never advertised.
        "page.html",
        "index.htm",
        "feature.diff",
        "fix.patch",
    ] {
        assert_eq!(
            super::read_hint_category_for(path),
            ReadHintCategory::TextData,
            "{path} should be classified TextData"
        );
    }
}

#[test]
fn read_hint_category_classifies_structured_data() {
    use super::ReadHintCategory;
    // #692 — JSON/YAML/CSV/lock/sourcemap/ndjson DO have dedicated
    // compactor routes via auto_compact. Pre-#692 these were in
    // None, so an agent reading a 30K-token kubectl-yaml or gh-pr
    // JSON got no nudge toward the format-specific compactor that
    // would have saved 50-95%.
    for path in [
        "config.json",
        "schema.yaml",
        "values.yml",
        "data.csv",
        "data.tsv",
        "Cargo.lock",
        "yarn.lock",
        "package-lock.json", // .json wins via ends_with
        "bundle.js.map",
        "stream.ndjson",
        "events.jsonl",
        // #707 — toml_compact / ini_compact / xml_compact /
        // notebook_compact all exist + are wired through
        // `kind_from_path_hint`; the hint just didn't advertise them.
        "Cargo.toml",
        "pyproject.toml",
        "app.ini",
        "redis.conf",
        "service.cfg",
        "java.properties",
        "build/manifest.xml",
        "doc.xsd",
        "transform.xsl",
        "shape.svg",
        "notebook.ipynb",
    ] {
        assert_eq!(
            super::read_hint_category_for(path),
            ReadHintCategory::StructuredData,
            "{path} should be classified StructuredData"
        );
    }
}

/// #1007 — lock-in: the hint body advertises
/// `TOKENADE_NO_SEARCH_HINT=1` as the silencer. Pre-fix the env var
/// was never read so the documented opt-out was fictional. The
/// regression target is the WIRE between hint text and env check —
/// a future "cleanup" deleting the new entry-gate would silently
/// restore the bug, so we assert both halves of the contract here.
#[test]
fn search_suggestion_hint_honours_documented_opt_out_env() {
    // #297 — THE guard, not a private mutex. Three modules had one each, and a
    // lock only serialises the callers that take the same one.
    let mut env = tokenade_core::platform::ScopedEnv::new();
    env.unset("TOKENADE_NO_SEARCH_HINT");
    // 0-match grep → hint normally fires.
    let baseline = super::search_suggestion_hint("grep needle haystack", "");
    assert!(
        baseline
            .as_deref()
            .is_some_and(|s| s.contains("[tokenade:search-hint]")),
        "baseline hint must fire when env is unset; got {baseline:?}"
    );
    // Documented opt-out must SILENCE.
    env.set("TOKENADE_NO_SEARCH_HINT", "1");
    assert!(
        super::search_suggestion_hint("grep needle haystack", "").is_none(),
        "TOKENADE_NO_SEARCH_HINT=1 must silence the hint"
    );
    // is_truthy_one vocabulary applies here too (mirror #992/#1005/#1006).
    env.set("TOKENADE_NO_SEARCH_HINT", "yes");
    assert!(
        super::search_suggestion_hint("grep needle haystack", "").is_none(),
        "=yes must silence (matched truthy vocabulary)"
    );
    // `env` restores what it found on drop; the test no longer has to.
}

#[test]
fn read_hint_category_returns_none_for_files_with_no_compactor() {
    use super::ReadHintCategory;
    // Files that lack BOTH a skeleton route (no AST) and an
    // auto_compact route (binary / opaque / handled elsewhere).
    // Emitting a hint would mis-direct.
    // #707 — Cargo.toml/pyproject.toml moved out of None into
    // StructuredData (toml_compact wires up via kind_from_path_hint).
    for path in [
        "binary.bin",
        "image.png",
        "photo.jpg",
        "video.mp4",
        "archive.tar",
        "archive.zip",
        "noext",
        "dir/", // pathological inputs
    ] {
        assert_eq!(
            super::read_hint_category_for(path),
            ReadHintCategory::None,
            "{path} should NOT trigger a hint"
        );
    }
}

#[test]
fn read_hint_category_is_case_insensitive() {
    use super::ReadHintCategory;
    // Some filesystems / sloppy callers send mixed case.
    assert_eq!(
        super::read_hint_category_for("FOO.RS"),
        ReadHintCategory::Code
    );
    assert_eq!(
        super::read_hint_category_for("README.MD"),
        ReadHintCategory::TextData
    );
    assert_eq!(
        super::read_hint_category_for("NOTES.TXT"),
        ReadHintCategory::TextData
    );
}

#[cfg(test)]
mod one_rule_for_booking_a_fold {
    use crate::hookwrap::fold_is_worth_booking;
    use tokenade_core::Savings;

    /// The clause that had drifted. The delivery path carried it and the
    /// lost-savings path — which books into the SAME ledger — did not, so a
    /// payload a first authority had already folded could be booked twice.
    #[test]
    fn an_already_folded_payload_is_never_booked() {
        let generous = Savings::new(1000, 100);
        assert!(fold_is_worth_booking(&generous, false));
        assert!(!fold_is_worth_booking(&generous, true));
    }

    /// The three numbers live in one place; each is load-bearing.
    #[test]
    fn each_threshold_is_load_bearing() {
        // Must SHRINK.
        assert!(!fold_is_worth_booking(&Savings::new(100, 100), false));
        assert!(!fold_is_worth_booking(&Savings::new(100, 120), false));
        // At least 5 %.
        assert!(!fold_is_worth_booking(&Savings::new(1000, 970), false));
        assert!(fold_is_worth_booking(&Savings::new(1000, 940), false));
        // At least 10 tokens absolutely, however good the ratio.
        assert!(!fold_is_worth_booking(&Savings::new(20, 12), false));
    }
}

/// The diff floor, in the terms the three read paths use it.
///
/// Measured 2026-09-03 on a real source file over 200-, 500- and 2000-line
/// windows: the win ratio is steeply bimodal (96, 88, 79, 54 … 15, 1, 0), but
/// the 20-39 band IS reachable — 36-38% when 11-12% of lines change. That band
/// is the whole disagreement between the paths, so it is what this pins.
#[test]
fn a_diff_is_served_only_when_it_is_much_smaller() {
    use crate::hooks::{diff_is_worth_serving, win_pct, MIN_DIFF_WIN_PCT};

    // Well above: an edit touching a few lines of a large file.
    assert!(diff_is_worth_serving(1000, 100));
    // Well below: a pervasive edit, where the diff is nearly the file again.
    assert!(!diff_is_worth_serving(1000, 950));

    // The band the paths disagreed over. A 25% win was served by the
    // paginated path and passed through by the other two.
    assert_eq!(win_pct(1000, 750), 25);
    assert!(
        !diff_is_worth_serving(1000, 750),
        "a diff saving 25% is served again; the paginated path's old floor is back"
    );
    assert!(
        !diff_is_worth_serving(1000, 610),
        "a diff saving 39% is served; the floor is below MIN_DIFF_WIN_PCT"
    );

    // Exactly at the floor is served: the constant is a floor, not a strict
    // threshold, and the two paths that already used 40 used `>=`.
    assert_eq!(win_pct(1000, 600), MIN_DIFF_WIN_PCT);
    assert!(diff_is_worth_serving(1000, 600));

    // Degenerate inputs are a 0% win, never a panic or a division by zero.
    assert_eq!(win_pct(0, 0), 0);
    assert!(!diff_is_worth_serving(0, 0));
    // A diff LARGER than the file saturates to 0 rather than wrapping.
    assert_eq!(win_pct(100, 400), 0);
    assert!(!diff_is_worth_serving(100, 400));
}
