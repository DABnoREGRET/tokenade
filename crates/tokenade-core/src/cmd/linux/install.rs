//! `install -v` — copy files into place, reporting each one.
//!
//! With `-v` GNU install prints one line per file in the form
//! `'src' -> 'dst'` (and, with `-d`, `creating directory 'x'`). Bulk
//! installs from a Makefile run to dozens or hundreds of these. The
//! signal is *what landed where* plus a sample; the noise is the long
//! homogeneous run.
//!
//! The `->` arrow is the locale-INVARIANT marker — install does not
//! translate it — so detection keys on the arrow, never on any word. We
//! fold long runs of arrow lines into a count, keeping head and tail
//! samples so the first and last installed paths survive.


const FOLD_THRESHOLD: usize = 20;
const KEEP_HEAD: usize = 8;
const KEEP_TAIL: usize = 4;

/// An install report line: `'src' -> 'dst'`. The spaced arrow is what
/// install emits verbatim in every locale.
fn is_copy_line(line: &str) -> bool {
    line.contains(" -> ")
}

pub fn detect(text: &str) -> bool {
    text.lines().filter(|l| is_copy_line(l)).count() >= 3
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.iter().filter(|l| is_copy_line(l)).count();
    if total <= FOLD_THRESHOLD {
        return Some(crate::archetypes::passthrough(text));
    }

    Some(crate::archetypes::fold_runs(
        text,
        &lines,
        is_copy_line,
        FOLD_THRESHOLD,
        KEEP_HEAD,
        KEEP_TAIL,
        "more file(s) installed",
    ))
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use super::*;

    #[test]
    fn detector_rejects_empty_and_plain() {
        assert!(!detect(""));
        assert!(!detect("nothing to do here\n"));
    }

    #[test]
    fn detector_accepts_copy_lines() {
        assert!(detect("'a' -> '/usr/bin/a'\n'b' -> '/usr/bin/b'\n'c' -> '/usr/bin/c'\n"));
    }

    #[test]
    fn folds_bulk_install() {
        let mut t = String::new();
        for i in 0..50 {
            let _ = writeln!(t, "'build/bin/tool_{i:02}' -> '/usr/local/bin/tool_{i:02}'");
        }
        let out = try_compact(&t).unwrap();
        assert!(out.contains("tool_00"), "head bound kept");
        assert!(out.contains("tool_49"), "tail bound kept");
        assert!(out.contains("more file(s) installed"));
        assert!(!out.contains("tool_25"), "middle folded");
        assert!(out.len() < t.len(), "must shrink");
    }

    #[test]
    fn short_install_passes_through() {
        // Detected (>=3 copy lines) but under the fold threshold.
        let t = "'a' -> '/usr/bin/a'\n'b' -> '/usr/bin/b'\n'c' -> '/usr/bin/c'\n";
        let out = try_compact(t).unwrap();
        assert!(out.contains("'a' -> '/usr/bin/a'"));
        assert!(!out.contains("installed) ..."));
    }
}
