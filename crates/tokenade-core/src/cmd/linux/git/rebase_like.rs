//! Extracted from `filter.rs` in the L92 refactor.
//!
//! Each function below was hand-tuned in filter.rs; the body is//! preserved verbatim — only the home changed.
use crate::compress::dedup_lines;
use crate::compress::truncate_middle;
use crate::ansi::strip_ansi;

pub fn compact(output: &str) -> String {
    let s = strip_ansi(output);
    let cleaned = dedup_lines(&s);
    truncate_middle(&cleaned, 50, 20)
}
