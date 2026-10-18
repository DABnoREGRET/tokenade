//! `az_keyvault_secret_list` — az keyvault secret list.

pub fn detect(text: &str) -> bool {
    let t = text.trim_start();
    !t.is_empty() && (t.starts_with('[')) && text.contains("\"contentType\"")
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
    #[test] fn detector_accepts_json() { assert!(detect(r#"[{"contentType":"val","id":"x"}]"#)); }
    #[test] fn compact_returns_string() {
        let out = compact(r#"[{"contentType":"val","id":"x"}]"#);
        assert!(out.contains("contentType"));
    }
}
