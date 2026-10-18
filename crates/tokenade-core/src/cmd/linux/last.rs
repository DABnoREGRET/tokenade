//! `last` / `lastlog` / `lastb` — login history.
//!
//! ```text
//! infected pts/0      :0               Mon May 29 09:15   still logged in
//! infected pts/1      :0               Mon May 29 09:30 - 09:45  (00:15)
//! reboot   system boot 5.15.0-1234     Mon May 29 08:00   still running
//! wtmp begins Mon Apr  1 00:00:00 2024
//! ```
//!
//! Compaction: keep top 30 entries + summary count.

use std::fmt::Write;

pub fn detect(text: &str) -> bool {
    let mut data = 0usize;
    for line in text.lines().take(20) {
        if (line.contains("pts/") || line.contains("tty") || line.contains("system boot"))
            && (line.contains("still logged in")
                || line.contains("still running")
                || line.contains(" - "))
        {
            data += 1;
        }
    }
    data >= 2 || text.contains("wtmp begins") || text.contains("btmp begins")
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() <= 30 {
        return text.to_string();
    }
    let mut out = String::new();
    for l in lines.iter().take(30) {
        out.push_str(l);
        out.push('\n');
    }
    let _ = writeln!(
        out,
        "  [... {} more session(s) elided]",
        lines.len() - 30
    );
    // wtmp/btmp footer always preserved if present.
    if let Some(footer) = lines.iter().find(|l| l.contains("wtmp begins") || l.contains("btmp begins")) {
        if !out.contains(footer) {
            out.push_str(footer);
            out.push('\n');
        }
    }
    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_last_output() {
        let s = "infected pts/0      :0               Mon May 29 09:15   still logged in\nreboot   system boot 5.15.0-1234     Mon May 29 08:00   still running\n";
        assert!(detect(s));
    }

    #[test]
    fn folds_long_history() {
        let mut s = String::new();
        for i in 0..50 {
            let _ = writeln!(s, "user_{i} pts/0 :0 Mon May 29 09:00 - 09:45  (00:45)");
        }
        s.push_str("wtmp begins Mon Apr  1 00:00:00 2024\n");
        let out = compact(&s);
        assert!(out.contains("more session"));
        assert!(out.contains("wtmp begins"));
    }

    #[test]
    fn rejects_random() {
        assert!(!detect("hello world"));
    }
}
