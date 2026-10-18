//! **Marking an image without pretending to have read it.**
//!
//! An image in a document is a PLACE, not a text. The mark says
//! there is something there, and its replacement text when it exists —
//! asserting more would be fabricating.

use super::*;

/// **An image's replacement text is content, and the
/// DOCUMENTS half threw it away.**
///
/// The web route renders `[image: <text>]`; the documents route
/// rendered nothing at all, while each of the three schemas carries that text —
/// it is the word the author typed to describe their figure, and in a report
/// it is often the chart caption that carries the number.
///
/// **Three schemas, three spellings, and all three are written** (a
/// list of ENCOUNTERED names is not the specification):
///
/// * OOXML puts it in a `descr` ATTRIBUTE on `docPr`, under several prefixes depending
///   on whether it describes a Word drawing (`wp:docPr`) or a PowerPoint shape
///   (`p:cNvPr`, `pic:cNvPr`);
/// * OpenDocument makes it a CHILD of the frame, `<svg:desc>`, with `<svg:title>`
///   for the short title;
/// * an EPUB's XHTML calls it `alt`.
///
/// Nothing is emitted for an image WITHOUT replacement text: a bare mark
/// on every decorative bullet of a report would cost tokens to say
/// « there was something », and the web route already paid the price of a mark that
/// asserts without proof.
pub(super) fn mark_images(xml: &str, schema: Schema) -> String {
    match schema {
        Schema::Ooxml => ooxml_image_marks(xml),
        Schema::Odf => odf_image_marks(xml),
        Schema::Html => re(r#"(?s)<img\b[^>]*?\balt="([^"]*)"[^>]*>"#)
            .replace_all(xml, |c: &regex::Captures| one_mark(&[&c[1]]))
            .into_owned(),
    }
}

/// The mark of ONE image, from what the document declares about it.
///
/// EMPTY texts do not count — the web route paid the price of a mark that
/// asserts without proof, and a decorative bullet has nothing to say. A repeated text
/// counts only once: Word writes the same word in the title and in the
/// description when the author filled in only one.
/// A producer with no description to give writes the image's FILE in its place:
/// LibreOffice's EPUB export sets `alt="OEBPS/images/image0001.png"`, which
/// came out as `[image: OEBPS/images/image0001.png]` — an archive path that
/// describes nothing (the same reproach as `## OEBPS/part1.xhtml`). One
/// token, a `/` or an image extension: no text a person writes.
fn is_a_file_name(t: &str) -> bool {
    const EXT: [&str; 9] = [
        ".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".bmp", ".tif", ".tiff",
    ];
    let lower = t.to_ascii_lowercase();
    !t.contains(char::is_whitespace) && (t.contains('/') || EXT.iter().any(|e| lower.ends_with(e)))
}

pub(super) fn one_mark(texts: &[&str]) -> String {
    let mut kept: Vec<&str> = Vec::new();
    for t in texts {
        let t = t.trim();
        if !t.is_empty() && !is_a_file_name(t) && !kept.contains(&t) {
            kept.push(t);
        }
    }
    if kept.is_empty() {
        return String::new();
    }
    format!(
        " {} ",
        crate::markdown::image_marker(Some(&kept.join(" — ")))
    )
}

/// **One image, one mark — OOXML side.**
///
/// Word describes ONE figure TWICE: on the drawing's `wp:docPr`, then on the
/// `pic:cNvPr` of the image it contains. Both carry exactly the same
/// `descr` and `title`, so the per-tag rule rendered `[image: X]  [image: X]`
/// for a single figure — two images where the document shows one.
/// The drawing is the unit: inside a `<w:drawing>`, only the FIRST description
/// is served, and what is inside it is part of it. Outside a drawing
/// (a PowerPoint shape, a spreadsheet drawing), the tag remains the unit — it
/// alone describes its shape.
///
/// And the `title` is read, not just the `descr`: it is the short name the
/// author gives their figure, and the OpenDocument of the same source already
/// rendered it (`svg:title`), so two spellings of the same document did not say
/// the same thing.
pub(super) fn ooxml_image_marks(xml: &str) -> String {
    let tag = re(r"(?s)<(?:[A-Za-z0-9]+:)?(?:docPr|cNvPr)\b([^>]*)>");
    let marked = tag.replace_all(xml, |c: &regex::Captures| {
        let mark = one_mark(&[&xml_attr(&c[1], "title"), &xml_attr(&c[1], "descr")]);
        // A tag that describes nothing stays what it is: removing it would
        // gain nothing and would make this function a stripper.
        if mark.is_empty() {
            c[0].to_string()
        } else {
            mark
        }
    });
    // The drawing is the unit. The next identical mark is the second
    // writing of the same fact.
    re(r"(?s)<w:drawing\b.*?</w:drawing>")
        .replace_all(&marked, |c: &regex::Captures| {
            let mut seen: Vec<String> = Vec::new();
            re(r"\[image: [^\]]*\]")
                .replace_all(&c[0], |m: &regex::Captures| {
                    let found = m[0].to_string();
                    if seen.contains(&found) {
                        String::new()
                    } else {
                        seen.push(found.clone());
                        found
                    }
                })
                .into_owned()
        })
        .into_owned()
}

/// **One image, one mark — OpenDocument side.**
///
/// The short title and the long description are two CHILDREN of the same frame, and
/// the per-element rule made two images of them. They are written next to each
/// other, in the order the specification sets (`svg:title` then
/// `svg:desc`); the reverse order is accepted too, because a producer is not
/// bound to respect it and a list of encountered shapes is not the
/// specification.
pub(super) fn odf_image_marks(xml: &str) -> String {
    let title = r"<svg:title>(.*?)</svg:title>";
    let desc = r"<svg:desc>(.*?)</svg:desc>";
    let mut out = std::borrow::Cow::Borrowed(xml);
    for (pat, swap) in [
        (format!(r"(?s){title}\s*{desc}"), false),
        (format!(r"(?s){desc}\s*{title}"), true),
    ] {
        out = std::borrow::Cow::Owned(
            re(&pat)
                .replace_all(&out, |c: &regex::Captures| {
                    let (a, b) = (&c[1], &c[2]);
                    if swap {
                        one_mark(&[b, a])
                    } else {
                        one_mark(&[a, b])
                    }
                })
                .into_owned(),
        );
    }
    // A frame that declares only one of the two.
    for pat in [title, desc] {
        out = std::borrow::Cow::Owned(
            re(&format!("(?s){pat}"))
                .replace_all(&out, |c: &regex::Captures| one_mark(&[&c[1]]))
                .into_owned(),
        );
    }
    out.into_owned()
}

/// The value of an attribute in the RAW text of a tag, without reparsing the
/// document. Returns an empty string when the attribute is absent.
pub(super) fn xml_attr(attrs: &str, name: &str) -> String {
    re(&format!(r#"(?s)\b{name}="([^"]*)""#))
        .captures(attrs)
        .map(|c| c[1].to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod a_file_name_is_no_description {
    use super::one_mark;

    #[test]
    fn a_path_or_file_name_gives_no_mark_and_a_sentence_still_does() {
        assert_eq!(one_mark(&["OEBPS/images/image0001.png"]), "");
        assert_eq!(one_mark(&["image1.JPG"]), "");
        assert!(one_mark(&["Revenue 2024/2025 per quarter"]).contains("2024/2025"));
        assert!(one_mark(&["Revenue chart"]).contains("Revenue chart"));
    }
}
