//! `kubectl_get_daemonset` — kubectl get daemonset.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("NODE SELECTOR")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table_priority(text, HEAD, "kubectl-daemonset", "DaemonSet(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    const HEADER: &str =
        "NAMESPACE   NAME    DESIRED   CURRENT   READY   UP-TO-DATE   AVAILABLE   NODE SELECTOR   AGE";
    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("default   ds-{i}   3   3   3   3   3   kubernetes.io/os=linux   5d\n"))
            .collect();
        format!("{HEADER}\n{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_ds_header() { assert!(detect(HEADER)); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(5)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:kubectl-daemonset] 30 DaemonSet(s)"));
        assert!(out.contains("(+10 more)"));
        assert!(out.contains("NODE SELECTOR"));
    }
    #[test] fn selector_fields_preserved() {
        let out = try_compact(&make_table(25)).unwrap();
        assert!(out.contains("NODE SELECTOR"));
        assert!(out.contains("DESIRED"));
    }
}
