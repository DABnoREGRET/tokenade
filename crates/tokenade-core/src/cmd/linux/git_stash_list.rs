//! `git_stash_list` — git stash list.

pub fn detect(text: &str) -> bool {
    text.contains("stash@")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    super::git_listing::fold(text, 20, "git-stash", "stash entry(s)")
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
    fn detector_accepts_stash_list() {
        let s = "stash@{0}: On main: WIP\nstash@{1}: On feature: save\n";
        assert!(detect(s));
    }

    #[test]
    fn short_stash_passes_through() {
        let s = "stash@{0}: On main: WIP\nstash@{1}: On feature: save\n";
        assert!(try_compact(s).is_none());
    }

    #[test]
    fn long_stash_folds() {
        let s: String = (0..25).map(|i| format!("stash@{{{i}}}: On main: WIP {i}\n")).collect();
        let out = try_compact(&s).unwrap();
        assert!(out.contains("[tokenade:git-stash] 25 stash entry(s)"));
        assert!(out.contains("(+5 more)"));
        assert!(out.contains("stash@{0}"));
        assert!(!out.contains("stash@{20}"));
    }
}
