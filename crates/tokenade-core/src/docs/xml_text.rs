//! **From Office XML to text, tables included.**
//!
//! These formats are deep XML where the text lives in leaves amid index
//! fields, properties and metadata. Removing the wrong element cuts a sentence
//! in two; keeping one too many delivers a table of contents regenerated on
//! every open.

use super::*;

/// Elements whose text is never document prose.
///
/// This function used to know only `<style>` and `<script>`, i.e. the HTML
/// family, and did not even receive the schema. A Word text frame (a callout,
/// a box) exposed the two holes OOXML left there, on a document produced by
/// LibreOffice in a one-line conversion:
///
/// ```text
/// Body sentence NEEDLE-TB-BODY carries the argument.
/// centertopNEEDLE-TB-INSIDE the callout says this.      <- "centertop"
/// NEEDLE-TB-INSIDE the callout says this.               <- twice
/// ```
///
/// **`centertop` is a FABRICATION**: these are the TEXT nodes of
/// `<wp:positionH><wp:align>center</wp:align></wp:positionH>` and its vertical
/// twin — anchoring geometry, rendered as if it were a sentence. This is not a
/// list of names encountered but the complete content model: the specification
/// gives `wp:positionH`/`wp:positionV` exactly one child, `wp:align` or
/// `wp:posOffset`, and neither is text.
///
/// **The duplicate is `mc:AlternateContent`** (the source writes a fact
/// twice): Word puts the SAME `w:txbxContent` in `mc:Choice` (modern form) and
/// in `mc:Fallback` (VML form, for a reader that does not understand the
/// first). A consumer picks ONE; we rendered both. `Fallback` is optional and
/// `Choice` mandatory, so dropping the fallback renders the text exactly once —
/// and cannot lose anything.
///
/// On the OpenDocument side, `office:annotation`, for a reason that was ALREADY
/// written elsewhere in this file. `ooxml_word_notes` deliberately excludes
/// `word/comments.xml`: "review annotations, not the document's text, and they
/// carry their author's NAME — personal data an extractor must not serve
/// unless asked to". OOXML stores its comments in a separate part, which we do
/// not read, so the rule applied there by itself; ODF stores them IN the body,
/// and the generic stripper rendered them. Same document, two spellings, two
/// answers — and the chattier one delivered a proper name.
///
/// In a workbook it is worse than an inconsistency: the annotation lives
/// inside the cell, so the VALUE was replaced. Measured on the same converted
/// document:
///
/// ```text
/// .ods   | CELL-REGION | RELECTEUR-NOMME2026-09-11T00:00:00ANNOTATION-… 42 |
/// .xlsx  | CELL-REGION | 42 |
/// ```
///
/// Whoever asks "how much for CELL-REGION?" gets a sentence instead of 42.
pub(super) fn strip_non_prose_elements(xml: &str, schema: Schema) -> String {
    // An annotation is an INLINE element, set within the sentence — and its end
    // bound (`annotation-end`, that of a comment on a RANGE) falls where the
    // range ends, often mid-sentence, sometimes mid-word. Replaced with a space
    // like a block, it rendered `See the  [link]`, or `spec ification`: a word
    // the document does not write.
    let xml = if schema == Schema::Odf && xml.contains("<office:annotation") {
        re(r"(?s)<office:annotation\b[^>]*>.*?</office:annotation>|<office:annotation-end\b[^>]*/?>")
            .replace_all(xml, "")
    } else {
        std::borrow::Cow::Borrowed(xml)
    };
    let out = re(&strip_pattern(schema)).replace_all(&xml, " ");
    // An OOXML index is not a pair of tags: it is a FIELD, bounded by two marks
    // in the run stream. This second copy of the document is only paid for by
    // the rare documents that carry one.
    if schema == Schema::Ooxml && out.contains("fldCharType") {
        return ooxml_without_index_fields(&out);
    }
    out.into_owned()
}

