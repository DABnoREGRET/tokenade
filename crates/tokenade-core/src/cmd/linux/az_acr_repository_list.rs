//! `az_acr_repository_list` — az acr repository list.

pub fn detect(text: &str) -> bool {
    let t = text.trim_start();
    !t.is_empty() && t.starts_with('[')
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
    #[test] fn detector_accepts_json() { assert!(detect(r#"["repo1","repo2","repo3"]"#)); }
    #[test] fn compact_returns_string() {
        let out = compact(r#"["repo1","repo2","repo3"]"#);
        assert!(!out.is_empty());
    }
}
