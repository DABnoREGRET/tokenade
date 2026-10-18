//! L77 — Compact multi-segment Bash outputs.
//!
//! Agentic workflows commonly chain multiple commands behind echo
//! headers to scope the work:
//!
//! ```sh
//! echo "=== smells ===" && grep -rn 'unwrap()' crates/ &&
//!   echo "=== tests ===" && cargo test --workspace
//! ```
//!
//! The combined stdout has heterogeneous sections separated by
//! `^=== ... ===$` (or `--- ... ---`, or `### ...`) markers. Before
//! this module the output landed under `auto::Kind::Unknown` →
//! passthrough — 90 records in the gain ledger sat at 300-3000
//! tokens with 0 % savings.
//!
//! The chain compactor splits on header lines, runs `auto::detect`
//! and `dispatch_kind` per section body, and reassembles with the
//! headers preserved. Each section gets the best per-format
//! compactor (logs to fold, JSON to schema, etc.).

use crate::auto;

/// Maximum body lines we try to split into. Beyond this we bail —
/// a 10 000-line concatenated output is more likely a malformed
/// blob than a real command chain.
const MAX_SECTIONS: usize = 100;

/// Minimum sections we require to consider this a chain. Two
/// headers is enough to confirm the pattern; one might just be a
/// label in a single-format file.
const MIN_SECTIONS: usize = 2;

/// Detect the chained-section pattern. Returns `true` when ≥
/// [`MIN_SECTIONS`] header lines are present AND not adjacent
/// (some body between them).
pub fn detect(text: &str) -> bool {
    let mut header_indices: Vec<usize> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if is_section_header(line) {
            header_indices.push(i);
            if header_indices.len() > MAX_SECTIONS {
                return false;
            }
        }
    }
    if header_indices.len() < MIN_SECTIONS {
        return false;
    }
    // At least one PAIR of headers must have body between them; a
    // pure-header file (rare but possible) isn't a chain we can
    // meaningfully compact.
    header_indices
        .windows(2)
        .any(|w| w[1].saturating_sub(w[0]) >= 2)
}

/// True for `=== X ===`, `--- X ---`, `### X` (markdown-ish), or
/// `--X-- end` style cli banners. Stripped from the test surface so
/// the detection rule stays explicit + easy to extend.
fn is_section_header(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return false;
    }
    (t.starts_with("===") && t.ends_with("===") && t.len() > 6 && names_a_section(t, '='))
        || (t.starts_with("---") && t.ends_with("---") && t.len() > 6 && names_a_section(t, '-'))
        || ((t.starts_with("###") || t.starts_with("##")) && t.len() > 3 && {
            let rest = t.trim_start_matches('#').trim();
            !rest.is_empty() && rest.chars().all(|c| !c.is_control())
        })
}

/// A section header NAMES its section. A bare horizontal rule does not.
///
/// `t.starts_with("---") && t.ends_with("---") && t.len() > 6` is satisfied by
/// `-----------------------------------------------------------`, which is the
/// rule every SIL Open Font License draws under its title. Measured 2026-08-26
/// on 10 276 `.log`, `.txt` and `.out` files: `CommandChain` claimed 116 and
/// folded NOTHING on 107 of them — font licences (`FiraSans-LICENSE`,
/// `UNIFONT_LICENSE`, `JETBRAINS_MONO_LICENSE`, `SourceCodePro-LICENSE`) and
/// pytest `test-stdout.txt` dumps.
///
/// The discriminator is what the delimiters are wrapped AROUND: strip the
/// marker runs off both ends and something has to be left that a reader could
/// call a name.
fn names_a_section(t: &str, marker: char) -> bool {
    let inner = t.trim_matches(marker).trim();
    !inner.is_empty() && inner.chars().any(|c| c.is_alphanumeric())
}

