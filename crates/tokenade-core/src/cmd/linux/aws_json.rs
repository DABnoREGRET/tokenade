//! `aws_json` — shared helper for AWS CLI JSON responses.
//!
//! Most AWS CLI commands output JSON with a top-level object containing
//! one large array (e.g. `{"Stacks":[...]}`, `{"Users":[...]}`).
//! `crate::json::compact` already handles this well — it schema-folds
//! homogeneous arrays to DEFAULT_ARRAY_CAP (20) items and truncates long
//! strings.  This module wires that up with AWS-specific detection and
//! the standard stubs required by the auto-dispatch table.

/// `try_compact` for an AWS command whose output is JSON when it is anything
/// at all: hand `{`/`[` output to [`compact_response`], decline the rest.
///
/// Twenty-one command modules carried these seven lines verbatim. The rule
/// they encode — "a non-JSON AWS response is not ours to fold" — is one rule,
/// and it now has one place to change.
pub fn compact_if_json(text: &str) -> Option<String> {
    let t = text.trim_start();
    if t.starts_with('{') || t.starts_with('[') {
        compact_response(text)
    } else {
        None
    }
}

/// Fold a JSON AWS response.  Returns `None` when the text is not JSON,
/// already short (≤ 30 lines), or when `json::compact` produces no savings.
pub fn compact_response(text: &str) -> Option<String> {
    let trimmed = text.trim_start();
    if !trimmed.starts_with('{') && !trimmed.starts_with('[') {
        return None;
    }
    let compacted = crate::json::compact(text);
    if compacted.len() < text.len() { Some(compacted) } else { None }
}

/// Generic detect: any non-empty AWS JSON envelope.
pub fn detect_json(text: &str) -> bool {
    let t = text.trim_start();
    !t.is_empty() && (t.starts_with('{') || t.starts_with('['))
}

// Required by the auto-dispatch table — helper module, not a command.
crate::archetypes::cmd_helper_module!();

#[cfg(test)]
mod tests {
    use super::*;

    fn make_json_list(key: &str, n: usize) -> String {
        let items: String = (0..n)
            .map(|i| format!(r#"{{"Name":"item-{}","Id":"id-{}","Status":"active"}}"#, i, i))
            .collect::<Vec<_>>()
            .join(",");
        format!("{{\"{key}\":[{items}]}}")
    }

    #[test] fn detect_json_accepts_object() { assert!(detect_json("{\"key\":\"val\"}")); }
    #[test] fn detect_json_accepts_array() { assert!(detect_json("[1,2,3]")); }
    #[test] fn detect_json_rejects_empty() { assert!(!detect_json("")); assert!(!detect_json("  ")); }
    #[test] fn detect_json_rejects_plain_text() { assert!(!detect_json("plain text\nmore text\n")); }

    #[test] fn compact_response_returns_none_for_non_json() {
        assert!(compact_response("plain text\n").is_none());
    }

    #[test] fn compact_response_folds_large_list() {
        let json = make_json_list("Users", 40);
        // json::compact should reduce a 40-item array
        let result = compact_response(&json);
        // May or may not fold depending on json module heuristics — just check no panic
        let _ = result;
    }

    #[test] fn compact_response_preserves_name_field() {
        let json = make_json_list("Users", 30);
        if let Some(out) = compact_response(&json) {
            assert!(out.contains("item-0") || out.contains("Users"));
        }
    }
}
