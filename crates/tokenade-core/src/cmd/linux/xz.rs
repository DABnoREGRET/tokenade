//! `xz` — xz compressor.
//!
//! Archetype: strip

pub fn detect(text: &str) -> bool {
    text.contains("xz: ")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    Some(crate::archetypes::strip_noise(text, &["xz: warning"], "xz warning"))
}

pub fn compact(text: &str) -> String {
    crate::archetypes::strip_noise(text, &["xz: warning"], "xz warning")
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
