//! Office document text extraction.
//!
//! Office Open XML (DOCX/XLSX/PPTX), OpenDocument (ODT/ODS/ODP) and EPUB are
//! all **ZIP containers full of XML**, so we unzip the parts that carry content
//! and turn them into text an agent can act on. PDF goes through a real parser.
//!
//! Supported (read-only):
//!   - DOCX → `word/document.xml`
//!   - XLSX → `xl/sharedStrings.xml` + `xl/worksheets/sheet*.xml`
//!   - PPTX → `ppt/slides/slide*.xml`
//!   - ODT/ODS/ODP → `content.xml`
//!   - EPUB → every XHTML chapter
//!   - PDF  → `pdf-extract`
//!
//! Two properties are load-bearing, and both were learned from corruption:
//!
//! 1. **Tables stay tables.** Stripping tags has no notion of a cell, so a
//!    spreadsheet row `[…, "257", "3"]` came out `2573` — two numbers fused into
//!    a third, unrecoverable. Where a format has tables we parse the cells and
//!    emit markdown; everything else takes the generic text path, in document
//!    order. See [`tables_aware`] / [`extract_xlsx`].
//! 2. **Containers are identified by CONTENT, not by the first zip entry.** Zip
//!    order is unspecified and producers differ; a LibreOffice .docx sniffed as
//!    a plain "zip" and its raw bytes got printed at the agent. See
//!    [`format_from_zip`].
//!
//! Each entry path that doesn't exist is gracefully skipped — a malformed or
//! foreign-layout file degrades to "no text extracted" rather than a panic.

mod epub;
mod fb2;
mod fields;
mod formulas;
mod spreadsheet;
use epub::*;
pub use spreadsheet::*;
mod detect;
mod pdf;
#[cfg(test)]
use zip_container::{format_from_zip, format_from_zip_seek};
mod outlook_msg;
mod ppt97;
mod rtf;
mod word97;
mod zip_container;
pub use detect::{detect_format, detect_format_at, Format, KNOWN_EXTENSIONS};

pub(crate) use rtf::extract_rtf;
pub(crate) use zip_container::{open_zip, raw_xml_entry};
mod tables;
use crate::tokenizer;
pub use pdf::*;
use regex::Regex;
use std::fmt::Write as _;
use std::io::Cursor;
use tables::*;
use zip::ZipArchive;

mod hidden;
mod images;
mod markup_marks;
mod notes_pages;
mod xml_text;
use hidden::*;
use images::*;
use markup_marks::*;
use notes_pages::*;
// `pub(crate)` no longer makes sense here: `rows_to_markdown` was the only item
// of `xml_text` that left the family, and it moved to `markdown`,
// its subject. The rest is `pub(super)` — the family and nobody else.
use xml_text::*;

/// Extract a document's text.
///
/// **This is the single place a document stops being bytes and becomes text an
/// agent can read, so it is where the credential scrub belongs.** Five call
/// sites reach here; on 2026-08-26 three redacted and two did not, and the two
/// that did not both served the text onward — one inlined it into a WebFetch
/// reason, the other built a spreadsheet skeleton out of it.
///
/// Per-caller redaction was the previous arrangement and it rotted exactly the
/// way a per-caller rule does: the coverage matrix of 2026-08-04 recorded nine
/// surfaces and zero leaks, and a tenth surface was added afterwards with no
/// scrub on it. A rule that must be remembered at every new call site is a rule
/// that will be forgotten at one of them.
///
/// `redact_for_model` is idempotent, so the three callers that already scrub
/// are unaffected and keep their own call as defence in depth.
pub fn extract(bytes: &[u8], fmt: Format) -> String {
    let text = align_list_children(sanitize_extracted(extract_inner(bytes, fmt)));
    crate::redact::redact_for_model(&text)
}

