//! `az_acr_create` — az acr create.

pub fn detect(text: &str) -> bool {
    crate::archetypes::json_object_with(text, "\"loginServer\"")
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
    #[test] fn detector_accepts_json() { assert!(detect(r#"{"loginServer":"val","id":"x"}"#)); }
    #[test] fn compact_returns_string() {
        let out = compact(r#"{"loginServer":"val","id":"x"}"#);
        assert!(out.contains("loginServer"));
    }
}
