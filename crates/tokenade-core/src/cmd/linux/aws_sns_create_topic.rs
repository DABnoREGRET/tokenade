//! `aws_sns_create_topic` — short JSON response.

pub fn detect(text: &str) -> bool {
    text.trim_start().starts_with('{') && text.contains("\"TopicArn\"")
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
        assert!(detect("{\"TopicArn\":\"value-123\"}"));
    }
    #[test] fn compact_does_not_panic() {
        let _ = compact("{\"TopicArn\":\"value-123\"}");
    }
    #[test] fn key_field_survives() {
        let out = compact("{\"TopicArn\":\"value-123\"}");
        assert!(out.contains("TopicArn") || out.contains("value-123"));
    }
}
