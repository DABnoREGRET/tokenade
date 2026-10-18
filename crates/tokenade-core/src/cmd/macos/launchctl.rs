//! macOS `launchctl list`.
//!
//! ```text
//! PID  Status  Label
//! 1234 0       com.apple.somed
//! -    0       com.apple.installerauthagent
//! 5678 -78     com.user.failedone
//! ...
//! ```
//!
//! Compaction: drop rows where Status is 0 AND PID is "-" (idle
//! daemon not currently running). Surface failed ones (negative
//! status code) at top.

use std::fmt::Write;

pub fn detect(text: &str) -> bool {
    text.contains("com.") || text.contains("system/")
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    let mut header: Option<&str> = None;
    let mut active: Vec<&str> = Vec::new();
    let mut failed: Vec<&str> = Vec::new();
    let mut idle: Vec<&str> = Vec::new();
    for line in text.lines() {
        if header.is_none() {
            header = Some(line);
            continue;
        }
        let mut toks = line.split_whitespace();
        let pid = toks.next().unwrap_or("-");
        let status = toks.next().unwrap_or("0");
        if pid == "-" && status == "0" {
            idle.push(toks.next().unwrap_or(""));
            continue;
        }
        if status.starts_with('-') || status.parse::<i64>().is_ok_and(|n| n != 0) {
            failed.push(line);
            continue;
        }
        active.push(line);
    }
    let mut out = String::new();
    if let Some(h) = header {
        out.push_str(h);
        out.push('\n');
    }
    if !failed.is_empty() {
        out.push_str("\n[failed]\n");
        for f in &failed {
            out.push_str(f);
            out.push('\n');
        }
    }
    if !active.is_empty() {
        out.push_str("\n[active]\n");
        for a in &active {
            out.push_str(a);
            out.push('\n');
        }
    }
    if !idle.is_empty() {
        // **A loaded job is named, even idle.** "Is X loaded?" is the
        // question `launchctl list` answers, and the label IS the answer;
        // their PID and status are the same on every one, said once.
        // Measured 2026-10-03 on a real `launchctl list` (macOS 15, 53 jobs,
        // 1.9 KB): 45 labels were a count. Named while that fits.
        let named = format!(
            "\n[idle: {} — PID -, Status 0] {}\n",
            idle.len(),
            idle.join(" · ")
        );
        if tk_base::delivery::fits(&format!("{out}{named}")) {
            out.push_str(&named);
        } else {
            let _ = write!(out, "\n  [elided: {} idle daemon(s) (PID=-, Status=0)]\n", idle.len());
        }
    }
    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "PID\tStatus\tLabel\n1234\t0\tcom.apple.somed\n-\t0\tcom.apple.installerauthagent\n5678\t-78\tcom.user.failedone\n-\t0\tcom.apple.idle1\n-\t0\tcom.apple.idle2\n9012\t0\tcom.user.active1\n";

    #[test]
    fn detects_launchctl() {
        assert!(detect(SAMPLE));
    }

    #[test]
    fn buckets_failed_active_idle() {
        let out = compact(SAMPLE);
        assert!(out.contains("[failed]"));
        assert!(out.contains("com.user.failedone"));
        assert!(out.contains("[active]"));
        assert!(out.contains("com.apple.somed"));
        // Idle jobs are named, their shared PID/status said once.
        assert!(out.ends_with("[idle: 3 — PID -, Status 0] com.apple.installerauthagent · com.apple.idle1 · com.apple.idle2"), "{out}");
    }
}
