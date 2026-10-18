//! **Notes, and pagination.**
//!
//! A footnote is not in the flow of the text, and a
//! speaker note is not the slide. Rendering them in their place — rather
//! than in the middle of the sentence that calls them — is what makes the document
//! readable again.

use super::*;

/// Sequentially extract every entry whose name starts with `prefix` and ends
/// with `suffix`, rendering each through `schema` (so slide TABLES survive as
/// tables). Used for PPTX slide* files and similar collections.
/// **An OOXML document's notes live in parts the body does not
/// name, and we read only one of them.**
///
/// OpenDocument puts everything in a single `content.xml`, so a footnote
/// arrives there by accident. OOXML splits the document into parts and tokenade
/// read only one per format. MEASURED on 2026-09-11 on a pair built for
/// the occasion — same source, same producer, one conversion apart:
/// `notes.odt` returns its four needles, `notes.docx` returns **two out of four**,
/// the footnote and the endnote simply being absent.
///
/// **Deliberately narrow scope, and here is what is EXCLUDED and why:**
///
/// * `word/header*.xml` and `word/footer*.xml`: furniture, repeated on every page.
///   The PDF path already carries `dedup_running_heads` precisely because a repeated
///   running title is not content; adding it here would add what
///   the other half of the product spends work removing.
/// * `word/comments.xml`: review annotations, not the document's text,
///   and they carry their author's NAME — personal data that an
///   extractor must not serve without being asked to.
///
/// The separators are removed: Word stores in `footnotes.xml` two technical
/// notes (`separator`, `continuationSeparator`) that only carry the horizontal
/// rule drawn above the notes.
pub(super) fn ooxml_word_notes<R: std::io::Read + std::io::Seek>(
    zip: &mut ZipArchive<R>,
    calls: &NoteCalls,
) -> String {
    let block = re(r#"(?s)<w:(footnote|endnote)\b([^>]*)>(.*?)</w:(?:footnote|endnote)>"#);
    let id = re(r#"w:id="(-?\d+)""#);
    let mut out = String::new();
    for (part, label, endnote) in [
        ("word/footnotes.xml", NOTE_SECTIONS[0], false),
        ("word/endnotes.xml", NOTE_SECTIONS[1], true),
    ] {
        let xml = raw_xml_entry(zip, part);
        if xml.is_empty() {
            continue;
        }
        // A note's links resolve through ITS part's relationships
        // (`word/_rels/footnotes.xml.rels`), not the body's: without them
        // `See the source.` lost the address the `.odt` keeps (2026-10-04).
        let parts = DocParts {
            links: ooxml_link_targets(&raw_xml_entry(
                zip,
                &part
                    .replace("word/", "word/_rels/")
                    .replace(".xml", ".xml.rels"),
            )),
            ..Default::default()
        };
        // In CALL order, as the reader meets them — the part keeps them in
        // `w:id` order, which Word is free to leave unsorted. A note no call
        // reaches goes last, unlabelled.
        let mut notes: Vec<(usize, String)> = Vec::new();
        for c in block.captures_iter(&without_note_separators(&xml)) {
            let text = tables_aware_with(&c[3], Schema::Ooxml, &parts);
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            let n = id
                .captures(&c[2])
                .and_then(|i| calls.position(endnote, &i[1]));
            notes.push(match n {
                Some(n) => (n, format!("{}: {text}", NoteCalls::label_of(endnote, n))),
                None => (usize::MAX, text.to_string()),
            });
        }
        notes.sort_by_key(|(n, _)| *n);
        let texts: Vec<String> = notes.into_iter().map(|(_, t)| t).collect();
        if !texts.is_empty() {
            let _ = write!(out, "\n## {label}\n{}\n", texts.join("\n"));
        }
    }
    out
}

/// **A note says WHERE it is called.** Word does not write the call number:
/// `<w:footnoteReference w:id="2"/>` is an empty element that Word numbers at
/// display time. Without it, « 30 days. » and the notes section no longer
/// met — with ten notes, nothing said which one talks about what.
/// The number is Word's: the order of appearance, footnotes and endnotes
/// counted separately. Rendered as a Markdown call (`[^1]`, `[^e1]` for an
/// endnote): glued to the word as in the document, but readable as a call —
/// not the bare digit that welded onto the previous word.
#[derive(Default)]
pub(super) struct NoteCalls {
    foot: Vec<String>,
    end: Vec<String>,
}

impl NoteCalls {
    fn position(&self, endnote: bool, id: &str) -> Option<usize> {
        let list = if endnote { &self.end } else { &self.foot };
        list.iter().position(|x| x == id)
    }

    fn label_of(endnote: bool, n: usize) -> String {
        format!("[^{}{}]", if endnote { "e" } else { "" }, n + 1)
    }

    fn label(&self, endnote: bool, id: &str) -> Option<String> {
        self.position(endnote, id)
            .map(|n| Self::label_of(endnote, n))
    }
}

/// `document.xml` with each note call replaced by its label, and
/// the order of the calls to number the notes.
pub(super) fn ooxml_note_calls(doc_xml: &str) -> (String, NoteCalls) {
    let call = re(r#"<w:(footnote|endnote)Reference\b[^>]*?w:id="(-?\d+)"[^>]*/>"#);
    let mut calls = NoteCalls::default();
    let out = call.replace_all(doc_xml, |c: &regex::Captures| {
        let endnote = &c[1] == "endnote";
        let list = if endnote {
            &mut calls.end
        } else {
            &mut calls.foot
        };
        if !list.iter().any(|x| x == &c[2]) {
            list.push(c[2].to_string());
        }
        format!(
            "<w:t>{}</w:t>",
            calls.label(endnote, &c[2]).unwrap_or_default()
        )
    });
    (out.into_owned(), calls)
}

/// **The two sections under which a document stores its notes.**
///
/// Written ONCE because two readers emit them — Word through its parts
/// `footnotes.xml`/`endnotes.xml`, OpenDocument through the class each note
/// DECLARES (`text:note-class`) — and a document converted from one format to the other
/// must read the same. The `.docx` published the part's INTERNAL path
/// (`## Notes (word/footnotes.xml)`): a string the document does not contain,
/// which tells the reader there is a file to go fetch.
pub(super) const NOTE_SECTIONS: [&str; 2] = ["Footnotes", "Endnotes"];

/// **A footnote is not a word of the sentence that calls it.**
///
/// OpenDocument writes the WHOLE note at the place of its call:
///
/// ```text
/// …carries the argument.<text:note><text:note-citation>1</text:note-citation>
///                       <text:note-body><text:p>NEEDLE-NOTE-FOOT the footnote says this.</text:p></text:note-body></text:note>
/// ```
///
/// The generic stripper therefore rendered `argument.1NEEDLE-NOTE-FOOT the footnote
/// says this.` — a WORD the document contains nowhere, welded to the
/// last word of the sentence by the call number, and a note that cuts the body
/// in two with nothing saying where it resumes. Word has always stored its own in a
/// separate part; it is the same source, converted, that read
/// two ways.
///
/// **Scope: TEXT documents.** An OpenDocument slideshow or drawing goes
/// through `render_odf_pages`, which already splits into pages; a workbook stores its
/// remarks in `office:annotation`, which `strip_non_prose_elements` removes.
/// Widening without measuring it would move text on formats where
/// nobody has shown that it welds.
///
/// Returns the XML stripped of its notes, and the notes gathered by CLASS — the one
/// the note declares, not the one its style suggests. The call number
/// (`text:note-citation`) does not come out as is — it is what welded — but
/// as a Markdown call `[^1]` / `[^e1]`, repeated before the note: without it, nothing
/// linked a note to the sentence that calls it.
pub(super) fn odf_lift_notes(xml: &str) -> (String, String) {
    let note = re(r"(?s)<text:note(?:\s[^>]*)?>(.*?)</text:note>");
    let body = re(r"(?s)<text:note-body\b[^>]*>(.*?)</text:note-body>");
    let class = re(r#"text:note-class="([^"]*)""#);
    let mut stripped = String::with_capacity(xml.len());
    let mut collected: [Vec<String>; 2] = Default::default();
    let mut cursor = 0usize;
    for m in note.find_iter(xml) {
        stripped.push_str(&xml[cursor..m.start()]);
        cursor = m.end();
        let Some(inner) = body.captures(m.as_str()).map(|c| c[1].to_string()) else {
            continue;
        };
        let text = tables_aware(&inner, Schema::Odf);
        if text.trim().is_empty() {
            continue;
        }
        let endnote = class
            .captures(m.as_str())
            .is_some_and(|c| &c[1] == "endnote");
        // Same call as Word's (`ooxml_note_calls`): numbered per class in order.
        let list = &mut collected[usize::from(endnote)];
        let label = format!("[^{}{}]", if endnote { "e" } else { "" }, list.len() + 1);
        stripped.push_str(&label);
        list.push(format!("{label}: {}", text.trim()));
    }
    stripped.push_str(&xml[cursor..]);
    // An empty line before the first section: the rendered body is trimmed, so
    // without it the heading sticks to the last sentence — and a heading stuck to the
    // sentence before it is exactly what this fix removes elsewhere.
    let mut notes = String::new();
    for (label, texts) in NOTE_SECTIONS.iter().zip(&collected) {
        if texts.is_empty() {
            continue;
        }
        let _ = write!(
            notes,
            "{}## {label}\n{}\n",
            if notes.is_empty() { "\n\n" } else { "\n" },
            texts.join("\n")
        );
    }
    (stripped, notes)
}

/// The notes XML, stripped of the two technical notes Word stores there.
pub(super) fn without_note_separators(xml: &str) -> String {
    re(r#"(?s)<w:(?:foot|end)note[^>]*w:type="(?:separator|continuationSeparator)"[^>]*>.*?</w:(?:foot|end)note>"#)
        .replace_all(xml, "")
        .into_owned()
}

/// A slide's « speaker notes » part, if it has one.
///
/// Found through the slide's RELATIONSHIP, not by its number: `slide3.xml`
/// is not required to point to `notesSlide3.xml`, and assuming it does is the
/// kind of assumption that holds on every fixture and breaks on a real deck
/// where a slide lost its notes.
pub(super) fn speaker_notes_of<R: std::io::Read + std::io::Seek>(
    zip: &mut ZipArchive<R>,
    slide: &str,
) -> Option<String> {
    let (dir, file) = slide.rsplit_once('/')?;
    let rels = raw_xml_entry(zip, &format!("{dir}/_rels/{file}.rels"));
    let target = re(r#"Target="([^"]*notesSlide[^"]*)""#)
        .captures(&rels)
        .map(|c| c[1].to_string())?;
    // `../notesSlides/notesSlide1.xml` is relative to the slide's folder.
    let path = match target.strip_prefix("../") {
        Some(rest) => format!(
            "{}/{rest}",
            dir.rsplit_once('/').map(|(p, _)| p).unwrap_or("ppt")
        ),
        None => format!("{dir}/{target}"),
    };
    let text = tables_aware(&raw_xml_entry(zip, &path), Schema::Ooxml);
    (!text.trim().is_empty()).then_some(text)
}

/// Does the slide declare itself hidden?
///
/// `show="0"` on the `<p:sld>` root, and **only** on the root: the same
/// attribute lives on animation elements inside the slide, where
/// it says nothing about its visibility. Looking for the attribute anywhere would make
/// a perfectly visible slide silent — loss for fabrication,
/// the trade this repo refuses in both directions.
pub(super) fn slide_is_hidden(xml: &str) -> bool {
    let Some(at) = xml.find("<p:sld") else {
        return false;
    };
    let Some(end) = xml[at..].find('>') else {
        return false;
    };
    let root = &xml[at..at + end];
    root.contains(r#"show="0""#) || root.contains("show='0'")
}

/// The word the format uses for a page: a presentation has
/// slides, a drawing has pages. Rendering « Slide » on an architecture
/// diagram would tell the reader something the document does not say.
pub(super) fn page_word(fmt: Format) -> &'static str {
    match fmt {
        Format::Odg => "Page",
        _ => "Slide",
    }
}

/// The rendering of an OpenDocument presentation: one page per section, under its
/// number and its name, in the same form as the PowerPoint rendering.
///
/// **What is in NO page remains content.** A first version
/// emitted only the pages and made `Workshop enrolment code:
/// ZK-9187.` disappear, a `<text:p>` placed directly under `<office:presentation>`,
/// after the last page — the ratchet refused it on the real document of the
/// audit corpus. The walk therefore keeps the intervals between pages, and
/// the tail after the last one.
///
/// A HIDDEN page still consumes its number, as on the PowerPoint side: the
/// numbers must be those of the slideshow, not those of what we kept of it.
///
/// The name in parentheses only when the document declares one: an
/// empty parenthesis would say we read something that is not there.
pub(super) fn render_odf_pages(styles_from: &str, body: &str, label: &str) -> String {
    // List styles live in `office:automatic-styles`, OUTSIDE the
    // page: read on the fragment alone, an `a)` starting from 3 came out as a bullet
    // (`- First`, 2026-10-04). Read once over the whole document.
    let parts = DocParts {
        ordered: odf_ordered_lists(styles_from),
        numbering: odf_number_styles(styles_from),
        ..Default::default()
    };
    let tables_aware = |x: &str, schema: Schema| tables_aware_with(x, schema, &parts);
    let hidden = spreadsheet::hidden_page_styles(styles_from);
    let style_re = re(r#"(?s)<draw:page\b[^>]*\bdraw:style-name="([^"]*)""#);
    let name_re = re(r#"(?s)<draw:page\b[^>]*\bdraw:name="([^"]*)""#);
    let mut out = String::new();
    let mut cursor = 0usize;
    let mut number = 0usize;
    let mut hidden_numbers: Vec<String> = Vec::new();
    let keep_between = |out: &mut String, frag: &str| {
        let text = tables_aware(frag, Schema::Odf);
        if !text.trim().is_empty() {
            let _ = write!(out, "\n{text}\n");
        }
    };
    for m in re(r"(?s)<draw:page\b.*?</draw:page>").find_iter(body) {
        keep_between(&mut out, &body[cursor..m.start()]);
        cursor = m.end();
        number += 1;
        if style_re
            .captures(m.as_str())
            .and_then(|c| c.get(1))
            .is_some_and(|st| hidden.contains(st.as_str()))
        {
            hidden_numbers.push(number.to_string());
            continue;
        }
        // **Speaker notes kept apart, and labelled like PowerPoint's.**
        // `<presentation:notes>` lives INSIDE the page: rendered with
        // it, the note melted into the slide's text, followed by
        // `<number>` — the « page number » field of the notes layout, which
        // only has a value when printing (2026-10-03, same presentation as
        // `.pptx`: `### Notes` and nothing else). Of the notes page we only
        // keep the `presentation:class="notes"` frame.
        let page_re = re(r"(?s)<presentation:notes\b.*?</presentation:notes>");
        let notes: String = page_re
            .find_iter(m.as_str())
            .flat_map(|n| {
                re(r#"(?s)<draw:frame\b[^>]*\bpresentation:class="notes".*?</draw:frame>"#)
                    .find_iter(n.as_str())
                    .map(|f| tables_aware(f.as_str(), Schema::Odf))
                    .collect::<Vec<_>>()
            })
            .filter(|t| !t.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        let slide = page_re.replace_all(m.as_str(), "");
        let slide =
            re(r#"(?s)<draw:frame\b[^>]*\bpresentation:class="page-number".*?</draw:frame>"#)
                .replace_all(&slide, "");
        let mut text = tables_aware(&slide, Schema::Odf);
        if !notes.is_empty() {
            let _ = write!(text, "\n\n### Notes\n{notes}");
        }
        if text.trim().is_empty() {
            continue;
        }
        let name = name_re
            .captures(m.as_str())
            .and_then(|c| c.get(1))
            .map(|x| decode_entities(x.as_str()));
        let title = re(r#"(?s)<draw:frame\b[^>]*\bpresentation:class="title".*?</draw:frame>"#)
            .find(&slide)
            .map(|f| {
                tables_aware(f.as_str(), Schema::Odf)
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .filter(|t| !t.is_empty());
        out.push_str(&slide_section(label, number, name, title, &text));
    }
    keep_between(&mut out, &body[cursor..]);
    format!(
        "{}{}",
        collapse_whitespace(&out),
        spreadsheet::hidden_note(&label.to_lowercase(), hidden_numbers.len(), &hidden_numbers)
    )
}

/// The slide parts in the order the PRESENTATION declares them.
///
/// `<p:sldIdLst>` gives the order and cites relationships; `ppt/_rels/presentation.xml.rels`
/// links each relationship to its file. The number in the file name is
/// the CREATION order: moving a slide reorders the list and
/// renames nothing.
///
/// Returns an EMPTY list when the package declares nothing usable, so that
/// the caller resumes its walk of entries — same fallback as for workbooks.
pub(super) fn declared_slides(presentation_xml: &str, rels_xml: &str) -> Vec<String> {
    let targets: std::collections::HashMap<String, String> =
        re(r#"(?s)<Relationship\b[^>]*\bId="([^"]*)"[^>]*\bTarget="([^"]*)""#)
            .captures_iter(rels_xml)
            .filter_map(|c| {
                Some((
                    c.get(1)?.as_str().to_string(),
                    c.get(2)?.as_str().to_string(),
                ))
            })
            .collect();
    if targets.is_empty() {
        return Vec::new();
    }
    let Some(list) = re(r"(?s)<p:sldIdLst\b.*?</p:sldIdLst>").find(presentation_xml) else {
        return Vec::new();
    };
    re(r#"(?s)<p:sldId\b[^>]*\br:id="([^"]*)""#)
        .captures_iter(list.as_str())
        .filter_map(|c| {
            let target = targets.get(c.get(1)?.as_str())?;
            // Targets are relative to `ppt/`, and sometimes absolute.
            Some(match target.strip_prefix('/') {
                Some(abs) => abs.to_string(),
                None => format!("ppt/{}", target.trim_start_matches("./")),
            })
        })
        .collect()
}

/// The name a slide gives itself, when it gives itself one.
pub(super) fn slide_name(slide_xml: &str) -> Option<String> {
    let name = re(r#"(?s)<p:cSld\b[^>]*\bname="([^"]*)""#)
        .captures(slide_xml)?
        .get(1)
        .map(|m| decode_entities(m.as_str()))?;
    (!name.trim().is_empty()).then_some(name)
}

/// One slide's section, the same for PowerPoint and OpenDocument: number,
/// declared name, then the title, which leads the heading so a deck's outline
/// names its slides — and is then not said again as the body's first line. A
/// name that only repeats the title (LibreOffice names a page after it) or is
/// a generator's default (`page3`) says nothing and is left out.
pub(super) fn slide_section(
    label: &str,
    number: usize,
    name: Option<String>,
    title: Option<String>,
    text: &str,
) -> String {
    let mut head = format!("## {label} {number}");
    let defaulted = |n: &str| re(r"^(?i:page|slide)\s*\d+$").is_match(n.trim());
    if let Some(n) =
        name.filter(|n| !n.trim().is_empty() && !defaulted(n) && Some(n) != title.as_ref())
    {
        let _ = write!(head, " ({n})");
    }
    let mut text = text;
    if let Some(t) = title {
        let _ = write!(head, " — {t}");
        let first = text.trim_start().lines().next().unwrap_or("");
        if first.split_whitespace().collect::<Vec<_>>().join(" ") == t {
            text = text.trim_start()[first.len()..].trim_start_matches('\n');
        }
    }
    format!("\n{head}\n{text}\n")
}

/// The text of the slide's TITLE placeholder (`<p:ph type="title|ctrTitle">`),
/// its paragraphs joined by a space. The deck's outline is its slide titles:
/// left in the body, `## Slide 7` was all a reader navigating a long deck saw.
pub(super) fn slide_title(slide_xml: &str) -> Option<String> {
    let title = re(r"(?s)<p:sp\b.*?</p:sp>")
        .find_iter(slide_xml)
        .map(|m| m.as_str())
        .find(|sp| re(r#"<p:ph\b[^>]*\btype="(?:title|ctrTitle)""#).is_match(sp))?;
    let text = re(r"(?s)<a:p\b.*?</a:p>")
        .find_iter(title)
        .map(|p| {
            re(r"<a:t>([^<]*)</a:t>")
                .captures_iter(p.as_str())
                .map(|c| decode_entities(&c[1]))
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join(" ");
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!text.is_empty()).then_some(text)
}

pub(super) fn extract_glob<R: std::io::Read + std::io::Seek>(
    zip: &mut ZipArchive<R>,
    prefix: &str,
    suffix: &str,
    schema: Schema,
    declared: &[String],
) -> String {
    let mut names: Vec<String> = (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok().map(|f| f.name().to_string()))
        .filter(|n| n.starts_with(prefix) && n.ends_with(suffix))
        .collect();
    // Numeric, not lexicographic: slide10 must not sort before slide2 — a deck
    // read out of order is a subtly wrong document.
    names.sort_by_key(|n| (slide_number(n), n.clone()));
    // **And the FILE number is not the presentation's order.** The
    // parts are numbered in CREATION order; moving a slide
    // in PowerPoint reorders `<p:sldIdLst>` and renames nothing. Workbook
    // sheets paid for exactly this confusion. We keep
    // numeric sorting as a FALLBACK, for a package that declares nothing
    // usable: coming out in an approximate order is better than coming out
    // empty.
    if !declared.is_empty() {
        let known: std::collections::HashSet<&str> = names.iter().map(String::as_str).collect();
        let mut ordered: Vec<String> = declared
            .iter()
            .filter(|d| known.contains(d.as_str()))
            .cloned()
            .collect();
        // A part present in the package and absent from the declared list
        // still exists: we serve it afterwards, rather than losing it.
        ordered.extend(names.iter().filter(|n| !declared.contains(n)).cloned());
        names = ordered;
    }
    let mut out = String::new();
    let mut hidden: Vec<String> = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let raw = super::formulas::with_ooxml_formulas(&raw_xml_entry(zip, name), "a");
        // A HIDDEN slide is not shown during the presentation.
        // Serving it is presenting as the document what no viewer
        // sees — the same reasoning as for Word's invisible text, at the
        // scale of a slide. The rule is set HERE, in a generic function,
        // because it has only ONE caller: the PowerPoint rendering.
        if slide_is_hidden(&raw) {
            hidden.push((i + 1).to_string());
            continue;
        }
        let text = match schema {
            // A slide's links resolve through ITS relationships.
            Schema::Ooxml => {
                let rels = match name.rsplit_once('/') {
                    Some((dir, file)) => format!("{dir}/_rels/{file}.rels"),
                    None => format!("_rels/{name}.rels"),
                };
                let parts = DocParts {
                    links: ooxml_link_targets(&raw_xml_entry(zip, &rels)),
                    ..Default::default()
                };
                tables_aware_with(&mark_pptx_bullets(&raw), schema, &parts)
            }
            _ => tables_aware(&raw, schema),
        };
        if !text.trim().is_empty() {
            // The internal path (`ppt/slides/slide1.xml`) is a detail nobody
            // asked for — the same reproach once made for a workbook's
            // sheets. The slide carries its name in
            // `<p:cSld name="…">`, and often carries none.
            out.push_str(&slide_section(
                "Slide",
                i + 1,
                slide_name(&raw),
                slide_title(&raw),
                &text,
            ));
        }
        // Speaker notes live in ANOTHER part, which the
        // slide does not name itself: it is its relationship that names it.
        if let Some(notes) = speaker_notes_of(zip, name) {
            let _ = write!(out, "\n### Notes\n{notes}\n");
        }
    }
    format!(
        "{}{}",
        collapse_whitespace(&out),
        spreadsheet::hidden_note("slide", hidden.len(), &hidden)
    )
}
