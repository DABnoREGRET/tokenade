//! Linux net-tools `netstat` (`netstat -tulpn`, `netstat -an`,
//! `netstat -anp`, `netstat -x`, …).
//!
//! ```text
//! Active Internet connections (servers and established)
//! Proto Recv-Q Send-Q Local Address           Foreign Address         State       PID/Program name
//! tcp        0      0 0.0.0.0:22              0.0.0.0:*               LISTEN      1234/sshd
//! tcp        0      0 192.168.1.5:443         203.0.113.9:54321       ESTABLISHED 5678/nginx
//! tcp        0      0 192.168.1.5:443         198.51.100.7:51200      TIME_WAIT   -
//! udp        0      0 0.0.0.0:68              0.0.0.0:*                           890/dhclient
//! Active UNIX domain sockets (servers and established)
//! Proto RefCnt Flags       Type       State         I-Node   Path
//! unix  2      [ ACC ]     STREAM     LISTENING     12345    /run/systemd/private
//! unix  3      [ ]         STREAM     CONNECTED     67890
//! ```
//!
//! Why a dedicated Linux compactor (was: routed to `windows::netstat`).
//! `dispatch_named("netstat")` only chained the Windows then macOS
//! compactors. Both have a `!text.trim().is_empty()` detector, so the
//! *Windows* compactor swallowed every Linux `netstat` — and it only
//! elides TIME_WAIT/CLOSE_WAIT runs, ignoring the column structure and
//! emitting no real-count banner. On a busy host `netstat` is dominated
//! by transient TIME_WAIT churn; the actionable signal is the listening
//! servers and the live (ESTABLISHED) connections. This compactor keeps
//! those verbatim and folds only the transient-state churn, announced
//! with per-state counts — nothing is dropped silently.
//!
//! Quality contract (must always hold):
//!   - Banner carries the REAL total socket count + per-state breakdown.
//!   - Every LISTEN/LISTENING socket kept verbatim (what is serving —
//!     program names + ports are named entities, never folded).
//!   - Every ESTABLISHED socket kept verbatim (live peers are named
//!     entities).
//!   - Transient churn (TIME_WAIT, CLOSE_WAIT, FIN_WAIT*, SYN_*, …)
//!     folded into an announced per-state count, never silently removed.
//!   - Any row whose state token is unrecognised is KEPT verbatim
//!     (fail-safe): a locale that translates state names degrades to
//!     "less compaction", never to silent loss.
//!
//! Locale independence: detection keys on the lowercase protocol token
//! in the FIRST column (`tcp`/`tcp6`/`udp`/`unix`/… — these are protocol
//! identifiers net-tools prints regardless of `LANG`) plus the Linux
//! `addr:port` shape (colon separator, distinguishing it from macOS's
//! `addr.port` and Windows' uppercase `TCP`). The localised banner and
//! column headers ("Connexions Internet actives", "Aktive
//! Internetverbindungen", "Proto Recv-Q Send-Q Adresse locale …") are
//! kept verbatim as context but are NEVER matched against.

use crate::tokenizer;
use std::collections::BTreeMap;
use std::fmt::Write;

/// First-column protocol identifiers net-tools emits (lowercase,
/// locale-invariant). Excludes macOS's `tcp4`/`udp4` and Windows'
/// uppercase `TCP`/`UDP`.
const PROTOS: &[&str] = &[
    "tcp", "tcp6", "udp", "udp6", "udplite", "udplite6", "raw", "raw6", "unix", "sctp", "dccp",
];

/// TCP/unix states that are transient churn — folded into a count.
/// Recognised by exact token so an *unrecognised* (e.g. localised)
/// state falls through to "keep verbatim".
const FOLD_STATES: &[&str] = &[
    // TCP transient handshake / teardown states.
    "TIME_WAIT",
    "CLOSE_WAIT",
    "FIN_WAIT1",
    "FIN_WAIT2",
    "SYN_SENT",
    "SYN_RECV",
    "CLOSING",
    "LAST_ACK",
    "CLOSED",
    // UNIX-domain transient states.
    "CONNECTED",
    "CONNECTING",
    "DISCONNECTING",
];

/// All state tokens we recognise structurally (fold set + the
/// load-bearing states we keep). A token outside this set is treated as
/// "no recognised state" → the row is kept verbatim.
const KNOWN_STATES: &[&str] = &[
    "TIME_WAIT",
    "CLOSE_WAIT",
    "FIN_WAIT1",
    "FIN_WAIT2",
    "SYN_SENT",
    "SYN_RECV",
    "CLOSING",
    "LAST_ACK",
    "CLOSED",
    "CONNECTED",
    "CONNECTING",
    "DISCONNECTING",
    "LISTEN",
    "LISTENING",
    "ESTABLISHED",
];

