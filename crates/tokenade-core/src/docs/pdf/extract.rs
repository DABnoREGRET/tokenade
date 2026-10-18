//! **Getting the text out, and knowing whether there was any.**
//!
//! A scan is a PDF without text: extraction succeeds and returns nothing, which
//! is the worst of answers because it does not look like a failure.
//!
//! `pdf_is_text_rich` and `extraction_looks_failed` are the two halves of the
//! same verdict, and `rescue_is_better` decides whether a second engine is worth
//! it — comparing two extractions is cheaper than getting one wrong.

use super::*;

/// A page needs at least this many extractable words for its text to stand in
/// for the page image. A prose or table page clears it by 5-15×; a scanned or
/// diagram-only page falls far below (its content lives in the raster, not the
/// text layer).
pub(crate) const PDF_TEXT_RICH_MIN_WORDS_PER_PAGE: usize = 50;

pub(crate) fn pdf_oxide_pages(bytes: &[u8]) -> Vec<String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Ok(doc) = pdf_oxide::PdfDocument::from_bytes(bytes.to_vec()) else {
            return Vec::new();
        };
        let n = doc.page_count().unwrap_or(0);
        let opts = pdf_oxide::converters::ConversionOptions::default();
        (0..n)
            .map(|i| {
                // `to_markdown` runs the library's LAYOUT analysis; `extract_text`
                // returns content-stream order, which is the order the producer
                // happened to emit glyphs in — not the order a reader reads them.
                //
                // MEASURED 2026-07-22 on arXiv:1706.03762 page 1: with
                // `extract_text` the abstract's first sentence lands at offset
                // 649 and the "Abstract" heading at 726 — the sentence comes out
                // BEFORE its own heading, and the rotated margin stamp is spliced
                // mid-sentence ("generalizes well to arXiv:1706.03762v7 [cs.CL]
                // 2 Aug 2023 other tasks"). With `to_markdown` the same page
                // gives 923 / 933: heading first, sentence after.
                //
                // It also recovers what the plain path GLUED: on the eval corpus
                // a table row read "Region Ingest Errors Retries" and now reads
                // `| Region | Ingest | Errors | Retries |`, in en/fr/ja alike.
                //
                // What this does NOT buy, stated because an earlier version of
                // this comment claimed it did: sentence-level fidelity is NOT
                // strictly better. Word counts are >= the plain path's on every
                // fixture (attn 1321/1314, ecma 3411/3385, …) but that aggregate
                // MASKS segmentation: measured page by page, only 61 % of the
                // plain path's long sentences appear verbatim here — and 57 % the
                // other way round, so the two engines segment differently rather
                // than one shedding text. A concrete case: on page 8 of
                // arXiv:1706.03762 the plain path reads "BLEU score of 28.4. The
                // configuration of this model is listed in …" while this one
                // splits it, and the number lands in another block on the page.
                // The words are all still there; the sentence is not.
                //
                // The switch stands because the gains are structural and verified
                // (reading order, headings feeding the outline fold, tables,
                // RAG breadcrumbs) while the loss is a re-segmentation both
                // engines commit. No shape metric found so far separates a
                // fragmented page from a clean one — a broken-sentence rate
                // scores THIS page 0.12 for markdown against 0.30 for plain text,
                // i.e. backwards — so there is nothing honest to gate on yet.
                //
                // Falls back to `extract_text` per page: one page failing layout
                // analysis must not lose that page.
                // And the layout pass must not touch an RTL page.
                //
                // `to_markdown` runs a layout analysis that groups runs by
                // position; on a right-to-left page that regroups the letters
                // of a word across cells. Measured on the corpus' `ar.pdf`,
                // same page, same build:
                //
                // ```text
                // to_markdown   | NEEDLE N04 | <RLI>ة | 128 | 5 |<PDI>  7/14 aiguilles
                // extract_text  NEEDLE N04 دفعة 128 5               8/14
                // ```
                //
                // `دفعة` survives whole in the plain path and comes out as the
                // single letter `ة` in the laid-out one. The library says why,
                // in `text/bidi.rs`: reordering visual-order runs is the
                // CALLER's job because only the caller can know the order, and
                // "the default markdown converter does NOT call this for that
                // reason". So its layout pass is not RTL-aware by design, and a
                // page that is mostly RTL is exactly where it must not run.
                //
                // Not gated on "contains any RTL character": one Arabic word in
                // an English paper would then cost that page its tables. The
                // test is DOMINANCE, and it is measured on letters.
                match doc.to_markdown(i, &opts) {
                    Ok(md) if layout_pass_refused(&md).is_none() => {
                        let spans = doc.extract_spans(i).unwrap_or_default();
                        let shape: Vec<(&str, f32, f32)> = spans
                            .iter()
                            .map(|s| (s.text.as_str(), s.bbox.y, s.font_size))
                            .collect();
                        let md = restore_line_end_punctuation(&md, &shape);
                        let md = mark_raised_figures(&md, &shape);
                        let boxes: Vec<super::links::Span> = spans
                            .iter()
                            .map(|s| {
                                (
                                    s.text.clone(),
                                    s.bbox.x,
                                    s.bbox.y,
                                    s.bbox.width,
                                    s.bbox.height,
                                )
                            })
                            .collect();
                        let md = super::links::without_misplaced_links(
                            &md,
                            &super::links::link_areas(&doc, i),
                            &boxes,
                        );
                        if md.lines().filter(|l| list_item_text(l).is_some()).count() < 2 {
                            return nest_list_items(&md, &[]);
                        }
                        let spans: Vec<(String, f32)> =
                            spans.into_iter().map(|s| (s.text, s.bbox.x)).collect();
                        nest_list_items(&md, &spans)
                    }
                    _ => doc.extract_text(i).unwrap_or_default(),
                }
            })
            .collect()
    }))
    .unwrap_or_default()
}

