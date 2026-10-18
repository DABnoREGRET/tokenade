//! `cargo fmt` — silent on success, prints diff on changes.

use crate::compress::dedup_lines;
use crate::compress::truncate_middle;
use crate::ansi::strip_ansi;

pub fn compact(output: &str) -> String {
    let s = strip_ansi(output);
    if s.trim().is_empty() {
        return "fmt: no changes needed".to_string();
    }
    let cleaned = dedup_lines(&s);
    truncate_middle(&cleaned, 80, 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_means_no_changes_needed() {
        assert_eq!(compact(""), "fmt: no changes needed");
        assert_eq!(compact("   \n\n  "), "fmt: no changes needed");
    }

    #[test]
    fn passes_diff_through() {
        let s = "Diff in src/main.rs at line 42:\n-old\n+new\n";
        let out = compact(s);
        assert!(out.contains("Diff in"));
    }
}
