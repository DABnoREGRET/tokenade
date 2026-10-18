//! `tcpdump` / packet-capture text-output compaction.
//!
//! `tcpdump -nn -i any` and similar capture tools emit one line per
//! packet:
//!
//! ```text
//! 10:30:00.123456 IP 192.168.1.1.443 > 10.0.0.5.34567: Flags [S.], seq 12345, ack 67890, win 65535, length 0
//! 10:30:00.234567 IP 10.0.0.5.34567 > 192.168.1.1.443: Flags [.], ack 12346, win 65535, length 0
//! 10:30:00.345678 IP 10.0.0.5.34567 > 192.168.1.1.443: Flags [P.], seq 1:101, ack 1, win 65535, length 100
//! ```
//!
//! A 1-minute capture on a busy interface = 10 000 – 1 000 000 lines.
//! The agent reading this wants:
//!
//!   - **flow summary** (per-(srcip:port ↔ dstip:port) packet + byte counts)
//!   - **flag distribution** (SYN, SYN-ACK, FIN, RST, etc.)
//!   - **protocol mix** (IP/IP6/ARP/ICMP)
//!   - **first/last timestamps** for the capture window
//!
//! Drops: the per-packet detail (sequence numbers, ack numbers, window
//! sizes) for non-flag-only packets.

use std::collections::BTreeMap;
use std::fmt::Write;

pub const FLOWS_CAP: usize = 50;

pub fn detect(text: &str) -> bool {
    let head: &str = text.get(..text.len().min(8192)).unwrap_or(text);
    let mut packet_lines = 0usize;
    let mut total_nonblank = 0usize;
    for line in head.lines().take(60) {
        if line.trim().is_empty() {
            continue;
        }
        total_nonblank += 1;
        if is_tcpdump_line(line) {
            packet_lines += 1;
        }
    }
    packet_lines >= 5 && total_nonblank > 0 && (packet_lines * 100) / total_nonblank >= 60
}

/// A line is tcpdump-shaped iff it has a `HH:MM:SS.NNNNNN` timestamp
/// prefix AND contains one of the protocol markers (`IP`, `IP6`, `ARP,`).
fn is_tcpdump_line(line: &str) -> bool {
    let t = line.trim_start();
    let bytes = t.as_bytes();
    if bytes.len() < 18 {
        return false;
    }
    // Position layout for `HH:MM:SS.NNN…`:
    //   0-1: HH  2: ':'  3-4: MM  5: ':'  6-7: SS  8: '.'
    if !(bytes[2] == b':' && bytes[5] == b':' && bytes[8] == b'.') {
        return false;
    }
    if !(bytes[0].is_ascii_digit() && bytes[1].is_ascii_digit()) {
        return false;
    }
    // Must contain a protocol marker further in the line.
    t.contains(" IP ") || t.contains(" IP6 ") || t.contains(" ARP, ") || t.contains(" ARP ")
}

#[derive(Default)]
struct FlowStats {
    packets: usize,
    bytes: u64,
    flags_seen: BTreeMap<String, usize>,
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum Proto {
    Ip,
    Ip6,
    Arp,
    Other,
}

impl Proto {
    fn as_str(self) -> &'static str {
        match self {
            Proto::Ip => "IP",
            Proto::Ip6 => "IP6",
            Proto::Arp => "ARP",
            Proto::Other => "?",
        }
    }
}

/// Returns the timestamp prefix (first 15 chars, `HH:MM:SS.NNNNNN`).
fn extract_timestamp(line: &str) -> Option<&str> {
    let t = line.trim_start();
    if t.len() < 15 {
        return None;
    }
    // Walk forward until we hit a space.
    let space_idx = t.find(' ')?;
    Some(&t[..space_idx])
}

