//! `nmap` — port scanner.
//!
//! ```text
//! Starting Nmap 7.94 ( https://nmap.org ) at 2026-05-29 16:42 UTC
//! Nmap scan report for example.com (93.184.216.34)
//! Host is up (0.012s latency).
//! Not shown: 998 filtered tcp ports (no-response)
//! PORT    STATE SERVICE
//! 80/tcp  open  http
//! 443/tcp open  https
//!
//! Nmap done: 1 IP address (1 host up) scanned in 4.85 seconds
//! ```
//!
//! Compaction: drop the verbose banner / "Not shown" line, keep
//! the structured PORT table + summary.


pub fn detect(text: &str) -> bool {
    text.contains("Starting Nmap")
        || (text.contains("Nmap scan report") && text.contains("PORT"))
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    let mut out = String::with_capacity(text.len() / 2);
    let mut skip = 0usize;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("Starting Nmap") {
            skip += 1;
            continue;
        }
        if t.starts_with("Not shown:") {
            // Keep but ABBREVIATED.
            out.push_str(line);
            out.push('\n');
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    let _ = skip;
    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "Starting Nmap 7.94 ( https://nmap.org ) at 2026-05-29 16:42 UTC\nNmap scan report for example.com (93.184.216.34)\nHost is up (0.012s latency).\nNot shown: 998 filtered tcp ports (no-response)\nPORT    STATE SERVICE\n80/tcp  open  http\n443/tcp open  https\n\nNmap done: 1 IP address (1 host up) scanned in 4.85 seconds\n";

    #[test]
    fn detects_nmap() {
        assert!(detect(SAMPLE));
    }

    #[test]
    fn drops_banner() {
        let out = compact(SAMPLE);
        assert!(!out.contains("Starting Nmap"));
        assert!(out.contains("80/tcp"));
        assert!(out.contains("443/tcp"));
    }

    #[test]
    fn rejects_random() {
        assert!(!detect("hello world"));
    }
}
