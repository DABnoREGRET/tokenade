//! `powershell_get_history` — powershell get history.
const HEAD: usize = 50;


pub fn detect(text: &str) -> bool {
    text.contains("CommandLine") || text.contains("ExecutionStatus")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "powershell-get-history", "history entry/entries")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    fn make_table(n: usize) -> String {
        let hdr = "Id   CommandLine   ExecutionStatus   StartExecutionTime   EndExecutionTime\n";
        let rows: String = (0..n).map(|i| format!("item{i}  col2  col3  col4\n")).collect();
        format!("{hdr}{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_header() { assert!(detect("Id   CommandLine   ExecutionStatus   StartExecutionTime   EndExecutionTime")); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(3)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(55)).unwrap();
        assert!(out.contains("[tokenade:powershell-get-history]"));
        assert!(out.contains("(+5 more)"));
    }
    #[test] fn header_preserved() {
        let out = try_compact(&make_table(55)).unwrap();
        assert!(out.contains("Id"));
        assert!(out.contains("ExecutionStatus"));
    }
}
