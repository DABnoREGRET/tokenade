//! DNS lookup output compaction (`dig`, `nslookup`, `host`).
//!
//! `dig` emits a structured comment-section format:
//!
//! ```text
//! ; <<>> DiG 9.18.12 <<>> example.com ANY
//! ;; global options: +cmd
//! ;; Got answer:
//! ;; ->>HEADER<<- opcode: QUERY, status: NOERROR, id: 12345
//! ;; flags: qr rd ra; QUERY: 1, ANSWER: 5, AUTHORITY: 4, ADDITIONAL: 1
//!
//! ;; QUESTION SECTION:
//! ;example.com.                  IN      ANY
//!
//! ;; ANSWER SECTION:
//! example.com.        3600    IN      A       93.184.216.34
//! example.com.        3600    IN      NS      ns1.example.com.
//! example.com.        3600    IN      MX      10 mail.example.com.
//! example.com.        3600    IN      TXT     "v=spf1 include:_spf.example.com ~all"
//!
//! ;; AUTHORITY SECTION:
//! example.com.        86400   IN      NS      ns1.example.com.
//!
//! ;; Query time: 23 msec
//! ;; SERVER: 8.8.8.8#53(8.8.8.8) (UDP)
//! ;; WHEN: Mon Jan 15 10:30:00 UTC 2024
//! ;; MSG SIZE  rcvd: 234
//! ```
//!
//! A `dig +trace example.com` can run 200 – 500 lines (one block per
//! authority level) and the agent reading it really only wants:
//!
//!   - the **question** (name + type)
//!   - the **status** (NOERROR / NXDOMAIN / SERVFAIL / REFUSED)
//!   - every **ANSWER** record verbatim (those are the result)
//!   - **counts** for authority / additional sections
//!   - **server + query time** as a one-line footer
//!
//! `nslookup` and `host` use different shapes but the agent extracts the
//! same fields.

use std::fmt::Write;

pub const ANSWERS_CAP: usize = 200;

/// `host`-style answer lines that differ only by their separator and record
/// type. Three `else if trimmed.contains(SEP) { if let Some(…) = split_once(SEP)
/// { … } }` branches said this, 12 lines each.
///
/// The `has address` branch is NOT here: it also fills `question`, which is a
/// different job, so it keeps its own arm. Order is preserved by `find_map`.
const HOST_RECORD_FORMS: &[(&str, &str)] = &[
    (" has IPv6 address ", "AAAA"),
    (" mail is handled by ", "MX"),
    (" is an alias for ", "CNAME"),
    // `nslookup -type=…` (BIND, the same on Linux and macOS): `name<TAB>mail
    // exchanger = 5 host.`. No leading space — the separator before it is a
    // tab — so the name is trimmed where it is split.
    ("mail exchanger = ", "MX"),
    ("text = ", "TXT"),
    ("nameserver = ", "NS"),
    ("canonical name = ", "CNAME"),
];

pub fn detect(text: &str) -> bool {
    let head: &str = text.get(..text.len().min(8192)).unwrap_or(text);
    let first_line = head.lines().next().unwrap_or("");
    head.contains(";; ANSWER SECTION:")
        || head.contains(";; AUTHORITY SECTION:")
        || head.contains(";; ->>HEADER<<-")
        || head.contains("; <<>> DiG")
        || head.contains(";; QUESTION SECTION:")
        || ((first_line.starts_with("Server:") || first_line.starts_with("Server\t"))
            && head.contains("Address:"))
        || head
            .lines()
            .take(20)
            .filter(|l| {
                l.contains(" has address ")
                    || l.contains(" has IPv6 address ")
                    || l.contains(" mail is handled by ")
                    || l.contains(" is an alias for ")
            })
            .nth(1)
            .is_some()
}

#[derive(Default)]
struct DnsAnswer {
    name: String,
    ttl: String,
    rtype: String,
    rdata: String,
}

/// dig/nslookup/host chrome that carries no information worth keeping.
fn is_dns_noise(t: &str) -> bool {
    t.starts_with("Server:")
        || t.starts_with("Address:")
        || t.starts_with("Non-authoritative answer")
        || t.starts_with("Authoritative answers can be found")
        || t.starts_with("Trying \"")
        || t.starts_with("Received ")
        || t.starts_with("Aliases:")
}

