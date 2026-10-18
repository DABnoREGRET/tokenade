//! `ip addr` / `ip link` / `ip route` (iproute2).
//!
//! `ip addr show` output:
//!
//! ```text
//! 1: lo: <LOOPBACK,UP,LOWER_UP> mtu 65536 qdisc noqueue state UNKNOWN group default qlen 1000
//!     link/loopback 00:00:00:00:00:00 brd 00:00:00:00:00:00
//!     inet 127.0.0.1/8 scope host lo
//!        valid_lft forever preferred_lft forever
//!     inet6 ::1/128 scope host
//!        valid_lft forever preferred_lft forever
//! 2: enp0s3: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 ...
//! ```
//!
//! Compaction: ONE LINE PER INTERFACE, keeping every value that says
//! something. Dropping the `valid_lft forever` lines alone was worth 7 % and
//! plateaued there (measured: 2 interfaces 2.2 %, 8 → 6.1 %, 20 → 6.8 %,
//! 60 → 7.1 %), because the cost is not the noise lines, it is the six lines
//! and four indentation levels each interface spends to say four facts.
//!
//! ```text
//! 2: enp0s3 UP mtu 1500 · 192.168.1.42/24 · fe80::a00:27ff:feaa:bbcc/64
//!    mac 08:00:27:aa:bb:cc · lease 86392s/75592s · qdisc fq_codel
//! ```
//!
//! **Nothing recognised is dropped unless it is provably redundant**, and
//! anything NOT recognised is appended verbatim rather than silently lost —
//! `ip addr` grows fields and a compactor that swallows the ones it has never
//! seen is worse than no compactor.

use std::fmt::Write;

pub fn detect(text: &str) -> bool {
    let mut interface_lines = 0usize;
    for line in text.lines().take(60) {
        // Interface line shape: leading digit + `:` + name + `:` + `<flags>`
        let t = line.trim_start();
        if let Some((num, rest)) = t.split_once(':') {
            if num.chars().all(|c| c.is_ascii_digit())
                && rest.contains(": <")
                && rest.contains("mtu ")
            {
                interface_lines += 1;
            }
        }
    }
    interface_lines >= 2
}

crate::archetypes::detect_gated_try_compact!();

/// Values that are the same on every machine, or derivable from what is kept.
///
///   * `brd ff:ff:ff:ff:ff:ff` — the broadcast MAC is that on every Ethernet
///     link in existence;
///   * `brd <ipv4>` — the broadcast address IS the address plus the prefix;
///   * `group default`, `qlen 1000` — the defaults;
///   * `valid_lft forever preferred_lft forever` — the absence of a lease;
///   * the interface name repeated at the end of its own `inet` line;
///   * `scope global` on a routable address and `scope link` on an `fe80::`
///     one — both restate the address.
fn is_redundant(tok: &str, next: Option<&str>, iface: &str, addr: &str) -> bool {
    match tok {
        "brd" => matches!(next, Some(n) if n == "ff:ff:ff:ff:ff:ff" || n.contains('.')),
        "group" => next == Some("default"),
        "qlen" => next == Some("1000"),
        "scope" => match next {
            Some("global") => !addr.is_empty() && !addr.starts_with("fe80:"),
            Some("link") => addr.starts_with("fe80:"),
            Some("host") => addr.starts_with("127.") || addr.starts_with("::1"),
            _ => false,
        },
        _ => tok == iface,
    }
}

/// One interface, rebuilt from its block of lines.
#[derive(Default)]
struct Iface {
    head: String,
    state: String,
    mtu: String,
    qdisc: String,
    flags: Vec<String>,
    mac: String,
    /// Which namespace a `veth` leads to (`link-netnsid 3`) and a bonded
    /// port's own address (`permaddr`), read on the `link/` line.
    link: Vec<String>,
    addrs: Vec<String>,
    extra: Vec<String>,
}

