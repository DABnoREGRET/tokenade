//! `kubectl_kustomize` — kubectl kustomize.
//!
//! Archetype: strip

pub fn detect(text: &str) -> bool {
    text.lines().any(|l| l.trim_start().starts_with("apiVersion:") || l.trim_start().starts_with("kind:") || l.trim_start().starts_with("name:"))
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    Some(crate::archetypes::strip_noise(text, &["---"], "yaml sep"))
}

pub fn compact(text: &str) -> String {
    crate::archetypes::strip_noise(text, &["---"], "yaml sep")
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn try_compact_some_or_none() {
        let _ = try_compact("sample\n");
        let _ = compact("sample\n");
        let _ = compact_with_savings("sample\n");
    }
}
