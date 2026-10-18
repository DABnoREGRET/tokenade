//! `git_branch_all` — git branch -a.

pub fn detect(text: &str) -> bool {
    text.contains("remotes/") || text.contains("* ")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    super::git_listing::fold(text, 30, "git-branch-all", "branch(es)")
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
    fn detector_accepts_branch_all() {
        let s = "* main\n  feature/x\n  remotes/origin/main\n";
        assert!(detect(s));
    }

    #[test]
    fn long_branch_list_folds() {
        let s: String = (0..50).map(|i| format!("  remotes/origin/feature-{i}\n")).collect();
        let out = try_compact(&s).unwrap();
        assert!(out.contains("[tokenade:git-branch-all] 50 branch(es)"));
        assert!(out.contains("(+20 more)"));
    }
}
