//! `rm` — remove files. Silent on success; locale-dependent error
//! text on failure. Passthrough preserves the locale message verbatim
//! (e.g. `rm: /nonexistent: Datei oder Verzeichnis nicht gefunden`).

pub fn detect(text: &str) -> bool {
    crate::archetypes::any_output(text)
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

    #[test]
    fn rejects_empty() {
        assert!(!detect(""));
    }

    #[test]
    fn preserves_locale_message() {
        // German locale variant should round-trip without UTF-8 mangling.
        let de = "rm: /x: Datei oder Verzeichnis nicht gefunden";
        assert_eq!(compact(de), de);
    }
}
