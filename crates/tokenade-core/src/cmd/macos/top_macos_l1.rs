//! `top_macos_l1` — top macos l1.
const HEAD: usize = 60;


pub fn detect(text: &str) -> bool {
    text.contains("PID") && text.contains("COMMAND")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    crate::table_fold::fold_table(text, HEAD, "top-macos-l1", "line(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    fn make_table(n: usize) -> String {
        let hdr = "PID   COMMAND   %CPU   TIME   #TH   #WQ   #PORT   MEM   PURG   CMPRS\n";
        let rows: String = (0..n).map(|i| format!("item{i}  col2  col3  col4\n")).collect();
        format!("{hdr}{rows}")
    }
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_header() { assert!(detect("PID   COMMAND   %CPU   TIME   #TH   #WQ   #PORT   MEM   PURG   CMPRS")); }
    #[test] fn short_table_passes_through() { assert!(try_compact(&make_table(3)).is_none()); }
    #[test] fn long_table_folds() {
        let out = try_compact(&make_table(65)).unwrap();
        assert!(out.contains("[tokenade:top-macos-l1]"));
        assert!(out.contains("(+5 more)"));
    }
    #[test] fn header_preserved() {
        let out = try_compact(&make_table(65)).unwrap();
        assert!(out.contains("PID"));
        assert!(out.contains("MEM"));
    }
}
