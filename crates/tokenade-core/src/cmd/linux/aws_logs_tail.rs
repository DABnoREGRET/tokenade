//! `aws_logs_tail` — aws logs tail (CloudWatch Logs streaming).


pub fn detect(text: &str) -> bool {
    crate::archetypes::any_output(text)
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    super::log_tail::tail(text, 5, 40, "aws-logs-tail")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    fn make_log(n: usize) -> String {
        (0..n).map(|i| format!("2024-01-01T00:00:{:02}Z INFO message {i}\n", i % 60)).collect()
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn short_log_passes_through() { assert!(try_compact(&make_log(20)).is_none()); }
    #[test] fn long_log_folds() {
        let out = try_compact(&make_log(100)).unwrap();
        assert!(out.contains("[tokenade:aws-logs-tail]"));
        assert!(out.contains("lines"));
        assert!(out.contains("message 0"));
    }
}
