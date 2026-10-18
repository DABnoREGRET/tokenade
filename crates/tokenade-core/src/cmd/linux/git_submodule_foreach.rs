//! `git_submodule_foreach` — git submodule foreach.

pub fn detect(text: &str) -> bool {
    text.contains("Entering '") || text.contains("submodule")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    super::git_listing::fold(text, 20, "git-submodule-foreach", "submodule(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
        assert!(!detect("   \n"));
    }

    #[test]
    fn detector_accepts_foreach_output() {
        let s = "Entering 'vendor/lib'\nEntering 'vendor/other'\n";
        assert!(detect(s));
    }
}
