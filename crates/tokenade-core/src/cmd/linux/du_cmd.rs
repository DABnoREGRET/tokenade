//! `du` — directory disk usage (no special flags). Delegates to the shared
//! [`super::du_size`] engine: parses the size column, surfaces the largest
//! paths first, head-folds the rest. The root total (du's largest,
//! last-printed line) is preserved because it sorts to the top.
//! Locale-independent.
const HEAD: usize = 50;

pub fn detect(text: &str) -> bool {
    super::du_size::detect(text)
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    super::du_size::fold(text, HEAD, "du")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
    }

    #[test]
    fn long_listing_surfaces_largest_and_keeps_total() {
        let mut t = String::new();
        for i in 0..60 {
            let _ = writeln!(t, "4,0K\t./dir{i}");
        }
        t.push_str("88M\t.\n");
        let out = try_compact(&t).unwrap();
        assert!(out.contains("[tokenade:du]"));
        assert!(out.contains("88M\t."));
    }
}
