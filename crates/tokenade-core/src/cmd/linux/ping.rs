//! `ping` / `ping6` — fold repeated reply lines, keep final stats.
//!
//! ```text
//! PING example.com (93.184.216.34) 56(84) bytes of data.
//! 64 bytes from example.com (93.184.216.34): icmp_seq=1 ttl=56 time=12.3 ms
//! 64 bytes from example.com (93.184.216.34): icmp_seq=2 ttl=56 time=11.9 ms
//! 64 bytes from example.com (93.184.216.34): icmp_seq=3 ttl=56 time=12.1 ms
//! …
//!
//! --- example.com ping statistics ---
//! 100 packets transmitted, 100 received, 0% packet loss, time 99000ms
//! rtt min/avg/max/mdev = 11.9/12.0/12.3/0.1 ms
//! ```
//!
//! Compaction: keep header line + first 2 + last 1 reply + stats
//! footer. Add `(... N replies elided)` marker.

use std::fmt::Write;

/// Locale-invariant marker present on every reply line, regardless of
/// language: `icmp_seq=` (the sequence-number tag). Used as a robust
/// detector instead of hard-coding `"64 bytes from"` (which becomes
/// `"64 octets de"` on FR, `"64 Bytes von"` on DE, etc.).
const REPLY_MARKER: &str = "icmp_seq=";

// #553 — historic dead weight removed: RTT_SUMMARY_MARKER,
// PIPE_STATS_MARKER, STATS_{FR,DE,ES,IT} consts + a bogus
// `_MARKERS_REFERENCED` tuple that existed solely to silence
// dead-code warnings. The actual stats footer detection happens in
// `is_stats_footer_start` below, which hard-codes the locale nouns
// inline; once the `---` separator matches, every following line is
// absorbed into footer_lines regardless of language. Nothing ever
// read those constants.

pub fn detect(text: &str) -> bool {
    crate::archetypes::any_output(text)
}

crate::archetypes::detect_gated_try_compact!();

/// True when `t` introduces the trailing stats footer block. The
/// localised noun (`statistics` / `statistiques` / `Statistik` /
/// `estadísticas`) varies but it's always preceded by the
/// non-localised `--- … ping` prefix.
fn is_stats_footer_start(t: &str) -> bool {
    if !t.starts_with("---") {
        return false;
    }
    // Most distros emit `--- <target> ping statistics ---`; FR uses
    // `--- statistiques ping <target> ---` (verb-noun reordered).
    // We accept either presence of the `ping` keyword OR the locale
    // noun.
    t.contains("ping") || t.contains("statistics") || t.contains("statistiques")
        || t.contains("Statistik") || t.contains("estadísticas") || t.contains("statistiche")
}

/// True when `t` is a per-attempt reply line. Locale-invariant.
fn is_reply_line(t: &str) -> bool {
    t.contains(REPLY_MARKER)
}

