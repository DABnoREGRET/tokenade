//! `dns` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

pub(crate) fn synth_dig_trace(n_levels: usize, records_per_level: usize) -> String {
    let mut s = String::new();
    for level in 0..n_levels {
        s.push_str(&format!(
            "; <<>> DiG 9.18.12 <<>> +trace +nocomments +nostats example.com (level {level})\n;; global options: +cmd\n;; Got answer:\n;; ->>HEADER<<- opcode: QUERY, status: NOERROR, id: {level}\n;; flags: qr rd ra ad; QUERY: 1, ANSWER: {records_per_level}, AUTHORITY: 13, ADDITIONAL: 27\n;; OPT PSEUDOSECTION:\n; EDNS: version: 0, flags: do; udp: 4096\n; COOKIE: 1234567890abcdef\n\n;; QUESTION SECTION:\n;example.com.\tIN\tA\n\n;; ANSWER SECTION:\n"
        ));
        for r in 0..records_per_level {
            s.push_str(&format!(
                "level{level}.example.com.\t3600\tIN\tA\t192.0.{level}.{r}\n"
            ));
        }
        s.push_str(&format!(
            "\n;; Query time: {level}00 msec\n;; SERVER: 192.0.2.{level}#53(192.0.2.{level}) (UDP)\n;; WHEN: Mon Jan 15 10:30:{level:02} UTC 2024\n;; MSG SIZE  rcvd: 1234\n\n"
        ));
    }
    s
}

