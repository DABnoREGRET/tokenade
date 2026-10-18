//! `kubectl_api_resources` — kubectl api-resources.
const HEAD: usize = 30;


pub fn detect(text: &str) -> bool {
    text.contains("APIVERSION") && text.contains("KIND")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "kubectl-api-resources", "API resource(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    const HEADER: &str = "NAME            SHORTNAMES   APIVERSION   NAMESPACED   KIND";
    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("resource{i}   rs{i}   v1   true   Resource{i}\n"))
            .collect();
        format!("{HEADER}\n{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_api_resources_header() { assert!(detect(HEADER)); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(10)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(50)).unwrap();
        assert!(out.contains("[tokenade:kubectl-api-resources] 50 API resource(s)"));
        assert!(out.contains("(+20 more)"));
    }
    #[test] fn api_fields_preserved() {
        let out = try_compact(&make_table(40)).unwrap();
        assert!(out.contains("APIVERSION"));
        assert!(out.contains("KIND"));
        assert!(out.contains("SHORTNAMES"));
    }
}