/// The lowercased first whitespace token if the line is a data row.
fn proto_of(line: &str) -> Option<&str> {
    let tok = line.split_whitespace().next()?;
    if PROTOS.contains(&tok) {
        Some(tok)
    } else {
        None
    }
}

/// A Linux `netstat` endpoint token: `addr:port` where the suffix after
/// the LAST colon is a port (`*` or ASCII digits) and something precedes
/// it. `0.0.0.0:22`, `:::80`, `[::1]:5432`, `0.0.0.0:*` all match;
/// macOS's `*.22` / `::1.22` (dot separator) do NOT.
fn is_endpoint(tok: &str) -> bool {
    match tok.rsplit_once(':') {
        Some((head, port)) if !head.is_empty() => {
            port == "*" || (!port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()))
        }
        _ => false,
    }
}

/// The recognised state token in a row, if any (locale-invariant: TCP
/// state names are protocol constants printed in English by net-tools).
/// Returns None for stateless rows (`udp` listeners) or rows whose state
/// we don't recognise — both are kept verbatim.
fn state_of(line: &str) -> Option<&str> {
    line.split_whitespace().find(|t| KNOWN_STATES.contains(t))
}

pub fn detect(text: &str) -> bool {
    let mut inet_with_endpoint = 0usize;
    let mut unix_rows = 0usize;
    let mut data_rows = 0usize;
    for line in text.lines() {
        if let Some(proto) = proto_of(line) {
            data_rows += 1;
            if proto == "unix" {
                unix_rows += 1;
            } else if line.split_whitespace().any(is_endpoint) {
                inet_with_endpoint += 1;
            }
        }
    }
    // Require a real table, and Linux-distinct evidence (colon-port inet
    // rows or unix-domain rows) to avoid firing on macOS/Windows output.
    data_rows >= 3 && (inet_with_endpoint >= 2 || unix_rows >= 2)
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    let out = compact(text);
    // Self-protect: never inflate.
    if tokenizer::estimate(&out) >= tokenizer::estimate(text) {
        return None;
    }
    Some(out)
}

