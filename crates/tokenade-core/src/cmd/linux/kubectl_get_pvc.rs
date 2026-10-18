//! `kubectl_get_pvc` — kubectl get pvc.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("ACCESS MODES") || text.contains("STORAGECLASS")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table_priority(text, HEAD, "kubectl-pvc", "PersistentVolumeClaim(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    const HEADER: &str =
        "NAMESPACE   NAME       STATUS   VOLUME   CAPACITY   ACCESS MODES   STORAGECLASS   AGE";
    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("default   pvc-{i}   Bound   pv-{i}   10Gi   RWO   standard   5d\n"))
            .collect();
        format!("{HEADER}\n{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_pvc_header() { assert!(detect(HEADER)); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(5)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:kubectl-pvc] 30 PersistentVolumeClaim(s)"));
        assert!(out.contains("(+10 more)"));
        assert!(out.contains("ACCESS MODES"));
    }
    #[test] fn storage_fields_preserved() {
        let out = try_compact(&make_table(25)).unwrap();
        assert!(out.contains("CAPACITY"));
        assert!(out.contains("STORAGECLASS"));
    }
}
