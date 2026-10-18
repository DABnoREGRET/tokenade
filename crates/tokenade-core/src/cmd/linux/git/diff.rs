//! `git diff` — all variants.
//!
//! | Flag                | Shape                          | Strategy            |
//! |---------------------|--------------------------------|---------------------|
//! | `--stat`            | `<file> \| N ++--` summary     | dedicated parser    |
//! | `--shortstat`       | one line summary               | pass-through        |
//! | `--name-only`       | filenames                      | dedicated parser    |
//! | `--name-status`     | `M\tfile` per change           | same as name-only   |
//! | `--word-diff`       | inline `{+added+}/[-removed-]` | dedicated parser    |
//! | `--check`           | whitespace error report        | pass-through        |
//! | default             | `diff --git` + hunks           | dedicated parser    |

use crate::compress::generic;
use crate::argv::{has_any_flag, has_flag_prefix};
use crate::compress::truncate_middle;
use crate::ansi::strip_ansi;

pub fn dispatch(argv: &[String], output: &str) -> String {
    if has_any_flag(argv, &["--numstat"]) {
        // `N\tN\tpath` rows — has its own magnitude-aware compactor (keeps head
        // + always surfaces the largest/binary changes from the folded tail).
        return crate::cmd::linux::git_diff_numstat::compact(output);
    }
    if has_any_flag(argv, &["--name-only"]) {
        return name_only(output);
    }
    if has_any_flag(argv, &["--name-status"]) {
        return name_status(output);
    }
    if has_any_flag(argv, &["--shortstat"]) {
        return shortstat(output);
    }
    if has_any_flag(argv, &["--stat"]) {
        return stat(output);
    }
    if has_any_flag(argv, &["--word-diff"]) || has_flag_prefix(argv, "--word-diff=") {
        return word(output);
    }
    if has_any_flag(argv, &["--check"]) {
        return check(output);
    }
    default(output)
}

pub fn name_only(output: &str) -> String {
    let s = strip_ansi(output);
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return "no changes".to_string();
    }
    truncate_middle(&lines.join("\n"), 80, 20)
}

pub fn name_status(output: &str) -> String {
    // Same shape as name-only.
    name_only(output)
}

pub fn stat(output: &str) -> String {
    let s = strip_ansi(output);
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return "no changes".to_string();
    }
    truncate_middle(&lines.join("\n"), 80, 5)
}

/// A `--stat` row's bar (`23 ++++++++++++++++++++-`) is git's DRAWING of its
/// count, scaled to the terminal: the count says how much, the bar adds only
/// whether lines went in, out, or both. Kept as that (`23 +-`), with the
/// column padding gone. Measured 2026-10-04: a real `git log --stat -n 15`
/// spent a third of its bytes on bars.
pub fn squeeze_stat_bars(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        out.push_str(&squeeze_stat_row(line).unwrap_or_else(|| line.to_string()));
        out.push('\n');
    }
    if !text.ends_with('\n') {
        out.pop();
    }
    out
}

fn squeeze_stat_row(line: &str) -> Option<String> {
    if !line.starts_with(' ') {
        return None;
    }
    let (path, rest) = line.split_once(" | ")?;
    let (count, bar) = rest.trim().split_once(' ').unwrap_or((rest.trim(), ""));
    if count.is_empty() || !count.bytes().all(|b| b.is_ascii_digit()) || !bar.bytes().all(|b| b == b'+' || b == b'-') {
        return None;
    }
    let sign = match (bar.contains('+'), bar.contains('-')) {
        (true, true) => " +-",
        (true, false) => " +",
        (false, true) => " -",
        (false, false) => "",
    };
    Some(format!(" {} | {count}{sign}", path.trim()))
}

pub fn shortstat(output: &str) -> String {
    let s = strip_ansi(output);
    s.trim().to_string()
}

pub fn word(output: &str) -> String {
    let s = strip_ansi(output);
    let mut out = Vec::new();
    for line in s.lines() {
        if line.starts_with("diff --git")
            || line.starts_with("@@")
            || line.starts_with("+++")
            || line.starts_with("---")
            || line.contains("{+")
            || line.contains("[-")
        {
            out.push(line);
        }
    }
    if out.is_empty() {
        return generic(output);
    }
    truncate_middle(&out.join("\n"), 200, 40)
}

pub fn check(output: &str) -> String {
    let s = strip_ansi(output);
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return "no whitespace issues".to_string();
    }
    lines.join("\n")
}

