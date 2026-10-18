//! `rg_files` — rg files.
const HEAD: usize = 80;


pub fn detect(text: &str) -> bool {
    text.contains('/') || text.contains('.')
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    super::git_listing::fold_paths(text, HEAD, "rg-files", "rg file(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_path() { assert!(detect("/path/to/file.txt")); }
    #[test] fn short_output_passes_through() {
        let s = "/path/to/file.txt\n/path/to/other.rs\n";
        assert!(try_compact(s).is_none());
    }
    #[test] fn long_output_folds() {
        let s: String = (0..85).map(|i| format!("/path/to/file{i}.txt\n")).collect();
        let out = try_compact(&s).unwrap();
        assert!(out.contains("[tokenade:rg-files]"));
        assert!(out.contains("(+5 more)"));
    }
}
