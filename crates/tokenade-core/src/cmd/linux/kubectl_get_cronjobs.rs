//! `kubectl_get_cronjobs` — kubectl get cronjobs.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("SCHEDULE") || text.contains("LAST SCHEDULE")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table_priority(text, HEAD, "kubectl-cronjobs", "CronJob(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    const HEADER: &str =
        "NAMESPACE   NAME          SCHEDULE      SUSPEND   ACTIVE   LAST SCHEDULE   AGE";
    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("default   cron-{i}   */5 * * * *   False   0   2m   5d\n"))
            .collect();
        format!("{HEADER}\n{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_cronjobs_header() { assert!(detect(HEADER)); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(5)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:kubectl-cronjobs] 30 CronJob(s)"));
        assert!(out.contains("(+10 more)"));
    }
    #[test] fn schedule_fields_preserved() {
        let out = try_compact(&make_table(25)).unwrap();
        assert!(out.contains("SCHEDULE"));
        assert!(out.contains("LAST SCHEDULE"));
    }
}
