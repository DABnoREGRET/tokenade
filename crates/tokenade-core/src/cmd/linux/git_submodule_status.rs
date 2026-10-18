//! `git_submodule_status` — git submodule status.
//!
//! Format: `[+- ]hash path (describe)`.

pub fn detect(text: &str) -> bool {
    text.lines().any(|l| {
        let t = l.trim_start();
        t.starts_with(['+', '-', ' ', 'U'])
            && t.len() > 40
            && t[1..].trim_start().as_bytes().iter().take(7).all(|b| b.is_ascii_hexdigit())
    })
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    super::git_listing::fold(text, 20, "git-submodule", "submodule(s)")
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
    fn short_submodule_passes_through() {
        let s = " abc1234def5678abc1234def5678abc1234def5678 vendor/lib (v1.0)\n";
        assert!(try_compact(s).is_none());
    }
}
