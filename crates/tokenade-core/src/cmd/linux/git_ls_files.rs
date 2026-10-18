//! `git_ls_files` — git ls-files.

pub fn detect(text: &str) -> bool {
    crate::archetypes::any_output(text)
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    // The paths ARE the answer to `git ls-files`: listed whole (grouped)
    // while that fits. Measured 2026-10-03, `git ls-files crates/tk-fold/src`
    // — 51 files, 1.8 KB — came back as 20 names and `+31 more`.
    super::git_listing::list_or_fold_paths(text, 20, "git-ls-files", "tracked file(s)")
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
    fn short_listing_passes_through() {
        let s = "src/main.rs\nsrc/lib.rs\nCargo.toml\n";
        assert!(try_compact(s).is_none());
    }

    #[test]
    fn long_listing_folds_with_dir_summary() {
        // Past the delivery threshold; under it every path is listed.
        let _g = tk_base::delivery::threshold_on_this_thread(40);
        let text: String = (0..30).map(|i| format!("src/file{i}.rs\n")).collect();
        let out = try_compact(&text).unwrap();
        assert!(out.contains("[tokenade:git-ls-files] 30 tracked file(s)"));
        assert!(out.contains("src/"));
        assert!(!out.contains("src/file20.rs"));
    }

    // Multilingual path test: non-ASCII filenames must survive verbatim.
    #[test]
    fn unicode_paths_preserved() {
        let text: String = (0..25).map(|i| format!("docs/文書{i}.md\n")).collect();
        let out = try_compact(&text).unwrap();
        assert!(out.contains("文書0"));
    }

    #[test]
    fn a_listing_that_fits_names_every_file() {
        let _g = tk_base::delivery::threshold_on_this_thread(5_000);
        let text: String = (0..51).map(|i| format!("crates/tk-fold/src/module_{i}.rs\n")).collect();
        let out = try_compact(&text).unwrap();
        for i in 0..51 {
            assert!(out.contains(&format!("module_{i}.rs")), "lost module_{i}.rs:\n{out}");
        }
    }
}
