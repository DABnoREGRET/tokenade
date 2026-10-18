//! CacheAligner — keep the provider KV-cache prefix byte-stable.
//!
//! Anthropic (and other providers) cache the system-prompt + tool-schema
//! prefix and reuse that KV cache across calls *only while the prefix stays
//! byte-identical*.  A single volatile token — a timestamp, a UUID, a session
//! id, a build version, an absolute home path — anywhere inside the cached
//! span invalidates the whole suffix, silently turning a cache hit into a full
//! re-encode and a cold, slower, more expensive request.
//!
//! This module is tokenade's take on headroom's CacheAligner. It:
//!
//! 1. **Detects** volatile tokens in any text tokenade contributes to a cached
//!    prefix ([`detect_dynamic`]).
//! 2. **Aligns** a prompt by relocating the volatile lines into a trailing
//!    block, leaving a stable cacheable head ([`align`] / [`split_static_tail`]).
//!
//! tokenade's own MCP `instructions` string and tool descriptions are already
//! compile-time literals (no `format!`), so the cached prefix is static *by
//! construction*.  The detector is wired as a regression guard so it stays that
//! way, and [`align`] is applied defensively at emit time (a no-op on already
//! static text) so any future dynamic token is pushed out of the cached span
//! instead of busting it.

use regex::Regex;
use std::sync::OnceLock;

/// A class of runtime-volatile token that would bust a cached prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DynamicKind {
    /// ISO-8601 calendar date, e.g. `2026-06-09`.
    IsoDate,
    /// Wall-clock time, e.g. `14:23:51`.
    ClockTime,
    /// RFC-4122 UUID (8-4-4-4-12 hex).
    Uuid,
    /// A long hex run (≥12) — session id, SHA, bearer/token fragment.
    HexBlob,
    /// A long decimal run (≥10) — unix epoch, large monotonic id.
    Epoch,
    /// Dotted three-part version, e.g. `v1.2.3` / `0.3.4`.
    SemVer,
    /// Absolute per-user path, e.g. `/home/alice/…` or `/Users/bob/…`.
    AbsHomePath,
}

/// One volatile token located in a piece of text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DynamicHit {
    pub kind: DynamicKind,
    pub matched: String,
    /// 0-based line index within the scanned text.
    pub line: usize,
}

/// Marker that separates the stable head from the relocated volatile tail in
/// [`align`] output.  Kept on its own line so it never merges into prose.
pub const VOLATILE_TAIL_MARKER: &str = "<!-- tokenade:volatile-tail -->";

/// Ordered (specific → general) so a UUID is reported as a UUID rather than as
/// an incidental hex blob; per-line de-duplication then drops the overlap.
fn patterns() -> &'static [(DynamicKind, Regex)] {
    static PATS: OnceLock<Vec<(DynamicKind, Regex)>> = OnceLock::new();
    PATS.get_or_init(|| {
        vec![
            (
                DynamicKind::Uuid,
                Regex::new(r"\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\b").unwrap(),
            ),
            (
                DynamicKind::IsoDate,
                Regex::new(r"\b\d{4}-\d{2}-\d{2}\b").unwrap(),
            ),
            (
                DynamicKind::ClockTime,
                Regex::new(r"\b\d{1,2}:\d{2}:\d{2}\b").unwrap(),
            ),
            (
                DynamicKind::SemVer,
                Regex::new(r"\bv?\d+\.\d+\.\d+\b").unwrap(),
            ),
            (
                DynamicKind::AbsHomePath,
                Regex::new(r"/(?:home|Users)/[A-Za-z0-9._-]+").unwrap(),
            ),
            (
                DynamicKind::Epoch,
                Regex::new(r"\b\d{10,}\b").unwrap(),
            ),
            (
                DynamicKind::HexBlob,
                Regex::new(r"\b[0-9a-fA-F]{12,}\b").unwrap(),
            ),
        ]
    })
}

/// Scan `text` and return every volatile token, in reading order.
///
/// Overlapping matches on the same line are de-duplicated by character span so
/// a UUID is not also double-counted as a hex blob.  This is a heuristic lint:
/// it flags tokens that *look* runtime-injected.  A hardcoded literal that
/// happens to match is technically still cache-stable; use it as a signal, not
/// a verdict.
pub fn detect_dynamic(text: &str) -> Vec<DynamicHit> {
    let pats = patterns();
    let mut hits = Vec::new();
    for (lineno, line) in text.lines().enumerate() {
        // Track claimed byte ranges so a more-specific pattern (UUID) wins over
        // a more-general one (HexBlob) overlapping the same span.
        let mut claimed: Vec<(usize, usize)> = Vec::new();
        for (kind, re) in pats {
            for m in re.find_iter(line) {
                let (s, e) = (m.start(), m.end());
                if claimed.iter().any(|&(cs, ce)| s < ce && cs < e) {
                    continue; // overlaps an already-claimed (more specific) span
                }
                // A 3-part version embedded in a longer dotted-numeric run is
                // an IPv4 octet group (e.g. "10.0.0" inside "10.0.0.5"), not a
                // SemVer — skip it so example IPs aren't false-flagged.
                if *kind == DynamicKind::SemVer {
                    let before = line[..s].ends_with(|c: char| c == '.' || c.is_ascii_digit());
                    let after = line[e..].starts_with('.');
                    if before || after {
                        continue;
                    }
                }
                claimed.push((s, e));
                hits.push(DynamicHit {
                    kind: *kind,
                    matched: m.as_str().to_string(),
                    line: lineno,
                });
            }
        }
    }
    hits
}

