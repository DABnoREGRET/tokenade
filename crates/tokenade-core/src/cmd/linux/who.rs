//! `who` / `w` — logged-in users.
//!
//! `who`:
//! ```text
//! infected tty1         2026-05-29 09:00 (:0)
//! infected pts/0        2026-05-29 09:15 (:0)
//! infected pts/1        2026-05-29 09:30 (:0)
//! ```
//!
//! `w`:
//! ```text
//!  16:42:18 up  3:14, 2 users, load average: 0.50, 0.42, 0.38
//! USER     TTY      FROM             LOGIN@   IDLE   JCPU   PCPU WHAT
//! root     pts/0    -                09:00    7:42m  0.04s  0.04s -bash
//! root     pts/1    -                09:30    0.00s  0.04s  0.04s w
//! ```
//!
//! Compaction: keep header / first 30 user lines + summary count.

use std::fmt::Write;

pub fn detect(text: &str) -> bool {
    crate::archetypes::any_output(text)
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
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
    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_who_output() {
        let s = "infected tty1         2026-05-29 09:00 (:0)\ninfected pts/0        2026-05-29 09:15 (:0)\ninfected pts/1        2026-05-29 09:30 (:0)\n";
        assert!(detect(s));
    }

    #[test]
    fn detects_w_output() {
        let s = " 16:42:18 up  3:14, 2 users, load average: 0.50, 0.42, 0.38\nUSER     TTY      FROM             LOGIN@   IDLE   JCPU   PCPU WHAT\n";
        assert!(detect(s));
    }

    #[test]
    fn keeps_short_session_lists() {
        let s = "infected pts/0  ...\ninfected pts/1  ...\n";
        let out = compact(s);
        assert_eq!(out, s);
    }

    #[test]
    fn folds_many_sessions() {
        let mut s = String::new();
        for i in 0..50 {
            let _ = writeln!(s, "user_{i} pts/{i} 2026-05-29 09:00 (:0)");
        }
        let out = compact(&s);
        assert!(out.contains("more session"));
    }
}
