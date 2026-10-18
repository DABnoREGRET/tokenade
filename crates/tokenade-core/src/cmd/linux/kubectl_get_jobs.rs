//! `kubectl_get_jobs` — kubectl get jobs.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("COMPLETIONS") || text.contains("DURATION")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table_priority(text, HEAD, "kubectl-jobs", "Job(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    const HEADER: &str = "NAMESPACE   NAME          COMPLETIONS   DURATION   AGE";
    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("default   job-{i}   1/1   5m   2h\n"))
            .collect();
        format!("{HEADER}\n{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_jobs_header() { assert!(detect(HEADER)); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(5)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:kubectl-jobs] 30 Job(s)"));
        assert!(out.contains("(+10 more)"));
    }
    #[test] fn completion_fields_preserved() {
        let out = try_compact(&make_table(25)).unwrap();
        assert!(out.contains("COMPLETIONS"));
        assert!(out.contains("DURATION"));
    }
}
