//! `kubectl_get_svc` — kubectl get svc.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("CLUSTER-IP") || text.contains("EXTERNAL-IP")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table_priority(text, HEAD, "kubectl-svc", "service(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    const HEADER: &str =
        "NAMESPACE   NAME         TYPE        CLUSTER-IP       EXTERNAL-IP   PORT(S)    AGE";
    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("default   svc-{i}   ClusterIP   10.96.0.{i}   <none>   80/TCP   5m\n"))
            .collect();
        format!("{HEADER}\n{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_svc_header() { assert!(detect(HEADER)); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(5)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:kubectl-svc] 30 service(s)"));
        assert!(out.contains("(+10 more)"));
        assert!(out.contains("CLUSTER-IP"));
    }
    #[test] fn ip_fields_preserved() {
        let out = try_compact(&make_table(25)).unwrap();
        assert!(out.contains("CLUSTER-IP"));
        assert!(out.contains("EXTERNAL-IP"));
    }
}
