//! `kubectl_get` — kubectl get (generic).
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    // Generic k8s table: has NAME column and at least one k8s-typical header word
    text.contains("NAME") && (
        text.contains("READY") || text.contains("STATUS") || text.contains("AGE") ||
        text.contains("CLUSTER-IP") || text.contains("RESTARTS")
    )
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "kubectl-get", "k8s resource(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    const HEADER: &str = "NAME   STATUS   AGE";
    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("resource-{i}   Active   5d\n"))
            .collect();
        format!("{HEADER}\n{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_generic_k8s_header() { assert!(detect(HEADER)); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(5)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:kubectl-get] 30 k8s resource(s)"));
        assert!(out.contains("(+10 more)"));
    }
    #[test] fn name_and_status_preserved() {
        let out = try_compact(&make_table(25)).unwrap();
        assert!(out.contains("NAME"));
        assert!(out.contains("STATUS"));
        assert!(out.contains("resource-0"));
    }
}
