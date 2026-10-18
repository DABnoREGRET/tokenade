//! `appcmd_list_apppool` — appcmd list apppool.
const HEAD: usize = 30;


pub fn detect(text: &str) -> bool {
    text.contains("APPPOOL") || text.contains("MgdVersion")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "appcmd-list-apppool", "apppool(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    fn make_table(n: usize) -> String {
        let hdr = "APPPOOL   AppPool   MgdVersion   MgdPipelineMode   state\n";
        let rows: String = (0..n).map(|i| format!("item{i}  col2  col3  col4\n")).collect();
        format!("{hdr}{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_header() { assert!(detect("APPPOOL   AppPool   MgdVersion   MgdPipelineMode   state")); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(3)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(35)).unwrap();
        assert!(out.contains("[tokenade:appcmd-list-apppool]"));
        assert!(out.contains("(+5 more)"));
    }
    #[test] fn header_preserved() {
        let out = try_compact(&make_table(35)).unwrap();
        assert!(out.contains("APPPOOL"));
        assert!(out.contains("state"));
    }
}