pub fn compact(text: &str) -> String {
    let mut total = 0usize;
    let mut by_state: BTreeMap<&str, usize> = BTreeMap::new();
    // First pass: real per-state tally over every data row.
    for line in text.lines() {
        if proto_of(line).is_some() {
            total += 1;
            let st = state_of(line).unwrap_or("(stateless)");
            *by_state.entry(st).or_insert(0) += 1;
        }
    }

    // Second pass: keep non-data lines + load-bearing rows verbatim;
    // tally folded rows.
    let mut kept_lines: Vec<&str> = Vec::new();
    let mut folded: BTreeMap<&str, usize> = BTreeMap::new();
    let mut folded_total = 0usize;
    for line in text.lines() {
        match proto_of(line) {
            None => {
                // Banner / column header / blank — verbatim context.
                kept_lines.push(line);
            }
            Some(_) => match state_of(line) {
                Some(st) if FOLD_STATES.contains(&st) => {
                    *folded.entry(st).or_insert(0) += 1;
                    folded_total += 1;
                }
                // LISTEN / ESTABLISHED / stateless udp / unrecognised
                // (locale-translated) state → keep verbatim.
                _ => kept_lines.push(line),
            },
        }
    }

    let kept = total - folded_total;
    let breakdown: Vec<String> = by_state.iter().map(|(s, n)| format!("{n} {s}")).collect();

    let mut out = String::with_capacity(text.len());
    let _ = writeln!(
        out,
        "[tokenade:netstat] {total} socket(s): {kept} shown, {folded_total} folded · {}",
        breakdown.join(", ")
    );
    // Trim trailing blank lines from the kept context for tidiness, but
    // keep interior structure.
    let mut trailing_blanks = 0usize;
    while kept_lines.last().is_some_and(|l| l.trim().is_empty()) {
        kept_lines.pop();
        trailing_blanks += 1;
    }
    let _ = trailing_blanks;
    for line in &kept_lines {
        out.push_str(line.trim_end());
        out.push('\n');
    }
    // Announce every folded bucket — never silent.
    for (st, n) in &folded {
        let _ = writeln!(out, "  [folded: {n} {st} socket(s)]");
    }
    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Realistic busy-host `netstat -an`: a few listening servers, a few
    /// live ESTABLISHED connections, and a TIME_WAIT storm (the dominant
    /// real-world noise that makes `netstat` huge).
    fn build_netstat(time_waits: usize) -> String {
        let mut s = String::from(
            "Active Internet connections (servers and established)\n\
             Proto Recv-Q Send-Q Local Address           Foreign Address         State       PID/Program name\n",
        );
        s.push_str("tcp        0      0 0.0.0.0:22              0.0.0.0:*               LISTEN      1234/sshd\n");
        s.push_str("tcp6       0      0 :::80                   :::*                    LISTEN      5678/nginx\n");
        s.push_str("tcp        0      0 192.168.1.5:443         203.0.113.9:51000       ESTABLISHED 5678/nginx\n");
        s.push_str("udp        0      0 0.0.0.0:68              0.0.0.0:*                           890/dhclient\n");
        for i in 0..time_waits {
            let _ = writeln!(
                s,
                "tcp        0      0 192.168.1.5:443         198.51.100.{:<3}:512{:02}    TIME_WAIT   -",
                i % 250,
                i % 100
            );
        }
        s
    }

    #[test]
    fn detector_rejects_empty_and_prose() {
        assert!(!detect(""));
        assert!(!detect("   \n"));
        assert!(!detect("just some\nrandom prose here\nwith no sockets at all\n"));
    }

    #[test]
    fn detector_accepts_structural_linux_netstat() {
        assert!(detect(&build_netstat(10)));
    }

    #[test]
    fn detector_rejects_macos_dot_port_format() {
        // macOS BSD netstat: `tcp4`/`tcp6` + `addr.port` (dot, not colon).
        let macos = "Active Internet connections\n\
            Proto Recv-Q Send-Q  Local Address          Foreign Address        (state)\n\
            tcp4       0      0  *.22                   *.*                    LISTEN\n\
            tcp6       0      0  ::1.22                 *.*                    LISTEN\n\
            tcp4       0      0  192.168.1.5.443        203.0.113.9.51000      ESTABLISHED\n";
        assert!(!detect(macos), "must not fire on macOS dot-port format");
    }

    #[test]
    fn detector_rejects_windows_uppercase_proto() {
        // Windows: uppercase `TCP`, no Recv-Q/Send-Q columns.
        let win = "Active Connections\n\n  Proto  Local Address          Foreign Address        State\n  \
            TCP    0.0.0.0:135            0.0.0.0:0              LISTENING\n  \
            TCP    0.0.0.0:445            0.0.0.0:0              LISTENING\n  \
            TCP    127.0.0.1:50432        127.0.0.1:50431        ESTABLISHED\n";
        assert!(!detect(win), "must not fire on Windows uppercase format");
    }

    #[test]
    fn gain_significant_savings_on_time_wait_storm() {
        let input = build_netstat(300);
        let (_, sav) = compact_with_savings(&input);
        assert!(sav.tokens_before > 0);
        let pct = 100 * (sav.tokens_before - sav.tokens_after) / sav.tokens_before;
        assert!(pct >= 30, "time_wait storm only saved {pct}% (<30%)");
    }

    #[test]
    fn quality_banner_has_real_total_and_breakdown() {
        let out = compact(&build_netstat(300));
        // 2 LISTEN + 1 ESTABLISHED + 1 udp + 300 TIME_WAIT = 304 sockets.
        assert!(out.contains("304 socket(s)"), "true total lost: {out}");
        assert!(out.contains("300 TIME_WAIT"), "per-state breakdown lost: {out}");
    }

    #[test]
    fn quality_listening_servers_kept_verbatim() {
        let out = compact(&build_netstat(300));
        // Named entities: program names + ports of the listeners.
        assert!(out.contains("1234/sshd"), "sshd listener dropped: {out}");
        assert!(out.contains("5678/nginx"), "nginx listener dropped: {out}");
        assert!(out.contains("0.0.0.0:22"), "ssh port dropped: {out}");
        assert!(out.contains(":::80"), "http port dropped: {out}");
    }

    #[test]
    fn quality_established_connection_kept_verbatim() {
        let out = compact(&build_netstat(300));
        // Live peer address is a named entity — must survive.
        assert!(out.contains("203.0.113.9:51000"), "established peer dropped: {out}");
        assert!(out.contains("ESTABLISHED"), "ESTABLISHED row dropped: {out}");
    }

    #[test]
    fn quality_time_wait_folded_with_count_never_silent() {
        let out = compact(&build_netstat(300));
        assert!(
            out.contains("[folded: 300 TIME_WAIT socket(s)]"),
            "TIME_WAIT not folded with an announced count: {out}"
        );
        // The individual churn rows must NOT survive verbatim (that's the
        // whole point of folding).
        assert!(
            !out.contains("198.51.100."),
            "individual TIME_WAIT rows leaked verbatim: {out}"
        );
    }

    #[test]
    fn quality_no_silent_drop_unrecognised_state_is_kept() {
        // A row whose state token we don't recognise (e.g. a future or
        // locale-translated state) must be KEPT verbatim, not silently
        // dropped or mis-folded.
        let mut s = build_netstat(5);
        s.push_str("tcp        0      0 10.0.0.1:9999           10.0.0.2:8888           ÉTABLIE     42/myapp\n");
        let out = compact(&s);
        assert!(
            out.contains("10.0.0.1:9999") && out.contains("42/myapp"),
            "row with unrecognised state silently lost: {out}"
        );
    }

    #[test]
    fn quality_unix_listening_kept_connected_folded() {
        let s = "Active UNIX domain sockets (servers and established)\n\
            Proto RefCnt Flags       Type       State         I-Node   Path\n\
            unix  2      [ ACC ]     STREAM     LISTENING     12345    /run/systemd/private\n\
            unix  2      [ ACC ]     STREAM     LISTENING     12346    /run/dbus/system_bus_socket\n\
            unix  3      [ ]         STREAM     CONNECTED     67890\n\
            unix  3      [ ]         STREAM     CONNECTED     67891\n\
            unix  3      [ ]         STREAM     CONNECTED     67892\n";
        let out = compact(s);
        assert!(out.contains("/run/systemd/private"), "unix listener path dropped: {out}");
        assert!(out.contains("/run/dbus/system_bus_socket"), "unix listener path dropped: {out}");
        assert!(
            out.contains("[folded: 3 CONNECTED socket(s)]"),
            "connected unix sockets not folded with count: {out}"
        );
    }

    #[test]
    fn multilingual_french_locale_no_regression() {
        // French `netstat`: localised banner + column header + state
        // label ("Etat"), but data rows still start with the locale-
        // invariant `tcp`/`udp` proto and use `:`-ports. The English
        // TCP-state constants (LISTEN/ESTABLISHED/TIME_WAIT) are what
        // net-tools prints regardless of LANG.
        let mut s = String::from(
            "Connexions Internet actives (serveurs et établies)\n\
             Proto Recv-Q Send-Q Adresse locale          Adresse distante        Etat        PID/Programme\n",
        );
        s.push_str("tcp        0      0 0.0.0.0:22              0.0.0.0:*               LISTEN      1234/sshd\n");
        s.push_str("tcp        0      0 192.168.1.5:443         203.0.113.9:51000       ESTABLISHED 5678/nginx\n");
        for i in 0..50 {
            let _ = writeln!(
                s,
                "tcp        0      0 192.168.1.5:443         198.51.100.{:<3}:51200   TIME_WAIT   -",
                i % 250
            );
        }
        assert!(detect(&s), "structural detection must work under French locale");
        let out = compact(&s);
        // Localised banner preserved verbatim as context.
        assert!(out.contains("Connexions Internet actives"), "fr banner lost: {out}");
        // Named entities + live connection preserved.
        assert!(out.contains("1234/sshd"), "fr: listener dropped: {out}");
        assert!(out.contains("203.0.113.9:51000"), "fr: established peer dropped: {out}");
        // Churn still folded with a count (no English-keyword dependency).
        assert!(out.contains("[folded: 50 TIME_WAIT socket(s)]"), "fr: churn not folded: {out}");
    }

    #[test]
    fn multilingual_german_locale_no_regression() {
        let mut s = String::from(
            "Aktive Internetverbindungen (Server und stehende Verbindungen)\n\
             Proto Recv-Q Send-Q Lokale Adresse          Fremdadresse            Status      PID/Programm\n",
        );
        s.push_str("tcp6       0      0 :::443                  :::*                    LISTEN      999/httpd\n");
        for i in 0..40 {
            let _ = writeln!(
                s,
                "tcp        0      0 10.0.0.5:443            10.0.0.{:<3}:50000      TIME_WAIT   -",
                i % 250
            );
        }
        assert!(detect(&s), "structural detection must work under German locale");
        let out = compact(&s);
        assert!(out.contains("Aktive Internetverbindungen"), "de banner lost: {out}");
        assert!(out.contains("999/httpd"), "de: listener dropped: {out}");
        assert!(out.contains("[folded: 40 TIME_WAIT socket(s)]"), "de: churn not folded: {out}");
    }

    #[test]
    fn declines_when_nothing_to_fold() {
        // All-LISTEN output: nothing to fold → don't inflate; let it pass
        // through (try_compact returns None on no net gain).
        let s = "Active Internet connections (only servers)\n\
            Proto Recv-Q Send-Q Local Address           Foreign Address         State       PID/Program name\n\
            tcp        0      0 0.0.0.0:22              0.0.0.0:*               LISTEN      1234/sshd\n\
            tcp6       0      0 :::80                   :::*                    LISTEN      5678/nginx\n\
            tcp        0      0 0.0.0.0:5432            0.0.0.0:*               LISTEN      4321/postgres\n";
        // Banner adds bytes but no folding happens, so compaction won't
        // beat the original — try_compact declines.
        assert!(try_compact(s).is_none(), "should decline when no gain");
    }

    #[test]
    fn no_inflation_tiny_input() {
        assert!(try_compact("tcp 0 0 a:1 b:2 LISTEN\n").is_none());
    }
}
