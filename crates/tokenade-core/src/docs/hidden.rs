//! **What the document DOES NOT SHOW.**
//!
//! A word processor keeps many things it does not paint: a
//! hidden slide, a deleted but tracked passage, a hidden text. Delivering
//! them is delivering what the author decided to remove — sometimes an
//! earlier version they did not want to publish.

use super::*;

/// Convert an Office-XML fragment to text, rendering its TABLES as markdown
/// instead of letting the tag-stripper flatten them.
///
/// Every format here embeds tables, and the generic stripper has no notion of a
/// cell: a 4x4 table came out as 16 bare lines with no row/column association,
/// so "how many errors for EMEA?" became unanswerable from the extract — the
/// data was all present and all useless. Text outside tables still goes through
/// the normal pipeline, and document order is preserved (a table lands where it
/// sits in the prose, not appended at the end).
///
/// Splitting on table blocks and converting each side separately (rather than
/// splicing markdown into the XML and stripping afterwards) keeps cell text that
/// contains `<` from being eaten as a tag.
/// Drop elements whose CONTENT is markup machinery, not text.
///
/// Nothing here strips tags — that happens downstream. This removes whole
/// elements whose body would otherwise be read as prose: a stylesheet or a
/// script is not something a reader sees, and an agent asked to summarise a book
/// should never meet `@page { margin-bottom: 5.000000pt; }`. EPUBs inline their
/// CSS, so this fired on every one of the 9 real books measured (64 lines of
/// stylesheet served as text). The token cost is small; the nonsense is not.
/// One regex per element: Rust's `regex` crate has NO backreferences, so
/// `<(style|script)...</\1>` does not compile — it panics at build time of the
/// pattern, and `catch_unwind` upstream turns that into an empty document. Which
/// is exactly what happened: every EPUB extracted to nothing.
/// **What a document STORES but does not SHOW.**
///
/// Two forms, and neither was known here:
///
/// * `<w:instrText>` carries the INSTRUCTION of a field, not its result. An
///   automatic table of contents stores `TOC \o "1-3" \h \z \u` there, a link
///   `HYPERLINK "https://…" \o "tooltip"`, a mail merge
///   `MERGEFIELD Name`. Since the tag stripper takes every text node,
///   these incantations arrived in the middle of the prose. The RESULT lives in an
///   ordinary `<w:t>` right next to it and is still served: nothing is lost.
/// * a run marked `<w:vanish/>` is text that Word DOES NOT DISPLAY. Serving
///   it is asserting that the document says something no human
///   reader sees — and it is the injection surface the PDF path already names
///   in so many words for render mode 3: « we hand PDF text to
///   agents, so anything hidden there is a prompt-injection surface ». The same
///   reasoning, the other format.
///
/// On the OpenDocument side, the equivalent of the second is a section declared
/// `text:display="none"`, or a passage (`text:span`) whose text style
/// declares it — that is how LibreOffice writes a character's « hidden », and the
/// `.odt` of a `.docx` with a hidden run served what the `.docx` kept silent
/// (2026-10-04). There is no equivalent of the first: ODF stores its fields as
/// elements whose text content IS what is displayed.
pub(super) fn without_what_the_document_does_not_show(xml: &str, schema: Schema) -> String {
    match schema {
        Schema::Ooxml => {
            // Nothing in their place, as for a tracked deletion: the runs
            // around carry their own spaces (`See page  7` otherwise).
            let no_fields = re(r"(?s)<w:instrText\b[^>]*>.*?</w:instrText>")
                .replace_all(xml, "")
                .into_owned();
            without_hidden_shapes(&without_hidden_runs(&no_fields))
        }
        Schema::Odf => without_hidden_spans(&without_elements(
            &re(r#"(?s)<text:section\b[^>]*text:display="none"[^>]*>.*?</text:section>"#)
                .replace_all(xml, " "),
            // A hidden drawing shape: `draw:display="none"` (ODF), which
            // LibreOffice writes `drawooo:display="none"`.
            |_, attrs| {
                attrs.contains(r#"draw:display="none""#)
                    || attrs.contains(r#"drawooo:display="none""#)
            },
            |_, _| true,
        )),
        Schema::Html => without_unpainted_html(xml),
    }
}

/// The OpenDocument passages whose text style says `text:display="none"`,
/// removed whole.
fn without_hidden_spans(xml: &str) -> String {
    let hidden: std::collections::HashSet<String> =
        re(r#"(?s)<style:style\b[^>]*\bstyle:name="([^"]+)"[^>]*>\s*<style:text-properties\b[^>]*\btext:display="none""#)
            .captures_iter(xml)
            .map(|c| c[1].to_string())
            .collect();
    if hidden.is_empty() {
        return xml.to_string();
    }
    let style = re(r#"\btext:style-name="([^"]+)""#);
    without_elements(
        xml,
        |name, attrs| {
            name == "text:span"
                && style
                    .captures(attrs)
                    .is_some_and(|c| hidden.contains(&c[1]))
        },
        |_, _| true,
    )
}

/// A hidden shape (« Selection » pane): never painted, neither in the
/// slide show nor on the page. The mark is on the shape's OWN description,
/// the first of its content (a group's is in its
/// `nvGrpSpPr`, before its children):
/// * PowerPoint — `<p:cNvPr … hidden="1">` of a `p:sp`, `p:pic`,
///   `p:graphicFrame`, `p:grpSp` or `p:cxnSp`;
/// * Word — `<wp:docPr … hidden="1">` of a `w:drawing`: the hidden text box
///   came out in the middle of the paragraph that anchors it (2026-10-04).
const HIDDEN_SHAPE_MARKS: [(&[&str], &str); 2] = [
    (
        &["p:sp", "p:pic", "p:graphicFrame", "p:grpSp", "p:cxnSp"],
        "p:cNvPr",
    ),
    (&["w:drawing"], "wp:docPr"),
];

fn without_hidden_shapes(xml: &str) -> String {
    if !xml.contains("hidden=") {
        return xml.to_string();
    }
    let hidden = re(r#"\bhidden="(?:1|true)""#);
    let mut out = xml.to_string();
    for (names, mark) in HIDDEN_SHAPE_MARKS {
        let own = re(&format!(r"<{mark}\b[^>]*>"));
        out = without_elements(
            &out,
            |name, _| names.contains(&name),
            |_, inner| own.find(inner).is_some_and(|m| hidden.is_match(m.as_str())),
        );
    }
    out
}

/// A book's XHTML that the reader does not paint: `hidden` attribute, inline
/// style `display:none` — the web path's `is_display_none_markup` rule,
/// on the same attributes. The web, for its part, keeps a page's `hidden` tab
/// (content a click reveals, measured on IKEA); a book has no click, and
/// that never-read text is an injection surface there, like the PDF's
/// render mode 3.
fn without_unpainted_html(xml: &str) -> String {
    if !xml.contains("hidden") && !xml.contains("display") {
        return xml.to_string();
    }
    let attr = |attrs: &str, name: &str| {
        re(r#"(?i)\s([a-z-]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+)))?"#)
            .captures_iter(attrs)
            .find(|c| c[1].eq_ignore_ascii_case(name))
            .map(|c| {
                c.get(2)
                    .or(c.get(3))
                    .or(c.get(4))
                    .map_or(String::new(), |m| m.as_str().to_string())
            })
    };
    without_elements(
        xml,
        |_, attrs| {
            crate::web_tree::is_display_none_markup(
                attr(attrs, "class").as_deref(),
                attr(attrs, "style").as_deref(),
                attr(attrs, "hidden").is_some(),
            )
        },
        |_, _| true,
    )
}

/// The elements of a class that the book's style sheet hides
/// (`epub::css::hidden_classes`).
pub(super) fn without_hidden_classes(
    xml: &str,
    classes: &std::collections::HashSet<String>,
) -> String {
    if classes.is_empty() {
        return xml.to_string();
    }
    let class = re(r#"\bclass\s*=\s*(?:"([^"]*)"|'([^']*)')"#);
    without_elements(
        xml,
        |_, attrs| {
            class
                .captures(attrs)
                .and_then(|c| c.get(1).or(c.get(2)))
                .is_some_and(|v| v.as_str().split_whitespace().any(|w| classes.contains(w)))
        },
        |_, _| true,
    )
}

/// Removes each element that its opening tag designates (`candidate`, on its
/// name and attributes) and that its content confirms (`unseen`, on its
/// attributes and content — a PowerPoint shape's `cNvPr` is INSIDE),
/// content included, by a SCAN that counts the openings and closings of
/// its name: a non-greedy rule would stop at the closing of an inner element
/// of the same name and let the rest leak. A self-closing tag
/// (`<x …/>`) opens nothing. A kept candidate is walked: its children
/// may be removed, themselves.
fn without_elements(
    xml: &str,
    candidate: impl Fn(&str, &str) -> bool,
    unseen: impl Fn(&str, &str) -> bool,
) -> String {
    let start_tag = re(r"<([A-Za-z][\w:.-]*)(\s[^>]*)?>");
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(m) = start_tag.captures(rest) {
        let Some(whole) = m.get(0) else { break };
        let name = &m[1];
        let attrs = m.get(2).map_or("", |a| a.as_str());
        let keep = |out: &mut String| out.push_str(&rest[..whole.end()]);
        if !candidate(name, attrs) {
            keep(&mut out);
            rest = &rest[whole.end()..];
            continue;
        }
        if whole.as_str().ends_with("/>") {
            if !unseen(attrs, "") {
                keep(&mut out);
            } else {
                out.push_str(&rest[..whole.start()]);
            }
            rest = &rest[whole.end()..];
            continue;
        }
        let (open, close) = (format!("<{name}"), format!("</{name}>"));
        let mut depth = 1usize;
        let mut at = whole.end();
        let end = loop {
            let Some(c) = rest[at..].find(&close).map(|i| at + i) else {
                break rest.len();
            };
            // An opening of the same NAME (not `<text:spanx`), not self-closing.
            let o = rest[at..c]
                .match_indices(&open)
                .map(|(i, _)| at + i)
                .find(|&o| {
                    rest[o + open.len()..].starts_with(|ch: char| ch == '>' || ch.is_whitespace())
                });
            match o {
                Some(o) => {
                    let tag_end = rest[o..].find('>').map_or(rest.len(), |i| o + i + 1);
                    if !rest[..tag_end].ends_with("/>") {
                        depth += 1;
                    }
                    at = tag_end;
                }
                None => {
                    depth -= 1;
                    at = c + close.len();
                    if depth == 0 {
                        break at;
                    }
                }
            }
        };
        let inner = rest.get(whole.end()..end).unwrap_or("");
        if unseen(attrs, inner) {
            out.push_str(&rest[..whole.start()]);
            rest = &rest[end..];
        } else {
            keep(&mut out);
            rest = &rest[whole.end()..];
        }
    }
    out.push_str(rest);
    out
}

/// Word runs marked invisible, removed whole.
///
/// Written as a SCAN and not as a regular expression, otherwise it would need
/// a negative lookahead — « a `<w:r>` that contains `<w:vanish/>` BEFORE its
/// `</w:r>` » — which this repo's engine does not have. A non-greedy rule without
/// it would start from the first `<w:r>` found and stop at the first `</w:r>`,
/// so it would judge the wrong run: it is the same class of trap as the
/// self-closing marks of tracked deletions, and it is settled here by the bound of the
/// scan rather than by the order of the passes.
pub(super) fn without_hidden_runs(xml: &str) -> String {
    if !xml.contains("<w:vanish") {
        return xml.to_string();
    }
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(start) = rest.find("<w:r ").or_else(|| rest.find("<w:r>")) {
        let (before, from_run) = rest.split_at(start);
        out.push_str(before);
        let Some(end) = from_run.find("</w:r>") else {
            // A run never closed: nothing to decide, return the rest as is.
            out.push_str(from_run);
            return out;
        };
        let run = &from_run[..end + "</w:r>".len()];
        // The mark only counts in the run's PROPERTIES: `<w:vanish/>`
        // elsewhere in the body would be text talking about the mark.
        let hidden = run
            .find("</w:rPr>")
            .is_some_and(|p| run[..p].contains("<w:vanish"));
        if !hidden {
            out.push_str(run);
        }
        rest = &from_run[end + "</w:r>".len()..];
    }
    out.push_str(rest);
    out
}

/// **A document under REVISION carried its deleted text as if it still
/// stood.**
///
/// Nothing here knew `w:del`, `w:delText` or `text:tracked-changes` — zero
/// occurrences in this file before — and the tag stripper keeps
/// every text node. A draft contract therefore delivered its STRUCK-OUT
/// clauses in the middle of those that stand, with nothing to tell them apart.
/// That is serving what the source does not say, which this repo ranks
/// above loss.
///
/// INSERTIONS stay: `<w:ins>` surrounds text that IS today's
/// document. Only the removed side goes.
///
/// **Two passes on the Word side, and the order is the guard.** `<w:del>` exists in
/// two forms: the container `<w:del …>…</w:del>` that surrounds the deleted
/// runs, and the SELF-CLOSING mark `<w:del w:id="1"/>` set in a
/// `<w:rPr>` to say that the paragraph mark was deleted. A single
/// non-greedy rule starting from a self-closing mark would run to the
/// NEXT `</w:del>` and take away the real text between them. The self-closing
/// marks are therefore removed first, after which none remains
/// to prime the second rule.
///
/// `w:moveFrom` is the same gesture under another name — the origin side of a
/// move — and it is written here rather than discovered later.
///
/// On the OpenDocument side there is nothing to tell apart: the whole log lives in
/// `<text:tracked-changes>`, **at the head of the body**, so the deletions
/// arrived BEFORE the document. This block also carries the `<dc:creator>` of
/// each change, that is, people's names — the very reason
/// `word/comments.xml` was left out.
pub(super) fn without_tracked_deletions(xml: &str, schema: Schema) -> String {
    let (empties, blocks): (&[&str], &[&str]) = match schema {
        Schema::Ooxml => (
            &[r"(?s)<w:(?:del|moveFrom)\b[^>]*/>"],
            &[
                r"(?s)<w:del\b[^>]*>.*?</w:del>",
                r"(?s)<w:moveFrom\b[^>]*>.*?</w:moveFrom>",
                r"(?s)<w:delText\b[^>]*>.*?</w:delText>",
            ],
        ),
        Schema::Odf => (
            &[],
            &[r"(?s)<text:tracked-changes\b.*?</text:tracked-changes>"],
        ),
        Schema::Html => (&[], &[]),
    };
    let mut out = std::borrow::Cow::Borrowed(xml);
    for pat in empties.iter().chain(blocks.iter()) {
        if out.is_empty() {
            break;
        }
        // Nothing in its place: the text around a deletion already carries its
        // own spaces, and a space put there split a word whose letter was
        // struck (`colo<w:del>u</w:del>r` read `colo r`) or doubled the one
        // before a replaced figure (`is  750`).
        out = std::borrow::Cow::Owned(re(pat).replace_all(&out, "").into_owned());
    }
    out.into_owned()
}

#[cfg(test)]
mod every_hidden_shape_mark {
    use super::*;

    /// Each shape of the table, marked hidden by its own description, goes;
    /// unmarked, it stays.
    #[test]
    fn each_shape_goes_only_when_its_own_mark_says_hidden() {
        for (names, mark) in HIDDEN_SHAPE_MARKS {
            for name in names {
                let shape =
                    |h: &str| format!(r#"<{name}><x><{mark} id="1"{h}/></x><t>BODY</t></{name}>"#);
                let gone = without_hidden_shapes(&format!("a{}b", shape(r#" hidden="1""#)));
                assert_eq!(gone, "ab", "{name}");
                let kept = format!("a{}b", shape(""));
                assert_eq!(without_hidden_shapes(&kept), kept, "{name}");
            }
        }
    }
}
