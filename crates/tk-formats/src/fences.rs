//! Lines that are the document's own syntax, not something it QUOTES.
//!
//! A fenced code block in a markdown document is a quotation. Reading its
//! contents as the document's syntax is a class this repository has now paid
//! for three times:
//!
//! * Five markdown documents show a unified diff inside a fence; hoisting
//!   the mid-body `diff --git` marker would have rebuilt them as patches. Closed
//!   by anchoring the marker instead.
//! * `import { Tabs } from "…"` counted as an HCL `import` block, and the
//!   HCL compactor deleted every `#` heading. Closed by requiring an HCL block
//!   header to END on its brace; that trigger was NOT inside a fence, so fences
//!   would not have helped there.
//! * A 96 KB implementation plan in markdown quotes a `Cargo.toml`, so
//!   `[package]`, `[dependencies]` and `[dev-dependencies]` read as three INI
//!   sections. `Kind::Ini` won and its compactor, which strips `#` comments,
//!   deleted **all 14 headings**. That one IS a fence, and this module is its
//!   fix.
//!
//! Deliberately a helper the vulnerable detectors opt INTO rather than a filter
//! applied to every detector: a file that genuinely IS TOML or INI and happens
//! to contain a fence should keep being read as what it is. Measured over the
//! corpus, 12 327 files carry two markdown headings and a fence, and 3 of them
//! are `.toml` — small, but not zero, and the opt-in keeps them safe.

/// CommonMark fence opener: at least three backticks **or** tildes.
///
/// One function because the elider (`markdown::fenced_code_at`) used to
/// see only ``` while `outside_fences`, `unpad_tables` and
/// `fold_gate::fenced_ranges` already saw `~~~`. A tilde block longer
/// than the markdown cap was then delivered whole.
pub fn fence_marker(line: &str) -> Option<&str> {
    let t = line.trim_start();
    if t.starts_with("```") {
        Some("```")
    } else if t.starts_with("~~~") {
        Some("~~~")
    } else {
        None
    }
}

/// Iterate the lines of `text` that sit OUTSIDE fenced code blocks.
///
/// Recognises both fence markers CommonMark allows, ``` and `~~~`, and treats
/// an unclosed fence as running to the end of the document — which is what a
/// renderer does. The fence lines themselves are not yielded.
pub fn outside_fences(text: &str) -> impl Iterator<Item = &str> {
    let mut fence: Option<char> = None;
    text.lines().filter(move |line| {
        let marker = fence_marker(line).map(|m| m.as_bytes()[0] as char);
        match (fence, marker) {
            // Opening a fence: the marker line itself is not content.
            (None, Some(c)) => {
                fence = Some(c);
                false
            }
            // Only the SAME marker closes; a ``` inside a ~~~ block is content.
            (Some(open), Some(c)) if open == c => {
                fence = None;
                false
            }
            (Some(_), _) => false,
            (None, None) => true,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::outside_fences;

    #[test]
    fn a_quoted_config_is_not_the_documents_syntax() {
        let doc = "# Plan\n\nThe manifest looks like this:\n\n```toml\n[package]\nname = \"x\"\n\n[dependencies]\nserde = \"1\"\n```\n\n## Next\n\nDone.\n";
        let kept: Vec<&str> = outside_fences(doc).collect();
        assert!(!kept.iter().any(|l| l.starts_with('[')), "{kept:?}");
        assert!(kept.contains(&"# Plan"));
        assert!(kept.contains(&"## Next"));
    }

    #[test]
    fn both_commonmark_openers_are_one_function() {
        assert_eq!(super::fence_marker("```rust"), Some("```"));
        assert_eq!(super::fence_marker("  ~~~"), Some("~~~"));
        assert_eq!(super::fence_marker("# heading"), None);
        assert_eq!(super::fence_marker("``not three"), None);
    }

    #[test]
    fn a_tilde_fence_closes_only_on_a_tilde() {
        let doc = "outside\n~~~\n```\nstill inside\n~~~\nafter\n";
        let kept: Vec<&str> = outside_fences(doc).collect();
        assert_eq!(kept, vec!["outside", "after"]);
    }

    #[test]
    fn an_unclosed_fence_runs_to_the_end() {
        let doc = "before\n```\n[package]\nname = \"x\"\n";
        let kept: Vec<&str> = outside_fences(doc).collect();
        assert_eq!(kept, vec!["before"]);
    }

    #[test]
    fn a_document_with_no_fence_is_unchanged() {
        let doc = "[core]\nkey = 1\n[user]\nname = z\n";
        let kept: Vec<&str> = outside_fences(doc).collect();
        assert_eq!(kept.len(), 4);
    }
}
