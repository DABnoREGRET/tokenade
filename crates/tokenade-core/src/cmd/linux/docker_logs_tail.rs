//! `docker_logs_tail` — docker logs --tail.


pub fn detect(text: &str) -> bool {
    crate::archetypes::any_output(text)
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    super::log_tail::tail(text, 5, 40, "docker-logs")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;

    fn make_logs(n: usize) -> String {
        (0..n)
            .map(|i| format!("2024-01-01T00:00:{i:02}Z INFO request processed id={i}\n"))
            .collect()
    }

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
        assert!(!detect("   \n"));
    }

    #[test]
    fn short_log_passes_through() {
        assert!(try_compact(&make_logs(10)).is_none());
    }

    #[test]
    fn long_log_folds() {
        let out = try_compact(&make_logs(120)).unwrap();
        assert!(out.contains("[tokenade:docker-logs]"));
        assert!(out.contains("lines omitted"));
    }

    #[test]
    fn recent_lines_kept_first() {
        let out = try_compact(&make_logs(100)).unwrap();
        // First HEAD lines present; later lines dropped
        assert!(out.contains("id=0"));
        assert!(out.contains("id=79"));
        assert!(!out.contains("id=30"));
    }
}
