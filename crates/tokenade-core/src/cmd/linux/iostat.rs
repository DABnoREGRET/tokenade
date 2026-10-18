//! `iostat` / `iostat -x` — block-device I/O stats.
//!
//! ```text
//! Linux 6.8.0-1-amd64 (host)   2026-05-29   _x86_64_   (8 CPU)
//!
//! avg-cpu:  %user   %nice %system %iowait  %steal   %idle
//!            3.20    0.00    1.40    0.40    0.00   94.90
//!
//! Device             tps    kB_read/s    kB_wrtn/s    kB_read    kB_wrtn
//! sda              12.34       123.45       678.90    1234567    8765432
//! loop0             0.00         0.00         0.00          0          0
//! ...
//! ```
//!
//! Compaction: drop /dev/loop* lines (snap mounts, always 0).


pub fn detect(text: &str) -> bool {
    text.contains("avg-cpu:") && text.contains("%user") && text.contains("Device")
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    crate::archetypes::elide_prefixed(text, &["loop"], false, "loop device(s)")
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_iostat() {
        let s = "Linux 6.8.0 (host)\n\navg-cpu:  %user   %nice %system %iowait  %steal   %idle\n           3.20    0.00    1.40    0.40    0.00   94.90\n\nDevice             tps    kB_read/s    kB_wrtn/s\n";
        assert!(detect(s));
    }
}
