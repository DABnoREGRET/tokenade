//! `ps axjf` — process tree (forest) format.
//!
//! Delegates to the shared [`super::ps_table`] engine: recognised by its
//! `PID`/`COMMAND` columns, head-folded (no `%CPU`/`%MEM` here). The tree
//! indentation lives inside the COMMAND field and is preserved verbatim in
//! the kept rows.
const HEAD: usize = 60;

pub fn detect(text: &str) -> bool {
    super::ps_table::detect(text)
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    super::ps_table::fold(text, HEAD, "ps-axjf")
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
    fn long_tree_folds() {
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        let mut t = String::from("PPID PID PGID SID TTY TPGID STAT UID TIME COMMAND\n");
        for i in 0..70 {
            let _ = writeln!(t, "0 {i} {i} {i} ? -1 S 0 0:00  \\_ proc{i}");
        }
        let out = try_compact(&t).unwrap();
        assert!(out.contains("[tokenade:ps-axjf]"));
        assert!(out.contains("COMMAND"));
    }
}