/// The text of a markdown list line (`- x`, `* x`, `1. x`), marker removed.
/// **A raised or lowered figure is not a digit of the word before it** — the
/// rule the other document readers follow (`raised_and_lowered`): a note call
/// after a year read `16481`, `m²` read `m2`, and the layout pass hands the
/// glyphs over with nothing between them. The page says it in the SPANS: the
/// figure is set smaller than the text it follows, its top level with that
/// text's when raised, lower when lowered (LibreOffice and Word exports,
/// 2026-10-04). Each such pair is found in the page's markdown, in order, as
/// the end of the previous span written straight against the figure; where
/// the markdown does not hold them glued, nothing changes.
/// **The layout pass drops a punctuation mark that ends a line after a change
/// of font.** `This is <b>bold</b>.` came back `This is **bold**`, a figure
/// set smaller lost the full stop after it, and so did inline code
/// (pdf_oxide 0.3.77, LibreOffice export, 2026-10-04; `extract_text` keeps
/// them, so the glyphs are there). Mid-line the same mark survives
/// (`*words*, then`). Each span made of punctuation alone is looked for, in
/// order, right after the end of the span before it — past any closing
/// emphasis or code marker — and put back where the line ends instead.
pub(crate) fn restore_line_end_punctuation(md: &str, spans: &[(&str, f32, f32)]) -> String {
    let mut out = String::with_capacity(md.len() + 8);
    let mut cursor = 0;
    // What the page says just before each span: a one-glyph span (`1`) found
    // on its own lands on the first `1` of the page.
    let mut before = String::new();
    for w in spans.windows(2) {
        before.push_str(w[0].0);
        let mark = w[1].0.trim();
        // On the line of the span it follows, or it is not that span's mark.
        if (w[1].1 - w[0].1).abs() >= w[0].2.max(w[1].2) {
            continue;
        }
        if mark.is_empty()
            || mark.chars().count() > 3
            || !mark
                .chars()
                .all(|c| matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']'))
        {
            continue;
        }
        let last8 = |t: &str| -> String {
            let rev: Vec<char> = t.trim_end().chars().rev().take(8).collect();
            rev.into_iter().rev().collect()
        };
        // The wider context first; the span alone when emphasis markers sit
        // inside that context (`is **bold**` is not `is bold`).
        let Some((off, tail)) = [last8(&before), last8(w[0].0)]
            .into_iter()
            .filter(|t| !t.is_empty())
            .find_map(|t| md[cursor..].find(t.as_str()).map(|o| (o, t)))
        else {
            continue;
        };
        let mut at = cursor + off + tail.len();
        at += md[at..].len() - md[at..].trim_start_matches(['*', '_', '`']).len();
        if md[at..].starts_with('\n') || at == md.len() {
            out.push_str(&md[cursor..at]);
            out.push_str(mark);
            cursor = at;
        } else {
            out.push_str(&md[cursor..at]);
            cursor = at;
        }
    }
    out.push_str(&md[cursor..]);
    out
}

