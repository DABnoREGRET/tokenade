//! **An EPUB 3 note**: the call `<a epub:type="noteref" href="#n1">1</a>`
//! in the sentence, the note `<aside epub:type="footnote" id="n1">` further on,
//! with its back link (`<sup><a href="#appel">1</a></sup>`).
//!
//! Read as text, the note came out glued at the end of the chapter and the call
//! as a superscript (`Note here^1 end.` … `^1See the source.`), where the
//! `.docx` and the `.odt` of the same document write `Note here[^1] end.` and a
//! `## Footnotes` section (2026-10-04). Same shape here. DPUB-ARIA writes the
//! same thing as roles (`doc-noteref`, `doc-footnote`, `doc-endnote`).
//! Only a call that targets a note of the SAME file is linked: a note stored
//! in another chapter stays where the book puts it.

use super::super::markup_marks::re;
use super::super::{tables_aware, Schema};

/// The chapter without its notes, each call turned into `[^N]` (N continues
/// `next`), and the notes so called: (N, text).
pub(super) fn lift(xml: &str, next: &mut usize) -> (String, Vec<(usize, String)>) {
    let is_note = |attrs: &str| {
        re(r#"\b(?:epub:type|role)="[^"]*\b(?:doc-)?(?:foot|end|rear)?note\b[^"]*""#)
            .is_match(attrs)
            && !attrs.contains("noteref")
    };
    let id_of = |attrs: &str| {
        re(r#"\bid="([^"]+)""#)
            .captures(attrs)
            .map(|c| c[1].to_string())
    };
    let mut bodies: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let without = re(r"(?s)<(aside|li)\b([^>]*)>(.*?)</(?:aside|li)>").replace_all(
        xml,
        |c: &regex::Captures| match id_of(&c[2]).filter(|_| is_note(&c[2])) {
            Some(id) => {
                bodies.insert(id, c[3].to_string());
                String::new()
            }
            None => c[0].to_string(),
        },
    );
    if bodies.is_empty() {
        return (xml.to_string(), Vec::new());
    }
    let mut notes = Vec::new();
    let call = re(
        r#"(?s)(?:<sup\b[^>]*>\s*)?<a\b([^>]*\b(?:epub:type|role)="[^"]*noteref[^"]*"[^>]*)>.*?</a>(?:\s*</sup>)?"#,
    );
    let body = call.replace_all(&without, |c: &regex::Captures| {
        let target = re(r##"\bhref="[^"#]*#([^"]+)""##).captures(&c[1]).map(|m| m[1].to_string());
        match target.and_then(|t| bodies.get(&t)) {
            Some(note) => {
                *next += 1;
                // The back link to the call is not the text of the note.
                let note = re(r##"(?s)(?:<sup\b[^>]*>\s*)?<a\b[^>]*href="#[^"]*"[^>]*>[^<]{0,4}</a>(?:\s*</sup>)?"##).replace_all(note, "");
                let text = tables_aware(&note, Schema::Html).split_whitespace().collect::<Vec<_>>().join(" ");
                notes.push((*next, text));
                format!("[^{}]", *next)
            }
            None => c[0].to_string(),
        }
    });
    (body.into_owned(), notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_called_note_leaves_the_text_and_its_call_becomes_a_marker() {
        let xml = r##"<body><p>Note here<sup id="c1"><a epub:type="noteref" href="#n1">1</a></sup> end, <a role="doc-noteref" href="#n2">2</a>; elsewhere <a epub:type="noteref" href="notes.xhtml#x">3</a>.</p><ul><li>Item</li></ul><aside epub:type="footnote" id="n1"><p><sup><a href="#c1">1</a></sup>See <a href="https://e.x/">the source</a>.</p></aside><aside role="doc-endnote" id="n2"><p>Second.</p></aside></body>"##;
        let mut next = 4;
        let (body, notes) = lift(xml, &mut next);
        assert!(body.contains(r##"Note here[^5] end, [^6]; elsewhere <a epub:type="noteref" href="notes.xhtml#x">3</a>."##), "{body}");
        assert!(
            body.contains("<li>Item</li>") && !body.contains("aside"),
            "{body}"
        );
        assert_eq!(
            notes,
            [
                (5, "See [the source](https://e.x/).".to_string()),
                (6, "Second.".to_string())
            ]
        );
        assert_eq!(next, 6);
    }
}
