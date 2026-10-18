//! macOS / BSD `ifconfig` — distinct from Linux net-tools.
//!
//! ```text
//! lo0: flags=8049<UP,LOOPBACK,RUNNING,MULTICAST> mtu 16384
//!     options=1203<RXCSUM,TXCSUM,TXSTATUS,SW_TIMESTAMP>
//!     inet 127.0.0.1 netmask 0xff000000
//!     inet6 ::1 prefixlen 128
//!     inet6 fe80::1%lo0 prefixlen 64 scopeid 0x1
//!     nd6 options=201<PERFORMNUD,DAD>
//! en0: flags=8863<UP,BROADCAST,SMART,RUNNING,SIMPLEX,MULTICAST> mtu 1500
//!     options=400<CHANNEL_IO>
//!     ether 8c:85:90:aa:bb:cc
//!     inet6 fe80::1c8d:8aff:fe23:4567%en0 prefixlen 64 secured scopeid 0xb
//!     inet 192.168.1.42 netmask 0xffffff00 broadcast 192.168.1.255
//!     ...
//!     media: autoselect
//!     status: active
//! ```
//!
//! BSD differences from Linux:
//!   * `netmask 0xff000000` hex form (vs Linux `255.0.0.0`).
//!   * `options=...` continuation line per block.
//!   * `media:` / `status:` trailers.
//!   * `nd6` ND6 options line.
//!   * No `RX packets ... TX packets ...` traffic counters by
//!     default (use `netstat -ib` for those on macOS).
//!
//! Compaction: drop idle loopback (`lo0` with status not "active"),
//! drop awdl0 / utun* / bridge0 interfaces when inactive.

use std::fmt::Write;

pub fn detect(text: &str) -> bool {
    text.contains("flags=") || text.contains("inet ") || text.contains("ether ")
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    let blocks = split_blocks(text);
    let mut out = String::with_capacity(text.len() / 2);
    let mut elided_inactive = 0usize;
    let mut elided_awdl_utun = 0usize;
    for block in &blocks {
        let name = block_name(block).unwrap_or("?");
        let status = block_status(block);
        let is_awdl_utun = name.starts_with("awdl")
            || name.starts_with("utun")
            || name.starts_with("llw")
            || name.starts_with("anpi");
        let inactive = status.as_deref() == Some("inactive")
            || status.as_deref() == Some("no carrier");
        if inactive && (is_awdl_utun || name.starts_with("bridge") || name == "ap1") {
            elided_awdl_utun += 1;
            continue;
        }
        if inactive {
            elided_inactive += 1;
            continue;
        }
        // Keep block but drop `options=...` and `nd6 options=...`
        // continuation lines (noise — most agents don't need to know
        // the chip's checksum offload).
        for line in block.lines() {
            let t = line.trim_start();
            if t.starts_with("options=") || t.starts_with("nd6 options=") {
                continue;
            }
            out.push_str(line);
            out.push('\n');
        }
        out.push('\n');
    }
    let mut elided_parts: Vec<String> = Vec::new();
    if elided_inactive > 0 {
        elided_parts.push(format!("{elided_inactive} inactive"));
    }
    if elided_awdl_utun > 0 {
        elided_parts.push(format!("{elided_awdl_utun} inactive awdl/utun/bridge"));
    }
    if !elided_parts.is_empty() {
        let _ = writeln!(out, "  [elided: {}]", elided_parts.join(", "));
    }
    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

fn split_blocks(text: &str) -> Vec<String> {
    let mut blocks: Vec<String> = Vec::new();
    let mut current = String::new();
    for line in text.lines() {
        let starts_block = !line.starts_with(' ')
            && !line.starts_with('\t')
            && line.contains("flags=")
            && !current.is_empty();
        if starts_block {
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
    first.split_once(':').map(|(name, _)| name)
}

fn block_status(block: &str) -> Option<String> {
    for line in block.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("status:") {
            return Some(rest.trim().to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"lo0: flags=8049<UP,LOOPBACK,RUNNING,MULTICAST> mtu 16384
	options=1203<RXCSUM,TXCSUM,TXSTATUS,SW_TIMESTAMP>
	inet 127.0.0.1 netmask 0xff000000
	inet6 ::1 prefixlen 128
	inet6 fe80::1%lo0 prefixlen 64 scopeid 0x1
	nd6 options=201<PERFORMNUD,DAD>
en0: flags=8863<UP,BROADCAST,SMART,RUNNING,SIMPLEX,MULTICAST> mtu 1500
	options=400<CHANNEL_IO>
	ether 8c:85:90:aa:bb:cc
	inet 192.168.1.42 netmask 0xffffff00 broadcast 192.168.1.255
	media: autoselect
	status: active
utun0: flags=8051<UP,POINTOPOINT,RUNNING,MULTICAST> mtu 1500
	options=6403<RXCSUM,TXCSUM,CHANNEL_IO,PARTIAL_CSUM,ZEROINVERT_CSUM>
	inet6 fe80::ce81:b1c:bd2c:69e%utun0 prefixlen 64 scopeid 0xc
	nd6 options=201<PERFORMNUD,DAD>
"#;

    #[test]
    fn detects_bsd_ifconfig() {
        assert!(detect(SAMPLE));
    }

    #[test]
    fn does_not_match_linux_ifconfig() {
        // Detector is now permissive (routes are gated by program
        // name through `dispatch_named`, not content shape). Only
        // empty input is rejected.
        assert!(!detect(""));
        assert!(!detect("   \n  "));
    }

    #[test]
    fn drops_options_lines() {
        let out = compact(SAMPLE);
        assert!(!out.contains("options=1203"));
        assert!(!out.contains("nd6 options="));
        assert!(out.contains("ether 8c:85:90:aa:bb:cc"));
    }

    #[test]
    fn savings_real_shape() {
        let (_o, sav) = compact_with_savings(SAMPLE);
        assert!(sav.percent() > 20.0, "{sav:?}");
    }
}
