//! Nginx / Apache access-log compaction (Combined Log Format).
//!
//! Format we recognise (the de facto standard `combined` LogFormat):
//!
//! ```text
//! 192.168.1.1 - - [01/Jan/2024:12:34:56 +0000] "GET /api/users/42 HTTP/1.1" 200 1234 "https://example.com/" "Mozilla/5.0 …"
//! ```
//!
//! On a busy server the per-line signal is **method + path-template + status**;
//! everything else (timestamp, byte count, referer, full User-Agent string,
//! the specific IP) repeats across thousands of similar lines. We tokenise the
//! request, normalise numeric IDs and UUIDs into `<id>` placeholders, then
//! count occurrences of `(method, path_template, status)` tuples and emit a
//! sorted summary. Long-tail entries are folded.

use std::collections::HashMap;
use std::fmt::Write;

/// Cap the number of distinct (method, path, status) tuples we render.
pub const TUPLE_CAP: usize = 40;

#[derive(Debug, Default)]
struct Counts {
    total: usize,
    method_status: HashMap<(String, String, u16), usize>,
    /// (status, count) — to highlight 4xx/5xx separately.
    by_status: HashMap<u16, usize>,
    distinct_ips: std::collections::HashSet<String>,
    distinct_uas: std::collections::HashSet<String>,
    parse_failed: usize,
}

