//! `kubectl_get_secrets` — kubectl get secrets.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    // TYPE + DATA together is specific to k8s secrets table
    text.contains("TYPE") && text.contains("DATA") && text.contains("AGE")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "kubectl-secrets", "secret(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    const HEADER: &str = "NAMESPACE   NAME                  TYPE                                  DATA   AGE";
    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("default   secret-{i}   kubernetes.io/service-account-token   3   5d\n"))
            .collect();
        format!("{HEADER}\n{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_secrets_header() { assert!(detect(HEADER)); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(5)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:kubectl-secrets] 30 secret(s)"));
        assert!(out.contains("(+10 more)"));
        assert!(out.contains("TYPE"));
    }
    #[test] fn type_and_data_preserved() {
        let out = try_compact(&make_table(25)).unwrap();
        assert!(out.contains("TYPE"));
        assert!(out.contains("DATA"));
    }
}
