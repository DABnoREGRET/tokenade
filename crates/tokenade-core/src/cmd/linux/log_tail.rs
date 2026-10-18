//! `log_tail` — shared log-tail archetype.
//!
//! Used by: journalctl, docker logs, kubectl logs, syslog, gcloud logging, etc.
//!
//! Strategy: show the LAST `tail_lines` lines (most recent = most useful for
//! debugging) with `head_ctx` lines of startup context at the top. This is
//! qualitatively different from `git_listing::fold` which shows the OLDEST
//! lines first — exactly the opposite of what an agent needs when diagnosing
//! a live service.

use std::fmt::Write;
/// Fold a log stream: keep the first `head_ctx` lines and the last
/// `tail_lines` lines, omit the middle.
/// Returns `None` when total ≤ head_ctx + tail_lines (nothing to fold).
pub fn tail(text: &str, head_ctx: usize, tail_lines: usize, label: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines()
        .map(|l| l.trim_end())
        .filter(|l| !l.is_empty())
        .collect();
    let total = lines.len();
    if total <= head_ctx + tail_lines {
        return None;
    }
    let omitted = total - head_ctx - tail_lines;
    let mut out = format!("[tokenade:{label}] {total} lines (most recent last)\n");
    for l in &lines[..head_ctx] {
        out.push_str(l);
        out.push('\n');
    }
    let _ = writeln!(out, "  ... ({omitted} lines omitted) ...");
    let tail_start = total - tail_lines;
    for l in &lines[tail_start..] {
        out.push_str(l);
        out.push('\n');
    }
    Some(out.trim_end().to_string())
}

// Required by the auto-dispatch table — helper module, not a command.
crate::archetypes::cmd_helper_module!();

#[cfg(test)]
mod tests {
    use super::*;

    fn make_log(n: usize) -> String {
        (0..n).map(|i| format!("2026-06-08T09:{:02}:{:02}Z INFO event {i}\n", i / 60 % 60, i % 60)).collect()
    }

    #[test]
    fn returns_none_when_short() {
        assert!(tail(&make_log(20), 5, 25, "test").is_none());
        assert!(tail(&make_log(30), 5, 25, "test").is_none()); // exactly at threshold
    }

    #[test]
    fn shows_head_and_tail_when_long() {
        let t = make_log(100);
        let out = tail(&t, 5, 25, "test-log").unwrap();
        assert!(out.contains("[tokenade:test-log] 100 lines"));
        assert!(out.contains("event 0"));  // first head_ctx line
        assert!(out.contains("event 4"));  // last head_ctx line
        assert!(out.contains("event 99")); // last line of tail
        assert!(out.contains("event 75")); // first line of tail
    }

    #[test]
    fn middle_lines_are_omitted() {
        let t = make_log(100);
        let out = tail(&t, 5, 25, "test").unwrap();
        assert!(!out.contains("event 30")); // middle — should be omitted
        assert!(out.contains("(70 lines omitted)"));
    }

    #[test]
    fn tail_shows_most_recent() {
        let t = make_log(60);
        let out = tail(&t, 5, 20, "test").unwrap();
        assert!(out.contains("event 59")); // very last
        assert!(out.contains("event 40")); // first of tail window
        assert!(!out.contains("event 20")); // middle, omitted
    }
}
