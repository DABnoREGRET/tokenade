//! `kubectl_get_nodes` — kubectl get nodes.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("ROLES") && text.contains("VERSION")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table_priority(text, HEAD, "kubectl-nodes", "node(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    const HEADER: &str = "NAME       STATUS   ROLES           AGE   VERSION";
    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("node-{i}   Ready   control-plane   10d   v1.28.0\n"))
            .collect();
        format!("{HEADER}\n{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_nodes_header() { assert!(detect(HEADER)); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(5)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:kubectl-nodes] 30 node(s)"));
        assert!(out.contains("(+10 more)"));
        assert!(out.contains("ROLES"));
    }
    #[test] fn version_and_roles_preserved() {
        let out = try_compact(&make_table(25)).unwrap();
        assert!(out.contains("ROLES"));
        assert!(out.contains("VERSION"));
    }
}
