//! `powershell_get_ciminstance` — powershell get ciminstance.
const HEAD: usize = 60;


pub fn detect(text: &str) -> bool {
    text.lines().any(|l| { let p: Vec<&str> = l.split_whitespace().collect(); p.len() >= 2 }) || text.contains("WMI") || text.contains("CIM")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "powershell-get-ciminstance", "property/properties")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    fn make_table(n: usize) -> String {
        let hdr = "Name   Value\n";
        let rows: String = (0..n).map(|i| format!("item{i}  col2  col3  col4\n")).collect();
        format!("{hdr}{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_header() { assert!(detect("Name   Value")); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(3)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(65)).unwrap();
        assert!(out.contains("[tokenade:powershell-get-ciminstance]"));
        assert!(out.contains("(+5 more)"));
    }
    #[test] fn header_preserved() {
        let out = try_compact(&make_table(65)).unwrap();
        assert!(out.contains("Name"));
        assert!(out.contains("Value"));
    }
}
