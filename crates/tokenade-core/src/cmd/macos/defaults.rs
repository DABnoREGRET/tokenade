//! macOS `defaults read <domain>` — preference dump.
//!
//! ```text
//! {
//!     AppleLanguages =     (
//!         "en-US",
//!         "fr-FR"
//!     );
//!     AppleLocale = "en_US";
//!     NSGlobalDomain = ...;
//!     ...
//! }
//! ```
//!
//! macOS plist-ish syntax (similar to JSON but with `=` and `;`).
//! Compaction: drop the noisy `_NS*` / `_DK*` internal AppKit keys.

use std::fmt::Write;

pub fn detect(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with('{')
        && (text.contains(" = (")
            || text.contains(" = \"")
            || text.contains(" = {")
            || text.contains(';'))
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut elided = 0usize;
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("_NS") || t.starts_with("_DK") || t.starts_with("_HI") {
            elided += 1;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if elided > 0 {
        let _ = writeln!(
            out,
            "  [elided: {elided} internal `_NS*`/`_DK*`/`_HI*` key(s)]"
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

    #[test]
    fn detects_defaults_dump() {
        let s = "{\n    AppleLanguages =     (\n        \"en-US\",\n        \"fr-FR\"\n    );\n    AppleLocale = \"en_US\";\n}";
        assert!(detect(s));
    }

    #[test]
    fn drops_internal_keys() {
        let s = "{\n    UserKey = \"value\";\n    _NSCacheCount = 42;\n    _DKDescriptor = 0;\n}";
        let out = compact(s);
        assert!(out.contains("UserKey"));
        assert!(!out.contains("_NSCacheCount"));
        assert!(out.contains("elided"));
    }
}
