//! `kubectl_get_networkpolicy` — kubectl get networkpolicies.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("POD-SELECTOR")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table_priority(text, HEAD, "kubectl-netpol", "NetworkPolicy(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    const HEADER: &str = "NAMESPACE   NAME             POD-SELECTOR   AGE";
    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("default   netpol-{i}   app=web   5d\n"))
            .collect();
        format!("{HEADER}\n{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_netpol_header() { assert!(detect(HEADER)); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(5)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:kubectl-netpol] 30 NetworkPolicy(s)"));
        assert!(out.contains("(+10 more)"));
    }
    #[test] fn pod_selector_preserved() {
        let out = try_compact(&make_table(25)).unwrap();
        assert!(out.contains("POD-SELECTOR"));
        assert!(out.contains("netpol-0"));
    }
}
