//! `du -a` — per-file disk usage. Delegates to the shared
//! [`super::du_size`] engine: parses the size column, surfaces the largest
//! paths first, head-folds the rest. Locale-independent (size parsing
//! accepts `,`/`.` decimals). `-a` lists files too, so the tree is large —
//! a high head keeps more of it.
const HEAD: usize = 100;

pub fn detect(text: &str) -> bool {
    super::du_size::detect(text)
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    super::du_size::fold(text, HEAD, "du-a")
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
    fn long_listing_surfaces_largest() {
        let mut t = String::new();
        for i in 0..120 {
            let _ = writeln!(t, "4,0K\t./src/file{i}.rs");
        }
        t.push_str("88M\t.\n");
        let out = try_compact(&t).unwrap();
        assert!(out.contains("[tokenade:du-a]"));
        assert!(out.lines().nth(1).unwrap().contains("88M"));
    }
}
