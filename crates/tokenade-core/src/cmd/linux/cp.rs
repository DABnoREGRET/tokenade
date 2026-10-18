//! `cp` — copy files. Output is usually empty on success, with error
//! messages on failure (locale-dependent text). Passthrough.

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
    fn rejects_empty_passes_through_otherwise() {
        assert!(!detect(""));
        assert_eq!(compact("cp: /x: No such file\n"), "cp: /x: No such file");
    }
}
