//! `ps -o …` — user-selected columns.
//!
//! Delegates to the shared [`super::ps_table`] engine when the chosen
//! columns include the recognisable `PID` + `COMMAND`/`CMD` shape (the
//! common case); arbitrary custom column sets that lack them fall through
//! to passthrough rather than being folded by a generic table heuristic
//! that might misread unknown columns. Locale-independent.
const HEAD: usize = 60;

pub fn detect(text: &str) -> bool {
    super::ps_table::detect(text)
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    super::ps_table::fold(text, HEAD, "ps-o")
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
    fn custom_columns_without_pid_pass_through() {
        // `ps -o user,pcpu` with no PID/COMMAND → not folded.
        let mut t = String::from("USER %CPU\n");
        for i in 0..70 {
            let _ = writeln!(t, "user{i} 0.0");
        }
        assert!(try_compact(&t).is_none());
    }

    #[test]
    fn pid_command_columns_fold() {
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        let mut t = String::from("PID COMMAND\n");
        for i in 0..70 {
            let _ = writeln!(t, "{i} cmd{i}");
        }
        let out = try_compact(&t).unwrap();
        assert!(out.contains("[tokenade:ps-o]"));
    }
}
