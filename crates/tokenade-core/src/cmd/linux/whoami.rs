//! `whoami` — single-line username output.

/// `whoami` without arguments returns ONE line: a user name.
///
/// This module is a passthrough — it returns the text unchanged — and it was
/// placed at the head of `whoami`'s `or_else` with `any_output` as detector.
/// It therefore swallowed ALL `whoami` output, including Windows'
/// `whoami /all` and macOS's `whoami -all`, and the two compactors written for
/// those outputs never ran. MEASURED on the corpus's `windows/whoami_all`,
/// a table of groups and privileges: returned WHOLE.
///
/// A passthrough at the head of a chain disarms everything after it; this one
/// now only answers for what its own comment describes.
pub fn detect(text: &str) -> bool {
    if !crate::archetypes::any_output(text) {
        return false;
    }
    text.trim().lines().count() == 1
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    Some(crate::archetypes::passthrough(text))
}

pub fn compact(text: &str) -> String {
    crate::archetypes::passthrough(text)
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_valid() { assert!(detect("root")); }
    /// And it DECLINES what is not one line: that refusal is what makes the
    /// `whoami /all` compactors reachable.
    #[test]
    fn detector_declines_a_table() {
        assert!(!detect("GROUP INFORMATION\n-----------------\nEveryone S-1-1-0\n"));
    }
    #[test] fn compact_returns_trimmed() { assert_eq!(compact("root\n"), "root"); }
    #[test] fn try_compact_returns_some() { assert!(try_compact("root\n").is_some()); }
}
