//! `openssl_cli` — OpenSSL Linux CLI.
//!
//! Archetype: strip

pub fn detect(text: &str) -> bool {
    text.contains("BEGIN CERTIFICATE") || text.contains("issuer=") || text.contains("subject=")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    // ONE spelling, so the two entry points cannot drift apart.
    Some(compact(text))
}

pub fn compact(text: &str) -> String {
    // This compactor's own rule — drop four-space-indented lines — matches
    // almost nothing in a real `s_client` transcript: measured 2026-09-06 on a
    // live handshake, 6 572 bytes in and 6 571 out while 4 416 of them were
    // certificate base64. What folds those is `crate::pem`, applied once for
    // every command route in `filter::shrink_only_post_passes`, so it is
    // deliberately NOT repeated here.
    crate::archetypes::strip_noise(text, &["    "], "openssl indent")
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
        assert!(!detect("   \n"));
    }

    #[test]
    fn try_compact_some_or_none() {
        let s = "sample output line\nsecond line\n";
        let _ = try_compact(s);
        let _ = compact(s);
        let _ = compact_with_savings(s);
    }
}
