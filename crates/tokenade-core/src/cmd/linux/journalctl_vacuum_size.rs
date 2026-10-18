//! `journalctl_vacuum_size` — journalctl vacuum size.


pub fn detect(text: &str) -> bool {
    text.contains("Journal") || text.lines().any(|l| l.len() > 30 && (l.contains("systemd") || l.contains(':')))
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    super::log_tail::tail(text, 5, 40, "journalctl-vacuum-size")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_valid() { assert!(detect("Jan  1 00:00:00 host daemon: message here")); }
    #[test] fn short_output_passes_through() {
        let line = "Jan 01 00:00:00 hostname systemd[1]: Started my.service.";
        let s: String = std::iter::repeat_n(line, 3).map(|l| format!("{l}\n")).collect();
        assert!(try_compact(&s).is_none());
    }
    #[test] fn long_output_folds() {
        let line = "Jan 01 00:00:00 hostname systemd[1]: Started my.service.";
        let s: String = std::iter::repeat_n(line, 55).map(|l| format!("{l}\n")).collect();
        let out = try_compact(&s).unwrap();
        assert!(out.contains("[tokenade:journalctl-vacuum-size]"));
        assert!(out.contains("lines omitted"));
    }
}
