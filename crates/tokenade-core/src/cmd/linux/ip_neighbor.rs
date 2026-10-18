//! `ip_neighbor` — ip neighbor.
const HEAD: usize = 50;


pub fn detect(text: &str) -> bool {
    text.contains("lladdr") || text.contains("REACHABLE") || text.contains("STALE")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "ip-neighbor", "neighbor(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    fn make_table(n: usize) -> String {
        let hdr = "10.0.0.1 dev eth0 lladdr aa:bb:cc:dd:ee:ff REACHABLE\n";
        let rows: String = (0..n).map(|i| format!("item{i}  col2  col3  col4\n")).collect();
        format!("{hdr}{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_header() { assert!(detect("10.0.0.1 dev eth0 lladdr aa:bb:cc:dd:ee:ff REACHABLE")); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(3)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(55)).unwrap();
        assert!(out.contains("[tokenade:ip-neighbor]"));
        assert!(out.contains("(+5 more)"));
    }
    #[test] fn header_preserved() {
        let out = try_compact(&make_table(55)).unwrap();
        assert!(out.contains("lladdr"));
        assert!(out.contains("dev"));
    }
}
