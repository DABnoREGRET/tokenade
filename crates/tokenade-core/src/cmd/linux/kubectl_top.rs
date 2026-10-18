//! `kubectl_top` — kubectl top pods/nodes.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("CPU(cores)") || text.contains("MEMORY(bytes)")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "kubectl-top", "top row(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    const HEADER: &str = "NAMESPACE   NAME   CPU(cores)   MEMORY(bytes)";
    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("default   pod-{i}   100m   256Mi\n"))
            .collect();
        format!("{HEADER}\n{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_top_header() { assert!(detect(HEADER)); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(5)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:kubectl-top] 30 top row(s)"));
        assert!(out.contains("(+10 more)"));
    }
    #[test] fn resource_columns_preserved() {
        let out = try_compact(&make_table(25)).unwrap();
        assert!(out.contains("CPU(cores)"));
        assert!(out.contains("MEMORY(bytes)"));
    }
}
