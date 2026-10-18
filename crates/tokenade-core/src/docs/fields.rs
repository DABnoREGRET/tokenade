//! **Word's `HYPERLINK` field**, which `.doc` and `.rtf` both
//! write: `HYPERLINK "https://…"` for an address, `HYPERLINK \l "bookmark"` or
//! `HYPERLINK "#bookmark"` for a reference INSIDE the document.
//!
//! An internal reference has no address to follow: `.docx` and `.odt`
//! render it as plain text, as `.xlsx` does its workbook links. The `.doc`
//! produced `[the intro](intro)` and the `.rtf` `[the intro](#intro)` — two
//! targets the document never named as addresses (2026-10-04).

/// The address of a `HYPERLINK …` instruction, or `None` for an internal reference
/// (or an instruction without a target).
pub(super) fn hyperlink_url(instr: &str) -> Option<String> {
    let rest = instr.trim_start().strip_prefix("HYPERLINK")?.trim_start();
    // The quoted target must come FIRST: `\l "bookmark"` starts
    // with the option, and so gives no address.
    let rest = rest.strip_prefix('"')?;
    let url = rest[..rest.find('"')?].trim();
    (!url.is_empty() && !url.starts_with('#')).then(|| url.to_string())
}

/// A field whose result COPIES the document: the table of contents
/// (`TOC`) carries each heading a second time, followed by its page number —
/// dropped in `.docx`, `.odt` and `.doc`, served by `.rtf` alone
/// (`TOC-STALE Chapter One ..... 1`, 2026-10-04).
pub(super) fn is_a_copy_of_the_text(instr: &str) -> bool {
    instr.trim_start().starts_with("TOC")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_address_is_a_link() {
        assert_eq!(
            hyperlink_url(r#" HYPERLINK "https://example.com/a?b=1" "#).as_deref(),
            Some("https://example.com/a?b=1")
        );
        assert_eq!(
            hyperlink_url(r#"HYPERLINK "mailto:ann@example.com""#).as_deref(),
            Some("mailto:ann@example.com")
        );
        assert_eq!(hyperlink_url(r#"HYPERLINK \l "intro""#), None);
        assert_eq!(hyperlink_url(r##"HYPERLINK "#intro""##), None);
        assert_eq!(hyperlink_url(r#"HYPERLINK """#), None);
        assert_eq!(hyperlink_url("PAGEREF intro"), None);
        assert!(
            is_a_copy_of_the_text(r#" TOC \z \o "1-3""#)
                && !is_a_copy_of_the_text("HYPERLINK \"https://e.x/\"")
        );
    }
}
