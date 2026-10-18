//! `route -n` / `ip route` / `netstat -rn` — routing table.
//!
//! ```text
//! Destination   Gateway        Genmask         Flags Metric Ref    Use Iface
//! 0.0.0.0       192.168.1.1    0.0.0.0         UG    100    0        0 enp0s3
//! 169.254.0.0   0.0.0.0        255.255.0.0     U     1000   0        0 enp0s3
//! 192.168.1.0   0.0.0.0        255.255.255.0   U     100    0        0 enp0s3
//! ```
//!
//! Compaction: drop the `169.254/16` link-local autoconf route
//! (always present, always noise). Keep default + real routes.


pub fn detect(text: &str) -> bool {
    let lower = text.lines().next().unwrap_or("").to_ascii_lowercase();
    (lower.starts_with("destination") && lower.contains("gateway") && lower.contains("genmask"))
        || lower.starts_with("kernel ip routing table")
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    crate::archetypes::elide_prefixed(text, &["169.254."], false, "link-local route(s)")
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "Destination   Gateway        Genmask         Flags Metric Ref    Use Iface\n0.0.0.0       192.168.1.1    0.0.0.0         UG    100    0        0 enp0s3\n169.254.0.0   0.0.0.0        255.255.0.0     U     1000   0        0 enp0s3\n192.168.1.0   0.0.0.0        255.255.255.0   U     100    0        0 enp0s3\n";

    #[test]
    fn detects_route_output() {
        assert!(detect(SAMPLE));
    }

    #[test]
    fn drops_link_local() {
        let out = compact(SAMPLE);
        assert!(out.contains("0.0.0.0"));
        assert!(!out.contains("169.254."));
        assert!(out.contains("link-local"));
    }
}
