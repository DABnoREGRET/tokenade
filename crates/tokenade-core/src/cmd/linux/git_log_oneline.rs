//! `git_log_oneline` — git log --oneline.

pub fn detect(text: &str) -> bool {
    // oneline format: "abc1234 commit message"
    text.lines().any(|l| {
        let t = l.trim();
        t.len() > 8 && t.as_bytes().iter().take(7).all(|b| b.is_ascii_hexdigit()) && t.as_bytes()[7] == b' '
    })
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    super::git_listing::fold(text, 20, "git-log-oneline", "commit(s)")
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
    fn detector_accepts_oneline_format() {
        let s = "abc1234 fix: correct typo in README\ndef5678 feat: add new command\n";
        assert!(detect(s));
    }

    #[test]
    fn short_log_passes_through() {
        let s: String = (0..10).map(|i| format!("{:07x} commit message {i}\n", i)).collect();
        assert!(try_compact(&s).is_none());
    }

    #[test]
    fn long_log_folds() {
        let s: String = (0..40).map(|i| format!("{:07x} commit {i}\n", i)).collect();
        let out = try_compact(&s).unwrap();
        assert!(out.contains("[tokenade:git-log-oneline] 40 commit(s)"));
        assert!(out.contains("(+20 more)"));
    }
}
