//! `journalctl` — journalctl.


/// A journal line, read by its shape (`<month> <day> <HH:MM:SS>`). The
/// detector named `Jan `, `Feb ` and `Mar ` — so from April to December, and
/// in any locale that does not spell those, this compactor never ran.
pub fn detect(text: &str) -> bool {
    text.lines().any(crate::systemd::is_journal_line)
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    super::log_tail::tail(text, 5, 40, "journalctl")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_valid() { assert!(detect("Jan 15 10:30:00 host app[1]: up")); }
    #[test] fn detector_accepts_any_month_in_any_locale() {
        assert!(detect("Oct 03 01:30:42 ai sshd[812]: Accepted key"));
        assert!(detect("sept. 12 18:45:05 ai systemd[1]: Started"));
        assert!(!detect("Mar  a word, no clock"));
    }
    #[test] fn short_output_passes_through() {
        let line = "Jan 15 10:30:00 host app[1]: test output";
        let s: String = std::iter::repeat_n(line, 3).map(|l| format!("{l}\n")).collect();
        assert!(try_compact(&s).is_none());
    }
    #[test] fn long_output_folds() {
        let line = "Jan 15 10:30:00 host app[1]: test output";
        let s: String = std::iter::repeat_n(line, 105).map(|l| format!("{l}\n")).collect();
        let out = try_compact(&s).unwrap();
        assert!(out.contains("[tokenade:journalctl]"));
        assert!(out.contains("lines omitted"));
    }
    /// Reached by NAME (`tokenade filter journalctl`) in October: the spec
    /// row that preceded this module matched `Jan `/`Feb `/`Mar ` only.
    #[test] fn an_october_journal_reaches_this_compactor_by_name() {
        let s: String = (0..105)
            .map(|i| format!("Oct 03 01:{:02}:{:02} ai app[812]: request {i} served\n", i / 60, i % 60))
            .collect();
        let out = crate::cmd::dispatch_named("journalctl", &s).unwrap();
        assert!(out.contains("[tokenade:journalctl]"), "{out}");
    }
}
