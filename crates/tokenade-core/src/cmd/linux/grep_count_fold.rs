//! `grep_c` — grep c.
//!
//! Form-aware: keeps this command's specific detector, but folds its
//! output by detected shape — columnar table (surfaces problem rows),
//! key/value records (never cut mid-record), else head+tail (the final
//! outcome line is never decapitated). Replaces the blind head-slice.
const HEAD: usize = 80;

pub fn detect(text: &str) -> bool {
    text.contains(':')
}

/// `grep -rc` prints a line for EVERY file it read, `path:0` included.
/// Measured 2026-10-02 on `grep -rci stash crates`: 2 011 lines, **1 802 of
/// them `:0`** — the files that do not match. The answer is the other 209.
/// The zeros are dropped and counted; what remains keeps its order and its
/// counts, and folds only if it is still long.
pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let zero = |l: &str| l.rsplit_once(':').is_some_and(|(_, n)| n.trim() == "0");
    let zeros = lines.iter().filter(|l| zero(l)).count();
    if zeros == 0 {
        return crate::archetypes::form_aware(text, HEAD, 5, "grep-c", "match count(s)");
    }
    let kept: Vec<&str> = lines.iter().filter(|l| !zero(l)).copied().collect();
    let mut out = format!(
        "[tokenade:grep-c] {} file(s) with matches; {zeros} file(s) counted 0, omitted\n",
        kept.len()
    );
    let body = kept.join("\n");
    match crate::archetypes::form_aware(&body, HEAD, 5, "grep-c", "match count(s)") {
        Some(folded) if !tk_base::delivery::fits(&body) => out.push_str(&folded),
        _ => out.push_str(&body),
    }
    Some(out)
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detector_rejects_empty() { assert!(!detect("")); }

    #[test]
    fn detector_accepts_specific_format() { assert!(detect("myfile.txt:42")); }

    #[test]
    fn short_output_passes_through() {
        let s: String = std::iter::repeat_n("myfile.txt:42", 3).map(|l| format!("{l}\n")).collect();
        assert!(try_compact(&s).is_none());
    }

    #[test]
    fn long_output_folds_non_decapitating() {
        let s: String = std::iter::repeat_n("myfile.txt:42", HEAD + 20).map(|l| format!("{l}\n")).collect();
        let out = try_compact(&s).unwrap();
        assert!(out.contains("[tokenade:grep-c"));
    }

    /// `grep -rci stash crates` : 1 802 lignes `:0` sur 2 011.
    #[test]
    fn zero_counts_are_dropped_and_counted() {
        let mut s = String::new();
        for i in 0..300 {
            s.push_str(&format!("src/f{i}.rs:{}\n", if i % 10 == 0 { i / 10 + 1 } else { 0 }));
        }
        let out = try_compact(&s).unwrap();
        assert!(out.contains("30 file(s) with matches; 270 file(s) counted 0, omitted"), "{out}");
        assert!(out.contains("src/f290.rs:30") && !out.contains("src/f1.rs:0"));
    }
}
