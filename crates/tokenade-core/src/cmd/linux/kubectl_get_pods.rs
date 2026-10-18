//! `kubectl_get_pods` — kubectl get pods.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("READY") && text.contains("STATUS") && text.contains("RESTARTS")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table_priority(text, HEAD, "kubectl-pods", "pod(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    const HEADER: &str =
        "NAMESPACE   NAME                     READY   STATUS    RESTARTS   AGE";
    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("default   pod-{i}   1/1   Running   0   5m\n"))
            .collect();
        format!("{HEADER}\n{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); assert!(!detect("   \n")); }
    #[test] fn detector_accepts_pods_header() { assert!(detect(HEADER)); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(5)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:kubectl-pods] 30 pod(s)"));
        assert!(out.contains("(+10 more)"));
        assert!(out.contains("RESTARTS"));
    }
    #[test] fn namespace_and_status_preserved() {
        let out = try_compact(&make_table(25)).unwrap();
        assert!(out.contains("NAMESPACE"));
        assert!(out.contains("STATUS"));
        assert!(out.contains("pod-0"));
    }
}
