//! `uname -a` / `uname -srm` / `hostnamectl`.
//!
//! `uname -a`: single line, no compaction. Detector exists for
//! routing only. `hostnamectl` has a multi-line key:value block;
//! pass through.


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
    fn detects_hostnamectl() {
        let s = "Static hostname: foo\nIcon name: computer-laptop\nChassis: laptop\nOperating System: Ubuntu 24.04\nKernel: Linux 6.8.0\n";
        assert!(detect(s));
    }
}
