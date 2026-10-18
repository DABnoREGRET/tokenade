//! `gcloud_compute_disks_list` — gcloud compute disks list.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("SIZE_GB")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "gcloud-compute-disks", "disk(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    fn make_table(n: usize) -> String {
        let header = "NAME   ZONE   SIZE_GB   TYPE   STATUS\n";
        let rows: String = (0..n).map(|i| format!("item{i}   zone-a   value   active\n")).collect();
        format!("{header}{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_header() { assert!(detect("NAME   ZONE   SIZE_GB   TYPE   STATUS")); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(5)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:gcloud-compute-disks]"));
        assert!(out.contains("(+10 more)"));
    }
    #[test] fn header_preserved() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("SIZE_GB"));
        assert!(out.contains("STATUS"));
    }
}