pub fn compact(text: &str) -> String {
    if !detect(text) {
        return text.to_string();
    }

    let mut question: Option<String> = None;
    let mut question_type: Option<String> = None;
    let mut status: Option<String> = None;
    let mut answers: Vec<DnsAnswer> = Vec::new();
    let mut unmatched: Vec<String> = Vec::new();
    let mut total_answers = 0usize;
    let mut authority_count = 0usize;
    let mut additional_count = 0usize;
    let mut query_time: Option<String> = None;
    let mut server: Option<String> = None;
    let mut non_authoritative = false;
    let mut current_section: Section = Section::None;
    // `nslookup` answers as `Name:\t<host>` then `Address: <ip>` — the same
    // `Address:` word the resolver line under `Server:` uses. Only one that
    // FOLLOWS a `Name:` is a record: measured 2026-10-02 on BIND's nslookup
    // (Linux and macOS 15), every such answer was dropped as chrome and the
    // header said `0 answers` — `nslookup github.com` came back with no address.
    let mut answer_name: Option<String> = None;
    // Windows writes several at once: `Addresses:  2a00:…` then indented
    // continuation lines, one address each.
    let mut in_addresses = false;
    #[derive(PartialEq, Eq, Clone, Copy)]
    enum Section {
        None,
        Question,
        Answer,
        Authority,
        Additional,
    }

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            in_addresses = false;
            continue;
        }
        if in_addresses
            && line.starts_with(char::is_whitespace)
            && trimmed.parse::<std::net::IpAddr>().is_ok()
        {
            total_answers += 1;
            if answers.len() < ANSWERS_CAP {
                answers.push(DnsAnswer {
                    name: answer_name.clone().unwrap_or_default(),
                    ttl: String::new(),
                    rtype: if trimmed.contains(':') { "AAAA" } else { "A" }.to_string(),
                    rdata: trimmed.to_string(),
                });
            }
            continue;
        }
        in_addresses = false;

        // Section header transitions.
        if trimmed == ";; QUESTION SECTION:" {
            current_section = Section::Question;
            continue;
        }
        if trimmed == ";; ANSWER SECTION:" {
            current_section = Section::Answer;
            continue;
        }
        if trimmed == ";; AUTHORITY SECTION:" {
            current_section = Section::Authority;
            continue;
        }
        if trimmed == ";; ADDITIONAL SECTION:" {
            current_section = Section::Additional;
            continue;
        }

        // dig HEADER line — extract status.
        if trimmed.starts_with(";; ->>HEADER<<-") {
            if let Some(idx) = trimmed.find("status: ") {
                let s = &trimmed[idx + "status: ".len()..];
                let st: String = s
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric())
                    .collect();
                if !st.is_empty() {
                    status = Some(st);
                }
            }
            continue;
        }

        // Footer fields.
        if let Some(rest) = trimmed.strip_prefix(";; Query time: ") {
            query_time = Some(rest.to_string());
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix(";; SERVER: ") {
            server = Some(rest.to_string());
            continue;
        }

        // Any other `;`-prefixed line — handle the question record specially.
        // `dig` prints a question record like `;example.com.  IN  ANY` (note
        // the single leading `;`).
        if trimmed.starts_with(';') {
            if current_section == Section::Question {
                let q = trimmed.trim_start_matches(';').trim();
                let parts: Vec<&str> = q.split_whitespace().collect();
                if parts.len() >= 3 {
                    if question.is_none() {
                        question = Some(parts[0].trim_end_matches('.').to_string());
                    }
                    if question_type.is_none() {
                        question_type = Some(parts[2].to_string());
                    }
                }
            }
            continue;
        }

        // Real DNS records (in ANSWER/AUTHORITY/ADDITIONAL).
        match current_section {
            Section::Answer => {
                total_answers += 1;
                let parts: Vec<&str> = trimmed.splitn(5, char::is_whitespace).collect();
                if parts.len() >= 5 && answers.len() < ANSWERS_CAP {
                    answers.push(DnsAnswer {
                        name: parts[0].to_string(),
                        ttl: parts[1].to_string(),
                        rtype: parts[3].to_string(),
                        rdata: parts[4].trim().to_string(),
                    });
                }
            }
            Section::Authority => authority_count += 1,
            Section::Additional => additional_count += 1,
            _ => {
                // nslookup / host(1) fallback parsing.
                if let Some(rest) = trimmed.strip_prefix("Name:") {
                    if question.is_none() {
                        question = Some(rest.trim().to_string());
                    }
                    answer_name = Some(rest.trim().to_string());
                } else if let (Some(ip), Some(name)) = (
                    trimmed
                        .strip_prefix("Address:")
                        .or_else(|| trimmed.strip_prefix("Addresses:")),
                    answer_name.as_ref(),
                ) {
                    in_addresses = trimmed.starts_with("Addresses:");
                    let ip = ip.trim();
                    total_answers += 1;
                    if answers.len() < ANSWERS_CAP {
                        answers.push(DnsAnswer {
                            name: name.clone(),
                            ttl: String::new(),
                            rtype: if ip.contains(':') { "AAAA" } else { "A" }.to_string(),
                            rdata: ip.to_string(),
                        });
                    }
                } else if let Some(rest) = trimmed.strip_prefix("** server can't find ") {
                    // `** server can't find x.invalid: NXDOMAIN` — the status, said once.
                    if let Some((name, st)) = rest.rsplit_once(": ") {
                        question.get_or_insert_with(|| name.to_string());
                        status = Some(st.trim().to_string());
                    }
                } else if trimmed.contains(" has address ") {
                    if let Some((n, ip)) = trimmed.split_once(" has address ") {
                        if answers.len() < ANSWERS_CAP {
                            answers.push(DnsAnswer {
                                name: n.to_string(),
                                ttl: String::new(),
                                rtype: "A".to_string(),
                                rdata: ip.to_string(),
                            });
                            total_answers += 1;
                            if question.is_none() {
                                question = Some(n.to_string());
                            }
                        }
                    }
                } else if let Some((n, rdata, rtype)) =
                    HOST_RECORD_FORMS.iter().find_map(|(sep, ty)| {
                        trimmed
                            .split_once(sep)
                            .map(|(n, v)| (n.trim(), v.trim(), *ty))
                    })
                {
                    // Windows writes `gmail.com<TAB>MX preference = 40, mail
                    // exchanger = alt4…`: the preference sits BEFORE the
                    // separator. Name = its first word; preference = rdata's.
                    let (n, rest) = n.split_once(char::is_whitespace).unwrap_or((n, ""));
                    let rdata = match rest.split_once("preference = ") {
                        Some((_, p)) => format!(
                            "{} {rdata}",
                            p.trim_end_matches(|c: char| c == ',' || c.is_whitespace())
                        ),
                        None => rdata.to_string(),
                    };
                    if answers.len() < ANSWERS_CAP {
                        answers.push(DnsAnswer {
                            name: n.to_string(),
                            ttl: String::new(),
                            rtype: rtype.to_string(),
                            rdata,
                        });
                        total_answers += 1;
                    }
                    question.get_or_insert_with(|| n.trim_end_matches('.').to_string());
                } else if let Some(rest) = trimmed.strip_prefix("Server:") {
                    // nslookup's resolver: WHICH server answered is the first
                    // question when a name resolves differently than expected.
                    server.get_or_insert_with(|| rest.trim().to_string());
                } else if trimmed.starts_with("Non-authoritative answer") {
                    non_authoritative = true;
                } else if !is_dns_noise(trimmed) {
                    // Not DNS output at all (`dig x.com; curl -I y` in one
                    // doc) — keep instead of silently destroying.
                    unmatched.push(line.to_string());
                }
            }
        }
    }

    // ---- Render ----
    let mut out = String::with_capacity(2048);
    let q = question.as_deref().unwrap_or("(unknown)");
    // dig's header says type, status and the three section counts; nslookup
    // and host say none of them. Written for every tool, the line read
    // `apple.com ? ?: 1 answers, 0 authority, 0 additional` on a real macOS
    // `nslookup` (2026-10-03): two `?` and two counts nobody printed.
    if let Some(qt) = question_type.as_deref() {
        let st = status.as_deref().unwrap_or("?");
        let _ = writeln!(
            out,
            "{q} {qt} {st}: {total_answers} answers, {authority_count} authority, {additional_count} additional"
        );
    } else {
        let _ = write!(out, "{q}: {total_answers} answer(s)");
        if let Some(st) = status.as_deref() {
            let _ = write!(out, ", {st}");
        }
        if non_authoritative {
            out.push_str(" (non-authoritative)");
        }
        out.push('\n');
    }

    if !answers.is_empty() {
        out.push_str("\nanswers:\n");
        let name_w = answers
            .iter()
            .map(|a| a.name.len())
            .max()
            .unwrap_or(20)
            .min(50);
        let type_w = answers
            .iter()
            .map(|a| a.rtype.len())
            .max()
            .unwrap_or(5)
            .min(8);
        for a in &answers {
            if a.ttl.is_empty() {
                let _ = writeln!(
                    out,
                    "  {:<name_w$}  {:<type_w$}  {}",
                    a.name,
                    a.rtype,
                    a.rdata,
                    name_w = name_w,
                    type_w = type_w
                );
            } else {
                let _ = writeln!(
                    out,
                    "  {:<name_w$}  {:>6}  {:<type_w$}  {}",
                    a.name,
                    a.ttl,
                    a.rtype,
                    a.rdata,
                    name_w = name_w,
                    type_w = type_w
                );
            }
        }
        if total_answers > answers.len() {
            let _ = writeln!(
                out,
                "  … {} more answer(s) elided",
                total_answers - answers.len()
            );
        }
    }

    let footer = match (server.as_deref(), query_time.as_deref()) {
        (Some(s), Some(t)) => Some(format!("server: {s}, query time: {t}")),
        (Some(s), None) => Some(format!("server: {s}")),
        (None, Some(t)) => Some(format!("query time: {t}")),
        (None, None) => None,
    };
    if let Some(f) = footer {
        let _ = write!(out, "\n{f}\n");
    }

    if !unmatched.is_empty() {
        out.push('\n');
        out.push_str(&crate::auto::compact_remainder(&unmatched.join("\n")));
    }
    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIG_SAMPLE: &str = "; <<>> DiG 9.18.12 <<>> example.com ANY\n;; global options: +cmd\n;; Got answer:\n;; ->>HEADER<<- opcode: QUERY, status: NOERROR, id: 12345\n;; flags: qr rd ra; QUERY: 1, ANSWER: 5, AUTHORITY: 4, ADDITIONAL: 1\n\n;; OPT PSEUDOSECTION:\n; EDNS: version: 0, flags:; udp: 4096\n\n;; QUESTION SECTION:\n;example.com.\t\t\tIN\tANY\n\n;; ANSWER SECTION:\nexample.com.\t\t3600\tIN\tA\t93.184.216.34\nexample.com.\t\t3600\tIN\tNS\tns1.example.com.\nexample.com.\t\t3600\tIN\tNS\tns2.example.com.\nexample.com.\t\t3600\tIN\tMX\t10 mail.example.com.\nexample.com.\t\t3600\tIN\tTXT\t\"v=spf1 include:_spf.example.com ~all\"\n\n;; AUTHORITY SECTION:\nexample.com.\t\t86400\tIN\tNS\tns1.example.com.\nexample.com.\t\t86400\tIN\tNS\tns2.example.com.\nexample.com.\t\t86400\tIN\tNS\tns3.example.com.\nexample.com.\t\t86400\tIN\tNS\tns4.example.com.\n\n;; ADDITIONAL SECTION:\nns1.example.com.\t172800\tIN\tA\t192.0.2.1\n\n;; Query time: 23 msec\n;; SERVER: 8.8.8.8#53(8.8.8.8) (UDP)\n;; WHEN: Mon Jan 15 10:30:00 UTC 2024\n;; MSG SIZE  rcvd: 234\n";

    const HOST_SAMPLE: &str = "example.com has address 93.184.216.34\nexample.com has IPv6 address 2606:2800:220:1::1\nexample.com mail is handled by 10 mail.example.com.\nwww.example.com is an alias for example.com.\n";

    const NSLOOKUP_SAMPLE: &str = "Server:\t\t8.8.8.8\nAddress:\t8.8.8.8#53\n\nNon-authoritative answer:\nName:\texample.com\nAddress: 93.184.216.34\n";

    #[test]
    fn detects_dig() {
        assert!(detect(DIG_SAMPLE));
    }

    #[test]
    fn detects_host() {
        assert!(detect(HOST_SAMPLE));
    }

    #[test]
    fn detects_nslookup() {
        assert!(detect(NSLOOKUP_SAMPLE));
    }

    #[test]
    fn rejects_random_text() {
        assert!(!detect("This is just prose about DNS lookups.\n"));
    }

    #[test]
    fn rejects_ini_with_semicolons() {
        // INI/.gitignore style comments use `;` but no DNS-specific shape.
        assert!(!detect(
            "; this is an ini comment\n; another comment\n[section]\nkey=value\n"
        ));
    }

    #[test]
    fn extracts_dig_answers() {
        let out = compact(DIG_SAMPLE);
        /* L122: banner removed; assertion deleted */
        assert!(out.contains("5 answers"), "out: {out}");
        assert!(out.contains("4 authority"), "out: {out}");
        assert!(out.contains("1 additional"), "out: {out}");
        // All 5 answer records present.
        assert!(out.contains("93.184.216.34"));
        assert!(out.contains("ns1.example.com."));
        assert!(out.contains("ns2.example.com."));
        assert!(out.contains("10 mail.example.com."));
        assert!(out.contains("v=spf1 include"));
        // Footer.
        assert!(out.contains("server: 8.8.8.8#53(8.8.8.8) (UDP)"));
        assert!(out.contains("query time: 23 msec"));
        // Boilerplate dropped.
        assert!(!out.contains("EDNS: version: 0"));
        assert!(!out.contains("MSG SIZE"));
        assert!(!out.contains("global options"));
    }

    #[test]
    fn extracts_host_answers() {
        let out = compact(HOST_SAMPLE);
        /* L122: banner removed; assertion deleted */
        assert!(out.contains("93.184.216.34"));
        assert!(out.contains("2606:2800:220:1::1"));
        assert!(out.contains("AAAA"));
        assert!(out.contains("MX"));
        assert!(out.contains("CNAME"));
    }

    #[test]
    fn extracts_nslookup_name() {
        let out = compact(NSLOOKUP_SAMPLE);
        /* L122: banner removed; assertion deleted */
        assert!(out.contains("example.com"));
        // The ANSWER, not just the name: `contains("example.com")` held while
        // the address was dropped and the header said `0 answers`.
        assert!(out.contains("93.184.216.34"), "{out}");
        // nslookup prints no type, status or section counts: none invented.
        assert!(
            out.starts_with("example.com: 1 answer(s) (non-authoritative)\n"),
            "{out}"
        );
        // The resolver is not an answer — it is named as the server, once.
        assert_eq!(out.matches("8.8.8.8").count(), 1, "{out}");
        assert!(out.contains("\nserver: 8.8.8.8"), "{out}");
    }

    #[test]
    fn unknown_passes_through() {
        let t = "this is not a DNS lookup";
        assert_eq!(compact(t), t);
    }

    #[test]
    fn handles_nxdomain() {
        let s = ";; ->>HEADER<<- opcode: QUERY, status: NXDOMAIN, id: 9999\n;; flags: qr rd ra; QUERY: 1, ANSWER: 0\n\n;; QUESTION SECTION:\n;nonexistent.example.com.\tIN\tA\n\n;; Query time: 5 msec\n;; SERVER: 8.8.8.8#53(8.8.8.8) (UDP)\n";
        let out = compact(s);
        assert!(out.contains("NXDOMAIN"));
        assert!(out.contains("0 answers"));
        assert!(out.contains("nonexistent.example.com"));
    }

    #[test]
    fn handles_txt_with_semicolons_in_value() {
        // TXT records routinely contain `;` (SPF syntax) — must NOT be
        // mistaken for a comment line, since real TXT records start at
        // column 0 with the name and don't begin with `;`.
        let s = ";; ANSWER SECTION:\nexample.com.\t3600\tIN\tTXT\t\"v=spf1 ip4:192.0.2.0/24 ip6:2001:db8::/32 ~all; not-a-comment\"\n";
        let out = compact(s);
        assert!(out.contains("v=spf1"));
        assert!(out.contains("not-a-comment"));
    }

    #[test]
    fn savings_huge_dig_trace() {
        // Synth `dig +trace` shape: multiple recursion levels, each with
        // the full boilerplate (HEADER + flags + QUESTION + Query time +
        // SERVER + WHEN + MSG SIZE) and a handful of records. Real `+trace`
        // output is dominated by the per-level boilerplate, hence the
        // compaction ratio.
        let mut s = String::new();
        for level in 0..15 {
            let _ = write!(
                s,
                "; <<>> DiG 9.18.12 <<>> +trace example.com (level {level})\n;; global options: +cmd\n;; Got answer:\n;; ->>HEADER<<- opcode: QUERY, status: NOERROR, id: {level}\n;; flags: qr rd ra ad; QUERY: 1, ANSWER: 4, AUTHORITY: 13, ADDITIONAL: 27\n;; OPT PSEUDOSECTION:\n; EDNS: version: 0, flags: do; udp: 4096\n\n;; QUESTION SECTION:\n;example.com.\tIN\tA\n\n;; ANSWER SECTION:\nexample.com.\t3600\tIN\tNS\tns-{level}.example.com.\n\n;; Query time: {level}00 msec\n;; SERVER: 192.0.2.{level}#53(192.0.2.{level}) (UDP)\n;; WHEN: Mon Jan 15 10:30:{level:02} UTC 2024\n;; MSG SIZE  rcvd: 1234\n\n"
            );
        }
        let (out, sav) = compact_with_savings(&s);
        assert!(out.contains("answer"));
        assert!(sav.percent() > 60.0, "got {sav:?}");
    }
}