/// **A child sits under its parent's TEXT, not two spaces in.**
///
/// Every format nests a list two spaces per level. Under `- ` that is the
/// item's text column; under `1. ` it is not — the text starts at column 3,
/// and in CommonMark `  1. Linux binaries` is then not a child of
/// `1. Build the artefacts` but a new item of the same list: the nesting the
/// document draws is lost to any reader that parses it (2026-10-03, the same
/// release checklist read as fodt, odt, docx, rtf and epub). Each nested list
/// line moves to its parent's text column; a list whose parents are all `- `
/// comes out byte for byte as before.
fn align_list_children(text: String) -> String {
    fn marker_len(t: &str) -> Option<usize> {
        if t.starts_with("- ") || t.starts_with("* ") {
            return Some(2);
        }
        let d = t.bytes().take_while(u8::is_ascii_digit).count();
        (d > 0 && t[d..].starts_with(". ")).then_some(d + 2)
    }
    // Only a NUMBERED parent moves anything.
    if !text.lines().any(|l| {
        let t = l.trim_start();
        t.starts_with(|c: char| c.is_ascii_digit()) && marker_len(t).is_some()
    }) {
        return text;
    }
    // (indent in the source, text column in the output) per open list level.
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let mut in_fence = false;
    let mut out = String::with_capacity(text.len() + 64);
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        let t = body.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
        }
        let indent = body.len() - t.len();
        match marker_len(t).filter(|_| !in_fence) {
            Some(m) => {
                while stack.last().is_some_and(|&(i, _)| i >= indent) {
                    stack.pop();
                }
                let at = stack.last().map_or(indent, |&(_, col)| col);
                stack.push((indent, at + m));
                out.push_str(&" ".repeat(at));
                out.push_str(&line[indent..]);
            }
            None => {
                if !t.is_empty() && indent == 0 {
                    stack.clear();
                }
                out.push_str(line);
            }
        }
    }
    out
}

/// Drop control characters from extracted text. They are never document
/// content: they are what an extractor emits for a glyph it could not map —
/// subsetted or CID fonts, i.e. every non-Latin script.
///
/// Measured on `ar.pdf`: 28 of 261 bytes came back as control characters
/// (10.7%), over the 5% that `disclose::detect_binary` uses to call a blob
/// binary — so the whole extract was stashed as `kind=bin` and the agent was
/// shown nothing at all. The Arabic was already lost to glyph mangling, but
/// the ASCII in the same document was intact and recoverable: needle recall
/// went 0.50 → 0.00 purely because a few artefact bytes condemned the readable
/// text next to them. Stripping them restores it.
///
/// `\n`/`\r`/`\t` are real layout and stay. Format characters (Cf) such as the
/// RTL/LTR marks are NOT `is_control` and stay too — they are meaningful in
/// bidirectional text.
fn sanitize_extracted(text: String) -> String {
    if !text
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return text; // overwhelmingly the common case — do not reallocate
    }
    text.chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t'))
        .collect()
}