impl Iface {
    fn render(&self, out: &mut String) {
        let _ = write!(out, "{} {}", self.head, self.state);
        if !self.mtu.is_empty() {
            let _ = write!(out, " mtu {}", self.mtu);
        }
        for a in &self.addrs {
            let _ = write!(out, " · {a}");
        }
        let mut tail: Vec<String> = Vec::new();
        if !self.mac.is_empty() {
            tail.push(format!("mac {}", self.mac));
        }
        tail.extend(self.link.iter().cloned());
        tail.extend(self.shared_tail());
        if !tail.is_empty() {
            let _ = write!(out, " · {}", tail.join(" · "));
        }
        out.push('\n');
    }

    fn shared_tail(&self) -> Vec<String> {
        let mut tail: Vec<String> = Vec::new();
        if !self.qdisc.is_empty() {
            tail.push(format!("qdisc {}", self.qdisc));
        }
        if !self.flags.is_empty() {
            tail.push(self.flags.join(","));
        }
        tail.extend(self.extra.iter().cloned());
        tail
    }

    /// Everything but what names THIS interface: its head, MAC and addresses
    /// (each address's qualifiers stay in).
    fn shape(&self) -> (String, String, Vec<String>, Vec<&str>) {
        let quals = self.addrs.iter().map(|a| a.split_once(' ').map_or("", |(_, q)| q)).collect();
        (self.state.clone(), self.mtu.clone(), self.shared_tail(), quals)
    }
}

/// Interfaces alike but for their names — the `veth` ports Docker hangs on
/// one bridge — said once, then one line each with what tells them apart.
/// Measured 2026-10-04: a real `ip addr` repeated `UP mtu 1500 · (proto
/// kernel_ll) · qdisc noqueue · master br-… · qlen 0` on seven `veth` lines.
fn render_all(ifaces: &[Iface], out: &mut String) {
    let mut k = 0;
    while k < ifaces.len() {
        let shape = ifaces[k].shape();
        let run = ifaces[k..].iter().take_while(|i| i.shape() == shape).count();
        if run < 3 {
            ifaces[k].render(out);
            k += 1;
            continue;
        }
        let (state, mtu, tail, quals) = &shape;
        let _ = write!(out, "{run} interfaces alike: {state}");
        if !mtu.is_empty() {
            let _ = write!(out, " mtu {mtu}");
        }
        for q in quals.iter().filter(|q| !q.is_empty()) {
            let _ = write!(out, " · {q}");
        }
        for t in tail {
            let _ = write!(out, " · {t}");
        }
        out.push('\n');
        for i in &ifaces[k..k + run] {
            let _ = write!(out, "  {}", i.head);
            for a in &i.addrs {
                let _ = write!(out, " · {}", a.split(' ').next().unwrap_or(a));
            }
            if !i.mac.is_empty() {
                let _ = write!(out, " · mac {}", i.mac);
            }
            for l in &i.link {
                let _ = write!(out, " · {l}");
            }
            out.push('\n');
        }
        k += run;
    }
}

/// Head-line tokens that introduce a value, so a `key value` pair is never
/// read as two adjacent keys.
const KEY_TOKENS: &[&str] = &[
    "mtu", "qdisc", "state", "mode", "group", "qlen", "master", "link-netnsid",
];

/// Flags that restate the state field or are true of almost every interface.
const IMPLIED_FLAGS: &[&str] = &[
    "UP", "LOWER_UP", "BROADCAST", "MULTICAST", "LOOPBACK", "M-DOWN",
];

