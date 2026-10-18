//! `kubectl_get_replicaset` — kubectl get replicaset.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("DESIRED") && text.contains("CURRENT") && text.contains("READY")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "kubectl-rs", "ReplicaSet(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    const HEADER: &str = "NAMESPACE   NAME                       DESIRED   CURRENT   READY   AGE";
    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("default   rs-{i}-abc123   3   3   3   5d\n"))
            .collect();
        format!("{HEADER}\n{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_rs_header() { assert!(detect(HEADER)); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(5)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:kubectl-rs] 30 ReplicaSet(s)"));
        assert!(out.contains("(+10 more)"));
    }
    #[test] fn replica_fields_preserved() {
        let out = try_compact(&make_table(25)).unwrap();
        assert!(out.contains("DESIRED"));
        assert!(out.contains("CURRENT"));
        assert!(out.contains("READY"));
    }
}