/// Extract human-readable text from an Office/EPUB document, given its raw
/// bytes and a known [`Format`]. Returns an empty string on a totally foreign
/// or empty archive.
fn extract_inner(bytes: &[u8], fmt: Format) -> String {
    // PDFs aren't ZIP containers — route them to the dedicated extractor.
    if matches!(fmt, Format::Pdf) {
        return extract_pdf(bytes);
    }
    // Legacy Excel is an OLE2/CFB container, not a zip: opening it as one below
    // fails and returns "", which is how a .xls used to look like an empty
    // document instead of a spreadsheet.
    if matches!(fmt, Format::Xls) {
        return extract_xls(bytes);
    }
    // These are not containers either: RTF is control words in plain text,
    // FictionBook and flat ODF are single XML files. Opening any of them as a
    // zip fails and yields "" — an empty document instead of its content.
    match fmt {
        Format::Rtf => return extract_rtf(bytes),
        Format::Doc => return word97::extract_doc(bytes).unwrap_or_default(),
        Format::Ppt => return ppt97::extract_ppt(bytes).unwrap_or_default(),
        Format::Msg => return outlook_msg::extract_msg(bytes).unwrap_or_default(),
        Format::Fb2 => return extract_fb2(&String::from_utf8_lossy(bytes)),
        Format::FlatOdf => return extract_flat_odf(bytes),
        _ => {}
    }
    let cursor = Cursor::new(bytes);
    let Some(mut zip) = open_zip(cursor) else {
        return String::new();
    };
    match fmt {
        Format::Docx => {
            let numbering = raw_xml_entry(&mut zip, "word/numbering.xml");
            let ordered = ooxml_ordered_lists(&numbering);
            let links =
                ooxml_link_targets(&raw_xml_entry(&mut zip, "word/_rels/document.xml.rels"));
            let doc_xml = with_style_numbering(
                &raw_xml_entry(&mut zip, "word/document.xml"),
                &raw_xml_entry(&mut zip, "word/styles.xml"),
            );
            let doc_xml = without_silent_numbering(&doc_xml, &ooxml_silent_lists(&numbering));
            let doc_xml = formulas::with_ooxml_formulas(&doc_xml, "w");
            let (doc_xml, calls) = ooxml_note_calls(&doc_xml);
            let body = tables_aware_with(
                &doc_xml,
                Schema::Ooxml,
                &DocParts {
                    ordered,
                    links,
                    numbering: ooxml_number_styles(&numbering),
                },
            );
            let notes = ooxml_word_notes(&mut zip, &calls);
            if notes.is_empty() {
                body
            } else {
                format!("{body}\n{notes}")
            }
        }
        Format::Xlsx => extract_xlsx(bytes),
        Format::Pptx => {
            let declared = declared_slides(
                &raw_xml_entry(&mut zip, "ppt/presentation.xml"),
                &raw_xml_entry(&mut zip, "ppt/_rels/presentation.xml.rels"),
            );
            extract_glob(
                &mut zip,
                "ppt/slides/slide",
                ".xml",
                Schema::Ooxml,
                &declared,
            )
        }
        // **A workbook renders as a workbook, whatever its
        // spelling.** The `.ods` went through the generic stripper, which
        // knows neither a sheet's name nor whether it is hidden: two
        // tables arrived glued together with nothing saying they come from
        // different sheets, and the sheet hidden by the author was served
        // like the rest. The hidden-sheet filter existed, with its tests and its
        // patch note — and `read` did not go through it.
        Format::Ods => {
            let xml = raw_xml_entry(&mut zip, "content.xml");
            format!(
                "{}{}",
                spreadsheet::render_workbook(spreadsheet::ods_tables(&xml, &xml)),
                spreadsheet::hidden_note(
                    "sheet",
                    spreadsheet::ods_hidden_sheet_count(&xml, &xml),
                    &[]
                )
            )
        }
        // A presentation renders page by page, and SILENCES those the
        // document hides — the same decision as for a sheet, and for
        // Word's invisible text, at the scale of a slide. The
        // PowerPoint path already did it; the same slideshow saved as
        // OpenDocument served the hidden page, without even saying where one page
        // ended and where the next began.
        // A drawing is made of PAGES, as a presentation is made of
        // slides: same walk, different word — that is what Draw
        // calls them. The `.odg` went through the generic stripper, so several
        // pages arrived glued together with nothing saying where one ends.
        Format::Odp | Format::Odg => {
            let xml = raw_xml_entry(&mut zip, "content.xml");
            let xml = formulas::with_odf_formulas(&xml, |name| odf_object(&mut zip, name));
            render_odf_pages(&xml, &xml, page_word(fmt))
        }
        Format::Odt => {
            // A NAMED list style (« Numbering 123 », or the `WWNum5` of a
            // converted `.docx`) lives in `styles.xml`, not in `content.xml`,
            // which only keeps the automatic styles: read without it, a numbered
            // list came out as bullets — the `.fodt`, which carries both in a
            // single file, numbered it.
            let content = raw_xml_entry(&mut zip, "content.xml");
            let content = formulas::with_odf_formulas(&content, |name| odf_object(&mut zip, name));
            let (body, notes) = odf_lift_notes(&content);
            let parts = DocParts {
                ordered: odf_ordered_lists(&raw_xml_entry(&mut zip, "styles.xml")),
                numbering: odf_number_styles(&raw_xml_entry(&mut zip, "styles.xml")),
                ..Default::default()
            };
            format!("{}{notes}", tables_aware_with(&body, Schema::Odf, &parts))
        }
        Format::Epub => extract_epub(&mut zip),
        Format::Pdf => unreachable!("PDF handled above"),
        Format::Xls => unreachable!("legacy Excel handled above"),
        Format::Rtf | Format::Fb2 | Format::FlatOdf | Format::Doc | Format::Ppt | Format::Msg => {
            unreachable!("non-container formats handled above")
        }
    }
}

