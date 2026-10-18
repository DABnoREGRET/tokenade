//! `sysinternals_autorunsc_m` — sysinternals autorunsc m.
const HEAD: usize = 100;


pub fn detect(text: &str) -> bool {
    text.contains("CLSID") || text.contains("Publisher") || text.contains("ImagePath")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "sysinternals-autorunsc-m", "autorun entry/entries")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    fn make_table(n: usize) -> String {
        let hdr = "Time   Entry Location   Entry   Enabled   Category   Description   Publisher\n";
        let rows: String = (0..n).map(|i| format!("item{i}  col2  col3  col4\n")).collect();
        format!("{hdr}{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_header() { assert!(detect("Time   Entry Location   Entry   Enabled   Category   Description   Publisher")); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(3)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(105)).unwrap();
        assert!(out.contains("[tokenade:sysinternals-autorunsc-m]"));
        assert!(out.contains("(+5 more)"));
    }
    #[test] fn header_preserved() {
        let out = try_compact(&make_table(105)).unwrap();
        assert!(out.contains("Entry Location"));
        assert!(out.contains("Entry"));
    }
}
