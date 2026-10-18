//! `git_rev_list` — git rev-list.

pub fn detect(text: &str) -> bool {
    // rev-list outputs full or short SHA1 hashes, one per line.
    text.lines().any(|l| {
        let t = l.trim();
        t.len() >= 7 && t.chars().all(|c| c.is_ascii_hexdigit())
    })
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    // The ids ARE the answer: `git rev-list -n 60 HEAD` (2.4 KB, measured
    // 2026-10-03) kept 20 of 60. Whole while it fits.
    if tk_base::delivery::fits(text) {
        return Some(text.trim_end().to_string());
    }
    super::git_listing::fold(text, 20, "git-rev-list", "rev(s)")
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
    fn detector_accepts_hashes() {
        let s = "abc1234def5678abc1234def5678abc1234def5678\ndef5678abc1234def5678\n";
        assert!(detect(s));
    }

    #[test]
    fn long_rev_list_folds() {
        // Past the delivery threshold; under it every id is listed.
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        let s: String = (0..50).map(|i| format!("{:040x}\n", i)).collect();
        let out = try_compact(&s).unwrap();
        assert!(out.contains("[tokenade:git-rev-list] 50 rev(s)"));
        assert!(out.contains("(+30 more)"));
    }

    #[test]
    fn a_rev_list_that_fits_lists_every_id() {
        let _g = tk_base::delivery::threshold_on_this_thread(5_000);
        let text: String = (0..60).map(|i| format!("{:040x}\n", i * 104729 + 7)).collect();
        let out = try_compact(&text).unwrap();
        assert_eq!(out, text.trim_end());
    }
}