/// The `content.xml` of an ODF sub-document (`Object 1`), if it exists.
fn odf_object<R: std::io::Read + std::io::Seek>(
    zip: &mut ZipArchive<R>,
    name: &str,
) -> Option<String> {
    Some(raw_xml_entry(zip, &format!("{name}/content.xml"))).filter(|x| !x.is_empty())
}

/// **A FictionBook carries its images INSIDE the file, in base64, and we
/// served them as prose.**
///
/// FB2 is not a container: it is ONE XML document, and the cover like
/// every illustration lives in a
/// `<binary id="cover.jpg" content-type="image/jpeg">…</binary>` at the end. The
/// tag stripper keeps every text node, so the base64 payload
/// arrived whole — dozens of kilobytes of an alphabet that says nothing,
/// billed per token and mixed into the novel.
///
/// The reference `<image l:href="#cover.jpg"/>` that DESIGNATES it stays and takes
/// the image mark: the reader learns there was an image, without paying for its
/// bytes. The mark is bare for lack of replacement text — FB2 declares none
/// — and that is exactly the case `image_marker` provides for.
fn extract_fb2(xml: &str) -> String {
    // **And what the FILE says about itself is not what the BOOK says.**
    //
    // The RTF path already drops `\info` as a destination, and its comment
    // says why in so many words: « author, company, comments LEAKED into
    // the agent's context ». FB2 stores the same thing in `<document-info>` —
    // who MADE the file, with which program, from which address, and its
    // conversion history — plus a free-form `<custom-info>`.
    //
    // Declared from the specification, not from memory: read on 2026-09-11 in
    // `FictionBook.xsd`, `<document-info>` has NO `minOccurs="0"`, so it
    // is mandatory and the rule cannot be dead code; `program-used`,
    // `src-url` and `history` are declared in it, `<custom-info>` is optional and
    // repeatable.
    //
    // `<title-info>` and `<publish-info>` stay: the title, the BOOK's author,
    // the publisher and the ISBN are what one asks of a book.
    let xml = &re(r"(?s)<(document-info|custom-info)\b[^>]*>.*?</(?:document-info|custom-info)>")
        .replace_all(xml, " ");
    let without_payloads = re(r"(?s)<binary\b[^>]*>.*?</binary>").replace_all(xml, " ");
    fb2::render(&without_payloads)
}

