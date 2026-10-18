//! `apropos <term>` / `man -k <term>` — man page search.
//!
//! ```text
//! grep (1)             - print lines that match patterns
//! egrep (1)            - print lines that match patterns
//! pcregrep (1)         - a grep with Perl-compatible regular expressions
//! zgrep (1)            - search compressed files
//! ...
//! ```
//!
//! Compaction: cap long result lists (apropos with a common term
//! like `man` can return 200+ matches).


pub fn detect(text: &str) -> bool {
    let mut hits = 0usize;
    for line in text.lines().take(30) {
        // Shape: `name (section)<spaces>- description`. apropos pads
        // the section column to align descriptions, so there can be
        // many spaces between `)` and `-`.
        if let Some((before_paren, after)) = line.split_once('(') {
            if before_paren.is_empty() {
                continue;
            }
            let section: String = after.chars().take_while(|c| *c != ')').collect();
            if section.is_empty() || !section.chars().all(|c| c.is_ascii_alphanumeric()) {
                continue;
            }
            // After the close paren, the remainder must contain `-`
            // preceded only by whitespace.
            if let Some(rest) = after.split_once(')').map(|(_, r)| r) {
                let trimmed = rest.trim_start();
                if trimmed.starts_with('-') {
                    hits += 1;
                }
            }
        }
    }
    hits >= 3
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    crate::archetypes::head_lines_with_tally(text, 30, "match(es) elided")
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    #[test]
    fn detects_apropos() {
        let s = "grep (1)             - print lines that match patterns\negrep (1)            - print lines that match patterns\nfgrep (1)            - print lines that match patterns\nzgrep (1)            - search compressed files\n";
        assert!(detect(s));
    }

    #[test]
    fn folds_long_result_list() {
        // Past the delivery threshold; under it the list is delivered whole.
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        let mut s = String::new();
        for i in 0..60 {
            let _ = writeln!(s, "tool{i} (1)             - some description {i}");
        }
        let out = compact(&s);
        assert!(out.contains("more match"));
    }
}
