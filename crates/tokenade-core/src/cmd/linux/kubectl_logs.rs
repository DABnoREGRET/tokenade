//! `kubectl_logs` — kubectl logs.


pub fn detect(text: &str) -> bool {
    crate::archetypes::any_output(text)
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    super::log_tail::tail(text, 5, 40, "kubectl-logs")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    fn make_log(n: usize) -> String {
        (0..n).map(|i| format!("2024-01-01T00:00:{:02}Z INFO message {i}\n", i % 60)).collect()
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); assert!(!detect("   \n")); }
    #[test] fn detector_accepts_log_lines() { assert!(detect(&make_log(5))); }
    #[test] fn short_log_passes_through() { assert!(try_compact(&make_log(20)).is_none()); }
    #[test] fn long_log_folds() {
        let out = try_compact(&make_log(100)).unwrap();
        assert!(out.contains("[tokenade:kubectl-logs]"));
        assert!(out.contains("lines omitted"));
    }
    #[test] fn recent_lines_in_tail() {
        let out = try_compact(&make_log(100)).unwrap();
        assert!(out.contains("message 0"));
        assert!(!out.contains("message 30"));
    }
}
