//! `kubectl_get_storageclass` — kubectl get storageclasses.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("PROVISIONER")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "kubectl-storageclass", "StorageClass(es)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    const HEADER: &str =
        "NAME            PROVISIONER             RECLAIMPOLICY   VOLUMEBINDINGMODE   AGE";
    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("sc-{i}   docker.io/hostpath   Delete   Immediate   5d\n"))
            .collect();
        format!("{HEADER}\n{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_sc_header() { assert!(detect(HEADER)); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(5)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:kubectl-storageclass] 30 StorageClass(es)"));
        assert!(out.contains("(+10 more)"));
    }
    #[test] fn provisioner_preserved() {
        let out = try_compact(&make_table(25)).unwrap();
        assert!(out.contains("PROVISIONER"));
        assert!(out.contains("sc-0"));
    }
}
