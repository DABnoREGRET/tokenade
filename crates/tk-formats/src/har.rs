//! HTTP Archive (`.har`) — what a browser's devtools export when a page is
//! debugged: every request, its headers, cookies, timings and base64 bodies.
//!
//! Read as generic JSON, a 90 KB capture came back as its FIRST few entries,
//! headers and cookie values included — and the request that failed (a `500`
//! with `database timeout`, the one the capture was taken for) was not among
//! them (2026-10-04). What a capture is read for: which requests failed, with
//! what answer; then the list of requests, one line each.

use serde_json::Value;
use std::fmt::Write;

/// Requests listed one per line before the rest is counted; failures are
/// always listed, whatever their position.
pub const REQUESTS_CAP: usize = 200;
/// Characters of a failed request's or response's body.
const BODY_CAP: usize = 600;

pub fn detect(text: &str) -> bool {
    // A char boundary, not a byte count: the corpus has UTF-8 at byte 4096.
    let mut cut = text.len().min(4096);
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let head = &text[..cut];
    head.contains("\"log\"")
        && head.contains("\"entries\"")
        && (head.contains("\"request\"") || head.contains("\"creator\""))
        && serde_json::from_str::<Value>(text)
            .ok()
            .as_ref()
            .and_then(entries)
            .is_some()
}

fn entries(v: &Value) -> Option<&Vec<Value>> {
    let e = v.get("log")?.get("entries")?.as_array()?;
    e.first().and_then(|x| x.get("request")).map(|_| e)
}

fn human_bytes(n: f64) -> String {
    if n >= 1_048_576.0 {
        format!("{:.1} MB", n / 1_048_576.0)
    } else if n >= 1024.0 {
        format!("{:.1} KB", n / 1024.0)
    } else {
        format!("{n:.0} B")
    }
}

struct Req<'a> {
    method: &'a str,
    url: &'a str,
    status: i64,
    ms: f64,
    size: f64,
    mime: &'a str,
    req_body: Option<&'a str>,
    resp_body: Option<&'a str>,
    base64: bool,
}

fn req(e: &Value) -> Req<'_> {
    fn s(v: Option<&Value>) -> &str {
        v.and_then(Value::as_str).unwrap_or("")
    }
    let r = e.get("request");
    let p = e.get("response");
    let content = p.and_then(|p| p.get("content"));
    let size = content
        .and_then(|c| c.get("size"))
        .and_then(Value::as_f64)
        .filter(|n| *n >= 0.0)
        .or_else(|| {
            p.and_then(|p| p.get("bodySize"))
                .and_then(Value::as_f64)
                .filter(|n| *n >= 0.0)
        })
        .unwrap_or(0.0);
    Req {
        method: s(r.and_then(|r| r.get("method"))),
        url: s(r.and_then(|r| r.get("url"))),
        status: p
            .and_then(|p| p.get("status"))
            .and_then(Value::as_i64)
            .unwrap_or(0),
        ms: e.get("time").and_then(Value::as_f64).unwrap_or(0.0),
        size,
        mime: s(content.and_then(|c| c.get("mimeType"))),
        req_body: r
            .and_then(|r| r.get("postData"))
            .and_then(|d| d.get("text"))
            .and_then(Value::as_str),
        resp_body: content.and_then(|c| c.get("text")).and_then(Value::as_str),
        base64: content
            .and_then(|c| c.get("encoding"))
            .and_then(Value::as_str)
            == Some("base64"),
    }
}

/// A body as text: as stored, or decoded when the capture stored the answer
/// in base64 (Chrome does, for some JSON error responses). What does not
/// decode to UTF-8 — an image, a font — is not text and is not served.
fn readable_body(body: &str, r: &Req) -> Option<String> {
    if !r.base64 {
        return Some(body.to_string());
    }
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(body.trim())
        .ok()?;
    String::from_utf8(bytes).ok()
}

fn line(r: &Req) -> String {
    let status = if r.status == 0 {
        "—".to_string()
    } else {
        r.status.to_string()
    };
    let mime = r.mime.split(';').next().unwrap_or("").trim();
    format!(
        "{} {status} {}  {:.0} ms  {}  {mime}",
        r.method,
        r.url,
        r.ms,
        human_bytes(r.size)
    )
}

