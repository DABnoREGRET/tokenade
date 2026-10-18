//! `ps aux` — all processes, BSD format with `%CPU`/`%MEM`.
//!
//! Delegates to the shared [`super::ps_table`] engine, which surfaces the
//! processes actually burning CPU or memory ahead of the long tail of
//! idle kernel threads. The `%CPU`/`%MEM` columns are located by header
//! label (a ps invariant), so this is locale-independent.
const HEAD: usize = 60;

pub fn detect(text: &str) -> bool {
    super::ps_table::detect(text)
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    super::ps_table::fold(text, HEAD, "ps-aux")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    fn make_table(n: usize) -> String {
        let mut t = String::from("USER PID %CPU %MEM VSZ RSS TTY STAT START TIME COMMAND\n");
        for i in 0..n {
            let _ = writeln!(t, "root {i} 0.0 0.0 27048 15224 ? Ss juin10 0:06 [kthread{i}]");
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
    fn surfaces_busy_process() {
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        let mut t = make_table(65);
        t.push_str("alice 4242 87.5 12.3 9999 8888 pts/0 R 10:00 5:00 cargo build\n");
        let out = try_compact(&t).unwrap();
        assert!(out.contains("[tokenade:ps-aux]"));
        let hog = out.find("cargo build").unwrap();
        let fold = out.find("more) ...").unwrap();
        assert!(hog < fold);
        assert!(out.contains("COMMAND"));
    }
}
