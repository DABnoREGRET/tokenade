//! `lspci` / `lspci -v` — PCI bus devices.


pub fn detect(text: &str) -> bool {
    let mut hits = 0usize;
    for line in text.lines().take(20) {
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.first().is_some_and(|f| f.contains(':'))
            && line.contains("controller") || line.contains("bridge") || line.contains("USB")
        {
            hits += 1;
        }
    }
    hits >= 3
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    crate::archetypes::head_lines_with_tally(text, 50, "device(s) elided")
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_lspci() {
        let s = "00:00.0 Host bridge: Intel Corporation Skylake Host Bridge\n00:02.0 VGA compatible controller: Intel Corporation HD Graphics\n00:14.0 USB controller: Intel Corporation Sunrise Point-LP USB 3.0 xHCI Controller\n";
        assert!(detect(s));
    }
}
