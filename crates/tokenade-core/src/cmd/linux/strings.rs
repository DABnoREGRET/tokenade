//! `strings <binary>` — printable strings extracted from a binary.
//!
//! Often produces 1000s of lines. Compaction: drop sub-4-char
//! strings (almost always noise / random), keep the first 200
//! "interesting" ones.

use std::fmt::Write;

pub fn detect(_text: &str) -> bool {
    // Can't reliably detect strings(1) output from content alone
    // (it's just printable strings). Recognised by program name in
    // the named dispatch table only.
    false
}

pub fn try_compact(text: &str) -> Option<String> {
    Some(compact(text))
}

pub fn compact(text: &str) -> String {
    let interesting: Vec<&str> = text
        .lines()
        .filter(|l| {
            let len = l.chars().count();
            // Drop super-short and sub-printable lines.
            len >= 6 && l.chars().all(|c| !c.is_control() || c == '\t')
        })
        .collect();
    if interesting.len() <= 200 {
        return interesting.join("\n");
    }
    let mut out = String::with_capacity(text.len() / 2);
    for line in interesting.iter().take(200) {
        out.push_str(line);
        out.push('\n');
    }
    let _ = writeln!(
        out,
        "  [... {} more interesting string(s) elided]",
        interesting.len() - 200
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
    fn drops_short_lines() {
        let s = "ab\ncd\n12345\nMeaningful String 1\nMeaningful String 2\n";
        let out = compact(s);
        assert!(!out.contains("ab\n"));
        assert!(out.contains("Meaningful String"));
    }

    #[test]
    fn folds_very_long_input() {
        let mut s = String::new();
        for i in 0..500 {
            let _ = writeln!(s, "Meaningful String {i}");
        }
        let out = compact(&s);
        assert!(out.contains("more interesting string"));
    }
}
