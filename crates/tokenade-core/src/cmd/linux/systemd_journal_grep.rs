//! `systemd_journal_grep` — systemd journal grep.


pub fn detect(text: &str) -> bool {
    text.lines().any(|l| l.len() > 20 && (l.contains(':') || l.contains('[')))
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    super::log_tail::tail(text, 5, 40, "systemd-journal-grep")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_valid() { assert!(detect("Jan  1 00:00:00 host sshd: message")); }
    #[test] fn short_output_passes_through() {
        let line = "2026-01-01T00:00:00Z INFO message";
        let s: String = std::iter::repeat_n(line, 3).map(|l| format!("{l}\n")).collect();
        assert!(try_compact(&s).is_none());
    }
    #[test] fn long_output_folds() {
        let line = "2026-01-01T00:00:00Z INFO message";
        let s: String = std::iter::repeat_n(line, 105).map(|l| format!("{l}\n")).collect();
        let out = try_compact(&s).unwrap();
        assert!(out.contains("[tokenade:systemd-journal-grep]"));
        assert!(out.contains("lines omitted"));
    }
}
