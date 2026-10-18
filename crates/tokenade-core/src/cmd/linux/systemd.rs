//! `systemd` — shared compactor for systemd-analyze subcommand output.
//!
//! systemd-analyze subcommands produce startup timing tables, config
//! dumps, and dependency chains. The shared compactor caps at
//! SYSTEMD_LINE_CAP non-empty lines.
//!
//! All generic `systemd_*` subcommand files delegate to this module.

const SYSTEMD_LINE_CAP: usize = 80;

pub fn detect(text: &str) -> bool {
    crate::archetypes::any_output(text)
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    Some(compact(text))
}

pub fn compact(text: &str) -> String {
    crate::archetypes::line_cap(text, SYSTEMD_LINE_CAP, "line(s)")
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    #[test]
    fn passthrough_short() {
        let s = "Startup finished in 1.234s (kernel) + 4.567s (userspace) = 5.801s\n";
        let out = compact(s);
        assert!(out.contains("Startup"), "short output must pass through");
    }

    #[test]
    fn caps_long() {
        let mut s = String::new();
        for i in 0..100 {
            let _ = writeln!(s, "{:>6}ms  service_{i:03}.service", i * 10);
        }
        let out = compact(&s);
        assert!(out.contains("more line(s) elided"), "long output must be capped");
    }

    #[test]
    fn rejects_empty() {
        assert!(!detect(""));
        assert!(!detect("  \n"));
    }
}
