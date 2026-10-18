//! `plutil_convert` — plutil convert.
const HEAD: usize = 60;


pub fn detect(text: &str) -> bool {
    { let t = text.trim_start(); t.starts_with(['{', '<', '[']) }
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    super::git_listing::fold(text, HEAD, "plutil-convert", "line(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_valid() { assert!(detect(r#"{"key": "val"}"#)); }
    #[test] fn short_output_passes_through() {
        let line = r#"{"key": "val"}"#;
        let s: String = std::iter::repeat_n(line, 3).map(|l| format!("{l}\n")).collect();
        assert!(try_compact(&s).is_none());
    }
    #[test] fn long_output_folds() {
        let line = r#"{"key": "val"}"#;
        let s: String = std::iter::repeat_n(line, 65).map(|l| format!("{l}\n")).collect();
        let out = try_compact(&s).unwrap();
        assert!(out.contains("[tokenade:plutil-convert]"));
        assert!(out.contains("(+5 more)"));
    }
}
