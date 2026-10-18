//! `free -h` — human-readable memory usage.
//!
//! Same output shape as plain `free` (only the units differ), so it shares
//! the [`super::free`] engine: structural, locale-independent detection via
//! the untranslated `Mem:` row, then alignment-padding collapse.

pub fn detect(text: &str) -> bool {
    super::free::detect(text)
}

pub fn try_compact(text: &str) -> Option<String> {
    super::free::try_compact(text)
}

pub fn compact(text: &str) -> String {
    super::free::compact(text)
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    super::free::compact_with_savings(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_french_mem_row() {
        assert!(detect(
            "               total       utilisé      libre\nMem:            61Gi        15Gi       5,7Gi\nÉchange:          0B          0B          0B\n"
        ));
    }

    #[test]
    fn collapses_padding_keeps_units() {
        let out = compact("Mem:            61Gi        15Gi       5,7Gi\n");
        assert_eq!(out, "Mem: 61Gi 15Gi 5,7Gi");
    }
}
