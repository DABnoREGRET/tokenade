//! `gh_codespace_list` — gh codespace list.
const HEAD: usize = 30;


pub fn detect(text: &str) -> bool {
    text.contains("DISPLAY NAME") || text.contains("codespace") || text.contains("github") || text.contains("Creating")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "gh-codespace-list", "codespace(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    fn make_table(n: usize) -> String {
        let hdr = "DISPLAY NAME   REPOSITORY   BRANCH   STATE   CREATED AT\n";
        let rows: String = (0..n).map(|i| format!("item{i}  col2  col3  col4\n")).collect();
        format!("{hdr}{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_header() { assert!(detect("DISPLAY NAME   REPOSITORY   BRANCH   STATE   CREATED AT")); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(3)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(35)).unwrap();
        assert!(out.contains("[tokenade:gh-codespace-list]"));
        assert!(out.contains("(+5 more)"));
    }
    #[test] fn header_preserved() {
        let out = try_compact(&make_table(35)).unwrap();
        assert!(out.contains("DISPLAY NAME"));
        assert!(out.contains("STATE"));
    }
}