/// Flat ODF: meta, settings, styles AND content live in ONE XML file, so
/// extracting the whole document leaks the generator string, print settings and
/// style names into the text ("LibreOffice/26.2$Linux_X86_64 … 0 0 true false").
/// Only `<office:body>` is content — same lesson as the RTF font table.
fn extract_flat_odf(bytes: &[u8]) -> String {
    let xml = String::from_utf8_lossy(bytes);
    let body = xml
        .find("<office:body")
        .and_then(|start| xml[start..].find('>').map(|o| start + o + 1))
        .map(|content_start| {
            let end = xml[content_start..]
                .find("</office:body>")
                .map(|e| content_start + e)
                .unwrap_or(xml.len());
            &xml[content_start..end]
        })
        // No body element (malformed / not really flat ODF) → fall back to the
        // whole document rather than returning nothing.
        .unwrap_or(&xml);
    // A flat spreadsheet is a spreadsheet: same rendering as the zipped `.ods`, through the
    // same reader. The body DECLARES it (`<office:spreadsheet>`), we do not
    // guess it from the extension — a `.fodt` can carry a table without
    // being a workbook.
    // The XML's indentation is not text — see below; holds for
    // the three bodies (a LibreOffice `.fodp` came out `            - Notes`).
    let body: &str = &re(r">[ \t\r]*\n\s*<").replace_all(body, "><");
    let body: &str = &formulas::with_odf_formulas(body, |_| None);
    if body.contains("<office:spreadsheet") {
        return format!(
            "{}{}",
            spreadsheet::render_workbook(spreadsheet::ods_tables(&xml, body)),
            spreadsheet::hidden_note(
                "sheet",
                spreadsheet::ods_hidden_sheet_count(&xml, body),
                &[]
            )
        );
    }
    if body.contains("<office:presentation") {
        return render_odf_pages(&xml, body, "Slide");
    }
    if body.contains("<office:drawing") {
        return render_odf_pages(&xml, body, "Page");
    }
    // **Third time the styles are ABOVE the body.** Hidden sheets,
    // hidden pages, and now list
    // styles: a flat document stores its styles outside `<office:body>`, so
    // the stage that receives only the body is blind and a numbered list
    // came out as bullets.
    //
    // **The XML's indentation is not text.** LibreOffice writes an
    // INDENTED `.fodt` (one tag per line); the zipped `.odt` is not.
    // Whitespace between two block elements is not content in ODF, but
    // it arrived as is: list marks preceded by five spaces,
    // an empty line between each item, the list turned into indented code
    // (2026-10-03). LibreOffice never indents INSIDE a paragraph (mixed
    // content), hence the rule: whitespace that contains a line break, between
    // two tags, goes.
    let (body, notes) = odf_lift_notes(body);
    let text = tables_aware_with(
        &body,
        Schema::Odf,
        &DocParts {
            ordered: odf_ordered_lists(&xml),
            numbering: odf_number_styles(&xml),
            ..Default::default()
        },
    );
    format!("{text}{notes}")
}

/// How many lines at each end of a page can be a running head/foot. Boilerplate
/// lives at the very edges; searching deeper starts eating body text.
const RUNNING_HEAD_BAND: usize = 3;
/// A line must appear in this share of pages to count as boilerplate. High on
/// purpose: dropping a real line is worse than keeping a repeated one.
const RUNNING_HEAD_MIN_SHARE: f64 = 0.6;
/// A digit-varying line counts as one template only if stripping its digits
/// leaves at most this much text — i.e. the line IS a number ("Page 3 of 100",
/// "42"), not prose that happens to contain one.
const NUMERIC_TEMPLATE_MAX_RESIDUE: usize = 20;
/// Below this, repetition proves nothing — two pages sharing a line is a
/// coincidence, not a template.
const RUNNING_HEAD_MIN_PAGES: usize = 4;

/// Below this many words per page an extraction is not "a sparse document", it
/// is a FAILURE. A real text PDF averages hundreds.
const EMPTY_EXTRACT_WORDS_PER_PAGE: f64 = 50.0;
/// The rescue engine must find SUBSTANTIALLY more, not merely more: on a
/// genuinely sparse document (slides, forms) both engines agree and switching
/// would buy nothing while inheriting the other engine's failure modes.
const RESCUE_MIN_GAIN: f64 = 2.0;