/// `true` when `text` contains any volatile token.
pub fn has_dynamic(text: &str) -> bool {
    let pats = patterns();
    text.lines()
        .any(|l| pats.iter().any(|(_, re)| re.is_match(l)))
}

/// Split `prompt` into `(static_head, volatile_tail)`: every line containing a
/// volatile token moves to the tail (preserving relative order), the rest stay
/// in the head.  Both retain their internal line order.  When no line is
/// volatile the tail is empty and the head equals the input.
pub fn split_static_tail(prompt: &str) -> (String, String) {
    let volatile_lines: std::collections::HashSet<usize> =
        detect_dynamic(prompt).into_iter().map(|h| h.line).collect();
    if volatile_lines.is_empty() {
        return (prompt.to_string(), String::new());
    }
    let mut head: Vec<&str> = Vec::new();
    let mut tail: Vec<&str> = Vec::new();
    for (i, line) in prompt.lines().enumerate() {
        if volatile_lines.contains(&i) {
            tail.push(line);
        } else {
            head.push(line);
        }
    }
    (head.join("\n"), tail.join("\n"))
}

/// Cache-align `prompt`: return the stable head followed by a marked tail block
/// holding the relocated volatile lines.  On already-static text this returns
/// the input unchanged (no marker added), so it is safe to apply unconditionally
/// at emit time.
pub fn align(prompt: &str) -> String {
    let (head, tail) = split_static_tail(prompt);
    if tail.is_empty() {
        head
    } else {
        format!("{head}\n\n{VOLATILE_TAIL_MARKER}\n{tail}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_each_dynamic_kind() {
        assert!(detect_dynamic("released 2026-06-09")
            .iter()
            .any(|h| h.kind == DynamicKind::IsoDate));
        assert!(detect_dynamic("at 14:23:51 UTC")
            .iter()
            .any(|h| h.kind == DynamicKind::ClockTime));
        assert!(
            detect_dynamic("session 550e8400-e29b-41d4-a716-446655440000")
                .iter()
                .any(|h| h.kind == DynamicKind::Uuid)
        );
        assert!(detect_dynamic("sha deadbeefcafe1234")
            .iter()
            .any(|h| h.kind == DynamicKind::HexBlob));
        assert!(detect_dynamic("ts 1717900000")
            .iter()
            .any(|h| h.kind == DynamicKind::Epoch));
        assert!(detect_dynamic("tokenade v0.3.4")
            .iter()
            .any(|h| h.kind == DynamicKind::SemVer));
        assert!(detect_dynamic("cwd /home/username/project")
            .iter()
            .any(|h| h.kind == DynamicKind::AbsHomePath));
    }

    #[test]
    fn uuid_not_double_counted_as_hexblob() {
        let hits = detect_dynamic("id 550e8400-e29b-41d4-a716-446655440000 end");
        // Exactly one hit for the UUID span — the trailing 12-hex group must not
        // also surface as a separate HexBlob.
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].kind, DynamicKind::Uuid);
    }

    #[test]
    fn semver_does_not_match_ipv4_octets() {
        // "10.0.0.5" is an IPv4 example, not a version — no SemVer hit.
        let hits = detect_dynamic("filter slot {\"N\":\"10.0.0.5\"} only");
        assert!(
            !hits.iter().any(|h| h.kind == DynamicKind::SemVer),
            "IPv4 octets false-flagged as SemVer: {hits:?}"
        );
        // A genuine version is still caught.
        assert!(detect_dynamic("upgraded to v2.10.3 today")
            .iter()
            .any(|h| h.kind == DynamicKind::SemVer));
    }

    #[test]
    fn static_prose_has_no_dynamics() {
        let s = "Token-optimized tools. PREFER over their raw equivalents:\n\
                 • exec_compact over raw Bash for noisy commands (60-95% fewer tokens).\n\
                 • semantic_search query=\"…\" for natural language.";
        assert!(!has_dynamic(s), "false positive: {:?}", detect_dynamic(s));
        // align is a pure no-op on static text.
        assert_eq!(align(s), s);
    }

    #[test]
    fn align_relocates_volatile_line_to_tail() {
        let prompt = "stable header line\n\
                      generated at 2026-06-09 14:23:51\n\
                      stable footer line";
        let out = align(prompt);
        assert!(out.contains(VOLATILE_TAIL_MARKER));
        // The cacheable head holds the stable lines, in order, before the marker.
        let head = out.split(VOLATILE_TAIL_MARKER).next().unwrap();
        assert!(head.contains("stable header line"));
        assert!(head.contains("stable footer line"));
        assert!(!head.contains("2026-06-09"));
        // The volatile date is parked after the marker.
        let tail = out.split(VOLATILE_TAIL_MARKER).nth(1).unwrap();
        assert!(tail.contains("2026-06-09 14:23:51"));
    }

    #[test]
    fn split_keeps_line_order_within_each_section() {
        let prompt = "a\nb 2026-01-01\nc\nd 99:99 not-time\ne 12:00:00";
        let (head, tail) = split_static_tail(prompt);
        assert_eq!(head, "a\nc\nd 99:99 not-time");
        assert_eq!(tail, "b 2026-01-01\ne 12:00:00");
    }
}
