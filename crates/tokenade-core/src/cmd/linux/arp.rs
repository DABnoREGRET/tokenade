//! `arp -a` / `arp -n` / `ip neigh` — ARP / neighbour table.
//!
//! ```text
//! ? (192.168.1.1) at aa:bb:cc:dd:ee:ff [ether] on enp0s3
//! ? (192.168.1.42) at 11:22:33:44:55:66 [ether] on enp0s3
//! (incomplete) (192.168.1.50) at <incomplete> on enp0s3
//! ```
//!
//! Compaction: drop `(incomplete)` rows (failed ARP requests, almost
//! always transient noise).

use std::fmt::Write;

pub fn detect(text: &str) -> bool {
    let mut hits = 0usize;
    for line in text.lines().take(40) {
        if (line.contains(" at ") && line.contains("[ether]"))
            || (line.contains("HWaddress") || line.contains("lladdr"))
        {
            hits += 1;
        }
    }
    hits >= 2
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut elided = 0usize;
    for line in text.lines() {
        if line.contains("(incomplete)") || line.contains("<incomplete>") {
            elided += 1;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if elided > 0 {
        let _ = writeln!(out, "  [elided: {elided} incomplete ARP entry/entries]");
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
    fn detects_arp_output() {
        let s = "? (192.168.1.1) at aa:bb:cc:dd:ee:ff [ether] on enp0s3\n? (192.168.1.42) at 11:22:33:44:55:66 [ether] on enp0s3\n";
        assert!(detect(s));
    }

    #[test]
    fn drops_incomplete() {
        let s = "? (192.168.1.1) at aa:bb:cc:dd:ee:ff [ether] on enp0s3\n? (192.168.1.42) at 11:22:33:44:55:66 [ether] on enp0s3\n(incomplete) (192.168.1.50) at <incomplete> on enp0s3\n";
        let out = compact(s);
        assert!(out.contains("192.168.1.1"));
        assert!(!out.contains("(incomplete)"));
        assert!(out.contains("1 incomplete"));
    }
}
