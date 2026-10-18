//! `fuser <path>` / `fuser -m <mountpoint>` — processes holding a
//! file / mount open.
//!
//! Output: short — one line of space-separated PIDs + optional
//! access-mode codes. No real compaction needed; we just detect to
//! prevent other parsers from misclassifying.


pub fn detect(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return false;
    }
    // Shape: optional "<path>:" prefix then PIDs and mode codes.
    let toks: Vec<&str> = t.split_whitespace().collect();
    toks.iter().any(|t| t.chars().all(|c| c.is_ascii_digit()) && t.len() <= 7)
        && (text.contains(": ") || text.contains("USER PID"))
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    Some(text.trim().to_string())
}

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
    fn detects_fuser_short() {
        let s = "/var/log/syslog: 1234 5678 9012\n";
        assert!(detect(s));
    }
}
