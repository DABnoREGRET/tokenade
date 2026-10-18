//! `kubectl_get_statefulset` — kubectl get statefulset.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    // StatefulSet header is "NAMESPACE   NAME   READY   AGE"
    // Use a data-line heuristic: READY field shows "n/m" pattern
    text.contains("READY") && text.lines().skip(1).any(|l| {
        l.split_whitespace().any(|t| t.contains('/') && t.chars().all(|c| c.is_ascii_digit() || c == '/'))
    })
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "kubectl-sts", "StatefulSet(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    const HEADER: &str = "NAMESPACE   NAME       READY   AGE";
    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("default   sts-{i}   3/3   5d\n"))
            .collect();
        format!("{HEADER}\n{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_sts_table() { assert!(detect(&make_table(2))); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(5)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:kubectl-sts] 30 StatefulSet(s)"));
        assert!(out.contains("(+10 more)"));
    }
    #[test] fn ready_field_preserved() {
        let out = try_compact(&make_table(25)).unwrap();
        assert!(out.contains("READY"));
        assert!(out.contains("sts-0"));
    }
}
