//! `java_runtime` — Java stack trace. Drops JDK internal frames.
//!
//! Archetype: strip

pub fn detect(text: &str) -> bool {
    text.contains("Exception in thread") || text.contains("    at ") || text.contains("Caused by:")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    Some(crate::archetypes::strip_noise(text, &["    at sun.reflect", "    at java.base/", "    at java.lang.reflect", "    at jdk.internal"], "JDK internal frames"))
}

pub fn compact(text: &str) -> String {
    crate::archetypes::strip_noise(text, &["    at sun.reflect", "    at java.base/", "    at java.lang.reflect", "    at jdk.internal"], "JDK internal frames")
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