/// PRIMARY engine. Benchmarked against poppler on 346 real PDFs
/// (`experiments/pdf-engines`, rerun it on any change here):
///
/// | engine                  | recall | failures | floor |
/// |-------------------------|--------|----------|-------|
/// | pdf_oxide               | 0.961  | 29/346   | 0.480 |
/// | pdf-extract + isolation | 0.935  | 52/346   | 0.000 |
///
/// pdf_oxide wins on all three, including the FLOOR, which is what settled it.
/// An early read on 52 books suggested pdf-extract was the safe one because
/// pdf_oxide fuses words on one scanned book (0.559) — but pdf-extract fuses
/// too, and worse: 22.4% of its tokens on `example-long.pdf` are impossibly long
/// against poppler's 7.1%. Both engines corrupt; pdf_oxide corrupts less often
/// and never returned nothing, while pdf-extract returned NOTHING on four
/// documents poppler reads whole.
/// Is this page's markdown a LAYOUT grid dressed as a table?
///
/// Layout analysis cannot tell a data table from text positioned in columns, so
/// a title page comes back as a table whose cells are mostly empty. The
/// separation is not a judgement call — MEASURED 2026-07-22 across the fixture
/// PDFs: the four real tables (en/fr/ja/ar eval corpus) hold **0 %** empty
/// cells, the ECMA-262 cover holds **50 %**. The threshold sits between two
/// populations that do not overlap, and the page falls back to plain text —
/// which is what it produced before layout analysis was switched on.
/// Below this a "table" is too small for any ratio over it to mean anything.
const TABLE_MIN_ROWS: usize = 2;

/// A candidate table scoring below this is not a table. Emit its text instead.
///
/// CALIBRATED 2026-07-22 against every table markdown emitted for the three
/// audit fixtures (see [`table_confidence`] for the per-block numbers): the
/// real tables land at 0.90-1.00, the section title dressed as a table at
/// 0.717, the half-empty diagram grid at 0.525, the ECMA-262 cover at 0.628.
/// 0.75 sits in that gap, but NOT centrally: 0.15 of clearance to the nearest
/// real table, only 0.033 to the nearest thing it condemns. The asymmetry is
/// deliberate — the cost of demoting a real table (an agent reads columns as a
/// sentence) is worse than the cost of keeping a false one — but it does mean
/// a block landing at 0.74 is a close call. Re-measure before moving this.
const TABLE_MIN_CONFIDENCE: f64 = 0.75;

/// Longest a cell can be and still read as a LABEL rather than a sentence.
///
/// MEASURED 2026-07-22: the widest label in the real `Cluster No` table of
/// `pdf_mid.pdf` is "Node expansion standard deviation" — 4 words. A tighter
/// bound scores that genuine table's own label column as prose.
const TABLE_LABEL_MAX_WORDS: usize = 5;

/// What share of a document's pages must show the column-merge signature before
/// the geometric repair pass is worth running. See [`repair_merged_columns`] for
/// the two measurements that set it.
const MIN_MERGED_PAGE_SHARE: f64 = 0.05;

pub fn extract_with_savings(bytes: &[u8], fmt: Format) -> (String, crate::Savings) {
    // Cost an agent pays when it naively cat-pipes the binary file: one
    // token per ~4 bytes (the same ratio our tokenizer assumes for text).
    // We intentionally do NOT use `String::from_utf8_lossy(bytes)` here:
    // lossy conversion collapses runs of invalid bytes into single U+FFFD
    // code points, *under*-counting tokens vs reality.
    let before = bytes.len().div_ceil(4);
    let out = extract(bytes, fmt);
    let after = tokenizer::estimate(&out);
    (out, crate::Savings::new(before, after))
}

/// Which table vocabulary a fragment speaks. WordprocessingML/DrawingML
/// (`w:tbl`/`a:tbl`) and OpenDocument (`table:table`) name the same three
/// concepts differently; everything downstream is identical.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Schema {
    /// `.docx` body and `.pptx` slides — both may carry `w:tbl` and `a:tbl`.
    Ooxml,
    /// `.odt` / `.ods` / `.odp` `content.xml`.
    Odf,
    /// EPUB chapters (XHTML). `<tr>` already forced a line break, so rows were
    /// separated — but the cells inside them still ran together.
    Html,
}

/// XML → text with tables rendered as markdown, for `schema`'s vocabulary.
fn tables_aware(xml: &str, schema: Schema) -> String {
    tables_aware_with(xml, schema, &DocParts::default())
}

