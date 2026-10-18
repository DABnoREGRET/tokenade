//! `kubectl_get_csr` — kubectl get certificatesigningrequests.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("SIGNERNAME") || text.contains("REQUESTOR")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table_priority(text, HEAD, "kubectl-csr", "CertificateSigningRequest(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    const HEADER: &str =
        "NAME   AGE   SIGNERNAME                 REQUESTOR   REQUESTEDDURATION   CONDITION";
    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("csr-{i}   5m   kubernetes.io/kube-apiserver-client   admin   <none>   Pending\n"))
            .collect();
        format!("{HEADER}\n{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_csr_header() { assert!(detect(HEADER)); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(5)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:kubectl-csr] 30 CertificateSigningRequest(s)"));
        assert!(out.contains("(+10 more)"));
    }
    #[test] fn signer_and_requestor_preserved() {
        let out = try_compact(&make_table(25)).unwrap();
        assert!(out.contains("SIGNERNAME"));
        assert!(out.contains("REQUESTOR"));
    }
}
