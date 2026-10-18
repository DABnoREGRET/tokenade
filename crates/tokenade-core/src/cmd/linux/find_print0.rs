//! `find_print0` — find print0.
const HEAD: usize = 50;


pub fn detect(text: &str) -> bool {
    text.contains('/')
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    super::git_listing::fold_paths(text, HEAD, "find-print0", "path(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_valid() { assert!(detect("/path/to/item")); }
    #[test] fn short_output_passes_through() {
        let line = "/path/to/item";
        let s: String = std::iter::repeat_n(line, 3).map(|l| format!("{l}\n")).collect();
        assert!(try_compact(&s).is_none());
    }
    #[test] fn long_output_folds() {
        let line = "/path/to/item";
        let s: String = std::iter::repeat_n(line, 55).map(|l| format!("{l}\n")).collect();
        let out = try_compact(&s).unwrap();
        assert!(out.contains("[tokenade:find-print0]"));
        assert!(out.contains("(+5 more)"));
    }
}
