//! `traceroute` / `tracepath`.
//!
//! ```text
//! traceroute to example.com (93.184.216.34), 30 hops max, 60 byte packets
//!  1  _gateway (192.168.1.1)  0.234 ms  0.178 ms  0.195 ms
//!  2  10.0.0.1 (10.0.0.1)  3.421 ms  3.498 ms  3.398 ms
//!  3  * * *
//!  4  some.router (...)  ...
//!  ...
//! ```
//!
//! Compaction:
//!   * Drop unreachable `* * *` hops in middle of trace (keep first
//!     and last). They're noise from filtered routers.
//!   * Keep the header + every named hop.

use std::fmt::Write;

pub fn detect(text: &str) -> bool {
    let first = text.lines().next().unwrap_or("");
    first.starts_with("traceroute to ")
        || first.starts_with("tracepath to ")
        || first.starts_with("Tracing route to ")
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut star_run = 0usize;
    let mut out = String::with_capacity(text.len());
    let mut total_star_elided = 0usize;
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        let is_star = t.split_whitespace().filter(|x| *x == "*").nth(2).is_some();
        if is_star && i > 0 && i < lines.len() - 1 {
            star_run += 1;
            continue;
        }
        if star_run > 0 {
            total_star_elided += star_run;
            let _ = writeln!(out, "  [... {star_run} unreachable hop(s) elided ...]");
            star_run = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    let _ = total_star_elided;
    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"traceroute to example.com (93.184.216.34), 30 hops max, 60 byte packets
 1  _gateway (192.168.1.1)  0.234 ms  0.178 ms  0.195 ms
 2  10.0.0.1 (10.0.0.1)  3.421 ms  3.498 ms  3.398 ms
 3  * * *
 4  * * *
 5  * * *
 6  * * *
 7  some.router.example.net (192.0.2.1)  8.234 ms  8.178 ms  8.195 ms
 8  93.184.216.34 (93.184.216.34)  12.345 ms  12.298 ms  12.412 ms
"#;

    #[test]
    fn detects_traceroute() {
        assert!(detect(SAMPLE));
    }

    #[test]
    fn folds_unreachable_run() {
        let out = compact(SAMPLE);
        assert!(out.contains("_gateway"));
        assert!(out.contains("93.184.216.34"));
        assert!(out.contains("4 unreachable hop"));
    }

    #[test]
    fn rejects_non_traceroute() {
        assert!(!detect("hello world"));
    }
}
