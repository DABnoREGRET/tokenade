//! `az_webapp_log_tail` — az webapp log tail.


pub fn detect(text: &str) -> bool {
    crate::archetypes::any_output(text)
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    super::log_tail::tail(text, 5, 40, "az-webapp-log-tail")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_valid() { assert!(detect("2024-01-01T00:00:00Z INFO Starting app")); }
    #[test] fn short_output_passes_through() {
        let s: String = (0..5).map(|i| format!("az-log-line-{i}\n")).collect();
        assert!(try_compact(&s).is_none());
    }
    #[test] fn long_output_folds() {
        let s: String = (0..110).map(|i| format!("az-log-line-{i}\n")).collect();
        let out = try_compact(&s).unwrap();
        assert!(out.contains("[tokenade:az-webapp-log-tail]"));
        assert!(out.contains("lines omitted"));
    }
}
