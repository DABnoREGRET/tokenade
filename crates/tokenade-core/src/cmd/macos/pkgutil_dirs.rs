//! `pkgutil_dirs` — pkgutil dirs.
const HEAD: usize = 50;


pub fn detect(text: &str) -> bool {
    text.contains('/')
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    super::git_listing::fold_paths(text, HEAD, "pkgutil-dirs", "dir(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_valid() { assert!(detect("/usr/local/lib")); }
    #[test] fn short_output_passes_through() {
        let line = "/usr/local/lib";
        let s: String = std::iter::repeat_n(line, 3).map(|l| format!("{l}
")).collect();
        assert!(try_compact(&s).is_none());
    }
    #[test] fn long_output_folds() {
        let line = "/usr/local/lib";
        let s: String = std::iter::repeat_n(line, 55).map(|l| format!("{l}
")).collect();
        let out = try_compact(&s).unwrap();
        assert!(out.contains("[tokenade:pkgutil-dirs]"));
        assert!(out.contains("(+5 more)"));
    }
}
