//! `ifconfig` (Linux net-tools variant).
//!
//! Output shape:
//!
//! ```text
//! enp0s3: flags=4163<UP,BROADCAST,RUNNING,MULTICAST>  mtu 1500
//!         inet 192.168.1.42  netmask 255.255.255.0  broadcast 192.168.1.255
//!         inet6 fe80::1234:5678:9abc:def0  prefixlen 64  scopeid 0x20<link>
//!         ether aa:bb:cc:dd:ee:ff  txqueuelen 1000  (Ethernet)
//!         RX packets 12345  bytes 1234567 (1.1 MiB)
//!         RX errors 0  dropped 0  overruns 0  frame 0
//!         TX packets 6789  bytes 987654 (964.5 KiB)
//!         TX errors 0  dropped 0 overruns 0  carrier 0  collisions 0
//!
//! lo: flags=73<UP,LOOPBACK,RUNNING>  mtu 65536
//!         inet 127.0.0.1  netmask 255.0.0.0
//!         …
//! ```
//!
//! Compaction strategy:
//!   * Detect the block-per-interface shape: name line + indented
//!     continuation lines.
//!   * Drop `lo` entirely UNLESS it has non-zero packet counters
//!     (rare but possible — bug-bisecting localhost traffic).
//!   * Drop `docker0` / `br-*` / `veth*` bridges when their
//!     interface is DOWN OR has zero counters.
//!   * Fold EVERY kept block (the first included — the older wording here
//!     said the first was verbatim and the code never did that) to its
//!     header + inet + inet6, its hardware address, a RX/TX summary line
//!     and a fault line. Whatever else the block carried is counted at the
//!     end rather than dropped in silence.
//!
//! Locale: the per-line keywords (`inet`, `inet6`, `ether`, `RX`,
//! `TX`) are part of `net-tools` and NOT translated by the system
//! locale — same on every Linux. Safe to hard-code.

use std::fmt::Write;

const MIN_BLOCKS: usize = 2;