pub(crate) fn mark_raised_figures(md: &str, spans: &[(&str, f32, f32)]) -> String {
    let mut out = String::with_capacity(md.len() + 8);
    let mut cursor = 0;
    for w in spans.windows(2) {
        let ((prev, prev_top, prev_size), (fig, top, size)) = (w[0], w[1]);
        let fig = fig.trim();
        if fig.is_empty()
            || fig.chars().count() > 4
            || fig.chars().all(char::is_alphabetic)
            || size > prev_size * 0.75
        {
            continue;
        }
        let tail: String = {
            let rev: Vec<char> = prev.chars().rev().take(8).collect();
            rev.into_iter().rev().collect()
        };
        if tail.is_empty() || tail.ends_with(char::is_whitespace) {
            continue;
        }
        let Some(off) = md[cursor..].find(&format!("{tail}{fig}")) else {
            continue;
        };
        let at = cursor + off + tail.len();
        out.push_str(&md[cursor..at]);
        out.push(if top >= prev_top - prev_size * 0.1 {
            '^'
        } else {
            '_'
        });
        cursor = at;
    }
    out.push_str(&md[cursor..]);
    out
}

fn list_item_text(line: &str) -> Option<&str> {
    let t = line.trim_start();
    if let Some(r) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
        return Some(r);
    }
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    (digits > 0)
        .then(|| t[digits..].strip_prefix(". "))
        .flatten()
}

/// **Give a PDF list its nesting back, from where its items START.**
///
/// The layout pass renders every item `- x` at level 0: `Debian` and
/// `Workstation`, under `Linux` and `Fedora`, came out as `Linux`'s siblings
/// while the page draws them 0.5 and 1 inch further right — the same list read
/// from the `.docx` nests (2026-10-03). Each list line is matched, in order, to
/// the span that carries its text, and the distinct left edges of one run of
/// list lines become its levels. A run with one edge, or a line whose span is
/// not found, is left exactly as it was.
pub(crate) fn nest_list_items(md: &str, spans: &[(String, f32)]) -> String {
    // The layout pass prefixes a NUMBERED item with its own bullet too:
    // `- 1. Build the artefacts`. Two markers on one item; the number is the
    // one the page shows.
    // And a slide's second-level items (drawn with `–`) come out as HEADINGS
    // holding a bullet: `# - Linux`. A heading whose text is a list item is
    // the list item (2026-10-03, a PowerPoint outline printed to PDF).
    let unheaded: Vec<&str> = md
        .lines()
        .map(|l| {
            let t = l.trim_start_matches('#');
            if t.len() < l.len() && t.starts_with(' ') && t.trim_start().starts_with("- ") {
                t.trim_start()
            } else {
                l
            }
        })
        .collect();
    let unbulleted: Vec<String> = unheaded
        .iter()
        .map(|l| match l.trim_start().strip_prefix("- ") {
            Some(r)
                if r.bytes().take_while(u8::is_ascii_digit).count() > 0
                    && r.trim_start_matches(|c: char| c.is_ascii_digit())
                        .starts_with(". ") =>
            {
                r.to_string()
            }
            _ => l.to_string(),
        })
        .collect();
    let lines: Vec<&str> = unbulleted.iter().map(String::as_str).collect();
    let mut cursor = 0usize;
    let mut x_of: Vec<Option<f32>> = vec![None; lines.len()];
    for (i, line) in lines.iter().enumerate() {
        let Some(item) = list_item_text(line) else {
            continue;
        };
        let key: String = item.trim().chars().take(12).collect();
        if key.chars().count() < 2 {
            continue;
        }
        let found = spans[cursor..].iter().position(|(t, _)| {
            let t =
                t.trim_start_matches(|c: char| c.is_whitespace() || "•◦▪▫‣⁃-*·●○■□".contains(c));
            !t.is_empty() && (t.starts_with(&key) || (t.chars().count() >= 4 && key.starts_with(t)))
        });
        if let Some(k) = found {
            x_of[i] = Some(spans[cursor + k].1);
            cursor += k + 1;
        }
    }
    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    let mut drop_line = vec![false; lines.len()];
    let mut i = 0;
    while i < lines.len() {
        if list_item_text(lines[i]).is_none() {
            i += 1;
            continue;
        }
        let start = i;
        // A run of list lines may be LOOSE (a blank between items): it is
        // still one list, and its levels are one set.
        while i < lines.len()
            && (list_item_text(lines[i]).is_some()
                || (lines[i].trim().is_empty()
                    && lines[i + 1..]
                        .iter()
                        .find(|l| !l.trim().is_empty())
                        .is_some_and(|l| list_item_text(l).is_some())))
        {
            i += 1;
        }
        let mut edges: Vec<f32> = x_of[start..i].iter().flatten().copied().collect();
        edges.sort_by(|a, b| a.total_cmp(b));
        edges.dedup_by(|b, a| (*b - *a).abs() < 3.0);
        // Nesting steps are small (a quarter to half an inch); edges far apart
        // are COLUMNS, and two columns' lists are not one list's levels.
        let columns = edges.windows(2).any(|w| w[1] - w[0] > 150.0);
        let unmatched = (start..i).any(|j| list_item_text(lines[j]).is_some() && x_of[j].is_none());
        if edges.len() < 2 || columns || unmatched {
            continue;
        }
        let ordered = lines[start..i]
            .iter()
            .any(|l| l.trim_start().starts_with(|c: char| c.is_ascii_digit()));
        let step = if ordered { 3 } else { 2 };
        // The blanks inside a run come from the layout pass putting every
        // line in its own paragraph; the list the page draws is tight.
        for j in (start..i).filter(|&j| lines[j].trim().is_empty()) {
            drop_line[j] = true;
        }
        for j in (start..i).filter(|&j| x_of[j].is_some()) {
            let x = x_of[j].unwrap_or(edges[0]);
            let level = edges.iter().rposition(|e| x >= *e - 3.0).unwrap_or(0);
            out[j] = format!("{}{}", " ".repeat(level * step), lines[j].trim_start());
        }
    }
    let mut joined = out
        .into_iter()
        .zip(drop_line)
        .filter_map(|(l, d)| (!d).then_some(l))
        .collect::<Vec<_>>()
        .join("\n");
    if md.ends_with('\n') {
        joined.push('\n');
    }
    joined
}

