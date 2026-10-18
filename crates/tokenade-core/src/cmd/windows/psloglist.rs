//! `psloglist` — psloglist.


pub fn detect(text: &str) -> bool {
    crate::archetypes::has_substantive_line(text)
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    super::log_tail::tail(text, 5, 40, "psloglist")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_valid() { assert!(detect("output line from psloglist")); }
    #[test] fn short_output_passes_through() {
        let line = "output line from psloglist";
        let s: String = std::iter::repeat_n(line, 3).map(|l| format!("{l}\n")).collect();
        assert!(try_compact(&s).is_none());
    }
    #[test] fn long_output_folds() {
        let line = "output line from psloglist";
        let s: String = std::iter::repeat_n(line, 55).map(|l| format!("{l}\n")).collect();
        let out = try_compact(&s).unwrap();
        assert!(out.contains("[tokenade:psloglist]"));
        assert!(out.contains("lines omitted"));
    }
}
