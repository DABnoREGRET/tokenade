//! Shared guards for the PreToolUse hook handlers (extracted from the
//! monolithic `hooks.rs`). These are tiny, behaviour-preserving helpers that
//! were copy-pasted verbatim across `handle_*_pretool`.

/// Read a string field out of a hook payload's `tool_input` object, defaulting
/// to `""` when absent — the shape repeated across handlers for `file_path`,
/// `command`, `url`, `prompt`, … Equivalent to `pointer("/tool_input/<key>")`
/// for a single top-level key.
pub(crate) fn tool_input_str<'a>(parsed: &'a serde_json::Value, key: &str) -> &'a str {
    parsed
        .get("tool_input")
        .and_then(|i| i.get(key))
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_input_str_reads_key_or_defaults_empty() {
        let v = serde_json::json!({"tool_input": {"file_path": "/a/b.rs", "n": 3}});
        assert_eq!(tool_input_str(&v, "file_path"), "/a/b.rs");
        assert_eq!(tool_input_str(&v, "missing"), "");
        // non-string value → default empty (matches the old as_str().unwrap_or)
        assert_eq!(tool_input_str(&v, "n"), "");
        assert_eq!(tool_input_str(&serde_json::json!({}), "file_path"), "");
    }
}