/// **A Word table of contents, and why it cannot be removed with a regular
/// expression.**
///
/// Word does not enclose its index in an element: it opens a FIELD
/// (`<w:fldChar w:fldCharType="begin"/>`), writes its instruction
/// (`<w:instrText> TOC \o "1-9" </w:instrText>`), sets a separator, then
/// leaves the RESULT — one paragraph per entry, `PREMIER CHAPITRE 1` — up to
/// the end mark. The result crosses paragraph boundaries, so no tag pair
/// bounds it.
///
/// And fields NEST: each entry of a table of contents itself carries a
/// `PAGEREF` field. So depth must be counted, otherwise the first end
/// encountered would close the table in the middle of its first entry.
///
/// Only fields whose instruction NAMES an index are removed. A field is also
/// what carries a date, a page number, a cross-reference: their result is text
/// the document shows and that is found nowhere else.
pub(super) fn ooxml_without_index_fields(xml: &str) -> String {
    /// The instructions whose RESULT is a copy of what the document already says
    /// elsewhere. `BIBLIOGRAPHY` is not among them, for the same reason as on the
    /// OpenDocument side: its entries are nowhere else.
    const INDEX_FIELDS: [&str; 4] = ["TOC", "TOA", "INDEX", "RD"];
    let instr = re(r"(?s)<w:instrText\b[^>]*>(.*?)</w:instrText>");
    // The FIELD: its instruction names it.
    let without_fields = drop_balanced_ranges(
        xml,
        re(r#"<w:fldChar\b[^>]*w:fldCharType="begin"[^>]*/?>"#),
        re(r#"<w:fldChar\b[^>]*w:fldCharType="end"[^>]*/?>"#),
        |range| {
            instr.captures(range).is_some_and(|c| {
                let i = c[1].trim_start();
                INDEX_FIELDS.iter().any(|f| {
                    i.strip_prefix(f)
                        .is_some_and(|r| !r.starts_with(|c: char| c.is_alphanumeric()))
                })
            })
        },
    );
    // The CONTAINER, when Word sets one. Without it, the title the author typed
    // above the table stays stuck above the first chapter, which it now seems to
    // announce (a pruner must take the promise along with the body). OpenDocument
    // stores this title INSIDE its index, so it already goes there: without
    // this, two saves of the same document would not read the same.
    if !without_fields.contains("docPartGallery") {
        return without_fields;
    }
    let gallery = re(r#"<w:docPartGallery\b[^>]*w:val="([^"]*)""#);
    drop_balanced_ranges(
        &without_fields,
        re(r"<w:sdt(?:\s[^>]*)?>"),
        re(r"</w:sdt>"),
        |range| {
            gallery
                .captures(range)
                .is_some_and(|c| c[1].starts_with("Table of "))
        },
    )
}

/// Removes the ranges BOUNDED by two nesting marks, when their content answers
/// `true` to the predicate.
///
/// Written once because both bounds of a Word index have the same shape — a
/// field, a `w:sdt` — and neither can be bounded by a regular expression: they
/// NEST (each entry of a table of contents carries its own `PAGEREF` field),
/// so the first end encountered is not the right one, and they cross paragraph
/// boundaries.
///
/// The range is replaced by a space rather than deleted: what remains is read
/// by a tag stripper, not a parser, and a space guarantees that the two words
/// that end up adjacent do not weld together.
pub(super) fn drop_balanced_ranges(
    xml: &str,
    open: &regex::Regex,
    close: &regex::Regex,
    keep_out: impl Fn(&str) -> bool,
) -> String {
    let mut marks: Vec<(usize, usize, bool)> = open
        .find_iter(xml)
        .map(|m| (m.start(), m.end(), true))
        .chain(close.find_iter(xml).map(|m| (m.start(), m.end(), false)))
        .collect();
    if marks.is_empty() {
        return xml.to_string();
    }
    marks.sort_unstable();
    let mut out = String::with_capacity(xml.len());
    let (mut cursor, mut depth) = (0usize, 0i32);
    let mut open_at: Option<usize> = None;
    for (start, stop, is_open) in marks {
        if is_open {
            if depth == 0 {
                open_at = Some(start);
            }
            depth += 1;
            continue;
        }
        depth = (depth - 1).max(0);
        if depth > 0 {
            continue;
        }
        let Some(from) = open_at.take() else { continue };
        if from < cursor || !keep_out(&xml[from..stop]) {
            continue;
        }
        out.push_str(&xml[cursor..from]);
        out.push(' ');
        cursor = stop;
    }
    out.push_str(&xml[cursor..]);
    out
}

/// **The stripper's branches, and why they are a single pass.**
///
/// Each one was a separate `replace_all`, so the WHOLE document was copied
/// once per pattern: twice for HTML, **five** times for OOXML or OpenDocument.
/// An alternation merges them into one copy, and the result is the same — two
/// patterns of this list cannot overlap other than by nesting, and nesting
/// renders the same thing in both orders (the container is removed with
/// everything inside it). This is not reasoning taken on faith: a test in
/// `docs_tests.rs` compares, document by document over the whole corpus, the
/// rendered text with that of the old path.
///
/// Flags are set PER BRANCH. `<style>` and `<script>` come from HTML and are
/// written there in any case; XML names, on the other hand, are case-sensitive,
/// and a global `(?i)` would have matched `<MC:FALLBACK>` — a tag no producer
/// writes and that we would therefore never have seen fail.
pub(super) fn strip_pattern(schema: Schema) -> String {
    let mut branches: Vec<&str> = vec![
        r"(?is:<style\b[^>]*>.*?</style\s*>)",
        r"(?is:<script\b[^>]*>.*?</script\s*>)",
    ];
    if schema == Schema::Html {
        // **The head of an XHTML document is not its body.**
        //
        // Each EPUB chapter is a complete document, with its `<head>`. Its `<title>`
        // was rendered as text, and since it precedes the body with nothing
        // separating them, it arrived WELDED to the chapter's first word:
        // `t# Table des matieres` on a fixture, and the book title repeated in front
        // of each of the fourteen chapters of the real book.
        //
        // The weld has a second effect, costlier than the word itself: the chapter no
        // longer seems to start with a heading, so the stage that adds a heading when
        // there is none adds one — and the chapter title comes out TWICE.
        branches.push(r"(?is:<head\b[^>]*>.*?</head\s*>)");
    }
    match schema {
        Schema::Ooxml => branches.extend([
            r"(?s:<mc:Fallback\b[^>]*>.*?</mc:Fallback>)",
            r"(?s:<wp:positionH\b[^>]*>.*?</wp:positionH>)",
            r"(?s:<wp:positionV\b[^>]*>.*?</wp:positionV>)",
        ]),
        // **An index is a COPY of what is already in the document.**
        //
        // A table of contents carries each heading a second time, followed by its
        // page number, and the number arrives WELDED to the heading
        // (`PREMIER CHAPITRE 1`): a sentence the document says nowhere, on top of the
        // duplicate. Nothing is lost by removing them — the headings are in the body,
        // and `mark_headings` has already turned them into the structure the reader
        // follows.
        //
        // The list comes from OpenDocument's content model, not from forms
        // encountered, and **the bibliography is NOT in it**: its entries are not
        // copies of the body, they are references found nowhere else. An index of
        // illustrations only names captions already served with their figures.
        Schema::Odf => branches.extend([
            r"(?s:<text:table-of-content\b[^>]*>.*?</text:table-of-content>)",
            r"(?s:<text:illustration-index\b[^>]*>.*?</text:illustration-index>)",
            r"(?s:<text:table-index\b[^>]*>.*?</text:table-index>)",
            r"(?s:<text:object-index\b[^>]*>.*?</text:object-index>)",
            r"(?s:<text:user-index\b[^>]*>.*?</text:user-index>)",
            r"(?s:<text:alphabetical-index\b[^>]*>.*?</text:alphabetical-index>)",
            r"(?s:<office:binary-data\b[^>]*>.*?</office:binary-data>)",
            // A DATA style (date, time, number, currency) writes its separators as
            // text — `<number:text>/</number:text>` — in the automatic styles preceding
            // the body: every `.odt` carrying a date field started with `//`
            // (2026-10-04). Only those go: text styles are still used for hiding, which
            // reads them after this stripping.
            r"(?s:<number:[a-z-]+-style\b[^>]*>.*?</number:[a-z-]+-style>)",
        ]),
        Schema::Html => {}
    }
    branches.join("|")
}

pub(super) fn xml_to_text_with_tables(
    xml: &str,
    tbl_re: &regex::Regex,
    row_re: &regex::Regex,
    cell_re: &regex::Regex,
) -> String {
    let mut out = String::new();
    let mut last = 0usize;
    for m in tbl_re.find_iter(xml) {
        out.push_str(&xml_to_text(&xml[last..m.start()]));
        let rows = xml_table_rows(m.as_str(), row_re, cell_re, false);
        let md = crate::markdown::rows_to_markdown(&rows);
        if !md.trim().is_empty() {
            out.push('\n');
            out.push_str(&md);
            out.push('\n');
        }
        last = m.end();
    }
    out.push_str(&xml_to_text(&xml[last..]));
    out
}

/// The pattern of an ELEMENT, in both its spellings.
///
/// Each of the three table readers carried its own `<X …>(.*?)</X>` pair, and
/// none sees the self-closing form. This is not an edge case: an empty but
/// STYLED cell is written exactly like that, and LibreOffice produces one
/// without being asked. Measured on a converted workbook:
///
/// ```text
/// <c r="A1" s="1" t="s"><v>0</v></c><c r="B1" s="1"/><c r="C1" s="0" t="s"><v>1</v></c>
/// ```
///
/// Cell B1 does not close, so `(.*?)</c>` runs to C1's closing tag: B1
/// swallows C1, inherits its `<v>1</v>` without inheriting its `t="s"`, and
/// the workbook delivers **`1`** — the shared string's INDEX — instead of
/// `COLONNE-C`. A fabrication that erases the real cell along the way. On the
/// OpenDocument side the same form (`<table:table-cell/>`) matches nothing:
/// the empty cell disappears and everything after it shifts one column, so
/// every value changes header.
///
/// Group 1 is the attribute list, group 2 the body — ABSENT when the element
/// closes itself, which is the right answer: an empty cell.
pub(super) fn element_pattern(name: &str) -> String {
    format!(r"(?s)<{name}\b([^>]*?)(?:/>|>(.*?)</{name}>)")
}

/// Rows of one table block. Cell text runs through the normal XML→text pass,
/// then folds to a single line so it can live in a markdown cell.
pub(super) fn xml_table_rows(
    block: &str,
    row_re: &regex::Regex,
    cell_re: &regex::Regex,
    spread_merges: bool,
) -> Vec<Vec<String>> {
    // OOXML merged cells: `<w:gridSpan w:val="N"/>` inside a `<w:tc>` makes that
    // cell occupy N grid columns. One `<w:tc>` is emitted, not N, so without
    // padding every later row shifts left and the whole table misaligns — the
    // same defect the web table converter had. Pad with `N-1` empty cells. The
    // span sits inside the captured cell body (a `<w:tcPr>` child), so this reads
    // it directly.
    let gridspan = re(r#"(?s)<(?:w|a):gridSpan\b[^>]*\bw:val="(\d+)""#);
    // The same merge, in the two other spellings, and on the OPENING TAG instead
    // of the body. Measured on a converted workbook, a title merged over two
    // columns let the next value move up a column, so every row below changed
    // header.
    let attr_span = re(r#"(?s)\b(?:table:number-columns-spanned|colspan)="(\d+)""#);
    let row_span = re(r#"(?s)\btable:number-rows-spanned="(\d+)""#);
    // **OpenDocument writes every covered position**: one `table:covered-table-
    // cell` per cell a merge covers, in width AS WELL AS height. The
    // `table:table-cell` pattern alone skipped them: a vertical merge
    // (`number-rows-spanned`) moved the whole row below one column up — measured
    // 2026-10-03, `| Pears | 20 | |` under `| Region | Product | Units |`,
    // `Pears` filed under Region. Where they exist, they are what holds the
    // columns, and padding by attribute steps aside so as not to count twice.
    let odf_covered = block.contains("table:covered-table-cell");
    let odf_cells = re(
        r"(?s)(<table:covered-table-cell\b[^>]*?(?:/>|>.*?</table:covered-table-cell>))|<table:table-cell\b([^>]*?)(?:/>|>(.*?)</table:table-cell>)",
    );
    // A workbook merge reads as the Microsoft workbook reads it
    // (`apply_merges`): the value spreads over the covered cells. Tables of a
    // text document keep empty cells, like Word's.
    // **A repeated cell is that many cells.** OpenDocument writes a run of
    // identical cells once, with `table:number-columns-repeated="N"` — two
    // empty cells as ONE element. Counted as one, every value after it moved a
    // column left: LibreOffice's `| Sum | | | 47,5 |` read `| Sum | | 47,5 |`,
    // the total under `Price` (2026-10-04, same workbook as `.xlsx`). Empty
    // cells wait until a value follows them, so a row's trailing
    // `number-columns-repeated="16379"` costs nothing.
    let repeated = re(r#"\btable:number-columns-repeated="(\d+)""#);
    let mut vertical: std::collections::HashMap<usize, (String, usize)> =
        std::collections::HashMap::new();
    let mut rows_out: Vec<Vec<String>> = Vec::new();
    for (ri, r) in row_re.captures_iter(block).enumerate() {
        let body = r.get(1).map(|m| m.as_str()).unwrap_or("");
        let mut row: Vec<String> = Vec::new();
        let mut horizontal: Option<(String, usize)> = None;
        let mut pending_empty = 0usize;
        let flush = |row: &mut Vec<String>, pending: &mut usize| {
            row.extend(std::iter::repeat_n(String::new(), (*pending).min(1024)));
            *pending = 0;
        };
        let caps: Vec<regex::Captures> = if odf_covered {
            odf_cells.captures_iter(body).collect()
        } else {
            cell_re.captures_iter(body).collect()
        };
        for c in caps {
            let times = repeated
                .captures(
                    c.get(0)
                        .map_or("", |m| m.as_str())
                        .split('>')
                        .next()
                        .unwrap_or(""),
                )
                .and_then(|m| m[1].parse::<usize>().ok())
                .unwrap_or(1)
                .clamp(1, 1024);
            // The LOGICAL column: cells still waiting count.
            let col = row.len() + pending_empty;
            if odf_covered && c.get(1).is_some() {
                flush(&mut row, &mut pending_empty);
                let fill = if !spread_merges {
                    String::new()
                } else if let Some((v, _)) = vertical.get(&col).filter(|(_, last)| ri <= *last) {
                    v.clone()
                } else if let Some((v, _)) = horizontal.as_ref().filter(|(_, end)| col < *end) {
                    v.clone()
                } else {
                    String::new()
                };
                row.extend(std::iter::repeat_n(fill, times));
                continue;
            }
            let attrs = c
                .get(if odf_covered { 2 } else { 1 })
                .map(|m| m.as_str())
                .unwrap_or("");
            let inner = c.get(c.len() - 1).map(|m| m.as_str()).unwrap_or("");
            let text = xml_to_text(inner)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let span = gridspan
                .captures(inner)
                .or_else(|| attr_span.captures(attrs))
                .and_then(|m| m[1].parse::<usize>().ok())
                .unwrap_or(1)
                .clamp(1, 100);
            if odf_covered {
                let down = row_span
                    .captures(attrs)
                    .and_then(|m| m[1].parse::<usize>().ok())
                    .unwrap_or(1)
                    .clamp(1, 10_000);
                if down > 1 {
                    for k in col..col + span {
                        vertical.insert(k, (text.clone(), ri + down - 1));
                    }
                }
                horizontal = (span > 1).then(|| (text.clone(), col + span));
                if text.is_empty() && down == 1 && span == 1 {
                    pending_empty += times;
                    continue;
                }
                flush(&mut row, &mut pending_empty);
                row.extend(std::iter::repeat_n(text, times));
                continue;
            }
            if text.is_empty() {
                pending_empty += span * times;
                continue;
            }
            flush(&mut row, &mut pending_empty);
            for _ in 0..times {
                row.push(text.clone());
                for _ in 1..span {
                    row.push(String::new());
                }
            }
        }
        rows_out.push(row);
    }
    rows_out
        .into_iter()
        .filter(|r: &Vec<String>| !r.is_empty())
        .collect()
}

// `rows_to_markdown` lives in `markdown`, its subject: rendering rows as a
// GFM table has nothing to do with a document reader. It was parked here,
// with its first caller, and `table_profile` called it by climbing one level —
// via `super::docs::`, a spelling the layer guard did not even read.

#[cfg(test)]
mod odf_covered_cells_hold_their_column {
    use super::{element_pattern, re, xml_table_rows};

    const TABLE: &str = r#"<table:table>
<table:table-row><table:table-cell><text:p>Region</text:p></table:table-cell><table:table-cell><text:p>Product</text:p></table:table-cell><table:table-cell><text:p>Units</text:p></table:table-cell></table:table-row>
<table:table-row><table:table-cell table:number-rows-spanned="2"><text:p>North</text:p></table:table-cell><table:table-cell><text:p>Apples</text:p></table:table-cell><table:table-cell><text:p>10</text:p></table:table-cell></table:table-row>
<table:table-row><table:covered-table-cell/><table:table-cell><text:p>Pears</text:p></table:table-cell><table:table-cell><text:p>20</text:p></table:table-cell></table:table-row>
<table:table-row><table:table-cell table:number-columns-spanned="2"><text:p>Both</text:p></table:table-cell><table:covered-table-cell/><table:table-cell><text:p>30</text:p></table:table-cell></table:table-row>
</table:table>"#;

    fn rows(spread: bool) -> Vec<Vec<String>> {
        let row_re = re(r"(?s)<table:table-row\b[^>]*>(.*?)</table:table-row>");
        let cell_re = re(&element_pattern("table:table-cell"));
        xml_table_rows(TABLE, row_re, cell_re, spread)
    }

    /// A vertical merge's covered cell sat outside the cell pattern: the row
    /// below came back `| Pears | 20 | |`, every value one column to the left.
    #[test]
    fn a_vertical_merge_keeps_the_row_below_aligned() {
        let sheet = rows(true);
        assert_eq!(sheet[2], ["North", "Pears", "20"]);
        assert_eq!(
            sheet[3],
            ["Both", "Both", "30"],
            "a horizontal merge is counted once"
        );
        let doc = rows(false);
        assert_eq!(doc[2], ["", "Pears", "20"]);
        assert_eq!(doc[3], ["Both", "", "30"]);
    }
}
