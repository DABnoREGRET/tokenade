//! `kubectl_get_clusterrole` — kubectl get clusterroles.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("CREATED AT")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table_priority(text, HEAD, "kubectl-clusterrole", "ClusterRole(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    const HEADER: &str = "NAME       CREATED AT";
    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("cluster-role-{i}   2024-01-01T00:00:00Z\n"))
            .collect();
        format!("{HEADER}\n{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_clusterrole_header() { assert!(detect(HEADER)); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(5)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:kubectl-clusterrole] 30 ClusterRole(s)"));
        assert!(out.contains("(+10 more)"));
    }
    #[test] fn created_at_preserved() {
        let out = try_compact(&make_table(25)).unwrap();
        assert!(out.contains("CREATED AT"));
        assert!(out.contains("cluster-role-0"));
    }
}
