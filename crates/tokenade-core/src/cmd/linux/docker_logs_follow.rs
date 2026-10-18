//! `docker_logs_follow` — docker logs -f.


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
            .map(|i| format!("2024-01-01T00:00:{i:02}Z INFO event={i}\n"))
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
        let out = try_compact(&make_logs(100)).unwrap();
        assert!(out.contains("[tokenade:docker-logs]"));
        assert!(out.contains("lines omitted"));
    }

    #[test]
    fn first_lines_kept() {
        let out = try_compact(&make_logs(80)).unwrap();
        assert!(out.contains("event=0"));
        assert!(out.contains("event=59"));
        assert!(!out.contains("event=20"));
    }
}
