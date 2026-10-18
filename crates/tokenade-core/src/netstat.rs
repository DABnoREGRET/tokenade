//! `netstat` / `ss` / `lsof -i` network-listing compaction.
//!
//! Network-debug sessions hit this constantly. Real production servers
//! produce thousands of connections:
//!
//! ```text
//! Active Internet connections (servers and established)
//! Proto Recv-Q Send-Q Local Address           Foreign Address         State       PID/Program
//! tcp        0      0 0.0.0.0:22              0.0.0.0:*               LISTEN      1234/sshd
//! tcp        0      0 0.0.0.0:80              0.0.0.0:*               LISTEN      5678/nginx
//! tcp        0      0 192.168.1.1:80          10.0.0.5:34567          ESTABLISHED 5678/nginx
//! tcp        0      0 192.168.1.1:80          10.0.0.6:34568          ESTABLISHED 5678/nginx
//! …5 000 more ESTABLISHED rows…
//! tcp        0      0 192.168.1.1:80          10.0.0.99:65432         TIME_WAIT   -
//! ```
//!
//! What the agent reading this actually wants:
//!
//!   - per-**state** count (LISTEN / ESTABLISHED / TIME_WAIT / CLOSE_WAIT / …)
//!   - every **LISTEN** entry verbatim (those are the open ports — diagnostic gold)
//!   - top **processes** by connection count
//!   - **state distribution** for the bulk of ESTABLISHED entries
//!
//! Folds: the per-connection detail into one line per ENDPOINT PAIR, the
//! ephemeral side written `*` — fan-out from a server port is one line with
//! its count, and every peer is still named. Drops the verbose `Active
//! Internet connections` banner.

use std::collections::BTreeMap;
use std::fmt::Write;

pub const LISTEN_CAP: usize = 100;

/// Up to this many connections, the table is given as it is (see `compact`).
pub const SMALL_TABLE: usize = 40;
pub const TOP_PROCESSES_CAP: usize = 20;

pub fn detect(text: &str) -> bool {
    let head: &str = text.get(..text.len().min(8192)).unwrap_or(text);

    // netstat: explicit header banner OR column header.
    if head.contains("Active Internet connections")
        || head.contains("Active UNIX domain sockets")
        || head.contains("Proto Recv-Q Send-Q")
    {
        return true;
    }

    // ss (1) headers: `State Recv-Q Send-Q Local Address` (legacy) or
    // `Netid State Recv-Q Send-Q` (modern).
    if (head.contains("State") && head.contains("Recv-Q") && head.contains("Send-Q"))
        || head.contains("Netid State")
    {
        return true;
    }

    // lsof -i header: `COMMAND PID USER FD TYPE ... NODE NAME`.
    //
    // The header alone is NOT enough, and claiming on it cost real tokens:
    // `lsof -n` — every open FILE, not just sockets — prints the same header,
    // so it landed here and the compactor, which knows about connections,
    // returned it untouched. MEASURED 2026-08-25 on 113 KB of `lsof -n`:
    // 113 580 bytes back (1.000), against 49 569 (0.436) from the generic
    // tabular compactor that should have had it.
    //
    // The discriminator is in the DATA, not the header: a network row carries
    // `IPv4`/`IPv6` in TYPE and `TCP`/`UDP` in NODE, while a file row carries
    // `REG`, `DIR`, `CHR`, `unknown`. Requiring a few such rows keeps
    // `lsof -i` here — where it belongs, 0.097 against tabular's 0.589 — and
    // lets everything else go where it folds.
    if head.contains("COMMAND") && head.contains("TYPE") && head.contains("NODE") {
        let net_rows = head
            .lines()
            .skip(1)
            .filter(|l| {
                (l.contains("IPv4") || l.contains("IPv6"))
                    && (l.contains("TCP") || l.contains("UDP"))
            })
            .count();
        if net_rows >= 2 {
            return true;
        }
    }

    // Fallback data-line detection: many lines start with tcp/udp/tcp6/udp6
    // and contain `:port` patterns.
    let mut tcp_lines = 0usize;
    let mut total = 0usize;
    for line in head.lines().take(50) {
        let t = line.trim_start();
        if t.is_empty() {
            continue;
        }
        total += 1;
        if (t.starts_with("tcp") || t.starts_with("udp") || t.starts_with("TCP")) && t.contains(':')
        {
            tcp_lines += 1;
        }
    }
    tcp_lines >= 5 && total > 0 && (tcp_lines * 100) / total >= 60
}

/// A connection line, parsed loosely from any of {netstat, ss, lsof}.
/// `foreign` was removed in #558 as never read, and with it every peer: a
/// real `ss -t` of 117 connections (2026-10-03) answered `117 ESTAB` and
/// nothing else. It is read by [`endpoint_census`].
#[derive(Default, Clone)]
struct Conn {
    proto: String,
    local: String,
    foreign: String,
    state: String,
    process: String,
}

/// ss state keywords (column 1 of `ss -tulpn` when Netid leads).
fn is_ss_state(w: &str) -> bool {
    matches!(
        w,
        "LISTEN"
            | "ESTAB"
            | "ESTABLISHED"
            | "SYN-SENT"
            | "SYN-RECV"
            | "FIN-WAIT-1"
            | "FIN-WAIT-2"
            | "TIME-WAIT"
            | "TIME_WAIT"
            | "CLOSE"
            | "CLOSE-WAIT"
            | "CLOSE_WAIT"
            | "LAST-ACK"
            | "CLOSING"
            | "UNCONN"
    )
}

