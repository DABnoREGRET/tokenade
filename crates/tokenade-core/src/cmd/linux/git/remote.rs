//! Extracted from `filter.rs` in the L92 refactor.
//!
//! Each function below was hand-tuned in filter.rs; the body is//! preserved verbatim — only the home changed.
use crate::compress::dedup_lines;
use crate::ansi::strip_ansi;

pub fn dispatch(argv: &[String], output: &str) -> String {
    let sub = argv.get(2).map(String::as_str).unwrap_or("");
    match sub {
        "show" => {
            // Multi-section info — keep as-is, dedup.
            let s = strip_ansi(output);
            dedup_lines(&s)
        }
        "add" | "remove" | "rm" | "set-url" | "rename" => {
            let s = strip_ansi(output);
            s.trim().to_string()
        }
        _ => {
            // Default / -v: short list. Pass through.
            let s = strip_ansi(output);
            s.trim().to_string()
        }
    }
}
