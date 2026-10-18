//! `dpkg_l` — dpkg l.
const HEAD: usize = 100;


pub fn detect(text: &str) -> bool {
    text.contains("ii  ") || text.contains("rc  ") || text.contains("un  ")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "dpkg-l", "dpkg row(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    fn make_table(n: usize) -> String {
        let hdr = "ii   package   version   arch   description\n";
        let rows: String = (0..n).map(|i| format!("item{i}  col2  col3  col4\n")).collect();
        format!("{hdr}{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_header() { assert!(detect("ii   package   version   arch   description")); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(3)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(105)).unwrap();
        assert!(out.contains("[tokenade:dpkg-l]"));
        assert!(out.contains("(+5 more)"));
    }
    #[test] fn header_preserved() {
        let out = try_compact(&make_table(105)).unwrap();
        assert!(out.contains("ii"));
        assert!(out.contains("arch"));
    }
}
