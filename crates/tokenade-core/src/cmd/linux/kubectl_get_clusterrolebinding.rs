//! `kubectl_get_clusterrolebinding` — kubectl get clusterrolebindings.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("ROLE") && text.contains("AGE")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table_priority(text, HEAD, "kubectl-clusterrolebinding", "ClusterRoleBinding(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    const HEADER: &str = "NAME         ROLE              AGE";
    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("crb-{i}   ClusterRole/admin   5d\n"))
            .collect();
        format!("{HEADER}\n{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_crb_header() { assert!(detect(HEADER)); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(5)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:kubectl-clusterrolebinding] 30 ClusterRoleBinding(s)"));
        assert!(out.contains("(+10 more)"));
    }
    #[test] fn role_and_age_preserved() {
        let out = try_compact(&make_table(25)).unwrap();
        assert!(out.contains("ROLE"));
        assert!(out.contains("crb-0"));
    }
}
