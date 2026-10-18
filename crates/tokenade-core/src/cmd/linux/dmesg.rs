//! `dmesg` — kernel ring buffer.
//!
//! ```text
//! [    0.000000] Linux version 5.15.0-1234
//! [    0.001234] Command line: BOOT_IMAGE=/boot/vmlinuz...
//! [    0.002345] x86/fpu: Supporting XSAVE feature
//! [    1.234567] CPU0: Thermal monitoring enabled
//! [   42.123456] sd 0:0:0:0: [sda] Attached SCSI disk
//! ...
//! ```
//!
//! Compaction: strip `[N.NNNNNN]` timestamp prefix (saves ~14 chars
//! per line), fold consecutive identical messages (`(×N)`), keep
//! the first warning/error verbatim then summarise.


pub fn detect(text: &str) -> bool {
    let mut bracket_lines = 0usize;
    for line in text.lines().take(60) {
        let t = line.trim_start();
        if let Some(after) = t.strip_prefix('[') {
            // shape `\d+\.\d+]` then space
            if after.chars().take_while(|c| *c != ']').all(|c| c.is_ascii_digit() || c == '.' || c == ' ') {
                bracket_lines += 1;
            }
        }
    }
    bracket_lines >= 10
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    // Strip the bracket timestamp prefix from each line, then fold
    // consecutive duplicates.
    let stripped: Vec<&str> = text
        .lines()
        .map(strip_dmesg_ts)
        .filter(|l| !l.is_empty())
        .collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < stripped.len() {
        let line = stripped[i];
        let mut j = i + 1;
        while j < stripped.len() && stripped[j] == line {
            j += 1;
        }
        let n = j - i;
        if n == 1 {
            out.push(line.to_string());
        } else {
            crate::fold_count::note_folded(n);
            out.push(format!("{line}  (×{n})"));
        }
        i = j;
    }
    out.join("\n")
}

fn strip_dmesg_ts(line: &str) -> &str {
    let t = line.trim_start();
    if !t.starts_with('[') {
        return line;
    }
    if let Some(close) = t.find(']') {
        let rest = &t[close + 1..];
        rest.trim_start_matches(' ')
    } else {
        line
    }
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    #[test]
    fn detects_dmesg() {
        let mut s = String::new();
        for i in 0..15 {
            let _ = writeln!(s, "[   {i:>3}.000000] kernel message {i}");
        }
        assert!(detect(&s));
    }

    #[test]
    fn strips_timestamps_and_folds_repeats() {
        let s = "[    0.000000] message\n[    0.001234] message\n[    0.002345] message\n[   42.123456] different\n";
        let out = compact(s);
        assert!(out.contains("(×3)"));
        assert!(out.contains("different"));
        assert!(!out.contains("0.000000"));
    }

    #[test]
    fn rejects_random() {
        assert!(!detect("hello world"));
    }
}