/// A netstat STATE column value: uppercase word (LISTEN, TIME_WAIT…),
/// never a `pid/program` token.
fn is_state_word(w: &str) -> bool {
    !w.is_empty()
        && !w.contains('/')
        && w.chars()
            .all(|c| c.is_ascii_uppercase() || c == '_' || c == '-' || c.is_ascii_digit())
        && w.chars().next().is_some_and(|c| c.is_ascii_uppercase())
}

/// The process `ss -p` names, read on the whole LINE: a name may hold a
/// space (`users:(("next-server (v1",pid=…))` — Next.js titles its process
/// so), and the whitespace-split token it used to come from stopped at
/// `users:(("next-server`. Measured 2026-10-03 on a real `ss -tulpn`.
fn ss_process_of_line(line: &str) -> Option<String> {
    let rest = &line[line.find("users:((\"")? + "users:((\"".len()..];
    Some(rest[..rest.find('"')?].to_string())
}

/// A socket bound for INPUT. TCP says `LISTEN`; a bound UDP socket has no
/// connection and `ss` calls it `UNCONN`, `netstat` gives it no state at all.
/// Counted as connections only, the 29 UDP ports of a real `ss -tulpn`
/// (mDNS, LLMNR, Steam, Chrome…) were "elided" — the `-u` of the command line
/// asked for exactly them.
fn is_listening(c: &Conn) -> bool {
    c.state == "LISTEN"
        || (c.proto.starts_with("udp") && matches!(c.state.as_str(), "UNCONN" | "STATELESS"))
}

/// `users:(("sshd",pid=123,fd=3))` → `sshd` ; `1234/dnsmasq` → `1234/dnsmasq`.
fn extract_process_tag(tok: &str) -> String {
    if let Some(idx) = tok.find("((\"") {
        let rest = &tok[idx + 3..];
        if let Some(end) = rest.find('"') {
            return rest[..end].to_string();
        }
    }
    tok.to_string()
}

/// `netstat -anp` style: `tcp 0 0 LOCAL FOREIGN STATE PID/PROG`.
fn parse_netstat_line(line: &str) -> Option<Conn> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 4 {
        return None;
    }
    let p0 = parts[0];
    if !matches!(
        p0,
        "tcp" | "tcp6" | "udp" | "udp6" | "raw" | "raw6" | "sctp"
            // BSD/macOS name the family: `tcp4`, `udp46` (dual-stack).
            | "tcp4" | "udp4" | "tcp46" | "udp46"
    ) {
        return None;
    }
    // Modern `ss -tulpn` rows ALSO start with the proto (`Netid` column):
    // `tcp LISTEN 0 4096 0.0.0.0:22 …` — but there the STATE is column 1
    // and the local address column 4. Parsing them with netstat's layout
    // read Send-Q as the address and the foreign addr as the state, so
    // every listening port vanished from the summary. Disambiguate on
    // column 1: a state WORD means ss layout, a number means netstat.
    if parts.get(1).is_some_and(|w| is_ss_state(w)) {
        let local = parts.get(4).unwrap_or(&"").to_string();
        let foreign = parts.get(5).unwrap_or(&"").to_string();
        let process = ss_process_of_line(line).unwrap_or_else(|| {
            parts
                .iter()
                .rev()
                .find(|p| p.contains("users:") || p.contains('/'))
                .map(|s| extract_process_tag(s))
                .unwrap_or_default()
        });
        return Some(Conn {
            proto: p0.to_string(),
            local,
            foreign,
            state: parts[1].to_string(),
            process,
        });
    }
    // Indices: [proto, recv-q, send-q, local, foreign, state, program]
    // udp lines may not have a state column.
    let local = parts.get(3).unwrap_or(&"").to_string();
    let foreign = parts.get(4).unwrap_or(&"").to_string();
    // udp rows have no state; column 5 is then the PID/program — showing
    // `1234/dnsmasq` as a STATE was wrong. Accept only state-shaped words.
    let state = parts
        .get(5)
        .filter(|w| is_state_word(w))
        .map(|s| s.to_string())
        .unwrap_or_default();
    // Program is the last field if it contains `/`.
    let process = parts
        .iter()
        .rev()
        .find(|p| p.contains('/'))
        .filter(|p| !p.starts_with('/'))
        .map(|s| s.to_string())
        .unwrap_or_default();
    Some(Conn {
        proto: p0.to_string(),
        local,
        foreign,
        state: if state.is_empty() {
            "STATELESS".to_string()
        } else {
            state
        },
        process,
    })
}

