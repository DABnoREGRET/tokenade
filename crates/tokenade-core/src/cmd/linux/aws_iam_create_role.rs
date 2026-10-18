//! `aws_iam_create_role` — short JSON response.

pub fn detect(text: &str) -> bool {
    text.trim_start().starts_with('{') && text.contains("\"Role\"")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    super::aws_json::compact_response(text)
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn detector_rejects_plain_text() { assert!(!detect("plain\n")); }
    #[test] fn detector_rejects_wrong_key() { assert!(!detect("{\"Other\":\"x\"}")); }
    #[test] fn detector_accepts_valid_json() {
        assert!(detect("{\"Role\":\"value-123\"}"));
    }
    #[test] fn compact_does_not_panic() {
        let _ = compact("{\"Role\":\"value-123\"}");
    }
    #[test] fn key_field_survives() {
        let out = compact("{\"Role\":\"value-123\"}");
        assert!(out.contains("Role") || out.contains("value-123"));
    }
}
