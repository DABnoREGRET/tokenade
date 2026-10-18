//! `aws_logs_describe_log_groups` — returns `logGroups`.

pub fn detect(text: &str) -> bool {
    text.trim_start().starts_with('{') && text.contains("\"logGroups\"")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    super::aws_json::compact_response(text)
}

crate::archetypes::cmd_entry_points!(crate::json::compact);

#[cfg(test)]
mod tests {
    use super::*;
    fn make_resp(n: usize) -> String {
        let items = (0..n)
            .map(|i| format!(r#"{{"logGroupName":"item-{}","Status":"active"}}"#, i))
            .collect::<Vec<_>>().join(",");
        format!("{{\"logGroups\":[{items}]}}")
    }
    #[test] fn detector_rejects_plain_text() { assert!(!detect("plain\n")); }
    #[test] fn detector_rejects_wrong_key() { assert!(!detect("{\"Other\":[]}")); }
    #[test] fn detector_accepts_valid_json() { assert!(detect(&make_resp(2))); }
    #[test] fn compact_does_not_panic() { let _ = compact(&make_resp(3)); }
    #[test] fn key_preserved_in_output() {
        let out = compact(&make_resp(25));
        assert!(out.contains("logGroups") || out.contains("logGroupName"));
    }
}
