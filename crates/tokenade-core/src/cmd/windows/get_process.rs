//! `get_process` — get process.
const HEAD: usize = 60;


pub fn detect(text: &str) -> bool {
    text.contains("Handles") || text.contains("ProcessName") || text.contains("NPM")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "get-process", "process(es)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    fn make_table(n: usize) -> String {
        let hdr = "Handles   NPM(K)   PM(K)   WS(K)   CPU(s)   Id   SI   ProcessName\n";
        let rows: String = (0..n).map(|i| format!("item{i}  col2  col3  col4\n")).collect();
        format!("{hdr}{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_header() { assert!(detect("Handles   NPM(K)   PM(K)   WS(K)   CPU(s)   Id   SI   ProcessName")); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(3)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(65)).unwrap();
        assert!(out.contains("[tokenade:get-process]"));
        assert!(out.contains("(+5 more)"));
    }
    #[test] fn header_preserved() {
        let out = try_compact(&make_table(65)).unwrap();
        assert!(out.contains("Handles"));
        assert!(out.contains("ProcessName"));
    }
}
