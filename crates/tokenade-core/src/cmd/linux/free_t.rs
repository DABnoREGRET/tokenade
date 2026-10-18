//! `free -t` — memory usage with a `Total:` summary row.
//!
//! Same output shape as plain `free` plus one trailing `Total:` row (which
//! the [`super::free`] engine's `is_mem_row` already recognises), so it
//! shares that engine: structural locale-independent detection + padding
//! collapse, nothing elided.

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
    fn detects_total_row() {
        // `-t` adds a Total: row; engine recognises it even if Mem: were absent.
        assert!(detect("               total       used\nTotal:         63437       15431\n"));
    }

    #[test]
    fn keeps_total_row_and_collapses() {
        let out = compact(
            "Mem:           63437       15431\nÉchange:           0           0\nTotal:         63437       15431\n",
        );
        assert!(out.contains("Total: 63437 15431"));
        assert!(out.contains("Échange: 0 0"));
    }
}
