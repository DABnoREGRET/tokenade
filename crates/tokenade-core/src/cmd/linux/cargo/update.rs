//! `cargo update` — `Updating <crate> from X.Y.Z to A.B.C`.

use crate::compress::whole_or_truncate_middle;
use crate::ansi::strip_ansi;

pub fn compact(output: &str) -> String {
    let s = strip_ansi(output);
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return "update: no changes".to_string();
    }
    whole_or_truncate_middle(&lines.join("\n"), 80, 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_message() {
        assert_eq!(compact(""), "update: no changes");
    }

    #[test]
    fn passes_updates() {
        let s = "    Updating serde v1.0.190 -> v1.0.193\n    Updating tokio v1.34.0 -> v1.35.0\n";
        let out = compact(s);
        assert!(out.contains("serde"));
        assert!(out.contains("tokio"));
    }
}