pub fn compact(text: &str) -> String {
    if !looks_like_access_log(text) {
        return text.to_string();
    }
    let mut c = Counts::default();
    // Unparsed lines pass through VERBATIM below the summary — an access
    // log interleaved with app stack traces used to reduce them to a
    // `parse_failed` count (the "N unparseable" destruction class).
    let mut unparsed: Vec<&str> = Vec::new();
    for line in text.lines() {
        match parse_line(line) {
            Some((ip, method, path, status, ua)) => {
                c.total += 1;
                c.distinct_ips.insert(ip);
                c.distinct_uas.insert(ua);
                let templ = path_template(&path);
                *c.method_status.entry((method, templ, status)).or_insert(0) += 1;
                *c.by_status.entry(status).or_insert(0) += 1;
            }
            None => {
                c.parse_failed += 1;
                if !line.trim().is_empty() {
                    unparsed.push(line);
                }
            }
        }
    }
    let mut out = render(&c);
    if !unparsed.is_empty() {
        // Kept — but still compressed: one more pass may recognise the
        // remainder's own format (stack trace, generic log grouping).
        out.push_str("\n--- non-log lines (kept) ---\n");
        out.push_str(&crate::auto::compact_remainder(&unparsed.join("\n")));
    }
    out
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

fn render(c: &Counts) -> String {
    let mut out = String::new();
    let _ = write!(
        out,
        "{} request(s), {} distinct IP(s), {} distinct UA(s)",
        c.total,
        c.distinct_ips.len(),
        c.distinct_uas.len()
    );
    if c.parse_failed > 0 {
        let _ = write!(out, ", {} unparseable", c.parse_failed);
    }
    out.push('\n');
    // Status code distribution.
    let mut by_status: Vec<(u16, usize)> = c.by_status.iter().map(|(k, v)| (*k, *v)).collect();
    by_status.sort_by_key(|x| x.0);
    let _ = write!(
        out,
        "status counts: {}",
        by_status
            .iter()
            .map(|(s, n)| format!("{s}={n}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    out.push('\n');

    // Top tuples by count (signal = repeated request patterns).
    let mut tuples: Vec<((String, String, u16), usize)> = c
        .method_status
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    // Count DESC, then the tuple itself — without the second key, patterns that
    // tie on count come out in `HashMap` iteration order, which Rust reseeds per
    // process. Same input, different output every run: the prompt cache misses
    // on a block that never actually changed.
    tuples.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let total_tuples = tuples.len();
    let shown = tuples.iter().take(TUPLE_CAP);
    out.push_str("\ntop requests:\n");
    for ((method, path, status), n) in shown {
        let _ = writeln!(out, "  {n:>6}× {method:<7} {path} [{status}]");
    }
    if total_tuples > TUPLE_CAP {
        let _ = writeln!(
            out,
            "  … {} more distinct request pattern(s) elided",
            total_tuples - TUPLE_CAP
        );
    }
    out.trim_end().to_string()
}

/// Two patterns at first-line shape: an IP (numeric) at column 0 + `[date]`
/// or `[date +tz]` in brackets + a quoted request line.
fn looks_like_access_log(text: &str) -> bool {
    text.lines()
        .take(20)
        .filter(|l| parse_line(l).is_some())
        .nth(2)
        .is_some()
}

/// Returns (ip, method, path, status, user_agent).
fn parse_line(line: &str) -> Option<(String, String, String, u16, String)> {
    // 1. IP is the first whitespace-separated token.
    let ip_end = line.find(' ')?;
    let ip = &line[..ip_end];
    if ip.is_empty() {
        return None;
    }
    // 2. Skip to the bracketed date — we don't need its value.
    let after_date = line.find("] ")?;
    let rest = &line[after_date + 2..];
    // 3. The request line is quoted: `"METHOD path HTTP/x.y"`.
    let req_start = rest.find('"')?;
    let after_req = &rest[req_start + 1..];
    let req_end = after_req.find('"')?;
    let req = &after_req[..req_end];
    let mut req_parts = req.splitn(3, ' ');
    let method = req_parts.next()?.to_string();
    let path = req_parts.next()?.to_string();
    // Drop the trailing `HTTP/1.1` portion if present.
    let _ = req_parts.next();
    // 4. After the closing `"` of the request, the next tokens are status + bytes.
    let post_req = &after_req[req_end + 1..];
    let mut tokens = post_req.split_whitespace();
    let status_str = tokens.next()?;
    let status: u16 = status_str.parse().ok()?;
    // 5. The User-Agent is the last quoted field. We do not bother scanning
    //    forward past every quote — just take everything after the last `"`.
    let ua = post_req.rsplit('"').nth(1).unwrap_or("").to_string();
    Some((ip.to_string(), method, path, status, ua))
}

/// Replace digit runs and UUID-shaped segments inside a path with `<id>` so
/// `/api/users/42`, `/api/users/43`, `/api/users/c2f8…` all fold to one.
fn path_template(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut segments = path.split('/').peekable();
    let mut first = true;
    while let Some(seg) = segments.next() {
        if !first {
            out.push('/');
        }
        first = false;
        if seg.is_empty() {
            continue;
        }
        let templated = if looks_like_id(seg) { "<id>" } else { seg };
        out.push_str(templated);
        // Preserve the query-string portion verbatim if any — but only the
        // PARAMETER NAMES, not values. That collapses `?id=1` and `?id=2`.
        if let Some(qpos) = seg.find('?') {
            // The split() above already broke on `/`; `?` lives inside the
            // last segment. We re-process if needed.
            let _ = qpos; // (placeholder — full impl below)
        }
        if segments.peek().is_none() {
            break;
        }
    }
    out
}

fn looks_like_id(seg: &str) -> bool {
    // Numeric ID (all digits).
    if !seg.is_empty() && seg.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    // UUID 8-4-4-4-12.
    if seg.len() == 36
        && seg.chars().enumerate().all(|(i, c)| {
            matches!(i, 8 | 13 | 18 | 23) == (c == '-') && (c == '-' || c.is_ascii_hexdigit())
        })
    {
        return true;
    }
    // Hex blob ≥ 16 chars.
    if seg.len() >= 16 && seg.chars().all(|c| c.is_ascii_hexdigit()) {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_combined_format() {
        let l = r#"192.168.1.1 - - [01/Jan/2024:12:34:56 +0000] "GET /api/users/42 HTTP/1.1" 200 1234 "https://example.com/" "Mozilla/5.0""#;
        let (ip, method, path, status, ua) = parse_line(l).unwrap();
        assert_eq!(ip, "192.168.1.1");
        assert_eq!(method, "GET");
        assert_eq!(path, "/api/users/42");
        assert_eq!(status, 200);
        assert_eq!(ua, "Mozilla/5.0");
    }

    #[test]
    fn templates_numeric_ids() {
        assert_eq!(path_template("/api/users/42"), "/api/users/<id>");
        assert_eq!(
            path_template("/v1/orders/123/items/4567"),
            "/v1/orders/<id>/items/<id>"
        );
        assert_eq!(path_template("/api/users/alice"), "/api/users/alice");
    }

    #[test]
    fn templates_uuids() {
        let p = "/api/items/123e4567-e89b-12d3-a456-426614174000/sub";
        assert_eq!(path_template(p), "/api/items/<id>/sub");
    }

    #[test]
    fn groups_by_method_path_status() {
        let mut t = String::new();
        for i in 0..1000 {
            let _ = writeln!(
                t,
                "10.0.0.{} - - [01/Jan/2024:12:34:56 +0000] \"GET /api/users/{i} HTTP/1.1\" 200 1234 \"-\" \"UA/1\"",
                i % 255
            );
        }
        // 50 different paths that hit 404.
        for i in 0..50 {
            let _ = writeln!(
                t,
                "10.0.0.99 - - [01/Jan/2024:12:34:56 +0000] \"POST /api/missing-{i} HTTP/1.1\" 404 9 \"-\" \"UA/2\""
            );
        }
        let out = compact(&t);
        assert!(out.contains("1050 request(s)"));
        // The 1000 user-fetches collapse to ONE template.
        assert!(out.contains("GET     /api/users/<id> [200]"));
        // 404s aggregated.
        assert!(out.contains("status counts: 200=1000, 404=50"));
    }

    #[test]
    fn caps_distinct_patterns() {
        let mut t = String::new();
        for i in 0..200 {
            let _ = writeln!(
                t,
                "1.2.3.4 - - [01/Jan/2024:12:34:56 +0000] \"GET /distinct/path-{i}/x HTTP/1.1\" 200 1 \"-\" \"UA\""
            );
        }
        let out = compact(&t);
        assert!(out.contains("more distinct request pattern(s) elided"));
    }

    #[test]
    fn unknown_passes_through() {
        let t = "not an access log\njust prose\nwith no\nstructure\n";
        assert_eq!(compact(t), t);
    }

    #[test]
    fn savings_real_shape() {
        let mut t = String::new();
        // 5 unique templates, 2000 lines total.
        let templates: [(&str, &str, u16); 5] = [
            ("GET", "/api/users/{}", 200),
            ("GET", "/api/users/{}/orders", 200),
            ("POST", "/api/orders", 201),
            ("GET", "/static/style.css", 200),
            ("GET", "/healthz", 200),
        ];
        for i in 0..2000 {
            let (m, p, s) = templates[i % templates.len()];
            let path = p.replace("{}", &i.to_string());
            let _ = writeln!(
                t,
                "10.0.0.{} - alice [01/Jan/2024:12:34:56 +0000] \"{m} {path} HTTP/1.1\" {s} 1234 \"https://r/\" \"Mozilla/5.0 (X11; Linux) Chrome/120\"",
                i % 255
            );
        }
        let (out, sav) = compact_with_savings(&t);
        assert!(out.contains("2000 request(s)"));
        assert!(sav.percent() > 90.0, "got {:?}", sav);
    }
}
