//! `strace_T` — strace T.
const HEAD: usize = 80;


pub fn detect(text: &str) -> bool {
    text.lines().any(|l| l.contains('(') && l.contains(')') && l.contains('<'))
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    super::strace::fold_trace(text, HEAD, "strace-T")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_accepts_valid() { assert!(detect("getpid() = 12345 <0.001234>")); }
    #[test] fn short_output_passes_through() {
        let line = "getpid() = 12345 <0.001234>";
        let s: String = std::iter::repeat_n(line, 3).map(|l| format!("{l}\n")).collect();
        assert!(try_compact(&s).is_none());
    }
    #[test] fn long_output_folds() {
        let line = "getpid() = 12345 <0.001234>";
        let s: String = std::iter::repeat_n(line, 85).map(|l| format!("{l}\n")).collect();
        let out = try_compact(&s).unwrap();
        assert!(out.contains("[tokenade:strace-T]"));
        assert!(out.contains("(+5 more)"));
    }
}
