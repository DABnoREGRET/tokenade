//! `kubectl_get_hpa` — kubectl get hpa.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("TARGETS") && text.contains("MINPODS")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table_priority(text, HEAD, "kubectl-hpa", "HorizontalPodAutoscaler(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    const HEADER: &str =
        "NAMESPACE   NAME   REFERENCE          TARGETS   MINPODS   MAXPODS   REPLICAS   AGE";
    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("default   hpa-{i}   Deployment/app   50%/80%   1   10   3   5d\n"))
            .collect();
        format!("{HEADER}\n{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_hpa_header() { assert!(detect(HEADER)); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(5)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:kubectl-hpa] 30 HorizontalPodAutoscaler(s)"));
        assert!(out.contains("(+10 more)"));
        assert!(out.contains("TARGETS"));
    }
    #[test] fn scaling_fields_preserved() {
        let out = try_compact(&make_table(25)).unwrap();
        assert!(out.contains("TARGETS"));
        assert!(out.contains("MINPODS"));
        assert!(out.contains("MAXPODS"));
    }
}
