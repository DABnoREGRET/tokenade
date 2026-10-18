//! `file <path>` / `file -b` — file type detection.
//!
//! ```text
//! /bin/ls: ELF 64-bit LSB pie executable, x86-64, version 1 (SYSV), ...
//! ```
//!
//! Output is one line per file. Already minimal. Recognise the
//! shape so other parsers don't try.


pub fn detect(text: &str) -> bool {
    crate::archetypes::any_output(text)
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
    fn detects_file_output() {
        let s = "/bin/ls: ELF 64-bit LSB pie executable, x86-64\n";
        assert!(detect(s));
    }
}
