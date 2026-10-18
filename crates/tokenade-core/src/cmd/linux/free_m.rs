//! `free -m` — memory usage in mebibytes.
//!
//! Same output shape as plain `free` (only the unit changes), so it shares
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
    fn detects_mem_row() {
        assert!(detect("               total       used\nMem:           63437       15431\n"));
    }

    #[test]
    fn collapses_padding() {
        let out = compact("Mem:           63437       15431        5878\n");
        assert_eq!(out, "Mem: 63437 15431 5878");
    }
}
