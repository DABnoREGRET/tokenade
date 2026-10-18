//! `ls -l` — long listing. Delegates to the shared [`super::ls_table`]
//! engine: elides the link column, names the dominant owner:group once
//! (rows that differ keep theirs) and keeps perms·size·date·name verbatim. Elides
//! COLUMNS not ROWS, so no filename is ever dropped. Locale-independent.

pub fn detect(text: &str) -> bool {
    super::ls_table::detect(text)
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    super::ls_table::fold(text, "ls-long")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
    }

    #[test]
    fn long_listing_elides_columns_keeps_all_names() {
        let mut t = String::from("total 4242\n");
        for i in 0..40 {
            t.push_str(&format!(
                "-rw-r--r-- 1 infected infected {i} 17 juin 08:10 file{i}.rs\n"
            ));
        }
        let out = try_compact(&t).unwrap();
        assert!(out.contains("[tokenade:ls-long]"));
        // Every filename survives (no decapitation).
        assert!(out.contains("file0.rs"));
        assert!(out.contains("file39.rs"));
        // The uniform owner is named once in the banner, on no row.
        assert_eq!(out.matches("infected:infected").count(), 1, "{out}");
    }
}
