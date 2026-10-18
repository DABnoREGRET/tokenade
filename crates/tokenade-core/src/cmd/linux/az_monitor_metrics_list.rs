//! `az_monitor_metrics_list` — az monitor metrics list.

pub fn detect(text: &str) -> bool {
    crate::archetypes::json_object_with(text, "\"timeseries\"")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    super::aws_json::compact_response(text)
}

crate::archetypes::cmd_entry_points!(crate::json::compact);

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_json() { assert!(detect(r#"{"timeseries":"val","id":"x"}"#)); }
    #[test] fn compact_returns_string() {
        let out = compact(r#"{"timeseries":"val","id":"x"}"#);
        assert!(out.contains("timeseries"));
    }
}
