//! `git_reflog` — git reflog.

pub fn detect(text: &str) -> bool {
    text.contains("HEAD@{") || text.contains("ORIG_HEAD")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    super::git_listing::fold(text, 20, "git-reflog", "reflog entry(s)")
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
    fn detector_accepts_reflog() {
        let s = "abc1234 HEAD@{0}: commit: fix bug\ndef5678 HEAD@{1}: checkout: moving\n";
        assert!(detect(s));
    }

    #[test]
    fn short_reflog_passes_through() {
        let s = "abc1234 HEAD@{0}: commit: fix\ndef5678 HEAD@{1}: checkout\n";
        assert!(try_compact(s).is_none());
    }

    #[test]
    fn long_reflog_folds() {
        let s: String = (0..35)
            .map(|i| format!("{:07x}0 HEAD@{{{i}}}: commit: message {i}\n", i))
            .collect();
        let out = try_compact(&s).unwrap();
        assert!(out.contains("[tokenade:git-reflog] 35 reflog entry(s)"));
        assert!(out.contains("(+15 more)"));
        assert!(out.contains("HEAD@{0}"));
        assert!(!out.contains("HEAD@{20}"));
    }
}
