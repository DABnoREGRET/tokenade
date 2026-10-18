//! `ps` (command-column variant) — delegates to the shared
//! [`super::ps_table`] engine (PID/COMMAND recognition, `%CPU`/`%MEM`
//! surfacing when present, head fold otherwise). Locale-independent.
const HEAD: usize = 40;

pub fn detect(text: &str) -> bool {
    super::ps_table::detect(text)
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    super::ps_table::fold(text, HEAD, "ps-cmd")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
    }

    #[test]
    fn long_listing_folds() {
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        let mut t = String::from("USER PID %CPU %MEM VSZ RSS TTY STAT START TIME COMMAND\n");
        for i in 0..50 {
            let _ = writeln!(t, "root {i} 0.0 0.0 100 200 ? S 10:00 0:00 cmd{i}");
        }
        let out = try_compact(&t).unwrap();
        assert!(out.contains("[tokenade:ps-cmd]"));
        assert!(out.contains("COMMAND"));
    }
}
