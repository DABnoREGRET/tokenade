//! `fsck` / `e2fsck` / `fsck.ext4` — filesystem check.
//!
//! ```text
//! fsck from util-linux 2.38.1
//! e2fsck 1.47.0 (5-Feb-2023)
//! Pass 1: Checking inodes, blocks, and sizes
//! Pass 2: Checking directory structure
//! Pass 3: Checking directory connectivity
//! Pass 4: Checking reference counts
//! Pass 5: Checking group summary information
//! /dev/sda1: 12345/65536 files (0.0% non-contiguous), 234567/262144 blocks
//! ```
//!
//! Useful tail: the per-pass progress is noise; the final summary
//! line (`/dev/x: N/M files ... blocks`) is the actionable result.

use std::fmt::Write;

pub fn detect(text: &str) -> bool {
    text.contains("fsck from util-linux")
        || text.starts_with("e2fsck ")
        || (text.contains("Pass 1") && text.contains("Pass 5"))
        || text.contains("non-contiguous")
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut elided_passes = 0usize;
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("Pass ") && t.contains("Checking") {
            elided_passes += 1;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if elided_passes > 0 {
        let _ = writeln!(out, "  [elided: {elided_passes} pass progress line(s)]");
    }
    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_fsck() {
        let s = "fsck from util-linux 2.38.1\ne2fsck 1.47.0 (5-Feb-2023)\nPass 1: Checking inodes, blocks, and sizes\n";
        assert!(detect(s));
    }

    #[test]
    fn drops_pass_progress() {
        let s = "fsck from util-linux 2.38.1\nPass 1: Checking inodes, blocks, and sizes\nPass 2: Checking directory structure\nPass 5: Checking group summary information\n/dev/sda1: 12345/65536 files (0.0% non-contiguous), 234567/262144 blocks\n";
        let out = compact(s);
        assert!(!out.contains("Pass 1:"));
        assert!(out.contains("non-contiguous"));
        assert!(out.contains("elided"));
    }
}
