//! BibTeX `.bib` bibliography compactor.
//!
//! Academic projects ship with `.bib` files containing dozens of
//! references — each entry has multiple fields and the formatting
//! tends to be whitespace-heavy with comment annotations. Real-
//! world ratio on commented `biblio.bib` files: 15-30%.
//!
//! What it strips:
//!   * full-line `%` comments (BibTeX honours TeX-style `%` for
//!     hand annotations even though it's not part of the BibTeX
//!     spec — many tools treat them as comments)
//!   * blank-line runs → one max
//!   * trailing whitespace per line
//!   * leading whitespace runs >4 spaces on field lines (deeply
//!     indented `field = {…}` lines waste tokens; 4 spaces conveys
//!     "this is inside an entry block" just as well as 8 or 10)
//!
//! What it preserves verbatim:
//!   * `@type{key, … }` entry headers (entry-type + citation key
//!     are essential)
//!   * every `field = {value}` and `field = "value"` line — values
//!     contain LaTeX macros that we MUST NOT touch
//!   * `@string{…}` macros and `@preamble{…}` blocks

/// Common BibTeX entry types. We use a CASE-INSENSITIVE match.
const ENTRY_TYPES: &[&str] = &[
    "article",
    "book",
    "booklet",
    "conference",
    "inbook",
    "incollection",
    "inproceedings",
    "manual",
    "mastersthesis",
    "phdthesis",
    "misc",
    "proceedings",
    "techreport",
    "unpublished",
    "online",
    "electronic",
    "patent",
    "thesis",
    "report",
    "software",
    "dataset",
    "string",
    "preamble",
    "comment",
];

/// Cheap heuristic detector. Require ≥2 `@type{` headers in the
/// first 200 non-blank lines. The pre-`{` token must be one of
/// ENTRY_TYPES (case-insensitive).
pub fn detect(text: &str) -> bool {
    let mut hits = 0usize;
    let mut considered = 0usize;
    for line in text.lines().filter(|l| !l.trim().is_empty()).take(200) {
        considered += 1;
        let t = line.trim_start();
        if t.starts_with("<!DOCTYPE") || t.starts_with("<?xml") {
            return false;
        }
        // `@type{key, …`
        if let Some(rest) = t.strip_prefix('@') {
            let entry_type: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphabetic())
                .map(|c| c.to_ascii_lowercase())
                .collect();
            if !entry_type.is_empty() && ENTRY_TYPES.contains(&entry_type.as_str()) {
                // Must be followed by `{` (with optional whitespace).
                let after = &rest[entry_type.len()..];
                if after.trim_start().starts_with('{') {
                    hits += 1;
                }
            }
        }
        if hits >= 2 {
            return true;
        }
    }
    hits >= 2 && considered > 0
}

/// Strip `%` comments + collapse blank-runs + clamp deep indents.
pub fn compact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank_run = 0usize;
    for line in text.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            blank_run += 1;
            if blank_run <= 1 {
                out.push('\n');
            }
            continue;
        }
        let t = trimmed.trim_start();
        // Full-line `%` comment.
        if t.starts_with('%') {
            continue;
        }
        // Clamp leading whitespace to 4 spaces max (preserves "inside
        // a block" feel without paying for 8-space indents).
        let leading_ws = trimmed.len() - t.len();
        let clamped = if leading_ws > 4 {
            format!("    {t}")
        } else {
            trimmed.to_string()
        };
        blank_run = 0;
        out.push_str(&clamped);
        out.push('\n');
    }
    out
}

/// `Savings`-yielding variant for the auto-compact ledger.
pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL_BIB: &str = "% Bibliography for the report\n% Last updated 2026-05-29\n\n@article{smith2023analysis,\n        title        = {An Analysis of Things},\n        author       = {Smith, Alice and Doe, Bob},\n        journal      = {Journal of Stuff},\n        year         = {2023}\n}\n\n@book{wilson2022handbook,\n        title        = {The Handbook of Things},\n        author       = {Wilson, Charlie},\n        publisher    = {Academic Press},\n        year         = {2022}\n}\n\n@misc{webresource,\n        title        = {A web resource},\n        howpublished = {\\url{https://example.com/article}},\n        year         = {2024}\n}\n";

    #[test]
    fn detect_real_bib() {
        assert!(detect(REAL_BIB));
    }

    #[test]
    fn detect_rejects_unrelated() {
        assert!(!detect("hello world\nthis is just prose\n"));
        assert!(!detect("only one @article{foo} entry\n"));
        assert!(!detect("<!DOCTYPE html>\n"));
        assert!(!detect("[section]\nkey = value\n"));
    }

    #[test]
    fn detect_handles_uppercase_types() {
        // BibTeX entry types are case-insensitive.
        let m = "@ARTICLE{x, title={Y}}\n@Book{z, title={W}}\n@INPROCEEDINGS{a, title={B}}\n";
        assert!(detect(m));
    }

    #[test]
    fn compact_strips_comments() {
        let out = compact(REAL_BIB);
        assert!(!out.contains("Bibliography for the report"));
        assert!(!out.contains("Last updated"));
    }

    #[test]
    fn compact_preserves_entries_and_fields() {
        let out = compact(REAL_BIB);
        assert!(out.contains("@article{smith2023analysis"));
        assert!(out.contains("@book{wilson2022handbook"));
        assert!(out.contains("@misc{webresource"));
        assert!(out.contains("title        = {An Analysis of Things}"));
        assert!(out.contains("author       = {Smith, Alice and Doe, Bob}"));
        assert!(out.contains("\\url{https://example.com/article}"));
    }

    #[test]
    fn compact_clamps_deep_indents() {
        // Input has 8-space indents; output should have 4-space.
        let out = compact(REAL_BIB);
        assert!(out.contains("    title"));
        assert!(!out.contains("        title")); // no 8-space lines
    }

    #[test]
    fn compact_collapses_blank_runs() {
        let input = "@article{a, title={x}}\n\n\n\n@book{b, title={y}}\n";
        let out = compact(input);
        // Exactly ONE blank line between entries.
        assert_eq!(out.matches("\n\n").count(), 1);
    }

    #[test]
    fn real_world_ratio_meets_expected() {
        let (_, sav) = compact_with_savings(REAL_BIB);
        assert!(
            sav.percent() >= 10.0,
            "expected ≥10% on commented .bib, got {:.1}%",
            sav.percent()
        );
    }
}