/// Detect Linux `ifconfig` output: at least 2 blocks AND each block's
/// header line has `flags=` + parenthesised hex value AND at least
/// one indented `inet` continuation.
pub fn detect(text: &str) -> bool {
    let mut blocks = 0usize;
    let mut saw_inet_in_block = false;
    for line in text.lines() {
        if line.is_empty() || line.starts_with(' ') || line.starts_with('\t') {
            if !saw_inet_in_block && line.trim_start().starts_with("inet") {
                saw_inet_in_block = true;
            }
            continue;
        }
        // New block header line: must contain "flags=" and end with a
        // digit (the mtu value at line end).
        if line.contains("flags=") && line.contains("mtu") {
            if blocks > 0 && !saw_inet_in_block {
                // Previous block had no inet — suspicious; could be
                // a different format. Bail.
                return false;
            }
            blocks += 1;
            saw_inet_in_block = false;
        }
    }
    blocks >= MIN_BLOCKS
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    let blocks = split_blocks(text);
    let mut out = String::with_capacity(text.len() / 2);
    let mut elided_loopback = 0usize;
    let mut elided_bridges = 0usize;
    let mut elided_idle = 0usize;
    let mut elided_rows = 0usize;
    for (idx, block) in blocks.iter().enumerate() {
        let name = block_name(block).unwrap_or("?");
        let stats = block_stats(block);
        // Elision decisions.
        let is_loopback = name == "lo";
        let is_bridge =
            name == "docker0" || name.starts_with("br-") || name.starts_with("veth");
        let idle = stats.rx_packets == 0 && stats.tx_packets == 0;
        if is_loopback && idle {
            elided_loopback += 1;
            continue;
        }
        if is_bridge && idle {
            elided_bridges += 1;
            continue;
        }
        if idle && !is_loopback && !is_bridge && idx > 0 {
            elided_idle += 1;
            continue;
        }
        // Header + inet + inet6, then the hardware address, then one summary
        // line for the counters.
        //
        // The `ether` line and the fault counters used to go with everything
        // else past line 3, silently — no count, no marker. Measured on this
        // machine's real `ifconfig -a`: `wlan0` reports `RX dropped 90792`
        // and `enp12s0` `RX dropped 87`, and a drop count is the single thing
        // an agent runs `ifconfig` to see when a link misbehaves. The MAC
        // address went the same way.
        let mut kept: Vec<&str> = block.lines().take(3).collect();
        let ether = block
            .lines()
            .map(str::trim_start)
            .find(|l| l.starts_with("ether "))
            .map(|l| l.split_whitespace().take(2).collect::<Vec<_>>().join(" "));
        let shown_lines = kept.len() + usize::from(ether.is_some());
        elided_rows += block.lines().filter(|l| !l.trim().is_empty()).count() - shown_lines;
        let summary = format!(
            "        RX packets {} ({} bytes)  TX packets {} ({} bytes)",
            stats.rx_packets, stats.rx_bytes, stats.tx_packets, stats.tx_bytes
        );
        // Zero is printed rather than omitted: an absent line cannot be told
        // apart from a line that was dropped, which is the whole complaint.
        let faults = if stats.any_fault() {
            format!(
                "        RX errors {} dropped {}  TX errors {} dropped {}",
                stats.rx_errors, stats.rx_dropped, stats.tx_errors, stats.tx_dropped
            )
        } else {
            "        errors/dropped: none".to_string()
        };
        out.push_str(&kept.join("\n"));
        out.push('\n');
        if let Some(e) = ether {
            let _ = writeln!(out, "        {e}");
        }
        out.push_str(&summary);
        out.push('\n');
        out.push_str(&faults);
        out.push_str("\n\n");
        kept.clear();
    }
    if elided_loopback > 0 || elided_bridges > 0 || elided_idle > 0 {
        let mut parts: Vec<String> = Vec::new();
        if elided_loopback > 0 {
            parts.push(format!("{elided_loopback} loopback"));
        }
        if elided_bridges > 0 {
            parts.push(format!("{elided_bridges} idle docker/veth bridge(s)"));
        }
        if elided_idle > 0 {
            parts.push(format!("{elided_idle} idle interface(s)"));
        }
        let _ = writeln!(out, "  [elided: {}]", parts.join(", "));
    }
    if elided_rows > 0 {
        let _ = writeln!(
            out,
            "  [{elided_rows} counter/address line(s) elided from the kept blocks; \
             faults and hardware addresses are shown above]"
        );
    }
    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[derive(Default, Debug)]
struct Stats {
    rx_packets: u64,
    rx_bytes: u64,
    tx_packets: u64,
    tx_bytes: u64,
    rx_errors: u64,
    rx_dropped: u64,
    tx_errors: u64,
    tx_dropped: u64,
}

impl Stats {
    fn any_fault(&self) -> bool {
        self.rx_errors | self.rx_dropped | self.tx_errors | self.tx_dropped != 0
    }
}

fn split_blocks(text: &str) -> Vec<String> {
    let mut blocks: Vec<String> = Vec::new();
    let mut current = String::new();
    for line in text.lines() {
        if line.is_empty() {
            if !current.trim().is_empty() {
                blocks.push(std::mem::take(&mut current));
            }
            continue;
        }
        // Non-indented + flags= signals a new interface block.
        if !line.starts_with(' ')
            && !line.starts_with('\t')
            && line.contains("flags=")
            && !current.is_empty()
        {
            blocks.push(std::mem::take(&mut current));
        }
        current.push_str(line);
        current.push('\n');
    }
    if !current.trim().is_empty() {
        blocks.push(current);
    }
    blocks
}

fn block_name(block: &str) -> Option<&str> {
    let first = block.lines().next()?;
    first.split_once(':').map(|(name, _)| name.trim())
}

fn block_stats(block: &str) -> Stats {
    let mut s = Stats::default();
    for line in block.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("RX packets") {
            if let Some(n) = first_num(rest) {
                s.rx_packets = n;
            }
            if let Some(b) = grab_bytes(rest) {
                s.rx_bytes = b;
            }
        } else if let Some(rest) = t.strip_prefix("TX packets") {
            if let Some(n) = first_num(rest) {
                s.tx_packets = n;
            }
            if let Some(b) = grab_bytes(rest) {
                s.tx_bytes = b;
            }
        } else if let Some(rest) = t.strip_prefix("RX errors") {
            s.rx_errors = first_num(rest).unwrap_or(0);
            s.rx_dropped = field_after(rest, "dropped");
        } else if let Some(rest) = t.strip_prefix("TX errors") {
            s.tx_errors = first_num(rest).unwrap_or(0);
            s.tx_dropped = field_after(rest, "dropped");
        }
    }
    s
}

