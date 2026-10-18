//! `az_role_assignment_list` — az role assignment list.

pub fn detect(text: &str) -> bool {
    let t = text.trim_start();
    !t.is_empty() && (t.starts_with('[')) && text.contains("\"roleDefinitionId\"")
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
    #[test] fn detector_accepts_json() { assert!(detect(r#"[{"roleDefinitionId":"val","id":"x"}]"#)); }
    #[test] fn compact_returns_string() {
        let out = compact(r#"[{"roleDefinitionId":"val","id":"x"}]"#);
        assert!(out.contains("roleDefinitionId"));
    }
}
