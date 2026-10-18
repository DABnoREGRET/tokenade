//! `diff_r` — diff r.
//!
//! Form-aware: keeps this command's specific detector, but folds its
//! output by detected shape — columnar table (surfaces problem rows),
//! key/value records (never cut mid-record), else head+tail (the final
//! outcome line is never decapitated). Replaces the blind head-slice.
const HEAD: usize = 100;

pub fn detect(text: &str) -> bool {
    text.lines().any(|l| l.starts_with(['+', '-', '<', '>']))
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    // The archetype decides WHETHER this output is worth folding at all — a
    // short diff still passes through untouched. Only once it has said yes
    // does the shape matter: a unified diff goes to the parser that keeps
    // every change, because the archetype keeps the first N lines and the
    // last 5, and a diff's changed lines are all it carries. See
    // `crate::diff::compact_if_unified`.
    let folded = crate::archetypes::form_aware(text, HEAD, 5, "diff-r", "diff row(s)")?;
    Some(crate::diff::compact_if_unified(text).unwrap_or(folded))
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detector_rejects_empty() { assert!(!detect("")); }

    #[test]
    fn detector_accepts_specific_format() { assert!(detect("+added line")); }

    #[test]
    fn short_output_passes_through() {
        let s: String = std::iter::repeat_n("+added line", 3).map(|l| format!("{l}\n")).collect();
        assert!(try_compact(&s).is_none());
    }

    #[test]
    fn long_output_folds_non_decapitating() {
        let s: String = std::iter::repeat_n("+added line", HEAD + 20).map(|l| format!("{l}\n")).collect();
        let out = try_compact(&s).unwrap();
        assert!(out.contains("[tokenade:diff-r"));
    }
}
