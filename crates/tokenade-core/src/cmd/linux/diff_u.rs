//! `diff_u` — diff u.
//!
//! Form-aware: keeps this command's specific detector, but folds its
//! output by detected shape — columnar table (surfaces problem rows),
//! key/value records (never cut mid-record), else head+tail (the final
//! outcome line is never decapitated). Replaces the blind head-slice.
const HEAD: usize = 50;

pub fn detect(text: &str) -> bool {
    text.lines().any(|l| l.starts_with('+') || l.starts_with('-') || l.starts_with("@@"))
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
    let folded = crate::archetypes::form_aware(text, HEAD, 5, "diff-u", "line(s)")?;
    Some(crate::diff::compact_if_unified(text).unwrap_or(folded))
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detector_rejects_empty() { assert!(!detect("")); }

    #[test]
    fn detector_accepts_specific_format() { assert!(detect("@@ -1,3 +1,4 @@")); }

    #[test]
    fn short_output_passes_through() {
        let s: String = std::iter::repeat_n("@@ -1,3 +1,4 @@", 3).map(|l| format!("{l}\n")).collect();
        assert!(try_compact(&s).is_none());
    }

    /// The archetype branch, and it needs a fixture the UNIFIED branch does
    /// not claim: bare `-` lines with no `@@` header are what `diff -u`
    /// prints for a file the other side does not have, and they carry no
    /// hunk for the unified parser to read.
    #[test]
    fn long_output_folds_non_decapitating() {
        let s: String = std::iter::repeat_n("-removed line", HEAD + 20).map(|l| format!("{l}\n")).collect();
        let out = try_compact(&s).unwrap();
        assert!(out.contains("[tokenade:diff-u"), "banner missing:\n{out}");
    }

    /// The unified branch. A real hunk's changed lines are the only thing the
    /// output carries, and the archetype keeps the first `HEAD` and the last
    /// 5 of them: measured on a 1 610-line `diff -u`, 27 of 810 changes came
    /// back under a banner naming the size of the INPUT. Both halves are
    /// asserted, because keeping every change is only worth stating while the
    /// answer is still smaller than the diff.
    #[test]
    fn a_unified_diff_keeps_every_change() {
        let mut s = String::from("--- a/inventory.txt\n+++ b/inventory.txt\n@@ -1,400 +1,400 @@\n");
        let mut changed = 0usize;
        for i in 0..400 {
            if i % 3 == 0 {
                s.push_str(&format!("-host-{i:04} state=running\n+host-{i:04} state=draining\n"));
                changed += 2;
            } else {
                s.push_str(&format!(" host-{i:04} state=running\n"));
            }
        }
        let out = try_compact(&s).unwrap();
        let kept = out
            .lines()
            .filter(|l| {
                (l.starts_with('-') || l.starts_with('+'))
                    && !l.starts_with("--") && !l.starts_with("++")
            })
            .count();
        assert_eq!(kept, changed, "changes dropped: {kept} of {changed}\n{out}");
        assert!(out.len() < s.len(), "no saving: {} vs {}", out.len(), s.len());
    }
}