/// Whether a PDF's extracted text is a faithful substitute for reading its
/// pages as images.
///
/// The Read hook lets PDFs pass through to agents that render them natively
/// (Claude Code), because for a scan or a diagram-heavy PDF the text layer is
/// near-empty and only the image carries the content. But most PDFs are prose:
/// there, the native image read costs ~1.5k tokens PER PAGE and the equivalent
/// text costs a fraction, with every word preserved. This decides which case a
/// given PDF is — text-rich → serve the text, image-poor → keep it native.
///
/// Returns false when nothing extracts (a scan is not text-rich), so the caller
/// falls back to the native read rather than replacing a page image with an
/// empty string.
/// Page count via the PDF's own catalog — cheap: parses the xref, not the page
/// streams. Lets a caller judge text density (words/page) from an already-
/// extracted text without extracting a SECOND time to count pages.
pub fn pdf_page_count(bytes: &[u8]) -> usize {
    std::panic::catch_unwind(|| {
        lopdf::Document::load_mem(bytes)
            .map(|d| d.get_pages().len())
            .unwrap_or(0)
    })
    .unwrap_or(0)
}

/// Whether extracted `text` (already in hand) is dense enough per page to stand
/// in for the page images — the one-pass form, so a hook that already has the
/// text does not re-extract just to decide.
/// Words that are CONTENT — a token must carry at least one alphanumeric.
///
/// Every gate below asks "did we get real text off this page?", and answering it
/// with `split_whitespace().count()` lets MARKUP answer for content. Since PDF
/// pages are rendered through layout analysis they can contain table syntax, and
/// an entirely EMPTY 3-column, 14-row table counts 53 whitespace-separated
/// tokens — over the 50-word bar — while carrying zero characters of text
/// (MEASURED 2026-07-22). The page that would then be declared "text-rich" is
/// exactly the diagram or scan whose content lives only in the raster, and
/// declaring it rich SUPPRESSES the native image read: the agent would receive a
/// grid of nothing instead of the page.
///
/// Counting alphanumeric-bearing tokens makes the gates measure what they claim
/// to measure, on any format.
pub(crate) fn content_words(text: &str) -> usize {
    text.split_whitespace()
        .filter(|w| w.chars().any(|c| c.is_alphanumeric()))
        .count()
}

pub fn text_is_pdf_text_rich(text: &str, pages: usize) -> bool {
    pages > 0 && content_words(text) >= pages * PDF_TEXT_RICH_MIN_WORDS_PER_PAGE
}

