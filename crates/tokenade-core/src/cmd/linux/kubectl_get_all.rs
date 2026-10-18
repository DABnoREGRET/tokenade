//! `kubectl_get_all` — kubectl get all.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    // "get all" mixes many resources; detect by k8s-specific column headers
    text.contains("READY") || text.contains("CLUSTER-IP") || text.contains("RESTARTS")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table_priority(text, HEAD, "kubectl-get-all", "k8s resource(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    const HEADER: &str =
        "NAME                     READY   STATUS    RESTARTS   AGE";
    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("pod/app-{i}   1/1   Running   0   5m\n"))
            .collect();
        format!("{HEADER}\n{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_k8s_headers() { assert!(detect(HEADER)); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(5)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:kubectl-get-all] 30 k8s resource(s)"));
        assert!(out.contains("(+10 more)"));
    }
    #[test] fn resource_prefix_preserved() {
        let out = try_compact(&make_table(25)).unwrap();
        assert!(out.contains("pod/app-0"));
    }
}
