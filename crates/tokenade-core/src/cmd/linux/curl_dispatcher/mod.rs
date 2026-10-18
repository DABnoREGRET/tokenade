//! Dispatcher for `curl`.
//! Extracted from `filter.rs` in the L108 refactor.

use crate::argv::has_any_flag;
use crate::ansi::strip_ansi;

pub fn dispatch(argv: &[String], output: &str) -> String {
    if has_any_flag(argv, &["-v", "--verbose", "--trace", "--trace-ascii"]) {
        // Already covered by the auto curl-verbose detector.
        let (_, compacted, _) = crate::auto::compact(output);
        return compacted;
    }
    if has_any_flag(argv, &["-I", "--head"]) {
        // HTTP headers — small. Pass through.
        let s = strip_ansi(output);
        return s.trim().to_string();
    }
    // -s / --silent and default both produce body content (with or
    // without curl's progress bar prefix — strip_ansi inside
    // auto::compact handles the CR-based redraws). Consolidated path:
    // the auto detector picks JSON/HTML/whatever the body shape is.
    let (_, compacted, _) = crate::auto::compact(output);
    compacted
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn verbose_routes_to_auto() {
        // -v / --verbose / --trace / --trace-ascii all hit the auto
        // detector, which has a dedicated curl-verbose compactor.
        let body = "* Trying 1.2.3.4:443...\n\
                    * Connected to api.example.com (1.2.3.4) port 443\n\
                    > GET /v1/users HTTP/2\n\
                    > Host: api.example.com\n\
                    < HTTP/2 200\n\
                    < content-type: application/json\n\
                    {\"users\":[]}\n";
        for flag in &["-v", "--verbose", "--trace", "--trace-ascii"] {
            let out = dispatch(&argv(&["curl", flag, "https://api/x"]), body);
            // auto's curl-verbose compactor preserves the request/response
            // markers (or at least the body if it falls through).
            assert!(
                !out.is_empty(),
                "{flag} dispatch produced empty output"
            );
        }
    }

    #[test]
    fn head_strips_ansi_and_trims() {
        // -I / --head: HTTP headers only. The dispatcher strips ANSI
        // and trims — does NOT run auto::compact (would mis-detect
        // headers as some structured format).
        let body = "\x1b[36mHTTP/2 200 \x1b[0m\n\
                    content-type: application/json\n\
                    cache-control: no-store\n\
                    \n\n";
        for flag in &["-I", "--head"] {
            let out = dispatch(&argv(&["curl", flag, "https://api/x"]), body);
            assert!(!out.contains("\x1b["), "{flag} must strip ANSI");
            assert!(out.contains("HTTP/2 200"));
            assert!(out.contains("content-type"));
            // Trim drops trailing blank lines.
            assert!(
                !out.ends_with("\n\n"),
                "{flag} must trim trailing whitespace; got: {out:?}"
            );
        }
    }

    #[test]
    fn silent_routes_body_to_auto() {
        // -s / --silent: body only. JSON body should round-trip through
        // the json compactor.
        let body = r#"{"users":[{"id":1,"name":"a"},{"id":2,"name":"b"},{"id":3,"name":"c"},{"id":4,"name":"d"},{"id":5,"name":"e"}]}"#;
        for flag in &["-s", "--silent"] {
            let out = dispatch(&argv(&["curl", flag, "https://api/users"]), body);
            assert!(out.contains("users") || out.contains("name"));
        }
    }

    #[test]
    fn default_routes_body_to_auto() {
        // No flag: progress bar + body. The CR-based progress is handled
        // by strip_ansi inside auto::compact; body content survives.
        let body = "% Total    % Received % Xferd  Average Speed   Time\r\n\
                    100   123  100   123    0     0   1234      0  0:00:01\r\n\
                    {\"ok\":true,\"items\":[1,2,3,4,5,6,7,8,9,10,11,12]}";
        let out = dispatch(&argv(&["curl", "https://api/x"]), body);
        // auto picks up the JSON body even with the progress prefix.
        assert!(out.contains("ok") || out.contains("items"));
    }
}
