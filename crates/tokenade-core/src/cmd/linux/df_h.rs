//! `df -h` — human-readable disk usage.
//!
//! Delegates to the shared [`super::df_table`] engine, which detects the
//! usage-percent column by content (locale-independent) and surfaces
//! near-full filesystems first.
const HEAD: usize = 30;

pub fn detect(text: &str) -> bool {
    super::df_table::detect(text)
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    super::df_table::fold(text, HEAD, "df-h")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    fn make_table(n: usize) -> String {
        let mut t = String::from("Filesystem Size Used Avail Use% Mounted on\n");
        for i in 0..n {
            let _ = writeln!(t, "/dev/loop{i} 100M 1M 99M 1% /snap/x{i}");
        }
        t
    }

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
    }

    #[test]
    fn detector_accepts_french_header() {
        // Real coreutils FR output — abbreviated header + `Uti%`.
        assert!(detect(
            "Sys. de fichiers Taille Utilisé Dispo Uti% Monté sur\n/dev/sda1 100G 50G 50G 50% /\n"
        ));
    }

    #[test]
    fn short_table_passes_through() {
        assert!(try_compact(&make_table(3)).is_none());
    }

    #[test]
    fn long_table_folds() {
        let out = try_compact(&make_table(45)).unwrap();
        assert!(out.contains("[tokenade:df-h]"));
        assert!(out.contains("more) ..."));
        assert!(out.contains("Filesystem"));
    }
}
