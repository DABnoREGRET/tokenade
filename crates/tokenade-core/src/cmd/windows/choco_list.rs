//! `choco_list` — choco list.
const HEAD: usize = 80;


pub fn detect(text: &str) -> bool {
    text.contains("packages installed") || text.contains("version") || text.lines().any(|l| { let p: Vec<&str> = l.split_whitespace().collect(); p.len() == 2 && p[1].contains('.') })
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "choco-list", "package(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    fn make_table(n: usize) -> String {
        let hdr = "package-name   version\n";
        let rows: String = (0..n).map(|i| format!("item{i}  col2  col3  col4\n")).collect();
        format!("{hdr}{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_header() { assert!(detect("package-name   version")); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(3)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(85)).unwrap();
        assert!(out.contains("[tokenade:choco-list]"));
        assert!(out.contains("(+5 more)"));
    }
    #[test] fn header_preserved() {
        let out = try_compact(&make_table(85)).unwrap();
        assert!(out.contains("package-name"));
        assert!(out.contains("version"));
    }
}
