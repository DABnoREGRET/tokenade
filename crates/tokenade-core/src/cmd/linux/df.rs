//! `df` — disk free, block counts.
//!
//! Delegates to the shared [`super::df_table`] engine: the usage-percent
//! column is found by content, so detection and full-disk surfacing work
//! in any locale (the previous hand-listed `Système de fichiers`/`Util%`
//! guesses never matched real coreutils FR output, which prints
//! `Sys. de fichiers … Uti%`).
const HEAD: usize = 40;

pub fn detect(text: &str) -> bool {
    super::df_table::detect(text)
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    super::df_table::fold(text, HEAD, "df")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    fn make_table(n: usize) -> String {
        let mut t = String::from("Filesystem 1K-blocks Used Available Use% Mounted on\n");
        for i in 0..n {
            let _ = writeln!(t, "/dev/loop{i} 102400 1024 101376 1% /snap/x{i}");
        }
        t
    }

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
    }

    #[test]
    fn detector_accepts_table() {
        assert!(detect(&make_table(3)));
    }

    #[test]
    fn short_table_passes_through() {
        assert!(try_compact(&make_table(3)).is_none());
    }

    #[test]
    fn long_table_folds() {
        let out = try_compact(&make_table(50)).unwrap();
        assert!(out.contains("[tokenade:df]"));
        assert!(out.contains("more) ..."));
        assert!(out.contains("Filesystem"));
    }
}
