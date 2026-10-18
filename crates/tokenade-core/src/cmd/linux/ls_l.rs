//! `ls -l` (alias variant) — delegates to the shared [`super::ls_table`]
//! engine: elides the link column, names the dominant owner:group once,
//! keeps perms·size·date·name
//! verbatim, never drops a row. Locale-independent.

pub fn detect(text: &str) -> bool {
    super::ls_table::detect(text)
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    super::ls_table::fold(text, "ls-l")
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
    fn elides_columns() {
        let t = "-rw-r--r-- 1 u g 42 Jan  1 12:00 file.rs\n\
                 drwxr-xr-x 2 u g 4096 Jan  1 12:00 dir\n";
        let out = try_compact(t).unwrap();
        assert!(out.contains("[tokenade:ls-l]"));
        assert!(out.contains("file.rs"));
    }
}
