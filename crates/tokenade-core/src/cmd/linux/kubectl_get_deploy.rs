//! `kubectl_get_deploy` — kubectl get deploy.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("UP-TO-DATE") || text.contains("AVAILABLE") && text.contains("READY")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table_priority(text, HEAD, "kubectl-deploy", "deployment(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    const HEADER: &str =
        "NAMESPACE   NAME         READY   UP-TO-DATE   AVAILABLE   AGE";
    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("default   deploy-{i}   3/3   3   3   10m\n"))
            .collect();
        format!("{HEADER}\n{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_deploy_header() { assert!(detect(HEADER)); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(5)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:kubectl-deploy] 30 deployment(s)"));
        assert!(out.contains("(+10 more)"));
        assert!(out.contains("UP-TO-DATE"));
    }
    #[test] fn availability_fields_preserved() {
        let out = try_compact(&make_table(25)).unwrap();
        assert!(out.contains("UP-TO-DATE"));
        assert!(out.contains("AVAILABLE"));
    }
}