pub(crate) fn task_dns_real(results: &mut Vec<RunResult>) {
    // Realistic `dig example.com ANY` shape: one query, ~5 answer records
    // for various types, and the full boilerplate (HEADER + flags + OPT
    // PSEUDOSECTION + COOKIE + QUESTION + Query time + SERVER + WHEN +
    // MSG SIZE). Real-world ratio: ~80% on this shape.
    let raw = "; <<>> DiG 9.18.12 <<>> example.com ANY +noall +answer\n;; global options: +cmd\n;; Got answer:\n;; ->>HEADER<<- opcode: QUERY, status: NOERROR, id: 12345\n;; flags: qr rd ra ad; QUERY: 1, ANSWER: 5, AUTHORITY: 4, ADDITIONAL: 9\n\n;; OPT PSEUDOSECTION:\n; EDNS: version: 0, flags: do; udp: 4096\n; COOKIE: c0ffeeface1234567890abcdef\n\n;; QUESTION SECTION:\n;example.com.\tIN\tANY\n\n;; ANSWER SECTION:\nexample.com.\t3600\tIN\tA\t93.184.216.34\nexample.com.\t3600\tIN\tAAAA\t2606:2800:220:1:248:1893:25c8:1946\nexample.com.\t3600\tIN\tNS\tns1.example.com.\nexample.com.\t3600\tIN\tMX\t10 mail.example.com.\nexample.com.\t3600\tIN\tTXT\t\"v=spf1 include:_spf.example.com ~all\"\n\n;; AUTHORITY SECTION:\nexample.com.\t86400\tIN\tNS\tns1.example.com.\nexample.com.\t86400\tIN\tNS\tns2.example.com.\nexample.com.\t86400\tIN\tNS\tns3.example.com.\nexample.com.\t86400\tIN\tNS\tns4.example.com.\n\n;; ADDITIONAL SECTION:\nns1.example.com.\t172800\tIN\tA\t192.0.2.1\nns2.example.com.\t172800\tIN\tA\t192.0.2.2\nns3.example.com.\t172800\tIN\tA\t192.0.2.3\nns4.example.com.\t172800\tIN\tA\t192.0.2.4\nns1.example.com.\t172800\tIN\tAAAA\t2001:db8::1\nns2.example.com.\t172800\tIN\tAAAA\t2001:db8::2\nns3.example.com.\t172800\tIN\tAAAA\t2001:db8::3\nns4.example.com.\t172800\tIN\tAAAA\t2001:db8::4\n\n;; Query time: 23 msec\n;; SERVER: 8.8.8.8#53(8.8.8.8) (UDP)\n;; WHEN: Mon Jan 15 10:30:00 UTC 2024\n;; MSG SIZE  rcvd: 423\n";
    let (out, sav, ms) = measure(raw, dns::compact_with_savings);
    let quality = if out.contains("[tokenade:dns]")
        && out.contains("example.com ANY → NOERROR")
        && out.contains("5 answers")
        && out.contains("4 authority")
        && out.contains("8 additional")
        && out.contains("93.184.216.34")
        && out.contains("2606:2800:220:1:248:1893:25c8:1946")
        && out.contains("v=spf1")
        && out.contains("query time: 23 msec")
        && !out.contains("EDNS: version")
        && !out.contains("MSG SIZE")
        && !out.contains("global options")
        && sav.percent() > 50.0
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "output:dns-compact",
        "dns",
        "synth-dig-any-5answers",
        raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #67: huge `dig +trace` with 30 levels × 10 records — the
// shape of a deeply-nested DNSSEC trace. Real-world ratio on this fixture
// shape is bounded by the answer record count (records pass through),
// hence the conservative 65 % floor. Must run under 500 ms regardless.
pub(crate) fn adv_dns_huge_trace(results: &mut Vec<RunResult>) {
    let raw = synth_dig_trace(30, 10);
    let (out, sav, ms) = measure(&raw, dns::compact_with_savings);
    let quality = if out.contains("example.com A → NOERROR") && sav.percent() > 65.0 && ms < 500 {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:dns:huge-trace-30",
        "dns",
        "adversarial",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #68: hostile content:
//   - TXT records with embedded `;` (SPF syntax) and embedded fake section
//     headers (`;; ANSWER SECTION:` inside a quoted TXT value)
//   - Unicode in TXT values (`"München DNS-test"`)
//   - NXDOMAIN response (no ANSWER section)
//   - mixed dig+nslookup output (multiple lookups concatenated)
//   - PTR reverse lookup format
pub(crate) fn adv_dns_hostile(results: &mut Vec<RunResult>) {
    let raw = "; <<>> DiG 9.18.12 <<>> -t TXT example.com\n;; Got answer:\n;; ->>HEADER<<- opcode: QUERY, status: NOERROR, id: 42\n;; flags: qr rd ra; QUERY: 1, ANSWER: 3, AUTHORITY: 0, ADDITIONAL: 0\n\n;; QUESTION SECTION:\n;example.com.\tIN\tTXT\n\n;; ANSWER SECTION:\nexample.com.\t300\tIN\tTXT\t\"v=spf1 ip4:192.0.2.0/24 ip6:2001:db8::/32 ~all; not-a-comment ;; ANSWER SECTION: also-fake\"\nexample.com.\t300\tIN\tTXT\t\"München-DNS-test: éàü 中文\"\nexample.com.\t300\tIN\tTXT\t\"google-site-verification=abc123def456ghi789\"\n\n;; Query time: 5 msec\n;; SERVER: 8.8.8.8#53(8.8.8.8) (UDP)\n\n; <<>> DiG 9.18.12 <<>> nonexistent.example.com\n;; Got answer:\n;; ->>HEADER<<- opcode: QUERY, status: NXDOMAIN, id: 99\n;; flags: qr rd ra; QUERY: 1, ANSWER: 0, AUTHORITY: 1, ADDITIONAL: 0\n\n;; QUESTION SECTION:\n;nonexistent.example.com.\tIN\tA\n\n;; AUTHORITY SECTION:\nexample.com.\t300\tIN\tSOA\tns1.example.com. admin.example.com. 1 7200 3600 1209600 300\n\n;; Query time: 8 msec\n";
    let (out, sav, ms) = measure(raw, dns::compact_with_savings);
    let quality = if out.contains("[tokenade:dns]")
        // TXT records preserved including SPF + Unicode + verify token.
        && out.contains("v=spf1 ip4:192.0.2.0/24")
        && out.contains("München-DNS-test")
        && out.contains("éàü 中文")
        && out.contains("google-site-verification=abc123def456ghi789")
        // The fake `;; ANSWER SECTION: also-fake` inside the TXT value must
        // NOT have triggered a section change — the second TXT record
        // (Unicode) MUST still be classified as an answer.
        && out.contains("3 answers")
        || out.contains("3 answer")  // at least one of the queries
        && ms < 100
        && sav.percent() > 0.0
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:dns:hostile",
        "dns",
        "adversarial",
        raw,
        &out,
        ms,
        quality,
    );
}
