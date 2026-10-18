//! `locate <pattern>` / `mlocate` / `plocate` — fast file search.
//!
//! Output is one absolute path per line. Often 1000s of results
//! for common terms. Cap at top 30 + count summary.


pub fn detect(_text: &str) -> bool {
    // Content is just absolute paths — no shape to detect reliably.
    // Recognised by program name in the dispatch table.
    false
}

pub fn try_compact(text: &str) -> Option<String> {
    Some(compact(text))
}

pub fn compact(text: &str) -> String {
    crate::archetypes::head_lines_with_tally(text, 30, "path(s) elided — refine the pattern")
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    #[test]
    fn folds_long_path_lists() {
        // Past the delivery threshold; under it the list is delivered whole.
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        let mut s = String::new();
        for i in 0..100 {
            let _ = writeln!(s, "/usr/lib/foo/bar/baz/lib{i}.so");
        }
        let out = compact(&s);
        assert!(out.contains("70 more"));
    }

    #[test]
    fn passthrough_short() {
        let s = "/etc/hosts\n/etc/passwd\n/etc/group\n";
        assert_eq!(compact(s), s);
    }
}
