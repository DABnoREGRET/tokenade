//! `cargo search <term>` — `name = "X.Y.Z" # description`.

use crate::compress::whole_or_truncate_middle;
use crate::ansi::strip_ansi;

pub fn compact(output: &str) -> String {
    let s = strip_ansi(output);
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return "no matches".to_string();
    }
    whole_or_truncate_middle(&lines.join("\n"), 30, 5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_means_no_matches() {
        assert_eq!(compact(""), "no matches");
    }

    #[test]
    fn passes_results() {
        let s = "serde = \"1.0\"   # A generic serialization/deserialization framework\nserde_json = \"1.0\" # A JSON serialization file format\n";
        let out = compact(s);
        assert!(out.contains("serde"));
    }
}