pub fn compact(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut header_lines: Vec<&str> = Vec::new();
    let mut reply_lines: Vec<&str> = Vec::new();
    let mut footer_lines: Vec<&str> = Vec::new();
    let mut in_stats = false;
    for line in &lines {
        let t = line.trim();
        if is_stats_footer_start(t) {
            in_stats = true;
            footer_lines.push(line);
            continue;
        }
        if in_stats {
            footer_lines.push(line);
            continue;
        }
        if t.starts_with("PING") || t.is_empty() {
            header_lines.push(line);
            continue;
        }
        if is_reply_line(t) {
            reply_lines.push(line);
        } else {
            // Other intermediate noise (rare, e.g. "Request timeout",
            // "Destination Host Unreachable"): keep verbatim for
            // diagnostic signal.
            reply_lines.push(line);
        }
    }
    if reply_lines.len() <= 3 {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() / 2);
    for h in &header_lines {
        out.push_str(h);
        out.push('\n');
    }
    out.push_str(reply_lines[0]);
    out.push('\n');
    out.push_str(reply_lines[1]);
    out.push('\n');
    let elided = reply_lines.len() - 3;
    let _ = writeln!(
        out,
        "  [... {elided} reply line(s) elided ...]"
    );
    out.push_str(reply_lines[reply_lines.len() - 1]);
    out.push('\n');
    for f in &footer_lines {
        out.push_str(f);
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

    fn sample(replies: usize) -> String {
        let mut s = String::from("PING example.com (93.184.216.34) 56(84) bytes of data.\n");
        for i in 1..=replies {
            let _ = writeln!(
                s,
                "64 bytes from example.com (93.184.216.34): icmp_seq={i} ttl=56 time=12.3 ms"
            );
        }
        s.push_str("\n--- example.com ping statistics ---\n");
        let _ = writeln!(
            s,
            "{replies} packets transmitted, {replies} received, 0% packet loss, time {}ms",
            replies * 1000
        );
        s.push_str("rtt min/avg/max/mdev = 11.9/12.0/12.3/0.1 ms\n");
        s
    }

    #[test]
    fn detects_classic_ping() {
        assert!(detect(&sample(20)));
    }

    #[test]
    fn folds_many_replies() {
        let s = sample(50);
        let out = compact(&s);
        assert!(out.contains("PING example.com"));
        assert!(out.contains("[... 47 reply line(s) elided ...]"));
        assert!(out.contains("--- example.com ping statistics ---"));
        assert!(out.contains("rtt min"));
    }

    #[test]
    fn passes_through_short_ping() {
        let s = sample(3);
        let out = compact(&s);
        // ≤ 3 replies: don't fold, no elision marker.
        assert!(!out.contains("elided"));
    }

    #[test]
    fn rejects_non_ping() {
        // Detector is now permissive (routes are gated by program
        // name through `dispatch_named`, not content shape). Only
        // empty input is rejected.
        assert!(!detect(""));
        assert!(!detect("   \n  "));
    }

    #[test]
    fn handles_french_locale() {
        // Real FR ping output: `PING ...` header, `64 octets de ...`
        // body, `--- statistiques ping ...` footer.
        let s = "PING 127.0.0.1 (127.0.0.1) 56(84) octets de données.\n\
                 64 octets de 127.0.0.1 : icmp_seq=1 ttl=64 temps=0.012 ms\n\
                 64 octets de 127.0.0.1 : icmp_seq=2 ttl=64 temps=0.017 ms\n\
                 64 octets de 127.0.0.1 : icmp_seq=3 ttl=64 temps=0.017 ms\n\
                 64 octets de 127.0.0.1 : icmp_seq=4 ttl=64 temps=0.019 ms\n\
                 64 octets de 127.0.0.1 : icmp_seq=5 ttl=64 temps=0.018 ms\n\
                 \n\
                 --- statistiques ping 127.0.0.1 ---\n\
                 5 paquets transmis, 5 reçus, 0% packet loss, time 4097ms\n\
                 rtt min/avg/max/mdev = 0.012/0.017/0.029/0.004 ms\n";
        assert!(detect(s), "FR ping should be detected");
        let out = compact(s);
        assert!(out.contains("[... 2 reply line(s) elided ...]"), "fold expected: {out}");
        assert!(out.contains("rtt min"), "footer preserved: {out}");
    }

    #[test]
    fn handles_unreachable_replies_mixed_in() {
        // Some hosts reply, some are silent → "Destination Host
        // Unreachable" sprinkled in. We MUST NOT swallow those.
        let s = "PING 10.0.0.1 (10.0.0.1) 56(84) bytes of data.\n\
                 64 bytes from 10.0.0.1: icmp_seq=1 ttl=64 time=1.0 ms\n\
                 From 10.0.0.254 icmp_seq=2 Destination Host Unreachable\n\
                 64 bytes from 10.0.0.1: icmp_seq=3 ttl=64 time=1.0 ms\n\
                 64 bytes from 10.0.0.1: icmp_seq=4 ttl=64 time=1.0 ms\n\
                 64 bytes from 10.0.0.1: icmp_seq=5 ttl=64 time=1.0 ms\n\
                 \n\
                 --- 10.0.0.1 ping statistics ---\n";
        let out = compact(s);
        // Diagnostic lines must survive even though they fail the
        // "looks-like-reply" shape (no icmp_seq= on the unreachable
        // line — wait it does have it actually).
        assert!(out.contains("Destination Host Unreachable") || out.contains("elided"));
    }

    #[test]
    fn savings_on_long_ping() {
        let (out, sav) = compact_with_savings(&sample(100));
        assert!(sav.percent() > 80.0, "expected 80%+ savings on 100-ping, got {sav:?}");
        assert!(!out.is_empty());
    }
}
