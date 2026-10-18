//! Dispatcher for `dig`.
//! Extracted from `filter.rs` in the L108 refactor.

use crate::compress::dedup_lines;
use crate::ansi::strip_ansi;

pub fn dispatch(argv: &[String], output: &str) -> String {
    let has_short = argv.iter().any(|a| a == "+short");
    let has_trace = argv.iter().any(|a| a == "+trace");
    if has_short {
        let s = strip_ansi(output);
        return s.trim().to_string();
    }
    if has_trace {
        let s = strip_ansi(output);
        return dedup_lines(&s);
    }
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
    fn short_strips_ansi_and_trims() {
        // `dig +short example.com` — one IP per line, no headers.
        let body = "\x1b[36m93.184.216.34\x1b[0m\n\n\n";
        let out = dispatch(&argv(&["dig", "+short", "example.com"]), body);
        assert!(!out.contains("\x1b["), "must strip ANSI");
        assert_eq!(out, "93.184.216.34");
    }

    #[test]
    fn trace_strips_ansi_and_dedups() {
        // `dig +trace` walks the delegation chain — many similar
        // referral lines that benefit from dedup.
        let body = ".\t518400\tIN\tNS\ta.root-servers.net.\n\
                    .\t518400\tIN\tNS\ta.root-servers.net.\n\
                    .\t518400\tIN\tNS\tb.root-servers.net.\n\
                    com.\t172800\tIN\tNS\ta.gtld-servers.net.\n";
        let out = dispatch(&argv(&["dig", "+trace", "example.com"]), body);
        assert_eq!(
            out.matches("a.root-servers.net").count(),
            1,
            "+trace must dedup duplicate referrals; got: {out:?}"
        );
        assert!(out.contains("b.root-servers.net"));
        assert!(out.contains("gtld-servers"));
    }

    #[test]
    fn default_routes_to_auto() {
        // Default dig output has a structured answer section + header.
        let body = "; <<>> DiG 9.18.18 <<>> example.com\n\
                    ;; ANSWER SECTION:\n\
                    example.com.\t86400\tIN\tA\t93.184.216.34\n\
                    \n\
                    ;; Query time: 12 msec\n";
        let out = dispatch(&argv(&["dig", "example.com"]), body);
        // Auto preserves the answer at minimum.
        assert!(out.contains("93.184.216.34") || out.contains("example.com"));
    }
}