pub fn compact(text: &str) -> String {
    let mut out = String::with_capacity(text.len() / 2);
    let mut cur: Option<Iface> = None;
    let mut done: Vec<Iface> = Vec::new();
    let mut last_addr = String::new();
    let mut iface_name = String::new();
    // `ip -s link` prints a counter HEADER row and its values on the NEXT
    // line. Held here so the two can be joined; see [`zip_counters`].
    let mut pending_cols: Option<(String, Vec<String>)> = None;

    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        // `2: enp0s3: <BROADCAST,…> mtu 1500 qdisc fq_codel state UP group default qlen 1000`
        let is_head = t
            .split_once(':')
            .is_some_and(|(n, rest)| n.chars().all(|c| c.is_ascii_digit()) && rest.contains(": <"));
        if is_head {
            if let (Some(i), Some((dir, names))) = (cur.as_mut(), pending_cols.take()) {
                i.extra.push(format!("{dir}: {}", names.join(" ")));
            }
            if let Some(i) = cur.take() {
                done.push(i);
            }
            let (idx, rest) = t.split_once(':').expect("is_head implies a colon");
            let (name, rest) = rest.split_once(':').expect("is_head implies two colons");
            iface_name = name.trim().to_string();
            last_addr.clear();
            let mut i = Iface {
                head: format!("{idx}: {iface_name}"),
                ..Default::default()
            };
            let toks: Vec<&str> = rest.split_whitespace().collect();
            let mut k = 0;
            while k < toks.len() {
                match toks[k] {
                    t if t.starts_with('<') => {
                        i.flags = t
                            .trim_matches(['<', '>'])
                            .split(',')
                            .filter(|f| !f.is_empty() && !IMPLIED_FLAGS.contains(f))
                            .map(str::to_string)
                            .collect();
                        k += 1;
                    }
                    "mtu" => {
                        i.mtu = toks.get(k + 1).unwrap_or(&"").to_string();
                        k += 2;
                    }
                    "qdisc" => {
                        i.qdisc = toks.get(k + 1).unwrap_or(&"").to_string();
                        k += 2;
                    }
                    "state" => {
                        i.state = toks.get(k + 1).unwrap_or(&"").to_string();
                        k += 2;
                    }
                    tok if is_redundant(tok, toks.get(k + 1).copied(), &iface_name, "") => k += 2,
                    // A `key value` pair the arms above do not name — `mode
                    // DORMANT`, and whatever iproute2 adds next. Kept AS A
                    // PAIR: pushed one token at a time they reach `render` as
                    // `mode · DORMANT`, two unrelated facts where there was
                    // one. The value must not look like a key itself, so a
                    // trailing bare token is still kept alone.
                    key if toks
                        .get(k + 1)
                        .is_some_and(|v| !v.starts_with('<') && !KEY_TOKENS.contains(v)) =>
                    {
                        i.extra.push(format!("{key} {}", toks[k + 1]));
                        k += 2;
                    }
                    other => {
                        i.extra.push((*other).to_string());
                        k += 1;
                    }
                }
            }
            cur = Some(i);
            continue;
        }

        let Some(i) = cur.as_mut() else {
            // Something before the first interface: keep it verbatim.
            out.push_str(line);
            out.push('\n');
            continue;
        };

        if let Some(rest) = t.strip_prefix("link/") {
            let mut toks = rest.split_whitespace();
            let _kind = toks.next();
            if let Some(mac) = toks.next() {
                // The loopback's all-zero MAC is not an address.
                if mac != "00:00:00:00:00:00" {
                    i.mac = mac.to_string();
                }
            }
            // Only what NAMES a peer: `ip -d` adds tuning knobs here
            // (`promiscuity 0 minmtu 68 …`) that the plain view folds away.
            let rest: Vec<&str> = toks.collect();
            for w in rest.windows(2) {
                if w[0].starts_with("link-netns") || w[0] == "permaddr" {
                    i.link.push(format!("{} {}", w[0], w[1]));
                }
            }
            continue;
        }
        if let Some(rest) = t.strip_prefix("inet6 ").or_else(|| t.strip_prefix("inet ")) {
            let toks: Vec<&str> = rest.split_whitespace().collect();
            let addr = toks.first().copied().unwrap_or("").to_string();
            last_addr = addr.clone();
            let mut kept: Vec<String> = Vec::new();
            let mut k = 1;
            while k < toks.len() {
                if is_redundant(toks[k], toks.get(k + 1).copied(), &iface_name, &addr) {
                    k += if toks[k] == iface_name { 1 } else { 2 };
                    continue;
                }
                kept.push(toks[k].to_string());
                k += 1;
            }
            i.addrs.push(if kept.is_empty() {
                addr
            } else {
                format!("{addr} ({})", kept.join(" "))
            });
            continue;
        }
        if t.starts_with("valid_lft") {
            // `forever` is the ABSENCE of a lease; a real one is a fact — and
            // it belongs to the address ABOVE it, not to the interface. An
            // interface routinely holds two leases (one per family), so a
            // single per-interface slot silently kept the last one.
            let toks: Vec<&str> = t.split_whitespace().collect();
            let v = toks.get(1).copied().unwrap_or("forever");
            let p = toks.get(3).copied().unwrap_or("forever");
            if (v != "forever" || p != "forever") && !i.addrs.is_empty() {
                let last = i.addrs.len() - 1;
                let entry = &mut i.addrs[last];
                if entry.ends_with(')') {
                    entry.truncate(entry.len() - 1);
                    entry.push_str(&format!(", lease {v}/{p})"));
                } else {
                    entry.push_str(&format!(" (lease {v}/{p})"));
                }
            }
            continue;
        }
        // `ip -s link` counters arrive as two lines: a header naming the
        // columns, then the values. Held apart they reach `render` as two
        // unrelated `·` items — `RX: bytes packets errors dropped missed
        // mcast · 49685952 323833 0 91637 0 0` — which is the names and the
        // numbers in the same output with nothing joining them, and a reader
        // has to count columns to learn that 91 637 of them were dropped.
        if let Some((dir, names)) = pending_cols.take() {
            let vals: Vec<&str> = t.split_whitespace().collect();
            if vals.len() == names.len()
                && vals.iter().all(|v| v.bytes().all(|b| b.is_ascii_digit()))
            {
                i.extra.push(zip_counters(&dir, &names, &vals));
                continue;
            }
            // Not the value row after all: give the header back untouched.
            i.extra.push(format!("{dir}: {}", names.join(" ")));
        }
        if let Some((dir, rest)) = t.split_once(':') {
            if matches!(dir, "RX" | "TX") && !rest.trim().is_empty() {
                pending_cols = Some((
                    dir.to_string(),
                    rest.split_whitespace().map(str::to_string).collect(),
                ));
                continue;
            }
        }
        // Unrecognised continuation (`altname`, vlan info, a field added by a
        // future iproute2): keep it rather than lose it.
        i.extra.push(t.to_string());
    }
    if let (Some(i), Some((dir, names))) = (cur.as_mut(), pending_cols.take()) {
        i.extra.push(format!("{dir}: {}", names.join(" ")));
    }
    if let Some(i) = cur.take() {
        done.push(i);
    }
    render_all(&done, &mut out);
    out.trim_end().to_string()
}

