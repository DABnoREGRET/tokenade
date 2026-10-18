//! Extracted from `filter.rs` in the L92 refactor.
//!
//! Each function below was hand-tuned in filter.rs; the body is//! preserved verbatim — only the home changed.
use crate::compress::whole_or_truncate_middle;
use crate::ansi::strip_ansi;

pub fn compact(output: &str) -> String {
    let s = strip_ansi(output);
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return "empty reflog".to_string();
    }
    whole_or_truncate_middle(&lines.join("\n"), 40, 5)
}
