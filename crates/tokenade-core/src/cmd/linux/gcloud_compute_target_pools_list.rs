//! `gcloud_compute_target_pools_list` — gcloud compute target-pools list.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("SESSION_AFFINITY") || text.contains("HEALTH_CHECKS")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "gcloud-compute-target-pools", "target pool(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    fn make_table(n: usize) -> String {
        let header = "NAME   REGION   SESSION_AFFINITY   BACKUP   HEALTH_CHECKS\n";
        let rows: String = (0..n).map(|i| format!("item{i}   zone-a   value   active\n")).collect();
        format!("{header}{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_header() { assert!(detect("NAME   REGION   SESSION_AFFINITY   BACKUP   HEALTH_CHECKS")); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(5)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:gcloud-compute-target-pools]"));
        assert!(out.contains("(+10 more)"));
    }
    #[test] fn header_preserved() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("SESSION_AFFINITY"));
        assert!(out.contains("HEALTH_CHECKS"));
    }
}
