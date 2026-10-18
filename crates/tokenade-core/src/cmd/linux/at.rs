//! `at` / `atq` — scheduled job list.
//!
//! ```text
//! 1     Mon Jun  2 03:00:00 2026 a infected
//! 2     Tue Jun  3 04:00:00 2026 a infected
//! ```
//!
//! Output is one line per scheduled job. Cap at 30.

use std::fmt::Write;

pub fn detect(text: &str) -> bool {
    let mut hits = 0usize;
    for line in text.lines().take(30) {
        let toks: Vec<&str> = line.split_whitespace().collect();
        // Shape: <id> <date words> <user> — the first token is numeric.
        if toks.len() >= 7
            && toks[0].chars().all(|c| c.is_ascii_digit())
            && toks.iter().any(|t| *t == "a" || *t == "b" || *t == "c")
        {
            hits += 1;
        }
    }
    hits >= 2
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
        "  [... {} more job(s) elided]",
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
    fn detects_atq_output() {
        let s = "1\tMon Jun  2 03:00:00 2026 a infected\n2\tTue Jun  3 04:00:00 2026 a infected\n";
        assert!(detect(s));
    }
}
