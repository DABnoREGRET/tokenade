//! `gcloud_logging_read` — gcloud logging read.


pub fn detect(text: &str) -> bool {
    text.contains(':')
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    super::log_tail::tail(text, 5, 40, "gcloud-logging-read")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_valid() { assert!(detect("2026-01-01T00:00:00Z INFO: log message")); }
    #[test] fn short_output_passes_through() {
        let s: String = (0..5).map(|i| format!("2026-01-01T00:00:0{i}Z INFO: log entry\n")).collect();
        assert!(try_compact(&s).is_none());
    }
    #[test] fn long_output_folds() {
        let s: String = (0..90).map(|i| format!("2026-01-01T00:00:{i:02}Z INFO: log entry\n")).collect();
        let out = try_compact(&s).unwrap();
        assert!(out.contains("[tokenade:gcloud-logging-read]"));
        assert!(out.contains("lines omitted"));
    }
}
