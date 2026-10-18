//! `az_storage_container_create` — az storage container create.

pub fn detect(text: &str) -> bool {
    crate::archetypes::json_object_with(text, "\"created\"")
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
    #[test] fn detector_accepts_json() { assert!(detect(r#"{"created":"val","id":"x"}"#)); }
    #[test] fn compact_returns_string() {
        let out = compact(r#"{"created":"val","id":"x"}"#);
        assert!(out.contains("created"));
    }
}
