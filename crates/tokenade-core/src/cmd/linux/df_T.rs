//! `df -T` — include filesystem type column.
//!
//! Delegates to the shared [`super::df_table`] engine. The extra `Type`
//! column does not change the fold: the usage-percent token is still found
//! by content, so detection and full-first surfacing stay locale-independent.
const HEAD: usize = 30;

pub fn detect(text: &str) -> bool {
    super::df_table::detect(text)
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    super::df_table::fold(text, HEAD, "df-T")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    fn make_table(n: usize) -> String {
        let mut t = String::from("Filesystem Type Size Used Avail Use% Mounted on\n");
        for i in 0..n {
            let _ = writeln!(t, "/dev/loop{i} squashfs 100M 1M 99M 1% /snap/x{i}");
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
        let out = try_compact(&make_table(35)).unwrap();
        assert!(out.contains("[tokenade:df-T]"));
        assert!(out.contains("more) ..."));
        assert!(out.contains("Type"));
    }
}
