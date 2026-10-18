//! `arp_win` — arp win.
const HEAD: usize = 40;


pub fn detect(text: &str) -> bool {
    text.contains("Interface") || text.contains("Internet Address")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "arp-win", "arp entry/entries")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    fn make_table(n: usize) -> String {
        let hdr = "Interface   Internet Address   Physical Address   Type\n";
        let rows: String = (0..n).map(|i| format!("item{i}  col2  col3  col4\n")).collect();
        format!("{hdr}{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_header() { assert!(detect("Interface   Internet Address   Physical Address   Type")); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(3)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(45)).unwrap();
        assert!(out.contains("[tokenade:arp-win]"));
        assert!(out.contains("(+5 more)"));
    }
    #[test] fn header_preserved() {
        let out = try_compact(&make_table(45)).unwrap();
        assert!(out.contains("Interface"));
        assert!(out.contains("Type"));
    }
}