pub fn pdf_is_text_rich(bytes: &[u8]) -> bool {
    let pages = pdf_oxide_pages(bytes);
    if pages.is_empty() {
        return false;
    }
    let words: usize = pages.iter().map(|p| content_words(p)).sum();
    words >= pages.len() * PDF_TEXT_RICH_MIN_WORDS_PER_PAGE
}

/// Did the primary engine fail outright? A real text PDF averages hundreds of
/// words per page; single digits mean the fonts defeated it, not that the
/// document is empty.
pub(crate) fn extraction_looks_failed(words: usize, pages: usize) -> bool {
    (words as f64 / pages.max(1) as f64) < EMPTY_EXTRACT_WORDS_PER_PAGE
}

/// Accept the rescue only on a decisive margin. "A bit more text" is no evidence
/// of anything — pdf_oxide's extra words are sometimes fused garbage — so a
/// genuinely sparse document, where both engines agree, stays on the primary.
pub(crate) fn rescue_is_better(primary_words: usize, rescued_words: usize) -> bool {
    rescued_words as f64 > (primary_words as f64).max(1.0) * RESCUE_MIN_GAIN
}

pub(crate) fn raw_pdf_text(bytes: &[u8]) -> String {
    // pdf_oxide leads — see `pdf_oxide_pages` for the numbers that settled it.
    // Per PAGE, because running heads/feet can only be recognised page by page.
    let mut paged = pdf_oxide_pages(bytes);
    let words = paged.iter().map(|p| content_words(p)).sum::<usize>();

    // Rescue with the other engine when the primary read ~nothing. It never
    // fired on the 346-PDF benchmark (identical score with and without), which
    // is exactly why it stays: it costs nothing here and is the only thing
    // standing between a user and an empty document on input this corpus does
    // not contain.
    if extraction_looks_failed(words, paged.len()) {
        let alt = pdf_extract_pages(bytes);
        let alt_words = alt.iter().map(|p| content_words(p)).sum::<usize>();
        if rescue_is_better(words, alt_words) {
            paged = alt;
        }
    }
    // Demotion runs per PAGE and BEFORE the pages are joined, because a table
    // block is a page-local run of `|` lines: once `dedup_running_heads` has
    // concatenated the pages, a table ending one page and another starting the
    // next become one run and would be judged as a single block.
    let paged: Vec<String> = repair_merged_columns(bytes, paged)
        .into_iter()
        .map(|p| demote_false_tables(&p))
        .collect();
    numbered_bold_headings(&reflow_lines(&dedup_running_heads(&paged)))
}

#[cfg(test)]
mod nesting_tests {
    use super::nest_list_items;

    fn spans(v: &[(&str, f32)]) -> Vec<(String, f32)> {
        v.iter().map(|(t, x)| (t.to_string(), *x)).collect()
    }

    /// The left edge of each item's text is its level.
    #[test]
    fn items_nest_by_where_their_text_starts() {
        let md = "# Plan\n- Linux\n- Debian\n- Workstation\n- Windows\n\nAfter.\n";
        let sp = spans(&[
            ("Plan", 56.0),
            ("•", 60.0),
            ("Linux", 74.0),
            ("◦", 96.0),
            ("Debian", 110.0),
            ("▪", 132.0),
            ("Workstation", 146.0),
            ("•", 60.0),
            ("Windows", 74.0),
            ("After.", 56.0),
        ]);
        assert_eq!(
            nest_list_items(md, &sp),
            "# Plan\n- Linux\n  - Debian\n    - Workstation\n- Windows\n\nAfter.\n"
        );
    }

    /// A numbered item rendered `- 1. x` keeps its number only, and nests.
    #[test]
    fn a_numbered_item_loses_the_extra_bullet() {
        let md = "- 1. Build the artefacts\n- 1. Linux binaries\n- 2. Publish the notes\n";
        let sp = spans(&[
            ("1.", 60.0),
            ("Build the artefacts", 74.0),
            ("1.", 96.0),
            ("Linux binaries", 110.0),
            ("2.", 60.0),
            ("Publish the notes", 74.0),
        ]);
        assert_eq!(
            nest_list_items(md, &sp),
            "1. Build the artefacts\n   1. Linux binaries\n2. Publish the notes\n"
        );
        assert_eq!(nest_list_items("- 1. Alone\n", &[]), "1. Alone\n");
    }

