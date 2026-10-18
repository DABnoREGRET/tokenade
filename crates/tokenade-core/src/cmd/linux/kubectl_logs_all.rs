//! `kubectl_logs_all` — kubectl logs --all-containers.


pub fn detect(text: &str) -> bool {
    crate::archetypes::any_output(text)
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    super::log_tail::tail(text, 5, 40, "kubectl-logs-all")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    fn make_log(n: usize) -> String {
        (0..n).map(|i| format!("[container-{i}] log message {i}\n")).collect()
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn short_log_passes_through() { assert!(try_compact(&make_log(20)).is_none()); }
    #[test] fn long_log_folds() {
        let out = try_compact(&make_log(100)).unwrap();
        assert!(out.contains("[tokenade:kubectl-logs-all]"));
        assert!(out.contains("lines omitted"));
    }
    #[test] fn container_prefix_preserved() {
        let out = try_compact(&make_log(100)).unwrap();
        assert!(out.contains("[container-0]"));
    }
}
