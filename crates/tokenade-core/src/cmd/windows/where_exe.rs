//! Windows `where <name>` — locate executables on PATH.
//!
//! ```text
//! C:\Windows\System32\cmd.exe
//! C:\Windows\SysWOW64\cmd.exe
//! ```
//!
//! Tiny output — passthrough, just register the program name so
//! other parsers don't claim it.


pub fn detect(text: &str) -> bool {
    text.contains("\\") || text.contains('/')
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    Some(text.trim_end().to_string())
}

pub fn compact(text: &str) -> String {
    text.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_where_output() {
        let s = "C:\\Windows\\System32\\cmd.exe\nC:\\Windows\\SysWOW64\\cmd.exe\n";
        assert!(detect(s));
    }
}