/// Parse a tcpdump line and return (proto, src_endpoint, dst_endpoint,
/// flags, length).
fn parse_packet(line: &str) -> Option<(Proto, String, String, String, u64)> {
    let t = line.trim_start();

    // Strip the timestamp prefix.
    let space = t.find(' ')?;
    let rest = t[space + 1..].trim_start();

    // Detect protocol.
    let (proto, after_proto) = if let Some(r) = rest.strip_prefix("IP ") {
        (Proto::Ip, r)
    } else if let Some(r) = rest.strip_prefix("IP6 ") {
        (Proto::Ip6, r)
    } else if let Some(r) = rest.strip_prefix("ARP, ") {
        (Proto::Arp, r)
    } else if let Some(r) = rest.strip_prefix("ARP ") {
        (Proto::Arp, r)
    } else {
        return Some((Proto::Other, String::new(), String::new(), String::new(), 0));
    };

    // For ARP we don't track flows; bail with proto-only.
    if proto == Proto::Arp {
        return Some((proto, String::new(), String::new(), String::new(), 0));
    }

    // Endpoint pattern: `src.port > dst.port:` (IPv4) or `src.port > dst.port:` (IPv6 uses `[addr]:port`).
    let arrow_idx = after_proto.find(" > ")?;
    let src = after_proto[..arrow_idx].trim().to_string();
    let after_arrow = &after_proto[arrow_idx + 3..];
    let colon_idx = after_arrow.find(':')?;
    let dst = after_arrow[..colon_idx].trim().to_string();
    let after_dst = after_arrow[colon_idx + 1..].trim_start();

    // Flags between brackets after `Flags `.
    let mut flags = String::new();
    if let Some(idx) = after_dst.find("Flags [") {
        let after_flags = &after_dst[idx + "Flags [".len()..];
        if let Some(end) = after_flags.find(']') {
            flags = after_flags[..end].to_string();
        }
    }

    // Length: `length N`.
    let mut length: u64 = 0;
    if let Some(idx) = after_dst.find("length ") {
        let after = &after_dst[idx + "length ".len()..];
        let n: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        length = n.parse().unwrap_or(0);
    }

    Some((proto, src, dst, flags, length))
}

/// Canonicalise (src, dst) into a stable flow key by sorting alphabetically
/// so packets in BOTH directions of a TCP connection land in the same flow.
fn flow_key(src: &str, dst: &str) -> String {
    if src < dst {
        format!("{src} <-> {dst}")
    } else {
        format!("{dst} <-> {src}")
    }
}

