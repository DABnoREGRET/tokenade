//! `tcpdump` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

pub(crate) fn synth_tcpdump(n_packets: usize, n_flows: usize) -> String {
    let mut s = String::with_capacity(n_packets * 120);
    for i in 0..n_packets {
        let flow = i % n_flows;
        let src_port = 30000 + flow;
        let dst_port = if flow.is_multiple_of(2) { 443 } else { 80 };
        let flags = match i % 5 {
            0 => "S.",
            1 => ".",
            2 => "P.",
            3 => "F.",
            _ => "R.",
        };
        let length = 100 + (i % 500) as u64;
        let direction = i % 2;
        if direction == 0 {
            s.push_str(&format!(
                "10:30:{:02}.{:06} IP 192.168.1.1.{dst_port} > 10.0.0.{flow}.{src_port}: Flags [{flags}], seq {i}, ack {i}, win 65535, length {length}\n",
                (i / 1000) % 60,
                i % 1_000_000
            ));
        } else {
            s.push_str(&format!(
                "10:30:{:02}.{:06} IP 10.0.0.{flow}.{src_port} > 192.168.1.1.{dst_port}: Flags [{flags}], seq {i}, ack {i}, win 65535, length {length}\n",
                (i / 1000) % 60,
                i % 1_000_000
            ));
        }
    }
    s
}

pub(crate) fn task_tcpdump_real(results: &mut Vec<RunResult>) {
    // Realistic shape: 5 000 packets across 20 flows.
    let raw = synth_tcpdump(5_000, 20);
    let (out, sav, ms) = measure(&raw, tcpdump::compact_with_savings);
    let quality = if out.contains("[tokenade:tcpdump] 5000 packet(s)")
        && out.contains("20 flow(s)")
        && out.contains("time range:")
        && out.contains("protocols:")
        && out.contains("IP")
        && out.contains("top flows")
        && out.contains("flag distribution")
        // Flags appear in distribution.
        && out.contains("S.")
        && out.contains("P.")
        && sav.percent() > 90.0
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "output:tcpdump-compact",
        "tcpdump",
        "synth-5kpkts-20flows",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #87: 100 000-packet capture across 50 flows. Must compact
// >97 % in <500 ms.
pub(crate) fn adv_tcpdump_huge(results: &mut Vec<RunResult>) {
    let raw = synth_tcpdump(100_000, 50);
    let (out, sav, ms) = measure(&raw, tcpdump::compact_with_savings);
    let quality = if out.contains("100000 packet(s)")
        && out.contains("50 flow(s)")
        && sav.percent() > 97.0
        && ms < 500
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:tcpdump:huge-100k",
        "tcpdump",
        "adversarial",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #88: hostile mix — IPv4 + IPv6 + ARP + prose lines + Unicode
// in `tshark -V` style mid-stream + edge protocol forms.
pub(crate) fn adv_tcpdump_hostile(results: &mut Vec<RunResult>) {
    let raw = "Some intro prose to ignore\n\
listening on eth0, link-type EN10MB (Ethernet), capture size 262144 bytes\n\
10:30:00.123456 IP 192.168.1.1.443 > 10.0.0.5.34567: Flags [S.], seq 12345, ack 67890, win 65535, length 0\n\
10:30:00.234567 IP 10.0.0.5.34567 > 192.168.1.1.443: Flags [.], ack 12346, win 65535, length 0\n\
10:30:00.345678 IP6 2001:db8::1.443 > 2001:db8::ff.55555: Flags [S.], seq 99, ack 100, win 65535, length 0\n\
10:30:00.456789 IP6 2001:db8::ff.55555 > 2001:db8::1.443: Flags [.], ack 100, win 65535, length 0\n\
10:30:00.567890 IP 192.168.1.2.80 > 10.0.0.6.55555: Flags [S.], seq 99, ack 100, win 65535, length 0\n\
10:30:01.000000 ARP, Request who-has 10.0.0.1 tell 10.0.0.5, length 28\n\
10:30:01.111111 ARP, Reply 10.0.0.1 is-at aa:bb:cc:dd:ee:ff, length 28\n\
10:30:01.222222 IP6 fe80::1%eth0.8080 > fe80::99%eth0.33333: Flags [S.], length 0\n\
10:30:01.333333 IP6 fe80::1%eth0.8080 > fe80::99%eth0.33333: Flags [P.], length 1024\n\
not a tcpdump line at all\n\
10:30:02.444444 IP 192.168.1.1.443 > 10.0.0.5.34567: Flags [F.], length 0\n";
    let (out, _sav, ms) = measure(raw, tcpdump::compact_with_savings);
    let quality = if out.contains("[tokenade:tcpdump]")
        // 4 IP + 4 IP6 + 2 ARP = 10 packets recognised. (`listening on eth0…`
        // and `not a tcpdump line…` are rejected by the timestamp-shape guard.)
        && out.contains("10 packet(s)")
        // Protocols breakdown.
        && out.contains("IP")
        && out.contains("IP6")
        && out.contains("ARP")
        // IPv6 addresses preserved.
        && out.contains("2001:db8::1.443")
        && out.contains("fe80::1%eth0.8080")
        // Flow canonicalisation: bidirectional flows merged.
        && out.contains("<->")
        // Time range.
        && out.contains("10:30:00.123456 → 10:30:02.444444")
        && ms < 100
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:tcpdump:hostile",
        "tcpdump",
        "adversarial",
        raw,
        &out,
        ms,
        quality,
    );
}
