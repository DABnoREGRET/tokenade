//! `accesslog` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

/// Realistic 10 000-line access log with 5 endpoint shapes + bot probes.
pub(crate) fn task_accesslog_real(results: &mut Vec<RunResult>) {
    let mut t = String::new();
    let endpoints: [(&str, &str, u16); 5] = [
        ("GET", "/api/users/{}", 200),
        ("GET", "/api/users/{}/orders", 200),
        ("POST", "/api/orders", 201),
        ("GET", "/static/app.css", 200),
        ("GET", "/healthz", 200),
    ];
    for i in 0..10_000 {
        let (m, p, s) = endpoints[i % endpoints.len()];
        let path = p.replace("{}", &i.to_string());
        t.push_str(&format!(
            "10.0.{}.{} - alice [01/Jan/2024:12:34:56 +0000] \"{m} {path} HTTP/1.1\" {s} 1234 \"https://app/\" \"Mozilla/5.0 (X11; Linux) Chrome/120\"\n",
            (i / 255) % 255, i % 255
        ));
    }
    // 200 bot probes hitting 404.
    for i in 0..200 {
        t.push_str(&format!(
            "8.8.8.{} - - [01/Jan/2024:12:34:56 +0000] \"GET /wp-admin/{i} HTTP/1.0\" 404 9 \"-\" \"BadBot/0.9\"\n",
            i % 255
        ));
    }
    let (out, sav, ms) = measure(&t, accesslog::compact_with_savings);
    // Quality: total count + templates aggregated + 404 surfaced.
    let total_ok = out.contains("10200 request(s)");
    let templates_grouped = out.contains("/api/users/<id>") && out.contains("/wp-admin/<id>");
    let status_summary = out.contains("status counts:") && out.contains("404=200");
    let quality = if total_ok && templates_grouped && status_summary {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "output:accesslog-compact",
        "accesslog",
        "synth-10k-mixed",
        &t,
        &out,
        ms,
        quality,
    );
    let _ = sav;
}

// Adversarial #41: 100 000 access-log lines must compact > 99 % AND complete
// in under 1 second.
pub(crate) fn adv_accesslog_perf(results: &mut Vec<RunResult>) {
    let mut t = String::with_capacity(100_000 * 200);
    for i in 0..100_000 {
        t.push_str(&format!(
            "10.0.0.{} - - [01/Jan/2024:12:34:56 +0000] \"GET /api/items/{i} HTTP/1.1\" 200 1234 \"-\" \"Mozilla/5.0\"\n",
            i % 255
        ));
    }
    let (out, sav, ms) = measure(&t, accesslog::compact_with_savings);
    let quality = if out.contains("100000 request(s)")
        && out.contains("/api/items/<id>")
        && sav.percent() > 99.0
        && ms < 1500
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:accesslog:huge-100k",
        "accesslog",
        "adversarial",
        &t,
        &out,
        ms,
        quality,
    );
}

// Adversarial #42: malformed lines (truncated, bytes-as-junk, missing fields)
// mixed with valid ones — the parser must skip+count the bad ones, not
// crash, and still aggregate the good ones.
pub(crate) fn adv_accesslog_malformed_mix(results: &mut Vec<RunResult>) {
    let mut t = String::new();
    for i in 0..100 {
        t.push_str(&format!(
            "1.2.3.4 - - [01/Jan/2024:12:34:56 +0000] \"GET /api/x/{i} HTTP/1.1\" 200 1 \"-\" \"UA\"\n"
        ));
    }
    // Junk lines.
    t.push_str("not an access log at all\n");
    t.push_str("\x00binary garbage with no shape\n");
    t.push_str("1.2.3.4 truncated line\n");
    t.push_str("[01/Jan/2024:12:34:56 +0000] missing IP\n");
    let (out, _sav, ms) = measure(&t, accesslog::compact_with_savings);
    let quality = if out.contains("100 request(s)")
        && out.contains("/api/x/<id>")
        && out.contains("failed to parse")
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:accesslog:malformed-mix",
        "accesslog",
        "adversarial",
        &t,
        &out,
        ms,
        quality,
    );
}
