//! `ps -ef` — all processes, System V format (`UID PID PPID C STIME …`).
//!
//! Delegates to the shared [`super::ps_table`] engine. This format has no
//! `%CPU`/`%MEM` columns, so the engine falls back to a header-preserving
//! head fold over the process list (column structure is locale-independent).
const HEAD: usize = 60;

pub fn detect(text: &str) -> bool {
    super::ps_table::detect(text)
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    super::ps_table::fold(text, HEAD, "ps-ef")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    fn make_table(n: usize) -> String {
        let mut t = String::from("UID PID PPID C STIME TTY TIME CMD\n");
        for i in 0..n {
            let _ = writeln!(t, "root {i} 0 0 juin10 ? 00:00:00 [kthread{i}]");
        }
        t
    }

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
    }

    #[test]
    fn detector_accepts_header() {
        assert!(detect(&make_table(2)));
    }

    #[test]
    fn short_table_passes_through() {
        assert!(try_compact(&make_table(3)).is_none());
    }

    #[test]
    fn long_table_folds() {
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        let out = try_compact(&make_table(65)).unwrap();
        assert!(out.contains("[tokenade:ps-ef]"));
        assert!(out.contains("more) ..."));
        assert!(out.contains("CMD"));
    }
}
