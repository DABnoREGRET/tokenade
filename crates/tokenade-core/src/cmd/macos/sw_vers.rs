//! macOS `sw_vers` — 3-line version dump. Already minimal; we just
//! recognise the shape so dispatch doesn't bother trying other
//! parsers.


pub fn detect(text: &str) -> bool {
    text.contains("ProductName") || text.contains("ProductVersion") || text.contains("macOS")
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    text.trim().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_sw_vers() {
        let s = "ProductName:\tmacOS\nProductVersion:\t14.4\nBuildVersion:\t23E214\n";
        assert!(detect(s));
    }
}
