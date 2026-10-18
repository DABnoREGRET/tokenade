//! The window of text around a match.
//!
//! Several PII rules only fire when a keyword sits near the value — a bare
//! number is not a phone number until something says "phone". These read the
//! neighbourhood without re-scanning the whole input.

/// Lower-cased slice of `s` spanning `[start-pad, end+pad]`, snapped to char
/// boundaries so UTF-8 input never panics.
pub(super) fn window_lower(s: &str, start: usize, end: usize, pad: usize) -> String {
    let mut lo = start.saturating_sub(pad);
    let mut hi = (end + pad).min(s.len());
    while lo > 0 && !s.is_char_boundary(lo) {
        lo -= 1;
    }
    while hi < s.len() && !s.is_char_boundary(hi) {
        hi += 1;
    }
    s[lo..hi].to_ascii_lowercase()
}

/// True when any keyword in `kws` appears within ±48 chars of the match. This
/// is the deterministic stand-in for Presidio's "context enhancer": proximity
/// to a domain word raises confidence, no model involved.
pub(super) fn ctx_near(s: &str, start: usize, end: usize, kws: &[&str]) -> bool {
    let win = window_lower(s, start, end, 48);
    kws.iter().any(|k| win.contains(k))
}