pub fn default(output: &str) -> String {
    let s = strip_ansi(output);
    // Same door as `diff` / `diff -u` / `diff -r`: a unified patch keeps
    // every `+`/`-` line. The previous 200+40 `truncate_middle` dropped
    // the middle of a multi-hunk `git diff` (ranking `auto:diff`/`git`).
    if let Some(folded) = crate::diff::compact_if_unified(&s) {
        return folded;
    }
    let mut out = Vec::new();
    for line in s.lines() {
        if line.starts_with("diff --git")
            || line.starts_with("@@")
            || line.starts_with("+++")
            || line.starts_with("---")
            || line.starts_with('+')
            || line.starts_with('-')
        {
            out.push(line);
        }
    }
    if out.is_empty() {
        let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
        return truncate_middle(&lines.join("\n"), 80, 20);
    }
    truncate_middle(&out.join("\n"), 200, 40)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(slice: &[&str]) -> Vec<String> {
        slice.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn default_keeps_hunks_drops_context() {
        let s = "diff --git a/foo b/foo\nindex abc..def 100644\n--- a/foo\n+++ b/foo\n@@ -1,3 +1,3 @@\n context line\n-old\n+new\n context2\n";
        let out = dispatch(&argv(&["git", "diff"]), s);
        assert!(out.contains("+new"));
        assert!(out.contains("-old"));
        assert!(out.contains("@@"));
        // `diff::compact` keeps ±1 context around a change (not a 200+40 cut).
        assert!(!out.contains("index abc"));
    }

    #[test]
    fn default_keeps_every_change_past_the_old_truncate() {
        let mut s = String::from("diff --git a/foo b/foo\n--- a/foo\n+++ b/foo\n");
        for i in 0..250 {
            s.push_str(&format!("@@ -{i} +{i} @@\n-old{i}\n+new{i}\n"));
        }
        let out = dispatch(&argv(&["git", "diff"]), &s);
        assert!(out.contains("+new0"), "{out}");
        assert!(out.contains("+new249"), "middle/tail hunk dropped:\n{out}");
        assert!(out.contains("-old124"), "middle hunk dropped:\n{out}");
    }

    #[test]
    fn a_stat_bar_becomes_its_sign() {
        let raw = " src/a.rs                 | 23 ++++++++++++++++++++-\n src/b.rs | 9 +++++++++\n src/c.rs | 4 ----\n img.png  | Bin 0 -> 12 bytes\n 4 files changed, 29 insertions(+), 5 deletions(-)";
        assert_eq!(
            squeeze_stat_bars(raw),
            " src/a.rs | 23 +-\n src/b.rs | 9 +\n src/c.rs | 4 -\n img.png  | Bin 0 -> 12 bytes\n 4 files changed, 29 insertions(+), 5 deletions(-)"
        );
    }

    /// The bar goes only when `--stat` drew it: a patch's context line that
    /// happens to look like a row stays as written.
    #[test]
    fn only_a_stat_view_is_squeezed() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let raw = "commit 0123456789abcdef0123456789abcdef01234567\nAuthor: A <a@b>\nDate:   Sun Oct 4 08:46:01 2026 +0200\n\n    subject\n\n src/a.rs | 23 ++++++++++++++++++++-\n 1 file changed, 22 insertions(+), 1 deletion(-)\n";
        let out = crate::filter::filter_command(&a(&["git", "log", "--stat"]), raw).output;
        assert!(out.contains(" src/a.rs | 23 +-\n"), "{out}");
        let out = crate::filter::filter_command(&a(&["git", "log"]), raw).output;
        assert!(out.contains("23 ++++++++++++++++++++-"), "{out}");
    }

    #[test]
    fn name_only_minimal() {
        let s = "foo.rs\nbar.rs\nbaz.rs\n";
        let out = dispatch(&argv(&["git", "diff", "--name-only"]), s);
        assert!(out.contains("foo.rs"));
    }

    #[test]
    fn shortstat_passthrough() {
        let s = " 3 files changed, 42 insertions(+), 17 deletions(-)\n";
        let out = dispatch(&argv(&["git", "diff", "--shortstat"]), s);
        assert!(out.contains("3 files changed"));
    }

    #[test]
    fn word_diff_keeps_inline_markers() {
        let s = "diff --git a/foo b/foo\n@@ -1 +1 @@\nhello {+world+} [-old-]\n";
        let out = dispatch(&argv(&["git", "diff", "--word-diff"]), s);
        assert!(out.contains("{+world+}"));
        assert!(out.contains("[-old-]"));
    }

    #[test]
    fn check_no_issues_message() {
        let out = dispatch(&argv(&["git", "diff", "--check"]), "");
        assert_eq!(out, "no whitespace issues");
    }

    #[test]
    fn name_only_empty_message() {
        let out = dispatch(&argv(&["git", "diff", "--name-only"]), "");
        assert_eq!(out, "no changes");
    }
}
