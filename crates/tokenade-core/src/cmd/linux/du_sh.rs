//! `du -sh *` — per-argument summary sizes (`du -sh */`, `du -sh *`).
//! Delegates to the shared [`super::du_size`] engine: parses the size
//! column and surfaces the largest entries first. A handful of summaries
//! passes through untouched; a directory expanded to many entries gets the
//! largest surfaced. Locale-independent.
const HEAD: usize = 50;

pub fn detect(text: &str) -> bool {
    super::du_size::detect(text)
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    super::du_size::fold(text, HEAD, "du-sh")
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
    fn short_summary_passes_through() {
        // `du -sh *` over a few entries — nothing to fold.
        assert!(try_compact("1,3M\tcli\n86M\tcore\n88M\tbench\n").is_none());
    }

    #[test]
    fn many_entries_surface_largest() {
        let mut t = String::new();
        for i in 0..60 {
            let _ = writeln!(t, "4,0K\tentry{i}");
        }
        t.push_str("512M\tbig\n");
        let out = try_compact(&t).unwrap();
        assert!(out.contains("[tokenade:du-sh]"));
        assert!(out.lines().nth(1).unwrap().contains("512M"));
    }
}
