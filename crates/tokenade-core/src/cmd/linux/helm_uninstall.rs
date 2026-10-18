//! `helm_uninstall` — helm uninstall.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("uninstalled") || text.contains("NAME")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "helm-uninstall", "release(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    fn make_table(n: usize) -> String {
        let hdr = "NAME   NAMESPACE   REVISION   UPDATED   STATUS   CHART   APP VERSION\n";
        let rows: String = (0..n).map(|i| format!("item{i}  col2  col3  col4\n")).collect();
        format!("{hdr}{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_header() { assert!(detect("NAME   NAMESPACE   REVISION   UPDATED   STATUS   CHART   APP VERSION")); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(2)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(HEAD + 5)).unwrap();
        assert!(out.contains("[tokenade:helm-uninstall]"));
        assert!(out.contains("(+5 more)"));
    }
    #[test] fn header_preserved() {
        let out = try_compact(&make_table(HEAD + 5)).unwrap();
        assert!(out.contains("NAME"));
        assert!(out.contains("STATUS"));
    }
}
