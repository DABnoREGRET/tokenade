//! `kubectl get -o name` — one `<kind>/<name>` per line.
//!
//! Cap long listings at 80 entries + summary tail. Below the cap,
//! pass through verbatim — the user wants the literal names.

use crate::ansi::strip_ansi;

pub fn compact(output: &str) -> String {
    let s = strip_ansi(output);
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() <= 100 {
        return lines.join("\n");
    }
    format!(
        "{}\n… [{} more] …\n{} resources total",
        lines[..80].join("\n"),
        lines.len() - 80,
        lines.len()
    )
}

pub fn try_compact(text: &str) -> Option<String> {
    if text.trim().is_empty() {
        return None;
    }
    Some(compact(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    #[test]
    fn passthrough_short() {
        let s = "pod/foo\npod/bar\npod/baz\n";
        assert_eq!(compact(s), "pod/foo\npod/bar\npod/baz");
    }

    #[test]
    fn caps_long_list() {
        let mut s = String::new();
        for i in 0..150 {
            let _ = writeln!(s, "pod/pod-{i}");
        }
        let out = compact(&s);
        assert!(out.contains("150 resources total"));
        assert!(out.contains("70 more"));
    }
}