/// The integer following `key` on a net-tools counter line.
///
/// `RX errors 0  dropped 87  overruns 0  frame 0` — the keywords are
/// net-tools' own and are not localised, so a keyword scan is safe here for
/// the same reason the module header gives for `inet`/`RX`/`TX`.
fn field_after(rest: &str, key: &str) -> u64 {
    let mut it = rest.split_whitespace();
    while let Some(tok) = it.next() {
        if tok == key {
            return it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        }
    }
    0
}

fn first_num(s: &str) -> Option<u64> {
    let mut buf = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            buf.push(c);
        } else if !buf.is_empty() {
            break;
        }
    }
    buf.parse().ok()
}

fn grab_bytes(rest: &str) -> Option<u64> {
    // "RX packets N  bytes M (X.Y MiB)" → return M.
    let after = rest.split("bytes").nth(1)?;
    first_num(after)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"enp0s3: flags=4163<UP,BROADCAST,RUNNING,MULTICAST>  mtu 1500
        inet 192.168.1.42  netmask 255.255.255.0  broadcast 192.168.1.255
        inet6 fe80::1234:5678:9abc:def0  prefixlen 64  scopeid 0x20<link>
        ether aa:bb:cc:dd:ee:ff  txqueuelen 1000  (Ethernet)
        RX packets 12345  bytes 1234567 (1.1 MiB)
        RX errors 0  dropped 0  overruns 0  frame 0
        TX packets 6789  bytes 987654 (964.5 KiB)
        TX errors 0  dropped 0 overruns 0  carrier 0  collisions 0

lo: flags=73<UP,LOOPBACK,RUNNING>  mtu 65536
        inet 127.0.0.1  netmask 255.0.0.0
        inet6 ::1  prefixlen 128  scopeid 0x10<host>
        loop  txqueuelen 1000  (Local Loopback)
        RX packets 0  bytes 0 (0.0 B)
        RX errors 0  dropped 0  overruns 0  frame 0
        TX packets 0  bytes 0 (0.0 B)
        TX errors 0  dropped 0 overruns 0  carrier 0  collisions 0

docker0: flags=4099<UP,BROADCAST,MULTICAST>  mtu 1500
        inet 172.17.0.1  netmask 255.255.0.0  broadcast 172.17.255.255
        ether 02:42:00:00:00:01  txqueuelen 0  (Ethernet)
        RX packets 0  bytes 0 (0.0 B)
        TX packets 0  bytes 0 (0.0 B)
"#;

    #[test]
    fn detects_classic_linux_ifconfig() {
        assert!(detect(SAMPLE));
    }

    #[test]
    fn drops_idle_loopback_and_bridges() {
        let out = compact(SAMPLE);
        assert!(out.contains("enp0s3"));
        assert!(out.contains("192.168.1.42"));
        assert!(!out.contains("docker0"), "idle docker0 must be elided:\n{out}");
        // Loopback elision noted in summary.
        assert!(out.contains("loopback"));
        assert!(out.contains("docker"));
    }

    #[test]
    fn keeps_active_loopback() {
        // Replace the idle lo block's zero counters with non-zero
        // ones to simulate a `lo` that's seen real traffic.
        let active_lo = SAMPLE.replace(
            "        RX packets 0  bytes 0 (0.0 B)\n        RX errors 0  dropped 0  overruns 0  frame 0\n        TX packets 0  bytes 0 (0.0 B)",
            "        RX packets 5000  bytes 9999 (9.7 KiB)\n        RX errors 0  dropped 0  overruns 0  frame 0\n        TX packets 5000  bytes 9999 (9.7 KiB)",
        );
        let out = compact(&active_lo);
        assert!(out.contains("lo:"), "active lo should be kept:\n{out}");
    }

    #[test]
    fn rejects_non_ifconfig_text() {
        assert!(!detect("just some prose\nnothing relevant"));
        assert!(try_compact("hello world").is_none());
    }

    #[test]
    fn savings_on_typical_real_world() {
        let (out, sav) = compact_with_savings(SAMPLE);
        assert!(!out.is_empty());
        assert!(sav.percent() > 30.0, "expected 30%+ savings, got {sav:?}");
    }
    const REAL: &str = "\
enp12s0: flags=4163<UP,BROADCAST,RUNNING,MULTICAST>  mtu 1500
        inet 192.168.1.183  netmask 255.255.255.0  broadcast 192.168.1.255
        inet6 2001:db8:3a41:7e50:6a41:83f9:9f1:449d  prefixlen 64  scopeid 0x0<global>
        inet6 fe80::eccc:22f4:3bb2:9fdb  prefixlen 64  scopeid 0x20<link>
        ether 04:7c:16:b9:ca:4e  txqueuelen 1000  (Ethernet)
        RX packets 83011889  bytes 116537413460 (108.5 GiB)
        RX errors 0  dropped 87  overruns 0  frame 0
        TX packets 17520852  bytes 5898076622 (5.4 GiB)
        TX errors 0  dropped 0 overruns 0  carrier 0  collisions 0

wlan0: flags=4163<UP,BROADCAST,RUNNING,MULTICAST>  mtu 1500
        inet 192.168.1.96  netmask 255.255.255.0  broadcast 192.168.1.255
        inet6 fe80::7051:7429:65d3:8104  prefixlen 64  scopeid 0x20<link>
        ether bc:f4:d4:c1:31:6f  txqueuelen 1000  (Ethernet)
        RX packets 322255  bytes 49541115 (47.2 MiB)
        RX errors 0  dropped 90792  overruns 0  frame 0
        TX packets 113488  bytes 13382432 (12.7 MiB)
        TX errors 0  dropped 6 overruns 0  carrier 0  collisions 0
";

    /// The number an agent runs `ifconfig` to see.
    ///
    /// Real output from this machine: `wlan0` reports `RX dropped 90792`. It
    /// used to vanish with no count and no marker, together with every
    /// hardware address, because each block was cut to its first three lines
    /// plus a packets/bytes summary.
    #[test]
    fn a_drop_count_is_not_a_detail() {
        let out = compact(REAL);
        assert!(out.contains("90792"), "the drop count is gone:\n{out}");
        assert!(out.contains("87"), "the other interface's drops are gone:\n{out}");
    }

    #[test]
    fn the_hardware_address_survives() {
        let out = compact(REAL);
        for mac in ["04:7c:16:b9:ca:4e", "bc:f4:d4:c1:31:6f"] {
            assert!(out.contains(mac), "lost {mac}:\n{out}");
        }
    }

    /// A clean interface says so rather than saying nothing: an absent fault
    /// line cannot be told apart from a fault line that was dropped, which is
    /// the whole complaint.
    #[test]
    fn a_clean_interface_says_it_is_clean() {
        let clean = REAL.replace("dropped 90792", "dropped 0").replace("dropped 87", "dropped 0");
        let out = compact(&clean);
        assert!(
            out.contains("errors/dropped: none"),
            "silence is not a statement:\n{out}"
        );
    }

    /// And whatever else the block carried is counted, not dropped quietly.
    #[test]
    fn the_lines_that_do_go_are_counted() {
        let out = compact(REAL);
        assert!(
            out.contains("line(s) elided from the kept blocks"),
            "no disclosure for the rest of the block:\n{out}"
        );
    }

}
