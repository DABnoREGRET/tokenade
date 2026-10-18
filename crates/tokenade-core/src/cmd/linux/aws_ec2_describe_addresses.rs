//! `aws_ec2_describe_addresses` — aws ec2 command returning `Addresses`.

pub fn detect(text: &str) -> bool {
    text.trim_start().starts_with('{') && text.contains("\"Addresses\"")
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
            .map(|i| format!(r#"{{"PublicIp":"id-{}","Status":"active"}}"#, i))
            .collect::<Vec<_>>().join(",");
        format!("{{\"Addresses\":[{items}]}}")
    }
    #[test] fn detector_rejects_plain_text() { assert!(!detect("plain text\n")); }
    #[test] fn detector_rejects_wrong_key() { assert!(!detect("{\"Other\":[]}")); }
    #[test] fn detector_accepts_Addresses_json() { assert!(detect(&make_resp(2))); }
    #[test] fn compact_does_not_panic_on_short() { let _ = compact(&make_resp(3)); }
    #[test] fn critical_field_preserved() {
        let out = compact(&make_resp(25));
        // json::compact keeps the key name and at least the first item's fields
        assert!(out.contains("Addresses") || out.contains("PublicIp"));
    }
}
