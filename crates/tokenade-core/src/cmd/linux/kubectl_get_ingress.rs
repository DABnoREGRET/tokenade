//! `kubectl_get_ingress` — kubectl get ingress.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("HOSTS") && text.contains("ADDRESS")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table_priority(text, HEAD, "kubectl-ingress", "ingress(es)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    const HEADER: &str =
        "NAMESPACE   NAME      CLASS     HOSTS             ADDRESS         PORTS     AGE";
    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("default   ing-{i}   nginx   app{i}.example.com   10.0.0.{i}   80   5d\n"))
            .collect();
        format!("{HEADER}\n{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_ingress_header() { assert!(detect(HEADER)); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(5)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:kubectl-ingress] 30 ingress(es)"));
        assert!(out.contains("(+10 more)"));
        assert!(out.contains("HOSTS"));
    }
    #[test] fn hosts_and_address_preserved() {
        let out = try_compact(&make_table(25)).unwrap();
        assert!(out.contains("HOSTS"));
        assert!(out.contains("ADDRESS"));
    }
}
