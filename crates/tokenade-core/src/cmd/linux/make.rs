//! `make` — GNU make — drops make[N] Entering/Leaving directory recursion + 'Nothing to be done'.
//!
//! Archetype: strip

pub fn detect(text: &str) -> bool {
    text.contains("make[") || text.contains("make: ") || text.contains("make[1]:") || text.contains("Entering directory") || text.contains("Leaving directory")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    Some(crate::archetypes::strip_noise(text, &["make[1]: Entering directory", "make[1]: Leaving directory", "make[2]: Entering directory", "make[2]: Leaving directory", "make[3]: Entering directory", "make[3]: Leaving directory", "make[1]: Nothing to be done"], "make recursion noise"))
}

pub fn compact(text: &str) -> String {
    crate::archetypes::strip_noise(text, &["make[1]: Entering directory", "make[1]: Leaving directory", "make[2]: Entering directory", "make[2]: Leaving directory", "make[3]: Entering directory", "make[3]: Leaving directory", "make[1]: Nothing to be done"], "make recursion noise")
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
