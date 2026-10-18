//! `git_branch_remote` — git branch -r.

pub fn detect(text: &str) -> bool {
    text.contains("remotes/") || text.contains("origin/") || text.contains("upstream/")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    super::git_listing::fold(text, 30, "git-branch-remote", "remote branch(es)")
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
    fn detector_accepts_remote_branches() {
        let s = "  origin/main\n  origin/feature\n  upstream/main\n";
        assert!(detect(s));
    }

    #[test]
    fn long_remote_list_folds() {
        let s: String = (0..50).map(|i| format!("  origin/feature-{i}\n")).collect();
        let out = try_compact(&s).unwrap();
        assert!(out.contains("[tokenade:git-branch-remote] 50 remote branch(es)"));
        assert!(out.contains("(+20 more)"));
    }
}
