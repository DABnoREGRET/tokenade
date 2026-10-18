//! `kubectl_events` — kubectl get events.
const HEAD: usize = 20;


pub fn detect(text: &str) -> bool {
    text.contains("LAST SEEN") && text.contains("REASON")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "kubectl-events", "event(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    const HEADER: &str =
        "LAST SEEN   TYPE      REASON    OBJECT          MESSAGE";
    fn make_table(n: usize) -> String {
        let rows: String = (0..n)
            .map(|i| format!("5m   Normal   Scheduled   pod/app-{i}   Successfully assigned\n"))
            .collect();
        format!("{HEADER}\n{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_events_header() { assert!(detect(HEADER)); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(5)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(30)).unwrap();
        assert!(out.contains("[tokenade:kubectl-events] 30 event(s)"));
        assert!(out.contains("(+10 more)"));
    }
    #[test] fn event_fields_preserved() {
        let out = try_compact(&make_table(25)).unwrap();
        assert!(out.contains("LAST SEEN"));
        assert!(out.contains("REASON"));
        assert!(out.contains("MESSAGE"));
    }
}
