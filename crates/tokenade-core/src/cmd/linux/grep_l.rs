//! `grep_l` — grep l.
const HEAD: usize = 80;


pub fn detect(text: &str) -> bool {
    text.contains('/') || text.contains('.')
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    super::git_listing::list_or_fold_paths(text, HEAD, "grep-l", "file(s) matching")
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
    /// Only above the delivery threshold: below it, the list is
    /// delivered whole (see `a_list_that_fits_is_given_whole`).
    #[test] fn long_output_folds() {
        let s: String = (0..4000).map(|i| format!("/path/to/file{i}.txt\n")).collect();
        let out = try_compact(&s).unwrap();
        assert!(out.contains("[tokenade:grep-l]"));
        assert!(out.contains("more)"), "{out}");
    }

    /// `grep -rlIiE 'tofu|mofu|bofu' …` : 85 chemins, rendus 80 + « (+5 more) ».
    #[test] fn a_list_that_fits_is_given_whole() {
        let s: String = (0..85).map(|i| format!("/srv/app/content/post_{i}.md\n")).collect();
        let out = try_compact(&s).unwrap();
        for i in 0..85 {
            assert!(out.contains(&format!("post_{i}.md")), "post_{i} lost:\n{out}");
        }
    }
}