/// `ss -tan` style: `STATE Recv-Q Send-Q LOCAL FOREIGN [users:(("name",pid=N,fd=K))]`.
fn parse_ss_line(line: &str) -> Option<Conn> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 5 {
        return None;
    }
    let p0 = parts[0];
    // Known ss states.
    if !matches!(
        p0,
        "LISTEN"
            | "ESTAB"
            | "ESTABLISHED"
            | "SYN-SENT"
            | "SYN-RECV"
            | "FIN-WAIT-1"
            | "FIN-WAIT-2"
            | "TIME-WAIT"
            | "TIME_WAIT"
            | "CLOSE"
            | "CLOSE-WAIT"
            | "CLOSE_WAIT"
            | "LAST-ACK"
            | "CLOSING"
            | "UNCONN"
    ) {
        return None;
    }
    // Modern ss can include `Netid` as the first column → state in index 1.
    let local = parts.get(3).unwrap_or(&"").to_string();
    let foreign = parts.get(4).unwrap_or(&"").to_string();
    // The process field looks like `users:(("name",pid=N,...))` — extract
    // just the name as our process tag.
    let process = ss_process_of_line(line).unwrap_or_default();
    Some(Conn {
        proto: String::from("tcp"),
        local,
        foreign,
        state: p0.replace('-', "_"),
        process,
    })
}

/// `lsof -i` style: `COMMAND PID USER FD TYPE DEVICE SIZE NODE NAME`
/// where NAME is `TCP 192.168.1.1:80->10.0.0.5:34567 (ESTABLISHED)`.
fn parse_lsof_line(line: &str) -> Option<Conn> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 8 {
        return None;
    }
    // First column is command name; we need `TCP` or `UDP` somewhere.
    let proto_idx = parts.iter().position(|p| *p == "TCP" || *p == "UDP")?;
    let proto = parts[proto_idx].to_lowercase();
    let addr = parts.get(proto_idx + 1)?;
    let state_paren = parts.iter().rev().find(|p| p.starts_with('(')).copied();
    let state = state_paren
        .and_then(|p| p.strip_prefix('(').and_then(|s| s.strip_suffix(')')))
        .unwrap_or("STATELESS")
        .to_string();
    // Address may be `local->foreign` or just `local`.
    let (local, foreign) = match addr.split_once("->") {
        Some((l, f)) => (l.to_string(), f.to_string()),
        None => (addr.to_string(), String::new()),
    };
    let process = parts.first().map(|s| s.to_string()).unwrap_or_default();
    Some(Conn {
        proto,
        local,
        foreign,
        state,
        process,
    })
}

fn parse_any(line: &str) -> Option<Conn> {
    parse_netstat_line(line)
        .or_else(|| parse_ss_line(line))
        .or_else(|| parse_lsof_line(line))
}

/// `dispatch_named` entry point: compact only when this looks like Linux
/// `netstat` / `ss` / `lsof -i` output, else return `None` so the caller can
/// fall through to the next variant (macOS/Windows) or the generic path.
pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    Some(compact(text))
}

/// **A small table is its own answer.** The summary (states, listening
/// ports, top processes) is for hundreds of connections; on a few it elides
/// exactly what is asked — measured 2026-10-03 on a real `lsof -i -P -n`
/// (macOS, 3 connections, 398 bytes): the two established SSH sessions, their
/// peer address and PID, gone, to deliver 284 bytes. Up to [`SMALL_TABLE`]
/// connections, and while it fits, the table is given as it is.
pub fn compact(text: &str) -> String {
    render(text, SMALL_TABLE)
}

/// The summary alone, at any size.
pub fn summary(text: &str) -> String {
    render(text, 0)
}