pub fn compact(text: &str) -> String {
    if !detect(text) {
        return text.to_string();
    }

    let mut flows: BTreeMap<String, FlowStats> = BTreeMap::new();
    let mut by_proto: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut by_flag: BTreeMap<String, usize> = BTreeMap::new();
    let mut total_packets = 0usize;
    let mut total_bytes: u64 = 0;
    let mut first_ts: Option<String> = None;
    let mut unmatched: Vec<String> = Vec::new();
    let mut banners: Vec<String> = Vec::new();
    let mut last_ts: Option<String> = None;
    for raw in text.lines() {
        if raw.trim().is_empty() {
            continue;
        }
        if !is_tcpdump_line(raw) {
            // stderr banners ARE tcpdump noise; anything else is another
            // command's output — keep it.
            let t = raw.trim();
            if !(t.starts_with("tcpdump:")
                || t.starts_with("listening on")
                || t.ends_with("packets captured")
                || t.ends_with("packets received by filter")
                || t.ends_with("packets dropped by kernel"))
            {
                unmatched.push(raw.to_string());
            } else {
                banners.push(raw.to_string());
            }
            continue;
        }

        let Some(ts) = extract_timestamp(raw) else {
            continue;
        };
        if first_ts.is_none() {
            first_ts = Some(ts.to_string());
        }
        last_ts = Some(ts.to_string());

        let Some((proto, src, dst, flags, length)) = parse_packet(raw) else {
            continue;
        };
        total_packets += 1;
        total_bytes = total_bytes.saturating_add(length);
        *by_proto.entry(proto.as_str()).or_insert(0) += 1;
        if !flags.is_empty() {
            *by_flag.entry(flags.clone()).or_insert(0) += 1;
        }

        if proto == Proto::Ip || proto == Proto::Ip6 {
            let key = flow_key(&src, &dst);
            let stats = flows.entry(key).or_default();
            stats.packets += 1;
            stats.bytes = stats.bytes.saturating_add(length);
            if !flags.is_empty() {
                *stats.flags_seen.entry(flags).or_insert(0) += 1;
            }
        }
    }

    if total_packets == 0 {
        return text.to_string();
    }

    let n_flows = flows.len();
    let mut out = String::with_capacity(2048);
    let _ = writeln!(out, "{total_packets} packet(s), {n_flows} flow(s)");
    if let (Some(first), Some(last)) = (first_ts.as_deref(), last_ts.as_deref()) {
        let _ = writeln!(out, "time range: {first} → {last}");
    }

    // Protocol mix.
    if !by_proto.is_empty() {
        let mut p: Vec<_> = by_proto.iter().collect();
        p.sort_by(|a, b| b.1.cmp(a.1));
        out.push_str("\nprotocols:\n");
        for (proto, count) in &p {
            let _ = writeln!(out, "  {proto:<6} {count}");
        }
    }

    // Top flows by packet count.
    let mut flows_sorted: Vec<_> = flows.iter().collect();
    flows_sorted.sort_by(|a, b| b.1.packets.cmp(&a.1.packets).then(a.0.cmp(b.0)));

    if !flows_sorted.is_empty() {
        let _ = write!(
            out,
            "\ntop flows by packet count (showing {} of {}):\n",
            flows_sorted.len().min(FLOWS_CAP),
            n_flows
        );
        for (key, stats) in flows_sorted.iter().take(FLOWS_CAP) {
            let flags_str = if stats.flags_seen.is_empty() {
                String::new()
            } else {
                let mut fs: Vec<&str> = stats.flags_seen.keys().map(String::as_str).collect();
                fs.sort();
                format!("  flags: [{}]", fs.join(", "))
            };
            let _ = writeln!(
                out,
                "  {:<50} {} pkt(s), {} byte(s){}",
                key, stats.packets, stats.bytes, flags_str
            );
        }
        if n_flows > FLOWS_CAP {
            let _ = writeln!(out, "  … {} more flow(s) elided", n_flows - FLOWS_CAP);
        }
    }

    // Flag distribution.
    if !by_flag.is_empty() {
        let mut f: Vec<_> = by_flag.iter().collect();
        f.sort_by(|a, b| b.1.cmp(a.1));
        out.push_str("\nflag distribution:\n");
        for (flag, count) in f.iter().take(20) {
            let _ = writeln!(out, "  [{flag:<6}] {count}");
        }
    }

    for b in &banners {
        out.push_str(b);
        out.push('\n');
    }
    if !unmatched.is_empty() {
        out.push_str(&crate::auto::compact_remainder(&unmatched.join("\n")));
        out.push('\n');
    }
    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "10:30:00.123456 IP 192.168.1.1.443 > 10.0.0.5.34567: Flags [S.], seq 12345, ack 67890, win 65535, length 0
10:30:00.234567 IP 10.0.0.5.34567 > 192.168.1.1.443: Flags [.], ack 12346, win 65535, length 0
10:30:00.345678 IP 10.0.0.5.34567 > 192.168.1.1.443: Flags [P.], seq 1:101, ack 1, win 65535, length 100
10:30:00.456789 IP 192.168.1.1.443 > 10.0.0.5.34567: Flags [.], ack 101, win 65535, length 0
10:30:00.567890 IP 192.168.1.1.443 > 10.0.0.5.34567: Flags [P.], seq 1:50, ack 101, win 65535, length 50
10:30:00.678901 IP 192.168.1.2.80 > 10.0.0.6.55555: Flags [S.], seq 99, ack 100, win 65535, length 0
10:30:01.000000 ARP, Request who-has 10.0.0.1 tell 10.0.0.5, length 28
10:30:01.111111 ARP, Reply 10.0.0.1 is-at aa:bb:cc:dd:ee:ff, length 28
";

    #[test]
    fn detects_tcpdump() {
        assert!(detect(SAMPLE));
    }

    #[test]
    fn rejects_random_text() {
        assert!(!detect(
            "Some prose about IPs and ports but no tcpdump shape.\n"
        ));
    }

    #[test]
    fn rejects_logs() {
        let s = "2024-01-15 10:30:00 INFO server started\n2024-01-15 10:30:01 INFO request handled\n2024-01-15 10:30:02 INFO done\n";
        assert!(!detect(s));
    }

    #[test]
    fn extracts_flow_aggregation() {
        let out = compact(SAMPLE);
        assert!(out.contains("8 packet(s)"));
        // 2 IP flows + ARP packets (which we don't count as flows).
        assert!(out.contains("flow(s)"));
        // Time range.
        assert!(out.contains("10:30:00.123456 → 10:30:01.111111"));
        // Protocol breakdown.
        assert!(out.contains("IP"));
        assert!(out.contains("ARP"));
        // Flow keys are canonicalised: srcip:port <-> dstip:port.
        assert!(out.contains("10.0.0.5.34567"));
        assert!(out.contains("192.168.1.1.443"));
        // Flag distribution.
        assert!(out.contains("flag distribution"));
    }

    #[test]
    fn handles_ipv6() {
        let s = "10:30:00.123456 IP6 2001:db8::1.443 > 2001:db8::ff.55555: Flags [S.], length 0
10:30:00.234567 IP6 2001:db8::ff.55555 > 2001:db8::1.443: Flags [.], length 0
10:30:00.345678 IP6 2001:db8::ff.55555 > 2001:db8::1.443: Flags [P.], length 200
10:30:00.456789 IP6 fe80::1%eth0.8080 > fe80::99%eth0.33333: Flags [S.], length 0
10:30:00.567890 IP6 fe80::99%eth0.33333 > fe80::1%eth0.8080: Flags [.], length 0
";
        assert!(detect(s));
        let out = compact(s);
        assert!(out.contains("5 packet(s)"));
        assert!(out.contains("IP6"));
        // IPv6 addresses preserved.
        assert!(out.contains("2001:db8::1.443"));
        assert!(out.contains("fe80::1%eth0.8080"));
    }

    #[test]
    fn unknown_passes_through() {
        let t = "not a packet capture";
        assert_eq!(compact(t), t);
    }

    #[test]
    fn savings_huge_capture() {
        // 50 000 packets across ~10 flows.
        let mut s = String::with_capacity(8_000_000);
        for i in 0..50_000 {
            let src_port = 30000 + (i % 100);
            let dst_port = if i % 2 == 0 { 443 } else { 80 };
            let flags = match i % 5 {
                0 => "S.",
                1 => ".",
                2 => "P.",
                3 => "F.",
                _ => "R.",
            };
            let length = 100 + (i % 500) as u64;
            let dir = i % 2;
            if dir == 0 {
                let _ = writeln!(
                    s,
                    "10:30:{:02}.{:06} IP 192.168.1.1.{dst_port} > 10.0.0.5.{src_port}: Flags [{flags}], seq {i}, ack {i}, win 65535, length {length}",
                    (i / 1000) % 60,
                    i % 1_000_000
                );
            } else {
                let _ = writeln!(
                    s,
                    "10:30:{:02}.{:06} IP 10.0.0.5.{src_port} > 192.168.1.1.{dst_port}: Flags [{flags}], seq {i}, ack {i}, win 65535, length {length}",
                    (i / 1000) % 60,
                    i % 1_000_000
                );
            }
        }
        let (out, sav) = compact_with_savings(&s);
        assert!(out.contains("50000 packet(s)"));
        // The flow set should be much smaller than packet count.
        assert!(sav.percent() > 95.0, "got {sav:?}");
    }

    #[test]
    fn parse_packet_works() {
        let p = parse_packet(
            "10:30:00.123456 IP 192.168.1.1.443 > 10.0.0.5.34567: Flags [S.], seq 12345, ack 67890, win 65535, length 0",
        );
        assert!(p.is_some());
        let (proto, src, dst, flags, length) = p.unwrap();
        assert_eq!(proto.as_str(), "IP");
        assert_eq!(src, "192.168.1.1.443");
        assert_eq!(dst, "10.0.0.5.34567");
        assert_eq!(flags, "S.");
        assert_eq!(length, 0);
    }

    #[test]
    fn parse_packet_with_length() {
        let p = parse_packet(
            "10:30:00.345678 IP 10.0.0.5.34567 > 192.168.1.1.443: Flags [P.], seq 1:101, ack 1, win 65535, length 100",
        );
        let (_proto, _src, _dst, flags, length) = p.unwrap();
        assert_eq!(flags, "P.");
        assert_eq!(length, 100);
    }

    #[test]
    fn flow_key_canonicalises() {
        let k1 = flow_key("a", "b");
        let k2 = flow_key("b", "a");
        assert_eq!(k1, k2);
        assert!(k1.contains("<->"));
    }
}
