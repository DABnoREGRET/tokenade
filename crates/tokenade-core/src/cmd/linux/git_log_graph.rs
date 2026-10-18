//! `git_log_graph` — git log --graph.

pub fn detect(text: &str) -> bool {
    text.contains("* ") || text.contains("|\\") || text.contains("|/") || text.contains("/ ")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    // Graph format: keep 40 lines (graph renders ~2 lines per commit).
    super::git_listing::fold(text, 40, "git-log-graph", "graph line(s)")
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
    fn detector_accepts_graph_output() {
        let s = "* abc1234 feat: add feature\n|\\ \n| * def5678 fix: bug\n";
        assert!(detect(s));
    }

    #[test]
    fn long_graph_folds() {
        let s: String = (0..60).map(|i| format!("* {:07x}0 commit {i}\n", i)).collect();
        let out = try_compact(&s).unwrap();
        assert!(out.contains("[tokenade:git-log-graph] 60 graph line(s)"));
        assert!(out.contains("(+20 more)"));
    }
}