#[cfg(test)]
mod real_nslookup_output {
    use super::compact;

    /// Real captures from 2026-10-02, BIND nslookup on macOS 15.7 (identical on Linux).
    #[test]
    fn mx_aaaa_and_nxdomain_say_what_they_found() {
        let mx = "Server:\t\t2001:db8::1\nAddress:\t2001:db8::1#53\n\nNon-authoritative answer:\ngmail.com\tmail exchanger = 5 gmail-smtp-in.l.google.com.\ngmail.com\tmail exchanger = 40 alt4.gmail-smtp-in.l.google.com.\n\nAuthoritative answers can be found from:\n\n";
        let out = compact(mx);
        assert!(
            out.starts_with("gmail.com: 2 answer(s) (non-authoritative)")
                && out.contains("5 gmail-smtp-in.l.google.com.")
                && out.contains("40 alt4"),
            "{out}"
        );
        let v6 = "Server:\t\t127.0.0.53\nAddress:\t127.0.0.53#53\n\nNon-authoritative answer:\nName:\tgoogle.com\nAddress: 172.217.22.110\nName:\tgoogle.com\nAddress: 2a00:1450:4007:80c::200e\n\n";
        let out = compact(v6);
        assert!(
            out.contains("2 answer(s)")
                && out.contains("AAAA")
                && out.contains("2a00:1450:4007:80c::200e"),
            "{out}"
        );
        assert!(out.ends_with("server: 127.0.0.53"), "{out}");
        // Windows (English): `Addresses:` then indented continuation lines.
        let win = "Server:  UnKnown\nAddress:  192.168.1.1\n\nNon-authoritative answer:\nName:    google.com\nAddresses:  2a00:1450:4007:809::200e\n\t  142.251.39.110\n\n";
        let out = compact(win);
        assert!(
            out.starts_with("google.com: 2 answer(s)")
                && out.contains("142.251.39.110")
                && out.ends_with("server: UnKnown"),
            "{out}"
        );
        let wmx = "Server:  UnKnown\nAddress:  192.168.1.1\n\nNon-authoritative answer:\ngmail.com\tMX preference = 40, mail exchanger = alt4.gmail-smtp-in.l.google.com\ngmail.com\tMX preference = 5, mail exchanger = gmail-smtp-in.l.google.com\n";
        let out = compact(wmx);
        assert!(
            out.contains("gmail.com  MX  40 alt4.gmail-smtp-in.l.google.com")
                && out.contains("MX  5 gmail-smtp-in"),
            "{out}"
        );
        let nx = "Server:\t\t127.0.0.53\nAddress:\t127.0.0.53#53\n\n** server can't find nonexistent-zz9.example.invalid: NXDOMAIN\n\n";
        let out = compact(nx);
        assert!(
            out.starts_with("nonexistent-zz9.example.invalid: 0 answer(s), NXDOMAIN\n"),
            "{out}"
        );
    }
}