fn render(text: &str, small_table_up_to: usize) -> String {
    if !detect(text) {
        return text.to_string();
    }

    let mut by_state: BTreeMap<String, usize> = BTreeMap::new();
    let mut listens: Vec<Conn> = Vec::new();
    let mut others: Vec<Conn> = Vec::new();
    let mut by_process: BTreeMap<String, usize> = BTreeMap::new();
    let mut total_conns = 0usize;
    let mut n_listen = 0usize;
    let mut unmatched: Vec<String> = Vec::new();
    let mut table: Vec<&str> = Vec::new();
    let mut last_header: Option<&str> = None;
    let mut last_conn = 0usize;

    for (idx, raw) in text.lines().enumerate() {
        if raw.trim().is_empty() {
            continue;
        }
        let Some(c) = parse_any(raw) else {
            // Headers ARE noise; anything else is another command's output.
            let t = raw.trim();
            let is_header = t.starts_with("Proto ")
                || t.starts_with("Netid ")
                || t.starts_with("State ")
                || t.starts_with("Active ")
                || t.starts_with("COMMAND ");
            if !is_header {
                unmatched.push(raw.to_string());
            } else if table.is_empty() {
                last_header = Some(raw);
            }
            continue;
        };
        if table.is_empty() {
            table.extend(last_header);
        }
        table.push(raw);
        last_conn = idx;
        total_conns += 1;
        *by_state.entry(c.state.clone()).or_insert(0) += 1;
        if !c.process.is_empty() {
            *by_process.entry(c.process.clone()).or_insert(0) += 1;
        }
        if is_listening(&c) {
            n_listen += 1;
            if listens.len() < LISTEN_CAP {
                listens.push(c);
            }
        } else {
            others.push(c);
        }
    }

    if total_conns == 0 {
        return text.to_string();
    }
    if total_conns <= small_table_up_to {
        if tk_base::delivery::fits(text) {
            return text.trim_end().to_string();
        }
        // macOS `netstat -an` follows its 36 connections with ~150 lines of
        // UNIX and kernel-control sockets: the whole did not fit, so the
        // connections went to the summary while those sockets did not.
        // The small table is still the answer; the rest is what folds.
        let mut out = table.join("\n");
        let rest = remainder(text, last_conn, &unmatched);
        if !rest.is_empty() {
            out.push_str("\n\n");
            out.push_str(&rest);
        }
        return out.trim_end().to_string();
    }

    // ---- Render ----
    let n_estab = *by_state.get("ESTABLISHED").unwrap_or(&0) + *by_state.get("ESTAB").unwrap_or(&0);

    let mut out = String::with_capacity(2048);
    let _ = writeln!(
        out,
        "{total_conns} connection(s), {n_listen} listening, {n_estab} ESTABLISHED"
    );

    // State distribution.
    out.push_str("\nstate distribution:\n");
    let mut state_sorted: Vec<_> = by_state.iter().collect();
    state_sorted.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (state, count) in &state_sorted {
        let _ = writeln!(out, "  {state:<14}{count:>6}");
    }

    // LISTEN entries (the open ports — diagnostic gold).
    if !listens.is_empty() {
        let _ = write!(out, "\nlistening ports ({}):\n", listens.len());
        // One socket per process THREAD: a real `ss -tulpn` held mDNS
        // (`224.0.0.251:5353`) seven times for chrome, alike but for pid/fd.
        let mut seen: Vec<((&str, &str, &str), usize)> = Vec::new();
        for c in &listens {
            let key = (c.proto.as_str(), c.local.as_str(), c.process.as_str());
            match seen.iter_mut().find(|(k, _)| *k == key) {
                Some((_, n)) => *n += 1,
                None => seen.push((key, 1)),
            }
        }
        for ((proto, local, process), n) in seen {
            let proc_str = if process.is_empty() {
                String::new()
            } else {
                format!("  {process}")
            };
            let times = if n > 1 {
                format!(" (×{n})")
            } else {
                String::new()
            };
            let _ = writeln!(out, "  {proto:<6} {local:<28} {proc_str}{times}");
        }
        if n_listen > listens.len() {
            let _ = writeln!(
                out,
                "  … {} more listening entr(y/ies) elided",
                n_listen - listens.len()
            );
        }
    }

    // The connections that are NOT rendered anywhere above: one line per
    // endpoint pair, every peer named. Counted by state alone (the shape this
    // replaced, which said so: `addresses not shown`), a real `ss -t` of 117
    // connections named none of them; by pair, they are 41 lines.
    if !others.is_empty() {
        let groups = endpoint_census(&others);
        let lines: Vec<String> = groups
            .iter()
            .map(|((state, pair, process), n)| {
                let proc_str = if process.is_empty() {
                    String::new()
                } else {
                    format!("  {process}")
                };
                format!("  ×{n} {state} {pair}{proc_str}")
            })
            .collect();
        let shown = if tk_base::delivery::fits(&lines.join("\n")) {
            lines.len()
        } else {
            ENDPOINT_CAP.min(lines.len())
        };
        let _ = writeln!(
            out,
            "\nconnections by endpoint ({} pair(s), ephemeral port as *):",
            groups.len()
        );
        for l in &lines[..shown] {
            let _ = writeln!(out, "{l}");
        }
        if shown < lines.len() {
            let hidden: usize = groups[shown..].iter().map(|(_, n)| n).sum();
            let _ = writeln!(
                out,
                "  … {} more pair(s), {hidden} connection(s), elided",
                lines.len() - shown
            );
        }
    }

    // Top processes by connection count.
    if !by_process.is_empty() {
        let mut proc_sorted: Vec<_> = by_process.iter().collect();
        proc_sorted.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        let _ = write!(
            out,
            "\ntop processes by connection count ({}):\n",
            proc_sorted.len()
        );
        for (proc_name, count) in proc_sorted.iter().take(TOP_PROCESSES_CAP) {
            let _ = writeln!(out, "  {proc_name:<32}{count:>6}");
        }
    }

    let rest = remainder(text, last_conn, &unmatched);
    if !rest.is_empty() {
        out.push('\n');
        out.push_str(&rest);
    }
    out.trim_end().to_string()
}

/// Pairs listed when they do not all fit.
pub const ENDPOINT_CAP: usize = 50;

/// `host:port` → (`host`, `port`); IPv6 keeps its brackets.
fn split_port(addr: &str) -> (&str, &str) {
    addr.rsplit_once(':').unwrap_or((addr, ""))
}

/// `local ↔ foreign`, the EPHEMERAL side's port written `*`: a named service
/// (`https`, `redis`) or the lower number is the service, the other end is
/// the client's ephemeral port. Grouped by (state, pair, process), largest
/// group first.
fn endpoint_census(conns: &[Conn]) -> Vec<((String, String, String), usize)> {
    let mut groups: BTreeMap<(String, String, String), usize> = BTreeMap::new();
    for c in conns {
        let ((lh, lp), (fh, fp)) = (split_port(&c.local), split_port(&c.foreign));
        let num = |p: &str| p.parse::<u32>().ok();
        let (lp, fp) = match (num(lp), num(fp)) {
            (Some(l), Some(f)) if l > f => ("*", fp),
            (Some(_), Some(_)) | (None, Some(_)) => (lp, "*"),
            (Some(_), None) if fp != "*" => ("*", fp),
            _ => (lp, fp),
        };
        let pair = if c.foreign.is_empty() {
            c.local.clone()
        } else {
            format!("{lh}:{lp} ↔ {fh}:{fp}")
        };
        *groups
            .entry((c.state.clone(), pair, c.process.clone()))
            .or_insert(0) += 1;
    }
    let mut v: Vec<_> = groups.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v
}

