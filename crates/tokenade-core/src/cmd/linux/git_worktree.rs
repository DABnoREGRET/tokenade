//! `git_worktree` — git worktree list.

pub fn detect(text: &str) -> bool {
    text.contains('[') || text.contains("HEAD ")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    // Worktrees are rarely >20; only fold when genuinely large.
    super::git_listing::fold(text, 20, "git-worktree", "worktree(s)")
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
    fn short_worktree_passes_through() {
        let s = "/repo  abc1234 [main]\n/repo/wt1  def5678 [feature]\n";
        assert!(try_compact(s).is_none());
    }
}
