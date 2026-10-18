//! `kubectl_get_rolebinding` — kubectl get rolebindings.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("ROLE") && text.contains("AGE")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table_priority(text, HEAD, "kubectl-rolebinding", "RoleBinding(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    const HEADER: &str = "NAMESPACE   NAME         ROLE             AGE";
    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("default   rb-{i}   Role/viewer   5d\n"))
            .collect();
        format!("{HEADER}\n{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_rb_header() { assert!(detect(HEADER)); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(5)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:kubectl-rolebinding] 30 RoleBinding(s)"));
        assert!(out.contains("(+10 more)"));
    }
    #[test] fn role_field_preserved() {
        let out = try_compact(&make_table(25)).unwrap();
        assert!(out.contains("ROLE"));
        assert!(out.contains("rb-0"));
    }
}
