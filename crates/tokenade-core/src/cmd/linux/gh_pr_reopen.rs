//! `gh_pr_reopen` — gh pr reopen.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("github.com") || text.contains("pull request") || text.contains("pull request") || text.contains("github.com") || text.contains('#') || text.contains("PR") || text.contains("NUMBER")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "gh-pr-reopen", "pr(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    fn make_table(n: usize) -> String {
        let hdr = "NUMBER   TITLE   BRANCH   STATUS\n";
        let rows: String = (0..n).map(|i| format!("item{i}  col2  col3  col4\n")).collect();
        format!("{hdr}{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_header() { assert!(detect("NUMBER")); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(2)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(HEAD + 5)).unwrap();
        assert!(out.contains("[tokenade:gh-pr-reopen]"));
        assert!(out.contains("(+5 more)"));
    }
    #[test] fn header_preserved() {
        let out = try_compact(&make_table(HEAD + 5)).unwrap();
        assert!(out.contains("NUMBER"));
        assert!(out.contains("TITLE"));
    }
}