pub fn compact(text: &str) -> String {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return text.to_string();
    };
    let Some(list) = entries(&v) else {
        return text.to_string();
    };
    let reqs: Vec<Req> = list.iter().map(req).collect();
    // A status of 0 is a request that never got an answer (blocked,
    // cancelled, CORS): a failure as much as a 5xx.
    let failed = |r: &Req| r.status >= 400 || r.status == 0;
    let n_fail = reqs.iter().filter(|r| failed(r)).count();
    let total: f64 = reqs.iter().map(|r| r.size).sum();
    let mut out = String::new();
    let _ = write!(
        out,
        "HAR: {} request(s), {n_fail} failed, {} received",
        reqs.len(),
        human_bytes(total)
    );
    let started = |e: &Value| {
        e.get("startedDateTime")
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    if let (Some(a), Some(b)) = (
        list.first().and_then(started),
        list.last().and_then(started),
    ) {
        let _ = write!(out, ", {a} → {b}");
    }
    out.push('\n');
    if n_fail > 0 {
        out.push_str("\nfailed:\n");
        for r in reqs.iter().filter(|r| failed(r)) {
            let _ = writeln!(out, "  {}", line(r));
            if let Some(b) = r.req_body.filter(|b| !b.trim().is_empty()) {
                let _ = writeln!(
                    out,
                    "    request body: {}",
                    crate::head_tail::head_and_tail(b, BODY_CAP).replace('\n', "\n    ")
                );
            }
            if let Some(b) = r
                .resp_body
                .and_then(|b| readable_body(b, r))
                .filter(|b| !b.trim().is_empty())
            {
                let _ = writeln!(
                    out,
                    "    response: {}",
                    crate::head_tail::head_and_tail(&b, BODY_CAP).replace('\n', "\n    ")
                );
            }
        }
    }
    out.push_str("\nrequests:\n");
    let mut shown = 0usize;
    for r in &reqs {
        if shown < REQUESTS_CAP || failed(r) {
            let _ = writeln!(out, "  {}", line(r));
            shown += 1;
        }
    }
    if reqs.len() > shown {
        let _ = writeln!(
            out,
            "  … {} more request(s), none failed",
            reqs.len() - shown
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

    fn entry(i: usize, method: &str, url: &str, status: i64, body: &str) -> String {
        format!(
            r#"{{"startedDateTime":"2026-10-04T06:00:{i:02}.000Z","time":{t},"request":{{"method":"{method}","url":"{url}","headers":[{{"name":"cookie","value":"session=SECRET{i}"}}],"postData":{{"mimeType":"application/json","text":"{{\"qty\":{i}}}"}}}},"response":{{"status":{status},"headers":[],"content":{{"size":{sz},"mimeType":"application/json; charset=utf-8","text":{body}}}}},"timings":{{}}}}"#,
            t = 100 + i,
            sz = 1000 + i,
            body = serde_json::to_string(body).unwrap()
        )
    }

    fn har(n: usize, fail_at: usize) -> String {
        let e: Vec<String> = (0..n)
            .map(|i| {
                if i == fail_at {
                    entry(
                        i,
                        "POST",
                        "https://app.example.com/api/orders",
                        500,
                        r#"{"error":"database timeout on orders query"}"#,
                    )
                } else {
                    entry(
                        i,
                        "GET",
                        &format!("https://app.example.com/api/items/{i}"),
                        200,
                        r#"{"ok":true}"#,
                    )
                }
            })
            .collect();
        format!(
            r#"{{"log":{{"version":"1.2","creator":{{"name":"WebInspector"}},"entries":[{}]}}}}"#,
            e.join(",")
        )
    }

    #[test]
    fn the_failed_request_is_named_with_its_answer() {
        let h = har(40, 33);
        assert!(detect(&h));
        let out = compact(&h);
        assert!(out.starts_with("HAR: 40 request(s), 1 failed"), "{out}");
        assert!(
            out.contains("failed:\n  POST 500 https://app.example.com/api/orders  133 ms"),
            "{out}"
        );
        assert!(
            out.contains("response: {\"error\":\"database timeout on orders query\"}"),
            "{out}"
        );
        assert!(out.contains("request body: {\"qty\":33}"), "{out}");
        assert!(
            out.contains("GET 200 https://app.example.com/api/items/39"),
            "{out}"
        );
        assert!(
            !out.contains("SECRET"),
            "headers and cookies are not served: {out}"
        );
        assert!(
            out.contains("application/json") && !out.contains("charset"),
            "{out}"
        );
    }

    #[test]
    fn past_the_cap_a_failure_is_still_listed() {
        let out = compact(&har(REQUESTS_CAP + 30, REQUESTS_CAP + 20));
        let listed = out.split("\nrequests:\n").nth(1).unwrap_or("");
        assert!(listed.contains("POST 500"), "{out}");
        assert!(
            listed.contains("… 29 more request(s), none failed"),
            "{out}"
        );
    }

    /// A multi-byte character straddling the sniffed prefix does not panic.
    #[test]
    fn a_utf8_char_across_the_sniff_window_is_safe() {
        let s = format!("{}é{}", "a".repeat(4095), "b".repeat(10));
        assert!(!detect(&s));
    }

    /// A JSON error answer stored in base64 is decoded; a binary one is not.
    #[test]
    fn a_base64_error_answer_is_decoded() {
        let body = "eyJlcnJvciI6InF1b3RhIGV4Y2VlZGVkIn0="; // {"error":"quota exceeded"}
        let h = format!(
            r#"{{"log":{{"creator":{{"name":"x"}},"entries":[{{"time":5,"request":{{"method":"GET","url":"https://a.example/q"}},"response":{{"status":429,"content":{{"size":26,"mimeType":"application/json","encoding":"base64","text":"{body}"}}}}}},{{"time":5,"request":{{"method":"GET","url":"https://a.example/img"}},"response":{{"status":404,"content":{{"size":3,"mimeType":"image/png","encoding":"base64","text":"iVBO"}}}}}}]}}}}"#
        );
        let out = compact(&h);
        assert!(
            out.contains("response: {\"error\":\"quota exceeded\"}"),
            "{out}"
        );
        assert!(!out.contains("iVBO"), "{out}");
    }

    #[test]
    fn plain_json_is_not_a_har() {
        assert!(!detect(r#"{"log":{"level":"info"},"entries":[1,2]}"#));
    }
}
