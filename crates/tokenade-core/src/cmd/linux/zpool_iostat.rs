//! `zpool_iostat` — zpool iostat.
const HEAD: usize = 30;


pub fn detect(text: &str) -> bool {
    text.contains("capacity") || text.contains("operations")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "zpool-iostat", "zpool iostat row(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    fn make_table(n: usize) -> String {
        let hdr = "              capacity     operations     bandwidth\n";
        let rows: String = (0..n).map(|i| format!("item{i}  col2  col3  col4\n")).collect();
        format!("{hdr}{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_header() { assert!(detect("              capacity     operations     bandwidth")); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(3)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(35)).unwrap();
        assert!(out.contains("[tokenade:zpool-iostat]"));
        assert!(out.contains("(+5 more)"));
    }
    #[test] fn header_preserved() {
        let out = try_compact(&make_table(35)).unwrap();
        assert!(out.contains("capacity"));
        assert!(out.contains("bandwidth"));
    }
}
