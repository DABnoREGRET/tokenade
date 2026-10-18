//! `pip list` / `pip freeze` / `pip show`.
//!
//! ```text
//! Package                   Version
//! ------------------------- -------
//! aiohttp                   3.9.5
//! anyio                     4.3.0
//! ...
//! ```
//!
//! Compaction: fold long `pip list` to a count + first 30.


pub fn detect(text: &str) -> bool {
    let mut lines = text.lines();
    let lower0 = lines.next().map(|l| l.to_ascii_lowercase()).unwrap_or_default();
    let header_ok = lower0.starts_with("package") && lower0.contains("version");
    let separator_ok = lines.next().is_some_and(|l| l.starts_with("--") || l.starts_with("==="));
    header_ok && separator_ok
}

pub fn try_compact(text: &str) -> Option<String> {
    if let Some(o) = fold_install_progress(text) {
        return Some(o);
    }
    if !detect(text) {
        return None;
    }
    Some(compact(text))
}

/// Lines pip prints while RESOLVING, one per package.
fn is_install_progress(t: &str) -> bool {
    let t = t.trim_start();
    t.starts_with("Collecting ")
        || t.starts_with("Downloading ")
        || t.starts_with("Using cached ")
        || t.starts_with("Requirement already satisfied")
}

/// Fold `pip install`'s per-package chatter, keeping everything else verbatim.
///
/// `pip install -r requirements.txt` prints a `Collecting …` line — often a
/// `Downloading …` too — for every package in the file. On a real project
/// that is hundreds of distinct lines, so nothing folds them: they differ by
/// name and version, and pip's own summary already says how many were
/// installed. The lines that matter, a resolver conflict and what caused it,
/// sit underneath and were the smallest part of what came back.
///
/// Everything that is not progress survives byte-for-byte, so an `ERROR:` and
/// the `The conflict is caused by:` block that explains it are untouched.
fn fold_install_progress(text: &str) -> Option<String> {
    const MIN_LINES: usize = 20;
    let mut kept: Vec<&str> = Vec::new();
    let mut folded = 0usize;
    for line in text.lines() {
        if is_install_progress(line) {
            folded += 1;
        } else {
            kept.push(line);
        }
    }
    if folded < MIN_LINES {
        return None;
    }
    let mut out = format!("[tokenade:pip] {folded} package resolution line(s) folded\n");
    out.push_str(kept.join("\n").trim_end());
    Some(out)
}

/// The table's padding is most of its bytes: `aiohappyeyeballs` then 25
/// blanks before `2.7.1`. Measured 2026-10-03, a real `pip list` of 372
/// packages: 17.9 KB padded did not fit, and the stage downstream sampled
/// every other run — 202 packages gone. Respaced, every package fits.
pub fn compact(text: &str) -> String {
    crate::tabular::whole_column_table(text)
        .unwrap_or_else(|| crate::archetypes::head_lines_with_tally(text, 30, "package(s) elided"))
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use std::fmt::Write;

    #[test]
    fn install_progress_folds_but_the_conflict_survives() {
        let mut input: String = (0..500)
            .map(|i| format!("Collecting pkg{i}==1.0.{i}\n  Downloading pkg{i}-1.0.{i}.whl (12 kB)\n"))
            .collect();
        input.push_str("ERROR: Cannot install pkg1 and pkg2 because these have conflicting dependencies.\n");
        input.push_str("The conflict is caused by:\n    pkg1 depends on urllib3<2\n");
        let out = super::try_compact(&input).expect("folded");
        assert!(out.contains("ERROR: Cannot install"), "the error survives:\n{out}");
        assert!(out.contains("pkg1 depends on urllib3<2"), "and its cause:\n{out}");
        assert!(!out.contains("Collecting pkg250"), "the chatter folds:\n{out}");
        assert!(out.contains("resolution line(s) folded"), "the fold is declared");
        assert!(out.len() * 5 < input.len(), "and it is a real fold: {} vs {}", out.len(), input.len());
    }

    #[test]
    fn a_short_install_is_left_alone() {
        let input = "Collecting requests\n  Downloading requests-2.31.0.whl\nSuccessfully installed requests-2.31.0\n";
        assert!(super::fold_install_progress(input).is_none());
    }
    use super::*;

    #[test]
    fn detects_pip_list() {
        let s = "Package                   Version\n------------------------- -------\naiohttp                   3.9.5\n";
        assert!(detect(s));
    }

    #[test]
    fn folds_long_list() {
        // Past the delivery threshold; under it the list is delivered whole.
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        let mut s = String::from("Package         Version\n--------------- -------\n");
        for i in 0..60 {
            let _ = writeln!(s, "pkg-{i:<10}  1.0.{i}");
        }
        let out = compact(&s);
        assert!(out.contains("more package"));
    }

    /// A real `pip list` is mostly padding: respaced, all of it fits.
    #[test]
    fn a_padded_list_is_respaced_and_delivered_whole() {
        let _g = tk_base::delivery::threshold_on_this_thread(5_000);
        let mut s = String::from(
            "Package                                  Version\n\
             ---------------------------------------- -------------------------------\n",
        );
        for i in 0..372 {
            let _ = writeln!(s, "{:<40} 1.{i}.0", format!("package-number-{i}"));
        }
        let out = super::compact(&s);
        assert!(out.contains("\npackage-number-0  1.0.0\n"), "{out}");
        assert!(out.trim_end().ends_with("package-number-371  1.371.0"), "{out}");
        assert!(!out.contains("more package"), "{out}");
    }
}