/// **What a fragment cannot know on its own.**
///
/// Both fields come from ELSEWHERE in the Word package — `word/numbering.xml`
/// for the list format, `word/_rels/document.xml.rels` for a link's
/// target — and only the branch that opens the zip can read them. OpenDocument
/// declares both in the same file as its text, and the stages that
/// receive only `<office:body>` miss them (it happened three times),
/// so we pass them rather than guess them again.
#[derive(Default)]
struct DocParts {
    /// `identifier#level` keys whose format declares a NUMBERING.
    ordered: std::collections::HashSet<String>,
    /// `rId…` → target, for a Word document's links.
    links: std::collections::HashMap<String, String>,
    /// Format, prefix, suffix and start of each numbered level.
    numbering: NumberStyles,
}

/// **The SOURCE's indentation is not the text's.**
///
/// An XHTML indents its tags for the file's reader; that whitespace between two
/// blocks arrived in the Markdown as a line indentation. Four
/// spaces, and the line becomes CODE: the calibre guide in French
/// (`fra.epub`, 2026-10-03), whose tags are shifted by four spaces
/// without an empty line, delivered `    ### 2.7 : DRM` — no longer a heading — and the
/// question « qu'est-ce que les DRM » no longer found its section. The English one,
/// shifted by two, passed under the threshold by luck.
///
/// Only the whitespace preceding a TAG at the start of a line goes; a `<pre>`
/// keeps its own, its indentation is code.
fn without_source_indentation(xml: &str) -> std::borrow::Cow<'_, str> {
    let indent = re(r"\n[ \t]+<");
    let pre = re(r"(?is)<pre\b.*?</pre\s*>");
    if !indent.is_match(xml) {
        return std::borrow::Cow::Borrowed(xml);
    }
    let mut out = String::with_capacity(xml.len());
    let mut last = 0;
    for m in pre.find_iter(xml) {
        out.push_str(&indent.replace_all(&xml[last..m.start()], "\n<"));
        out.push_str(m.as_str());
        last = m.end();
    }
    out.push_str(&indent.replace_all(&xml[last..], "\n<"));
    std::borrow::Cow::Owned(out)
}

fn tables_aware_with(xml: &str, schema: Schema, parts: &DocParts) -> String {
    if xml.is_empty() {
        return String::new();
    }
    let unindented = if schema == Schema::Html {
        without_source_indentation(xml)
    } else {
        std::borrow::Cow::Borrowed(xml)
    };
    let xml: &str = &unindented;
    let mut ordered = parts.ordered.clone();
    let mut numbering = parts.numbering.clone();
    if schema == Schema::Odf {
        ordered.extend(odf_ordered_lists(xml));
        numbering.extend(odf_number_styles(xml));
    }
    let xml = &mark_lists(
        &mark_links(
            &mark_images(
                &without_what_the_document_does_not_show(
                    &without_tracked_deletions(&strip_non_prose_elements(xml, schema), schema),
                    schema,
                ),
                schema,
            ),
            schema,
            &parts.links,
        ),
        &ordered,
        &numbering,
    );
    let (tbl, row, cell) = match schema {
        // OOXML: `w:tbl` (Word) and `a:tbl` (DrawingML, used by PowerPoint and
        // by charts/shapes inside Word) share row/cell shapes closely enough to
        // parse with one alternation.
        Schema::Ooxml => (
            r"(?s)<(?:w|a):tbl\b.*?</(?:w|a):tbl>",
            r"(?s)<(?:w|a):tr\b[^>]*>(.*?)</(?:w|a):tr>",
            "(?:w|a):tc",
        ),
        Schema::Odf => (
            r"(?s)<table:table\b.*?</table:table>",
            r"(?s)<table:table-row\b[^>]*>(.*?)</table:table-row>",
            "table:table-cell",
        ),
        // `th` and `td` both, so a header row renders as the header.
        Schema::Html => (
            r"(?s)<table\b.*?</table>",
            r"(?s)<tr\b[^>]*>(.*?)</tr>",
            "t[hd]",
        ),
    };
    let flat = flatten_nested_tables(xml, schema);
    xml_to_text_with_tables(&flat, re(tbl), re(row), re(&element_pattern(cell)))
}

#[cfg(test)]
include!("docs_tests.rs");