/// Compact a command-chain output: split on section headers, run
/// `auto::dispatch` per body, reassemble.
pub fn compact(text: &str) -> String {
    if !detect(text) {
        return text.to_string();
    }
    let lines: Vec<&str> = text.lines().collect();
    // Walk and group: when we hit a header, flush the prior body,
    // then accumulate the new section.
    let mut sections: Vec<(Option<String>, Vec<String>)> = Vec::new();
    let mut current_header: Option<String> = None;
    let mut current_body: Vec<String> = Vec::new();
    for line in lines {
        if is_section_header(line) {
            if current_header.is_some() || !current_body.is_empty() {
                sections.push((current_header.take(), std::mem::take(&mut current_body)));
            }
            current_header = Some(line.to_string());
        } else {
            current_body.push(line.to_string());
        }
    }
    if current_header.is_some() || !current_body.is_empty() {
        sections.push((current_header.take(), std::mem::take(&mut current_body)));
    }
    let mut out = String::with_capacity(text.len());
    for (i, (header, body)) in sections.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if let Some(h) = header {
            out.push_str(h);
            out.push('\n');
        }
        let body_text = body.join("\n");
        if body_text.trim().is_empty() {
            // Empty section — keep the header, drop the trailing blank.
            continue;
        }
        // Per-section compaction: re-run auto::detect on this slice
        // and dispatch. We guard against infinite recursion by NOT
        // re-routing to CommandChain itself (the detector checks
        // for ≥2 headers; one section has 0).
        let (_kind, compacted, _sav) = auto::compact(&body_text);
        out.push_str(&compacted);
    }
    out
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    #[test]
    fn detect_two_equals_sections() {
        let t = "=== A ===\nfoo\n=== B ===\nbar\n";
        assert!(detect(t));
    }

    #[test]
    fn detect_dash_sections() {
        let t = "--- A ---\nfoo\n--- B ---\nbar\n";
        assert!(detect(t));
    }

    #[test]
    fn detect_markdown_h3_sections() {
        let t = "### A\nfoo\n### B\nbar\n";
        assert!(detect(t));
    }

    #[test]
    fn rejects_single_section() {
        let t = "=== A ===\nfoo\nbar\nbaz\n";
        assert!(!detect(t));
    }

    #[test]
    fn rejects_no_sections() {
        let t = "just some plain output\nwith no markers\n";
        assert!(!detect(t));
    }

    #[test]
    fn rejects_adjacent_headers() {
        let t = "=== A ===\n=== B ===\n";
        assert!(!detect(t));
    }

    #[test]
    fn compact_preserves_headers() {
        let t = "=== smells ===\nfoo bar\nbaz\n=== tests ===\ntest result: ok\n";
        let out = compact(t);
        assert!(out.contains("=== smells ==="));
        assert!(out.contains("=== tests ==="));
    }

    #[test]
    fn compact_routes_log_section_through_logs_compactor() {
        let mut t = String::from("=== logs ===\n");
        for i in 0..40 {
            let _ = writeln!(t, "2024-01-01T00:00:{:02}Z INFO repeated event", i % 60);
        }
        t.push_str("=== done ===\nok\n");
        let out = compact(&t);
        // The logs section folds via Option-C: a `[×N lines` marker
        // should appear in the compacted body.
        assert!(out.contains("[×"), "logs should fold: {out}");
        // Section headers remain.
        assert!(out.contains("=== logs ==="));
        assert!(out.contains("=== done ==="));
    }

    #[test]
    fn passthrough_when_not_a_chain() {
        let t = "some plain output";
        assert_eq!(compact(t), t);
    }
}

#[cfg(test)]
mod a_rule_is_not_a_header {
    use super::*;

    /// The line every SIL Open Font License draws under its title.
    const FONT_LICENSE_RULE: &str = "-----------------------------------------------------------";

    #[test]
    fn a_bare_horizontal_rule_is_refused() {
        for l in [
            FONT_LICENSE_RULE,
            "==========================================",
            "--- ---",
            "===   ===",
            "-------",
        ] {
            assert!(
                !is_section_header(l),
                "a rule names no section: {:?}",
                &l[..l.len().min(20)]
            );
        }
    }

    #[test]
    fn a_header_that_names_something_still_counts() {
        for l in [
            "=== locales ===",
            "--- git status ---",
            "===== raw suite output: base.log =====",
            "=============================== warnings summary ===============================",
            "### Section 2",
            "--- 3 ---",
        ] {
            assert!(is_section_header(l), "refused: {l}");
        }
    }

    /// A font licence is a DOCUMENT. It was claimed as a command chain on two
    /// horizontal rules, and its compactor returned it byte for byte — measured
    /// 2026-08-26, 107 of the 116 files `CommandChain` claimed across 10 276
    /// `.log`/`.txt`/`.out` folded nothing at all.
    #[test]
    fn a_licence_is_not_a_chain() {
        let mut doc = String::from("Copyright 2020 The JetBrains Mono Project Authors\n\n");
        doc.push_str(FONT_LICENSE_RULE);
        doc.push_str("\nSIL OPEN FONT LICENSE Version 1.1\n");
        doc.push_str(FONT_LICENSE_RULE);
        doc.push('\n');
        for i in 0..40 {
            doc.push_str(&format!(
                "clause {i}: permission is hereby granted, free of charge, to any person\n"
            ));
        }
        assert!(!detect(&doc), "two rules made it a chain");
    }

    /// And a real chain still is one — the rule must not cost the format it
    /// exists for.
    #[test]
    fn a_real_chain_survives() {
        let mut chain = String::from("=== git status ===\n");
        for i in 0..5 {
            chain.push_str(&format!(" M src/file_{i}.rs\n"));
        }
        chain.push_str("=== cargo build ===\n");
        for i in 0..5 {
            chain.push_str(&format!("   Compiling crate_{i} v0.1.0\n"));
        }
        chain.push_str("=== done ===\nok\n");
        assert!(
            detect(&chain),
            "a named three-section chain must still detect"
        );
    }
}
