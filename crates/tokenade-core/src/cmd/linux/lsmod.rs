//! `lsmod` — loaded kernel modules.


pub fn detect(text: &str) -> bool {
    let first = text.lines().next().unwrap_or("");
    first.starts_with("Module") && first.contains("Size") && first.contains("Used by")
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    crate::archetypes::head_lines_with_tally(text, 40, "module(s) elided")
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_lsmod() {
        let s = "Module                  Size  Used by\nbnep                   24576  2\nbluetooth             802816  19 bnep\n";
        assert!(detect(s));
    }
}
