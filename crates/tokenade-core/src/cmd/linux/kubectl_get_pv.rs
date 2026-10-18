//! `kubectl_get_pv` — kubectl get pv.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("RECLAIM POLICY") || text.contains("CLAIM")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table_priority(text, HEAD, "kubectl-pv", "PersistentVolume(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    const HEADER: &str =
        "NAME       CAPACITY   ACCESS MODES   RECLAIM POLICY   STATUS   CLAIM   STORAGECLASS   AGE";
    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("pv-{i}   10Gi   RWO   Retain   Bound   default/pvc-{i}   standard   5d\n"))
            .collect();
        format!("{HEADER}\n{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_pv_header() { assert!(detect(HEADER)); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(5)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:kubectl-pv] 30 PersistentVolume(s)"));
        assert!(out.contains("(+10 more)"));
        assert!(out.contains("RECLAIM POLICY"));
    }
    #[test] fn policy_and_claim_preserved() {
        let out = try_compact(&make_table(25)).unwrap();
        assert!(out.contains("RECLAIM POLICY"));
        assert!(out.contains("CLAIM"));
    }
}