/// A BSD section title: `Active LOCAL (UNIX) domain sockets`,
/// `Registered kernel control modules`.
fn is_bsd_section_title(line: &str) -> bool {
    line.starts_with("Active ") || line.starts_with("Registered ")
}

/// What follows the connection table. macOS `netstat -an` appends ~150 lines
/// of UNIX, kernel-event and kernel-control sockets — kernel addresses that
/// mean nothing off this boot — under section titles; each section becomes
/// one line: its count and what in it has a NAME (a bound path, a control
/// module). Anything else keeps the generic remainder fold.
fn remainder(text: &str, last_conn: usize, unmatched: &[String]) -> String {
    let after: Vec<&str> = text.lines().skip(last_conn + 1).collect();
    if !after.iter().any(|l| is_bsd_section_title(l)) {
        if unmatched.is_empty() {
            return String::new();
        }
        return crate::auto::compact_remainder(&unmatched.join("\n"));
    }
    let mut out = String::new();
    let mut i = 0;
    while i < after.len() {
        let line = after[i];
        i += 1;
        if !is_bsd_section_title(line) {
            if !line.trim().is_empty() {
                let _ = writeln!(out, "{line}");
            }
            continue;
        }
        let columns = after.get(i).map_or(0, |h| h.split_whitespace().count());
        i += 1;
        let mut rows = 0usize;
        let mut names: Vec<(&str, usize)> = Vec::new();
        let mut odd: Vec<&str> = Vec::new();
        while i < after.len() && !is_bsd_section_title(after[i]) {
            let row = after[i];
            i += 1;
            let fields: Vec<&str> = row.split_whitespace().collect();
            if fields.is_empty() {
                continue;
            }
            if fields.len() + 1 < columns {
                odd.push(row);
                continue;
            }
            rows += 1;
            let last = fields[fields.len() - 1];
            let named = fields.len() >= columns
                && last.chars().any(|c| c.is_ascii_alphabetic() || c == '/')
                && !last.chars().all(|c| c.is_ascii_hexdigit());
            if named {
                match names.iter_mut().find(|(n, _)| *n == last) {
                    Some((_, k)) => *k += 1,
                    None => names.push((last, 1)),
                }
            }
        }
        let listed: Vec<String> = names
            .iter()
            .map(|(n, k)| {
                if *k > 1 {
                    format!("{n} ×{k}")
                } else {
                    n.to_string()
                }
            })
            .collect();
        let _ = write!(out, "{line}: {rows}");
        if !listed.is_empty() {
            let _ = write!(out, " — {}", listed.join(" · "));
        }
        out.push('\n');
        for o in odd {
            let _ = writeln!(out, "{o}");
        }
    }
    if !out.is_empty() {
        out.insert_str(
            0,
            "[tokenade:netstat] socket sections counted; kernel addresses not shown\n",
        );
    }
    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ss -tp`, 66 connections: fan-out to one redis is one line with its
    /// count, the client's ephemeral port written `*`, and every peer named.
    #[test]
    fn connections_are_counted_by_endpoint_pair_with_every_peer_named() {
        let mut t =
            String::from("State Recv-Q Send-Q Local Address:Port Peer Address:Port Process\n");
        for i in 0..50 {
            t.push_str(&format!(
                "ESTAB 0 0 172.23.0.1:{} 172.23.0.7:redis\n",
                40000 + i
            ));
        }
        for i in 0..10 {
            t.push_str(&format!(
                "ESTAB 0 0 192.168.1.183:{} 160.79.104.{i}:https users:((\"claude\",pid=7,fd={i}))\n",
                50000 + i
            ));
        }
        // `ss -tn`: both ports are numbers, the lower one is the service —
        // on either side.
        for i in 0..3 {
            t.push_str(&format!("ESTAB 0 0 10.0.0.5:{} 10.0.0.9:5432\n", 51000 + i));
            t.push_str(&format!("ESTAB 0 0 10.0.0.5:5432 10.0.0.8:{}\n", 52000 + i));
        }
        let out = compact(&t);
        assert!(
            out.contains("  ×3 ESTAB 10.0.0.5:* ↔ 10.0.0.9:5432\n"),
            "{out}"
        );
        assert!(
            out.contains("  ×3 ESTAB 10.0.0.5:5432 ↔ 10.0.0.8:*\n"),
            "{out}"
        );
        assert!(
            out.contains("  ×50 ESTAB 172.23.0.1:* ↔ 172.23.0.7:redis\n"),
            "{out}"
        );
        for i in 0..10 {
            assert!(
                out.contains(&format!(
                    "  ×1 ESTAB 192.168.1.183:* ↔ 160.79.104.{i}:https  claude"
                )),
                "{out}"
            );
        }
        assert!(out.contains("(13 pair(s), ephemeral port as *)"), "{out}");
    }

    const NETSTAT_SAMPLE: &str = "Active Internet connections (servers and established)
Proto Recv-Q Send-Q Local Address           Foreign Address         State       PID/Program
tcp        0      0 0.0.0.0:22              0.0.0.0:*               LISTEN      1234/sshd
tcp        0      0 0.0.0.0:80              0.0.0.0:*               LISTEN      5678/nginx
tcp        0      0 0.0.0.0:443             0.0.0.0:*               LISTEN      5678/nginx
tcp6       0      0 :::22                   :::*                    LISTEN      1234/sshd
tcp        0      0 192.168.1.1:80          10.0.0.5:34567          ESTABLISHED 5678/nginx
tcp        0      0 192.168.1.1:80          10.0.0.6:34568          ESTABLISHED 5678/nginx
tcp        0      0 192.168.1.1:80          10.0.0.7:34569          ESTABLISHED 5678/nginx
tcp        0      0 192.168.1.1:80          10.0.0.99:65432         TIME_WAIT   -
tcp        0      0 192.168.1.1:443         10.0.0.8:11111          CLOSE_WAIT  5678/nginx
udp        0      0 0.0.0.0:53              0.0.0.0:*                           1234/dnsmasq
";

    const SS_SAMPLE: &str =
        "State    Recv-Q Send-Q Local Address:Port    Peer Address:Port   Process
LISTEN   0      128    0.0.0.0:22            0.0.0.0:*           users:((\"sshd\",pid=1234,fd=3))
LISTEN   0      128    0.0.0.0:80            0.0.0.0:*           users:((\"nginx\",pid=5678,fd=6))
ESTAB    0      0      192.168.1.1:80        10.0.0.5:34567      users:((\"nginx\",pid=5678,fd=12))
ESTAB    0      0      192.168.1.1:80        10.0.0.6:34568      users:((\"nginx\",pid=5678,fd=13))
TIME-WAIT 0     0      192.168.1.1:80        10.0.0.99:65432
";

    const LSOF_SAMPLE: &str = "COMMAND   PID    USER   FD   TYPE   DEVICE   SIZE/OFF NODE NAME
sshd      1234   root    3u  IPv4   12345     0t0  TCP *:22 (LISTEN)
nginx     5678   www     6u  IPv4   23456     0t0  TCP *:80 (LISTEN)
nginx     5678   www     7u  IPv4   34567     0t0  TCP *:443 (LISTEN)
nginx     5678   www    12u  IPv4   45678     0t0  TCP 192.168.1.1:80->10.0.0.5:34567 (ESTABLISHED)
nginx     5678   www    13u  IPv4   45679     0t0  TCP 192.168.1.1:80->10.0.0.6:34568 (ESTABLISHED)
";

    #[test]
    fn detects_netstat() {
        assert!(detect(NETSTAT_SAMPLE));
    }

    #[test]
    fn detects_ss() {
        assert!(detect(SS_SAMPLE));
    }

    #[test]
    fn detects_lsof() {
        assert!(detect(LSOF_SAMPLE));
    }

    #[test]
    fn rejects_prose() {
        assert!(!detect(
            "This text talks about tcp connections but isn't netstat output.\n"
        ));
    }

    #[test]
    fn rejects_code() {
        assert!(!detect(
            "fn handle_tcp_conn() {\n    let proto = \"tcp\";\n}\n"
        ));
    }

    /// Real `ss -tulpn` rows: a bound UDP socket is a listening port, and a
    /// process name may hold a space.
    #[test]
    fn ss_udp_bound_sockets_listen_and_names_keep_their_spaces() {
        let s = "Netid State  Recv-Q Send-Q Local Address:Port  Peer Address:PortProcess\n\
                 udp   UNCONN 0      0        224.0.0.251:5353       0.0.0.0:*    users:((\"chrome\",pid=2516415,fd=89))\n\
                 tcp   LISTEN 0      511        127.0.0.1:3790       0.0.0.0:*    users:((\"next-server (v1\",pid=3160242,fd=21))\n\
                 tcp   ESTAB  0      0        10.0.0.2:51000   140.82.121.4:443   users:((\"git\",pid=4242,fd=3))\n";
        let out = summary(s);
        assert!(out.contains("3 connection(s), 2 listening"), "{out}");
        assert!(out.contains("udp    224.0.0.251:5353"), "{out}");
        assert!(
            out.contains("127.0.0.1:3790                 next-server (v1"),
            "{out}"
        );
    }

    #[test]
    fn a_small_table_is_given_as_it_is() {
        let t = "COMMAND     PID USER   FD   TYPE             DEVICE SIZE/OFF NODE NAME\n\
                 node      84733 paul   14u  IPv4 0x660bb6711cc5a95e      0t0  TCP 127.0.0.1:54138 (LISTEN)\n\
                 sshd-sess 94574 paul    5u  IPv4 0xbb487f7d4efde30b      0t0  TCP 192.168.1.190:22->192.168.1.183:41774 (ESTABLISHED)\n";
        assert_eq!(compact(t), t.trim_end());
        // Past the small-table bound, the summary.
        let big: String = std::iter::once("COMMAND     PID USER   FD   TYPE             DEVICE SIZE/OFF NODE NAME\n".to_string())
            .chain((0..SMALL_TABLE + 1).map(|i| format!("node      {} paul   14u  IPv4 0x660bb6711cc5a95e      0t0  TCP 127.0.0.1:{} (LISTEN)\n", 1000 + i, 20000 + i)))
            .collect();
        assert!(
            compact(&big).contains("listening ports"),
            "{}",
            compact(&big)
        );
    }

    #[test]
    fn netstat_state_distribution() {
        let out = summary(NETSTAT_SAMPLE);
        // Counts: 4 LISTEN (tcp + tcp6) and 1 bound udp socket (no state) =
        // 5 listening, 3 ESTABLISHED, 1 TIME_WAIT, 1 CLOSE_WAIT.
        assert!(out.contains("10 connection(s)"));
        assert!(out.contains("5 listening"));
        assert!(out.contains("3 ESTABLISHED"));
        // State distribution table.
        assert!(out.contains("LISTEN"));
        assert!(out.contains("ESTABLISHED"));
        assert!(out.contains("TIME_WAIT"));
        assert!(out.contains("CLOSE_WAIT"));
    }

    #[test]
    fn netstat_lists_listen_ports() {
        let out = summary(NETSTAT_SAMPLE);
        assert!(out.contains("0.0.0.0:22"));
        assert!(out.contains("0.0.0.0:80"));
        assert!(out.contains("0.0.0.0:443"));
        // IPv6.
        assert!(out.contains(":::22"));
        // Processes.
        assert!(out.contains("1234/sshd"));
        assert!(out.contains("5678/nginx"));
    }

    #[test]
    fn netstat_top_processes() {
        let out = summary(NETSTAT_SAMPLE);
        assert!(out.contains("top processes"));
        // nginx has the most connections: 3 LISTEN (80, 443, and one is duplicate proto) +
        // 3 ESTABLISHED + 1 CLOSE_WAIT = 7.
        // sshd has 2 LISTEN.
        // dnsmasq has 1 STATELESS udp.
        assert!(out.contains("5678/nginx"));
    }

    #[test]
    fn ss_parsed_correctly() {
        let out = summary(SS_SAMPLE);
        assert!(out.contains("5 connection(s)"));
        assert!(out.contains("2 listening"));
        // ss writes ESTAB not ESTABLISHED.
        assert!(out.contains("ESTAB"));
        // Process names extracted from `users:(("name",pid=N,fd=K))`.
        assert!(out.contains("sshd"));
        assert!(out.contains("nginx"));
    }

    #[test]
    fn lsof_parsed_correctly() {
        let out = summary(LSOF_SAMPLE);
        assert!(out.contains("5 connection(s)"));
        assert!(out.contains("3 listening"));
        assert!(out.contains("2 ESTABLISHED"));
        assert!(out.contains("sshd"));
        assert!(out.contains("nginx"));
    }

    #[test]
    fn unknown_passes_through() {
        let t = "this is not netstat output";
        assert_eq!(summary(t), t);
    }

    #[test]
    fn savings_huge_server() {
        // 10 000 ESTABLISHED + 50 LISTEN: real busy-server shape.
        let mut s = String::from("Active Internet connections (servers and established)\n");
        s.push_str("Proto Recv-Q Send-Q Local Address           Foreign Address         State       PID/Program\n");
        for p in 0..50 {
            let _ = writeln!(
                s,
                "tcp        0      0 0.0.0.0:{}\t\t0.0.0.0:*               LISTEN      {}/svc{p}",
                8000 + p,
                1000 + p
            );
        }
        for i in 0..10_000 {
            let _ = writeln!(
                s,
                "tcp        0      0 192.168.1.1:80          10.0.{}.{}:{}        ESTABLISHED 5678/nginx",
                (i / 256) % 256,
                i % 256,
                30000 + i % 35000
            );
        }
        let (out, sav) = compact_with_savings(&s);
        assert!(out.contains("10050 connection(s)"));
        assert!(out.contains("50 listening"));
        assert!(out.contains("10000 ESTABLISHED"));
        assert!(sav.percent() > 95.0, "got {sav:?}");
    }

    #[test]
    fn handles_ipv6_addresses() {
        // The LISTEN entries are surfaced verbatim, ESTABLISHED are only
        // counted (since real servers can have thousands of those, that's
        // the whole point of the compactor).
        let s = "Proto Recv-Q Send-Q Local Address           Foreign Address         State       PID/Program
tcp6       0      0 :::22                   :::*                    LISTEN      1234/sshd
tcp6       0      0 :::80                   :::*                    LISTEN      5678/nginx
tcp6       0      0 2001:db8::1:443         2001:db8::ff:55555      ESTABLISHED 5678/nginx
tcp6       0      0 fe80::1%eth0:8080       fe80::99%eth0:33333     ESTABLISHED 9999/app
";
        let out = summary(s);
        assert!(out.contains("4 connection(s)"));
        // LISTEN entries preserved verbatim, including IPv6.
        assert!(out.contains(":::22"));
        assert!(out.contains(":::80"));
        // ESTABLISHED counted but not enumerated.
        assert!(out.contains("2 ESTABLISHED"));
        // IPv6 process name extracted.
        assert!(out.contains("9999/app"));
    }
}

#[cfg(test)]
mod lsof_variants {
    use super::*;

    const NET: &str = "\
COMMAND       PID     USER  FD   TYPE  DEVICE SIZE/OFF NODE NAME
polybar      1539 infected  19u  IPv4   25933      0t0  TCP 127.0.0.1:53842->127.0.0.1:6600 (ESTABLISHED)
firefox      2201 infected  62u  IPv6   31002      0t0  TCP [::1]:443->[::1]:51222 (ESTABLISHED)
sshd         1101     root   3u  IPv4    9911      0t0  TCP *:22 (LISTEN)
";

    const FILES: &str = "\
COMMAND       PID     TID TASKCMD       USER   FD      TYPE             DEVICE  SIZE/OFF      NODE NAME
systemd      1354                   infected  cwd   unknown                                        /proc/1354/cwd
systemd      1354                   infected  rtd   unknown                                        /proc/1354/root
bash         2010                   infected  txt       REG              254,1   1234567    998877 /usr/bin/bash
bash         2010                   infected  mem       REG              254,1    223344    112233 /usr/lib/libc.so.6
";

    /// `lsof -i` is a connection listing and belongs here: 0.097 against the
    /// generic tabular compactor's 0.589 on a real capture.
    #[test]
    fn lsof_i_is_still_claimed() {
        assert!(detect(NET), "the network variant stopped being recognised");
    }

    /// `lsof -n` prints the SAME header and is not a connection listing. It
    /// used to land here and come back untouched — 113 580 bytes of 113 580 —
    /// while the tabular compactor gives 0.436.
    #[test]
    fn lsof_without_i_is_not_a_connection_listing() {
        assert!(
            !detect(FILES),
            "the file listing was claimed on its header alone"
        );
    }

    /// The other headers are untouched by this.
    #[test]
    fn the_real_netstat_shapes_still_match() {
        assert!(detect(
            "Proto Recv-Q Send-Q Local Address\ntcp 0 0 1.2.3.4:80\n"
        ));
        assert!(detect(
            "Active Internet connections (servers and established)\n"
        ));
        assert!(detect("Netid State Recv-Q Send-Q\n"));
    }
}

#[cfg(test)]
mod bsd_sections {
    use super::*;

    /// macOS `netstat -an`, shape measured 2026-10-03: `tcp4`/`udp46` rows,
    /// then sections of UNIX and kernel sockets. The rows were not read as
    /// connections, and the sockets pushed the whole past the threshold.
    fn macos_netstat() -> String {
        let mut t = String::from(
            "Active Internet connections (including servers)\n\
             Proto Recv-Q Send-Q  Local Address          Foreign Address        (state)\n\
             tcp4       0      0  192.168.1.190.22       192.168.1.183.41780    ESTABLISHED\n\
             tcp6       0      0  *.22                   *.*                    LISTEN\n\
             tcp4       0      0  127.0.0.1.54138        *.*                    LISTEN\n\
             udp46      0      0  *.5353                 *.*                    \n\
             Active LOCAL (UNIX) domain sockets\n\
             Address          Type   Recv-Q Send-Q            Inode             Conn             Refs          Nextref Addr\n",
        );
        for i in 0..300u64 {
            let a = 0x187e_e5da_7a1b_b86e_u64.wrapping_mul(i + 7);
            t.push_str(&format!("{a:16x} stream      0      0                0 {:16x}                0                0\n", a ^ 0xffff));
        }
        t.push_str("187ee5da7a1bb86e stream      0      0                0 ab38a167a38a6eb9                0                0 /var/run/mDNSResponder\n");
        t.push_str("Active kernel control sockets\nProto Recv-Q Send-Q   unit     id name\n");
        for u in 1..=24 {
            t.push_str(&format!(
                "kctl       0      0     {u}      7 com.apple.netsrc\n"
            ));
        }
        t
    }

    #[test]
    fn a_macos_table_is_delivered_and_its_socket_sections_counted() {
        let _g = tk_base::delivery::threshold_on_this_thread(5_000);
        let out = compact(&macos_netstat());
        for row in [
            "tcp4       0      0  192.168.1.190.22       192.168.1.183.41780    ESTABLISHED",
            "tcp4       0      0  127.0.0.1.54138        *.*                    LISTEN",
            "udp46      0      0  *.5353                 *.*",
        ] {
            assert!(
                out.lines().any(|l| l.trim_end() == row),
                "lost {row}:\n{out}"
            );
        }
        assert!(
            out.contains("Active LOCAL (UNIX) domain sockets: 301 — /var/run/mDNSResponder\n"),
            "{out}"
        );
        assert!(
            out.contains("Active kernel control sockets: 24 — com.apple.netsrc ×24"),
            "{out}"
        );
        assert!(
            !out.contains("ab38a167a38a6eb9"),
            "kernel addresses go:\n{out}"
        );
    }

    /// `ss -tulpn` lists one socket per process thread: seven chrome mDNS
    /// sockets differ only by pid/fd, and are one port.
    #[test]
    fn a_port_held_by_several_threads_is_said_once() {
        let mut s = String::from(
            "Netid State Recv-Q Send-Q Local Address:Port Peer Address:Port Process\n",
        );
        for fd in 0..7 {
            s.push_str(&format!("udp UNCONN 0 0 224.0.0.251:5353 0.0.0.0:* users:((\"chrome\",pid=9{fd},fd={fd}))\n"));
        }
        for p in 0..30 {
            s.push_str(&format!(
                "tcp LISTEN 0 128 127.0.0.1:{} 0.0.0.0:* users:((\"node\",pid={p},fd=3))\n",
                3000 + p
            ));
        }
        let out = summary(&s);
        assert_eq!(out.matches("224.0.0.251:5353").count(), 1, "{out}");
        assert!(out.contains("chrome (×7)"), "{out}");
        assert!(out.contains("127.0.0.1:3029"), "{out}");
    }
}