    /// A slide outline printed to PDF: second-level items come out as
    /// headings holding a bullet, every item in its own paragraph.
    #[test]
    fn a_heading_bullet_is_an_item_and_a_loose_run_is_one_list() {
        let md = "# Plan\n\n- Build\n\n# - Linux\n\n- Debian\n\n- Publish\n";
        let sp = spans(&[
            ("Plan", 200.0),
            ("•", 40.0),
            ("Build", 60.0),
            ("–", 80.0),
            ("Linux", 100.0),
            ("•", 120.0),
            ("Debian", 140.0),
            ("•", 40.0),
            ("Publish", 60.0),
        ]);
        assert_eq!(
            nest_list_items(md, &sp),
            "# Plan\n\n- Build\n  - Linux\n    - Debian\n- Publish\n"
        );
    }

    /// Edges far apart are two COLUMNS, not two levels; and a line whose span
    /// is not found leaves the run exactly as it was.
    #[test]
    fn columns_and_unmatched_lines_are_left_alone() {
        let md = "- Left item\n- Right item\n";
        assert_eq!(
            nest_list_items(md, &spans(&[("Left item", 74.0), ("Right item", 320.0)])),
            md
        );
        assert_eq!(
            nest_list_items(md, &spans(&[("Left item", 74.0), ("Elsewhere", 110.0)])),
            md
        );
    }
}

#[cfg(test)]
mod raised_tests {
    use super::mark_raised_figures;

    /// The spans of a LibreOffice export, as `extract_spans` gives them: the
    /// raised figures' top level with the line's, the lowered one's below.
    const LINE: [(&str, f32, f32); 7] = [
        ("Area 5 m", 708.25, 11.0),
        ("2", 708.25, 6.4),
        (", CO", 708.25, 11.0),
        ("2", 706.95, 6.4),
        (" level in 1648", 708.25, 11.0),
        ("1", 708.25, 6.4),
        (".", 708.25, 11.0),
    ];

    #[test]
    fn a_smaller_figure_is_marked_raised_or_lowered() {
        let md = mark_raised_figures("Area 5 m2, CO2 level in 16481.", &LINE);
        assert_eq!(md, "Area 5 m^2, CO_2 level in 1648^1.");
    }

    /// Where the markdown does not hold the pair glued, or the small span is a
    /// word, nothing is touched.
    #[test]
    fn nothing_changes_where_the_pair_is_not_glued() {
        assert_eq!(mark_raised_figures("in 1648 1.", &LINE[4..7]), "in 1648 1.");
        let word = [("See", 700.0, 11.0), ("note", 700.0, 6.0)];
        assert_eq!(mark_raised_figures("Seenote", &word), "Seenote");
        let same = [("in 1648", 700.0, 11.0), ("1", 700.0, 11.0)];
        assert_eq!(mark_raised_figures("in 16481", &same), "in 16481");
    }
}

#[cfg(test)]
mod line_end_punctuation_tests {
    use super::restore_line_end_punctuation;

    /// Spans of a LibreOffice export; `to_markdown` dropped each final mark.
    #[test]
    fn a_mark_after_a_font_change_is_put_back_at_the_line_end() {
        let spans = [
            ("This is ", 700.0, 11.0),
            ("bold", 700.0, 11.0),
            (".", 700.0, 11.0),
            ("Ends in italic ", 680.0, 11.0),
            ("words", 680.0, 11.0),
            (", then more.", 680.0, 11.0),
            (" level in 1648", 660.0, 11.0),
            ("1", 660.0, 6.4),
            (".", 660.0, 11.0),
        ];
        let md = "This is **bold**\n\nEnds in italic *words*, then more.\n\nlevel in 16481\n";
        assert_eq!(
            restore_line_end_punctuation(md, &spans),
            "This is **bold**.\n\nEnds in italic *words*, then more.\n\nlevel in 16481.\n"
        );
    }

    /// Mid-line the layout pass keeps the mark: nothing is added twice.
    #[test]
    fn a_mark_kept_mid_line_is_not_doubled() {
        let spans = [
            ("in italic ", 680.0, 11.0),
            ("words", 680.0, 11.0),
            (",", 680.0, 11.0),
            (" then", 680.0, 11.0),
        ];
        assert_eq!(
            restore_line_end_punctuation("in italic *words*, then\n", &spans),
            "in italic *words*, then\n"
        );
    }

    /// A mark from another line (another column) is not this line's.
    #[test]
    fn a_mark_from_another_line_is_not_put_here() {
        let spans = [("Introduction", 700.0, 14.0), (".", 400.0, 11.0)];
        assert_eq!(
            restore_line_end_punctuation("# Introduction\n", &spans),
            "# Introduction\n"
        );
    }
}