/// One counter row, names joined to their values.
///
/// A zero column is dropped — `0 carrier` on every interface on every host is
/// noise — EXCEPT `errors` and `dropped`, which are printed at zero on
/// purpose: an absent fault column cannot be told apart from one that was
/// removed, and that ambiguity is the whole problem.
fn zip_counters(dir: &str, names: &[String], vals: &[&str]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (n, v) in names.iter().zip(vals) {
        let always = matches!(n.as_str(), "errors" | "dropped");
        if always || *v != "0" {
            parts.push(format!("{v} {n}"));
        }
    }
    if parts.is_empty() {
        format!("{dir} idle")
    } else {
        format!("{dir} {}", parts.join(", "))
    }
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"1: lo: <LOOPBACK,UP,LOWER_UP> mtu 65536 qdisc noqueue state UNKNOWN group default qlen 1000
    link/loopback 00:00:00:00:00:00 brd 00:00:00:00:00:00
    inet 127.0.0.1/8 scope host lo
       valid_lft forever preferred_lft forever
    inet6 ::1/128 scope host
       valid_lft forever preferred_lft forever
2: enp0s3: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 qdisc fq_codel state UP group default qlen 1000
    link/ether 08:00:27:aa:bb:cc brd ff:ff:ff:ff:ff:ff
    inet 192.168.1.42/24 brd 192.168.1.255 scope global dynamic noprefixroute enp0s3
       valid_lft 86392sec preferred_lft 75592sec
    inet6 fe80::a00:27ff:feaa:bbcc/64 scope link noprefixroute
       valid_lft forever preferred_lft forever
"#;

    /// How the gain scales with the machine. The two-interface sample above is
    /// a laptop; the question that matters is what a SERVER looks like,
    /// where the repeated lifetime lines are two per address and the addresses
    /// multiply. Printed, not asserted: this is the measurement the decision
    /// needs, and pinning it would only teach the test to the fixture.
    #[test]
    fn how_the_gain_scales_with_interface_count() {
        for n in [2usize, 8, 20, 60] {
            let mut s = String::new();
            for i in 0..n {
                s.push_str(&format!(
                    "{i}: eth{i}: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 qdisc fq_codel state UP group default qlen 1000\n    link/ether 08:00:27:aa:bb:{i:02x} brd ff:ff:ff:ff:ff:ff\n    inet 10.0.{i}.42/24 brd 10.0.{i}.255 scope global dynamic noprefixroute eth{i}\n       valid_lft 86392sec preferred_lft 75592sec\n    inet6 fe80::a00:27ff:feaa:{i:04x}/64 scope link noprefixroute\n       valid_lft forever preferred_lft forever\n"
                ));
            }
            let (_out, sav) = compact_with_savings(&s);
            println!(
                "{n:>3} interfaces: {} -> {} tokens ({:.1}%)",
                sav.tokens_before,
                sav.tokens_after,
                sav.percent()
            );
        }
    }

    #[test]
    fn detects_ip_addr() {
        assert!(detect(SAMPLE));
    }

    /// The contract of the reformulation: the SHAPE changes, the FACTS do not.
    /// Every address, every MAC, every real lease, the state, the mtu and any
    /// field the parser does not recognise must still be in the output — a
    /// compactor that quietly loses a value is worse than one that saves less.
    #[test]
    fn every_fact_survives_the_reformulation() {
        let src = "1: lo: <LOOPBACK,UP,LOWER_UP> mtu 65536 qdisc noqueue state UNKNOWN group default qlen 1000\n    \
                   link/loopback 00:00:00:00:00:00 brd 00:00:00:00:00:00\n    \
                   inet 127.0.0.1/8 scope host lo\n       \
                   valid_lft forever preferred_lft forever\n\
                   2: enp0s3: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 qdisc fq_codel state UP group default qlen 1000\n    \
                   link/ether 08:00:27:aa:bb:cc brd ff:ff:ff:ff:ff:ff\n    \
                   altname enx080027aabbcc\n    \
                   inet 192.168.1.42/24 brd 192.168.1.255 scope global dynamic noprefixroute enp0s3\n       \
                   valid_lft 86392sec preferred_lft 75592sec\n    \
                   inet6 2001:db8::1/64 scope global dynamic\n       \
                   valid_lft 1200sec preferred_lft 600sec\n    \
                   inet6 fe80::a00:27ff:feaa:bbcc/64 scope link noprefixroute\n       \
                   valid_lft forever preferred_lft forever\n\
                   3: docker0: <NO-CARRIER,BROADCAST,MULTICAST,UP> mtu 1500 qdisc noqueue state DOWN group default\n    \
                   link/ether 02:42:1a:2b:3c:4d brd ff:ff:ff:ff:ff:ff\n";
        let out = compact(src);
        for fact in [
            // names and states
            "lo", "enp0s3", "docker0", "UNKNOWN", "UP", "DOWN",
            // every address, both families, all three interfaces
            "127.0.0.1/8", "192.168.1.42/24", "2001:db8::1/64", "fe80::a00:27ff:feaa:bbcc/64",
            // every MAC that is not the all-zero loopback placeholder
            "08:00:27:aa:bb:cc", "02:42:1a:2b:3c:4d",
            // mtu, and the flag that is NOT implied by the state
            "65536", "1500", "NO-CARRIER",
            // BOTH leases of the same interface — one per family
            "86392sec", "75592sec", "1200sec", "600sec",
            // a field the parser does not know
            "altname enx080027aabbcc",
        ] {
            assert!(out.contains(fact), "lost {fact:?} in:\n{out}");
        }
        // …and the reformulation is what earns the tokens: one line each.
        assert_eq!(out.lines().count(), 3, "one line per interface:\n{out}");
    }

    /// The redundancy rules, stated so they cannot drift into dropping facts.
    #[test]
    fn only_provably_redundant_values_are_dropped() {
        let src = "2: enp0s3: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 qdisc fq_codel state UP group default qlen 1000\n    \
                   link/ether 08:00:27:aa:bb:cc brd ff:ff:ff:ff:ff:ff\n    \
                   inet 192.168.1.42/24 brd 192.168.1.255 scope global enp0s3\n       \
                   valid_lft forever preferred_lft forever\n";
        let out = compact(src);
        for gone in [
            "ff:ff:ff:ff:ff:ff", // the broadcast MAC of every Ethernet link
            "192.168.1.255",     // the broadcast address IS addr + prefix
            "group default",     // the default
            "qlen 1000",         // the default
            "valid_lft",         // `forever` is the absence of a lease
            "scope global",      // restates a routable address
        ] {
            assert!(!out.contains(gone), "kept redundant {gone:?}:\n{out}");
        }
        // The interface name is dropped from the END of its own inet line, not
        // from the line that names the interface.
        assert!(out.contains("enp0s3"), "lost the interface name:\n{out}");
    }

    /// A `link/none` interface (WireGuard, tun) has no MAC at all, and the
    /// loopback's all-zero one is a placeholder rather than an address.
    #[test]
    fn an_interface_without_a_mac_does_not_grow_one() {
        let src = "3: wg0: <POINTOPOINT,NOARP,UP,LOWER_UP> mtu 1420 qdisc noqueue state UNKNOWN group default qlen 1000\n    \
                   link/none\n    \
                   inet 10.8.0.2/32 scope global wg0\n       \
                   valid_lft forever preferred_lft forever\n";
        let out = compact(src);
        assert!(out.contains("10.8.0.2/32"), "lost the address:\n{out}");
        assert!(out.contains("POINTOPOINT") && out.contains("NOARP"), "lost a flag:\n{out}");
        assert!(!out.contains("mac "), "invented a MAC:\n{out}");
    }

    #[test]
    fn savings_real_shape() {
        let (_out, sav) = compact_with_savings(SAMPLE);
        // 48 % since the reformulation (reformulated 2026-08-03).
        // It was 6,7 % when the compactor only dropped the `valid_lft forever`
        // lines — the cost of `ip addr` is not its noise lines, it is the six
        // lines and four indentation levels each interface spends to say four
        // facts. Flat across machine size: 2 interfaces 48,5 %, 60 → 47,9 %.
        assert!(sav.percent() > 40.0, "got {sav:?}");
    }

    #[test]
    fn rejects_random_text() {
        assert!(!detect("hello world"));
    }
    const STATS: &str = "\
1: lo: <LOOPBACK,UP,LOWER_UP> mtu 65536 qdisc noqueue state UNKNOWN mode DEFAULT group default qlen 1000
    link/loopback 00:00:00:00:00:00 brd 00:00:00:00:00:00
    RX:  bytes  packets errors dropped  missed   mcast
    1889466881 19004325      0       0       0       0
    TX:  bytes  packets errors dropped carrier collsns
    1889466881 19004325      0       0       0       0
3: wlan0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 qdisc noqueue state UP mode DORMANT group default qlen 1000
    link/ether bc:f4:d4:c1:31:6f brd ff:ff:ff:ff:ff:ff
    RX:  bytes packets errors dropped  missed   mcast
    49685952  323833      0   91637       0       0
    TX:  bytes packets errors dropped carrier collsns
    13428850  113941      0       6       0       0
";

    /// `ip -s link` prints the column names on one line and the values on the
    /// next. Kept apart they reach the reader as `RX: bytes packets errors
    /// dropped missed mcast · 49685952 323833 0 91637 0 0`, and learning that
    /// 91 637 packets were dropped means counting columns by hand.
    #[test]
    fn a_counter_row_is_joined_to_its_column_names() {
        let out = compact(STATS);
        assert!(
            out.contains("91637 dropped"),
            "the drop count is not joined to its name:\n{out}"
        );
        assert!(
            out.contains("49685952 bytes"),
            "the byte count is not joined to its name:\n{out}"
        );
        assert!(
            !out.contains("errors dropped"),
            "the bare header row survived:\n{out}"
        );
    }

    /// A zero column is noise — `0 carrier` on every host — EXCEPT the fault
    /// columns, which are printed at zero on purpose: an absent fault column
    /// cannot be told from a removed one.
    #[test]
    fn a_zero_fault_column_is_still_printed() {
        let out = compact(STATS);
        assert!(out.contains("0 errors"), "faults went silent:\n{out}");
        assert!(!out.contains("0 carrier"), "zero noise kept:\n{out}");
        assert!(!out.contains("0 collsns"), "zero noise kept:\n{out}");
    }

    /// A `key value` pair the head parser does not name stays ONE fact.
    #[test]
    fn a_key_value_head_token_stays_one_fact() {
        let out = compact(STATS);
        assert!(out.contains("mode DORMANT"), "split into two facts:\n{out}");
        assert!(!out.contains("mode · DORMANT"), "{out}");
    }

    /// A counter header with no value row after it is handed back, not eaten.
    #[test]
    fn a_header_without_values_is_returned() {
        let text = "1: lo: <LOOPBACK,UP> mtu 65536 qdisc noqueue state UNKNOWN\n\
                        link/loopback 00:00:00:00:00:00 brd 00:00:00:00:00:00\n\
                    RX:  bytes  packets errors dropped\n\
                    something that is not a value row\n";
        let out = compact(text);
        assert!(out.contains("RX:"), "the header was swallowed:\n{out}");
        assert!(out.contains("not a value row"), "{out}");
    }

    /// Docker's bridge ports: the facts they share said once, each one's
    /// name, address and MAC still on its own line.
    #[test]
    fn interfaces_alike_are_described_once() {
        let mut s = String::from("350: br-08a8: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 qdisc noqueue state UP group default\n    link/ether da:2c:9c:1d:f4:da brd ff:ff:ff:ff:ff:ff\n    inet 172.23.0.1/16 brd 172.23.255.255 scope global br-08a8\n       valid_lft forever preferred_lft forever\n");
        for k in 0..4 {
            s.push_str(&format!("{n}: veth{k}@if2: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 qdisc noqueue master br-08a8 state UP group default qlen 0\n    link/ether 92:44:0c:bb:c4:5{k} brd ff:ff:ff:ff:ff:ff link-netnsid {k}\n    inet6 fe80::7cd8:78ff:fea7:cb9{k}/64 scope link proto kernel_ll\n       valid_lft forever preferred_lft forever\n", n = 1650 + k));
        }
        let out = compact(&s);
        assert_eq!(out.matches("master br-08a8").count(), 1, "{out}");
        assert!(out.contains("4 interfaces alike"), "{out}");
        for k in 0..4 {
            assert!(out.contains(&format!("veth{k}@if2 · fe80::7cd8:78ff:fea7:cb9{k}/64 · mac 92:44:0c:bb:c4:5{k} · link-netnsid {k}")), "{out}");
        }
        assert!(out.contains("172.23.0.1/16"), "{out}");
        assert!(!out.contains("brd"), "the broadcast MAC is not a fact of its own:\n{out}");
    }
}
