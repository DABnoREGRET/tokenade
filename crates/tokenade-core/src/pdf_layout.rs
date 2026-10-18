//! Reading-order reconstruction for PDF text.
//!
//! ## Why this exists
//!
//! `pdf-extract`'s `PlainTextOutput` is a purely SEQUENTIAL emitter: it writes
//! glyphs in content-stream order, starting a new line when `y` jumps and
//! inserting a space when `x` gaps. It never asks where text sits on the page.
//!
//! For a multi-column page that is wrong, and measurably so. A producer that
//! composes visually emits line-by-line ACROSS columns, so the extract reads:
//!
//! ```text
//! GAUCHE-01 texte de la colonne de gauche DROITE-01 texte de la colonne de droite
//! ```
//!
//! Every output line splices two unrelated sentences. Nothing is missing, so
//! nothing looks broken — but an agent asked to summarise a two-column paper is
//! reading interleaved halves of two different arguments. Same family as the
//! fused spreadsheet cells: the data survives, its meaning does not.
//!
//! We therefore collect glyphs WITH their positions (`output_character` hands us
//! the text-rendering matrix), rebuild lines, detect a column gutter, and emit
//! column by column.
//!
//! ## Scope
//!
//! Single-column pages — the overwhelming majority — must come out byte-for-byte
//! as before, so [`Page::to_text`] only reorders when a gutter is actually found.
//! Font decoding, CMaps and encodings stay entirely `pdf-extract`'s job: we
//! consume the characters it already decoded.

// Some geometry on the structures below is carried but not read: the
// reading-order pass needs only the ordering, while the column/row geometry
// (`height`, `x_start`, `x_end`, `lines()`, `gutter()`) is what a TABLE pass
// would consume. That pass was measured and deliberately not shipped, and
// throwing the geometry away would mean re-deriving it from the PDF the day it
// is. Annotated rather than deleted, and rather than left to warn on every
// build — a warning list nobody can empty is a warning list nobody reads.
#![allow(dead_code)]

use pdf_extract::{MediaBox, OutputDev, OutputError, Transform};

/// One WORD with its position, y measured DOWNWARD from the top (the flip
/// `PlainTextOutput` applies, kept so ordering reads naturally).
///
/// A word, not a glyph, because `pdf-extract` tells us where words begin
/// (`begin_word`, driven by the PDF's own text-showing structure) and that is
/// knowledge we cannot re-derive from geometry. Re-deriving it is not
/// hypothetical: assembling glyph-by-glyph turned the letter-spaced title
/// "MARINA DI VEZZA" into "MARINA  DI  VEZZA", because in tracked-out text every
/// inter-letter gap clears the space threshold.
#[derive(Debug, Clone)]
struct Word {
    x: f64,
    y: f64,
    /// Rightmost edge, i.e. where the next word would start with no gap.
    x_end: f64,
    size: f64,
    ch: String,
}

/// A page's glyphs plus the page box they live in.
#[derive(Debug, Default)]
pub struct Page {
    words: Vec<Word>,
    width: f64,
    height: f64,
}

/// One page read in column order, plus what it takes to tell whether somebody
/// else's extraction of the same page respected those columns.
#[derive(Debug, Default)]
pub struct PageOrder {
    /// The page's text, columns read one after the other.
    pub text: String,
    /// Whether a gutter was provable anywhere on the page. False means this is
    /// an ordinary single-column page and `text` is worth nothing over the
    /// caller's own extraction.
    pub columnar: bool,
    /// One entry per vertical cut: the line texts on each side of it.
    /// See [`PageOrder::merged_in`].
    cuts: Vec<ColumnCut>,
}

/// The two sides of one vertical cut, as assembled line texts.
#[derive(Debug)]
struct ColumnCut {
    left: Vec<String>,
    right: Vec<String>,
}

impl PageOrder {
    /// Did `text` — somebody else's extraction of this same page — merge the
    /// columns instead of reading down each in turn?
    ///
    /// The test is a whole line of one column and a whole line of the other
    /// sharing an output line. Reading a page column by column, those two are
    /// separated by the entire rest of the first column, so they cannot land
    /// together; an extractor sweeping the page in visual rows puts them
    /// together on every row. That makes this evidence rather than a heuristic.
    ///
    /// It deliberately does NOT pair the two lines by baseline. MEASURED
    /// 2026-07-22 on page 1 of `pdf_mid.pdf`: the columns are offset by about
    /// one line, so the row the other extractor emitted joins the left line to
    /// the right line BELOW the one sharing its baseline. Requiring the pairing
    /// found nothing on a page that is merged on every row.
    pub fn merged_in(&self, text: &str) -> bool {
        text.lines().any(|line| {
            line.len() >= MIN_MERGE_EVIDENCE_CHARS * 2
                && self.cuts.iter().any(|c| {
                    c.left.iter().any(|l| line.contains(l.as_str()))
                        && c.right.iter().any(|r| line.contains(r.as_str()))
                })
        })
    }
}

/// A run of words sharing a baseline.
#[derive(Debug)]
struct Line {
    y: f64,
    x_start: f64,
    x_end: f64,
    text: String,
}

/// Collects positioned glyphs for every page. See the module docs.
#[derive(Default)]
pub struct PositionedText {
    pages: Vec<Page>,
    current: Page,
    flip_height: f64,
    /// The word being accumulated between `begin_word` and `end_word`.
    word: Option<Word>,
}

impl PositionedText {
    pub fn new() -> Self {
        Self::default()
    }

    fn flush_word(&mut self) {
        if let Some(w) = self.word.take() {
            if !w.ch.is_empty() {
                self.current.words.push(w);
            }
        }
    }

    pub fn into_pages(mut self) -> Vec<Page> {
        self.flush_word();
        if !self.current.words.is_empty() {
            self.pages.push(std::mem::take(&mut self.current));
        }
        self.pages
    }
}

impl OutputDev for PositionedText {
    fn begin_page(
        &mut self,
        _page_num: u32,
        media_box: &MediaBox,
        _art_box: Option<(f64, f64, f64, f64)>,
    ) -> Result<(), OutputError> {
        self.word = None;
        self.flip_height = media_box.ury - media_box.lly;
        self.current = Page {
            words: Vec::new(),
            width: media_box.urx - media_box.llx,
            height: self.flip_height,
        };
        Ok(())
    }

    fn end_page(&mut self) -> Result<(), OutputError> {
        self.flush_word();
        self.pages.push(std::mem::take(&mut self.current));
        Ok(())
    }

    fn output_character(
        &mut self,
        trm: &Transform,
        width: f64,
        _spacing: f64,
        font_size: f64,
        char: &str,
    ) -> Result<(), OutputError> {
        // Mirrors PlainTextOutput's own maths. Flipping is `y' = H - y` (compose
        // with row_major(1,0,0,-1,0,H)), so it reduces to reading m31 and m32
        // directly — no euclid vector algebra needed for the position.
        let x = trm.m31;
        let y = self.flip_height - trm.m32;
        // The font size as it lands on the page: scale the nominal size through
        // the matrix and take the geometric mean of the two axes, which is what
        // PlainTextOutput compares gaps against.
        let sx = font_size * (trm.m11 + trm.m21);
        let sy = font_size * (trm.m12 + trm.m22);
        let size = (sx * sy).abs().sqrt();
        let x_end = x + width * size;
        match self.word.as_mut() {
            // Same word: extend it. Its glyphs' own gaps are letter spacing, not
            // word spacing, and must not become spaces.
            Some(w) if (y - w.y).abs() <= w.size.max(size) * 0.5 => {
                w.ch.push_str(char);
                w.x_end = x_end;
                w.size = w.size.max(size);
            }
            // A glyph that jumps baselines mid-word is a new word in practice.
            Some(_) => {
                self.flush_word();
                self.word = Some(Word {
                    x,
                    y,
                    x_end,
                    size,
                    ch: char.to_string(),
                });
            }
            None => {
                self.word = Some(Word {
                    x,
                    y,
                    x_end,
                    size,
                    ch: char.to_string(),
                })
            }
        }
        Ok(())
    }

    fn begin_word(&mut self) -> Result<(), OutputError> {
        self.flush_word();
        Ok(())
    }
    fn end_word(&mut self) -> Result<(), OutputError> {
        self.flush_word();
        Ok(())
    }
    fn end_line(&mut self) -> Result<(), OutputError> {
        Ok(())
    }
}

/// A gutter must be at least this wide, relative to page width, to be a column
/// separator rather than the ordinary space between two words.
const MIN_GUTTER_FRAC: f64 = 0.04;
/// Each side of a gutter must carry at least this many BASELINES to be a
/// column. Fewer means a table, a signature block or a margin note — pages we
/// must not reorder.
const MIN_LINES_FOR_COLUMNS: usize = 4;
/// A floor on how much text a page must hold before columns mean anything. Runs
/// are what `pdf-extract` calls words, and one `Tj` can be a whole line, so this
/// counts TEXT RUNS and stays low on purpose — the real guard is
/// [`MIN_LINES_FOR_COLUMNS`] baselines on EACH side of the gutter.
const MIN_RUNS_FOR_COLUMNS: usize = 8;
/// A horizontal whitespace band must exceed this multiple of the block's
/// typical font size to be a block separator rather than ordinary leading
/// between two lines of one paragraph.
const MIN_BAND_GAP_FACTOR: f64 = 1.5;
/// Depth cap on the recursive cut. Real pages nest two or three levels — page,
/// then header band and body band, then the body's columns — and each level
/// costs a pass over the block's words, so this only bounds pathological input.
const MAX_CUT_DEPTH: usize = 6;
/// How long a column line must be before finding it inside another extractor's
/// output line counts as evidence. A short fragment can collide by chance; a
/// half-line of prose cannot.
const MIN_MERGE_EVIDENCE_CHARS: usize = 24;

impl Page {
    /// Group text runs into baselines. Runs arrive in stream order, which for a
    /// visually-composed page hops between columns, so grouping is by POSITION.
    fn lines(&self) -> Vec<Line> {
        self.lines_of(&self.words.iter().collect::<Vec<_>>())
    }

    /// Group a GIVEN set of glyphs into baselines. Takes a subset so columns can
    /// be lined up independently: a left-column line and a right-column line
    /// share a baseline, and merging them is exactly the bug.
    fn lines_of(&self, words: &[&Word]) -> Vec<Line> {
        let mut sorted: Vec<&Word> = words.to_vec();
        sorted.sort_by(|a, b| {
            a.y.partial_cmp(&b.y)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal))
        });
        let mut lines: Vec<Vec<&Word>> = Vec::new();
        for g in sorted {
            // Same baseline when within half a font size — the tolerance
            // PlainTextOutput uses to decide a line break.
            let same = lines
                .last()
                .and_then(|l| l.first())
                .is_some_and(|f| (g.y - f.y).abs() <= f.size.max(g.size) * 0.5);
            if same {
                lines.last_mut().expect("checked").push(g);
            } else {
                lines.push(vec![g]);
            }
        }
        lines
            .into_iter()
            .filter(|l| !l.is_empty())
            .map(|mut l| {
                l.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));
                let mut text = String::new();
                let mut prev_end: Option<f64> = None;
                for g in &l {
                    if let Some(pe) = prev_end {
                        // PlainTextOutput's rule, applied BETWEEN words: a gap
                        // wider than a tenth of the font size is a space.
                        if g.x > pe + g.size * 0.1 {
                            text.push(' ');
                        }
                    }
                    text.push_str(&g.ch);
                    prev_end = Some(g.x_end);
                }
                Line {
                    y: l[0].y,
                    x_start: l[0].x,
                    x_end: l.iter().map(|g| g.x_end).fold(f64::MIN, f64::max),
                    text,
                }
            })
            .collect()
    }

    /// Find a vertical gutter: an x band no TEXT occupies. Returns the x to
    /// split at, or `None` when the page is single-column — the common case,
    /// which must stay on the untouched path.
    ///
    /// Deliberately computed from the raw runs, not from assembled lines: in a
    /// two-column page the left and right columns SHARE baselines, so a line
    /// built across them already spans the full width and hides the very gutter
    /// we are looking for. (My first attempt did that and found nothing.)
    fn gutter(&self) -> Option<f64> {
        self.gutter_of(&self.words.iter().collect::<Vec<_>>())
    }

    /// The same search over an arbitrary BLOCK of the page rather than the whole
    /// of it. A page-global x projection is only correct when the page is
    /// uniformly columnar, and most two-column documents are not: a title, an
    /// abstract or a running foot spans the full width, its x span bridges the
    /// gutter, and the union of all x spans comes out continuous.
    ///
    /// MEASURED 2026-07-22 on page 1 of `pdf_mid.pdf` (a two-column paper): the
    /// whole-page projection finds NO empty x band at all — 73.7 to 521.6 is
    /// covered end to end — while the same computation restricted to the body
    /// band (y 500-600) finds a 22.7 pt gap at x=286, dead centre of a 595 pt
    /// page. The gutter was always there; the projection could not see it.
    fn gutter_of(&self, words: &[&Word]) -> Option<f64> {
        if self.width <= 0.0 || words.len() < MIN_RUNS_FOR_COLUMNS {
            return None;
        }
        let text_left = words.iter().map(|g| g.x).fold(f64::MAX, f64::min);
        let text_right = words.iter().map(|g| g.x_end).fold(f64::MIN, f64::max);
        let text_width = text_right - text_left;
        if text_width <= 0.0 {
            return None;
        }
        // Widest x band crossed by no glyph.
        let mut spans: Vec<(f64, f64)> = words.iter().map(|g| (g.x, g.x_end)).collect();
        spans.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut best: Option<(f64, f64)> = None; // (gap, split_x)
        let mut reach = spans[0].1;
        for (start, end) in spans.iter().skip(1) {
            if *start > reach {
                let gap = start - reach;
                if best.is_none_or(|(w, _)| gap > w) {
                    best = Some((gap, reach + gap / 2.0));
                }
            }
            reach = reach.max(*end);
        }
        let (gap, split) = best?;
        if gap < text_width * MIN_GUTTER_FRAC {
            return None;
        }
        // Both sides must carry real text on several BASELINES. A wide gap on a
        // handful of lines is a table, a signature block, or a margin note —
        // reordering those would corrupt an ordinary page.
        let baselines = |side: &dyn Fn(&Word) -> bool| -> usize {
            let mut ys: Vec<i64> = words
                .iter()
                .filter(|g| side(g))
                .map(|g| (g.y / g.size.max(1.0)).round() as i64)
                .collect();
            ys.sort_unstable();
            ys.dedup();
            ys.len()
        };
        let left = baselines(&|g: &Word| g.x_end <= split);
        let right = baselines(&|g: &Word| g.x >= split);
        if left < MIN_LINES_FOR_COLUMNS || right < MIN_LINES_FOR_COLUMNS {
            return None;
        }
        Some(split)
    }

    /// Split a block at its widest horizontal whitespace band, so a full-width
    /// header stops hiding the gutter of the columnar body underneath it.
    ///
    /// Unlike the vertical cut this one cannot corrupt reading order — top is
    /// read before bottom whatever the page looks like — so it needs no
    /// structural guard, only a threshold that keeps ordinary leading between
    /// two lines of the same paragraph from counting as a band.
    fn band_split(words: &[&Word]) -> Option<f64> {
        // A word occupies roughly one font size upward from its baseline.
        let mut spans: Vec<(f64, f64)> = words.iter().map(|g| (g.y - g.size, g.y)).collect();
        spans.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut sizes: Vec<f64> = words.iter().map(|g| g.size).collect();
        sizes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let typical = sizes[sizes.len() / 2].max(1.0);
        let mut best: Option<(f64, f64)> = None; // (gap, split_y)
        let mut reach = spans[0].1;
        for (start, end) in spans.iter().skip(1) {
            if *start > reach {
                let gap = start - reach;
                if best.is_none_or(|(w, _)| gap > w) {
                    best = Some((gap, reach + gap / 2.0));
                }
            }
            reach = reach.max(*end);
        }
        let (gap, split) = best?;
        (gap >= typical * MIN_BAND_GAP_FACTOR).then_some(split)
    }

    /// Recursive XY cut: cut into columns where a gutter is provable, otherwise
    /// into stacked bands, and assemble baselines only at the leaves.
    ///
    /// Vertical first, because a gutter is the strong claim and the one the
    /// guards in [`Page::gutter_of`] exist to police; the horizontal cut is what
    /// makes that claim REACHABLE on a mixed page, by handing the body band to
    /// the next level without the header bridging it.
    ///
    /// `columnar` records whether any vertical cut fired anywhere on the page.
    /// It is what lets the caller keep the untouched path for pages we have no
    /// business rewriting — see [`reordered_pages`].
    fn read_order(
        &self,
        words: &[&Word],
        depth: usize,
        out: &mut Vec<Line>,
        columnar: &mut bool,
        cuts: &mut Vec<ColumnCut>,
    ) {
        if words.is_empty() {
            return;
        }
        if depth < MAX_CUT_DEPTH {
            if let Some(x) = self.gutter_of(words) {
                *columnar = true;
                let left: Vec<&Word> = words.iter().copied().filter(|g| g.x < x).collect();
                let right: Vec<&Word> = words.iter().copied().filter(|g| g.x >= x).collect();
                self.collect_cut(&left, &right, cuts);
                self.read_order(&left, depth + 1, out, columnar, cuts);
                self.read_order(&right, depth + 1, out, columnar, cuts);
                return;
            }
            if let Some(y) = Self::band_split(words) {
                let top: Vec<&Word> = words.iter().copied().filter(|g| g.y <= y).collect();
                let bottom: Vec<&Word> = words.iter().copied().filter(|g| g.y > y).collect();
                // A cut that puts everything on one side is no cut; recursing on
                // it would not terminate.
                if !top.is_empty() && !bottom.is_empty() {
                    self.read_order(&top, depth + 1, out, columnar, cuts);
                    self.read_order(&bottom, depth + 1, out, columnar, cuts);
                    return;
                }
            }
        }
        out.extend(self.lines_of(words));
    }

    /// Record the line texts on each side of a cut, so a caller can test its own
    /// output for a merged reading. See [`PageOrder::merged_in`].
    ///
    /// Short lines are dropped: a column's last line, a page number or a stray
    /// fragment can occur inside an unrelated line by chance, and the whole
    /// value of this test is that it does not fire by chance.
    fn collect_cut(&self, left: &[&Word], right: &[&Word], out: &mut Vec<ColumnCut>) {
        let keep = |ws: &[&Word]| -> Vec<String> {
            self.lines_of(ws)
                .into_iter()
                .map(|l| l.text.trim().to_string())
                .filter(|t| t.len() >= MIN_MERGE_EVIDENCE_CHARS)
                .collect()
        };
        let (left, right) = (keep(left), keep(right));
        if !left.is_empty() && !right.is_empty() {
            out.push(ColumnCut { left, right });
        }
    }

    /// The page's text in reading order, and whether the page was columnar at
    /// all. A page with no gutter anywhere comes back exactly as
    /// [`Page::lines`] would have assembled it.
    fn read_order_text(&self) -> (String, bool) {
        let o = self.order();
        (o.text, o.columnar)
    }

    /// The page read in column order, with the evidence that would expose a
    /// different extractor having merged those columns.
    fn order(&self) -> PageOrder {
        let all: Vec<&Word> = self.words.iter().collect();
        let mut lines = Vec::new();
        let mut columnar = false;
        let mut cuts = Vec::new();
        self.read_order(&all, 0, &mut lines, &mut columnar, &mut cuts);
        PageOrder {
            text: lines
                .iter()
                .map(|l| l.text.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            columnar,
            cuts,
        }
    }

    /// The page's text in reading order.
    pub fn to_text(&self) -> String {
        self.read_order_text().0
    }
}

/// Reading-order text for the pages that ACTUALLY have columns; `None` for every
/// other page.
///
/// Returning `None` for single-column pages is the whole safety story. Our line
/// assembly is not `PlainTextOutput` — it makes its own spacing decisions — so
/// running it over every page would rewrite the output of every PDF we handle to
/// chase a fix that only concerns multi-column ones. The caller keeps
/// `pdf-extract`'s own text wherever we return `None`, so a page with no gutter
/// comes out exactly as before, and the blast radius is the pages that are
/// genuinely broken today.
pub fn reordered_pages(bytes: &[u8]) -> Option<Vec<Option<String>>> {
    Some(
        page_orders(bytes)?
            .into_iter()
            .map(|o| o.columnar.then_some(o.text))
            .collect(),
    )
}

/// Every page's column-order reading and its merge evidence.
///
/// Separate from [`reordered_pages`] because the two callers want different
/// things: the fallback extractor wants the TEXT for pages it would otherwise
/// get wrong, while the primary extractor already has text of its own and wants
/// to know whether that text merged the columns.
pub fn page_orders(bytes: &[u8]) -> Option<Vec<PageOrder>> {
    let doc = lopdf::Document::load_mem(bytes).ok()?;
    let mut dev = PositionedText::new();
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        pdf_extract::output_doc(&doc, &mut dev).ok()
    }))
    .ok()??;
    Some(dev.into_pages().iter().map(|p| p.order()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(width: f64, lines: &[(f64, f64, &str)]) -> Page {
        let mut words = Vec::new();
        for (x, y, text) in lines {
            let mut cx = *x;
            for ch in text.chars() {
                words.push(Word {
                    x: cx,
                    y: *y,
                    x_end: cx + 5.0,
                    size: 10.0,
                    ch: ch.to_string(),
                });
                cx += 5.0;
            }
        }
        Page {
            words,
            width,
            height: 792.0,
        }
    }

    /// End-to-end through the real `OutputDev`, on a real PDF whose content
    /// stream interleaves the columns line by line — what a visually-composing
    /// producer emits, and the case `PlainTextOutput` gets wrong.
    #[test]
    fn real_two_column_pdf_is_read_column_by_column() {
        let mut stream = String::from("BT /F1 9 Tf\n");
        for i in 1..=20 {
            let y = 740 - i * 12;
            stream.push_str(&format!(
                "1 0 0 1 60 {y} Tm (GAUCHE-{i:02} texte de la colonne de gauche) Tj\n\
                 1 0 0 1 320 {y} Tm (DROITE-{i:02} texte de la colonne de droite) Tj\n"
            ));
        }
        stream.push_str("ET\n");
        let pdf = crate::docs::tests::make_pdf(&stream);
        let pages = reordered_pages(&pdf).expect("layout");
        let text = pages[0]
            .clone()
            .expect("page has columns but was not reordered");
        let last_gauche = text.rfind("GAUCHE").expect("left column missing");
        let first_droite = text.find("DROITE").expect("right column missing");
        assert!(
            last_gauche < first_droite,
            "columns still interleaved — each line splices two arguments:\n{text}"
        );
        assert_eq!(
            text.matches("GAUCHE-").count(),
            20,
            "left column lost lines"
        );
        assert_eq!(
            text.matches("DROITE-").count(),
            20,
            "right column lost lines"
        );
        // Letter spacing must not become word spacing (`MARINA  DI  VEZZA`).
        assert!(!text.contains("  "), "double spaces introduced:\n{text}");
    }

    /// Ordinary prose must be left to pdf-extract untouched: a page with no
    /// gutter returns `None`, so nothing is rewritten.
    #[test]
    fn real_single_column_pdf_is_left_to_pdf_extract() {
        let mut stream = String::from("BT /F1 9 Tf\n");
        for i in 1..=20 {
            let y = 740 - i * 12;
            stream.push_str(&format!(
                "1 0 0 1 60 {y} Tm (Ligne {i:02} de prose ordinaire qui court sur toute la largeur utile) Tj\n"
            ));
        }
        stream.push_str("ET\n");
        let pdf = crate::docs::tests::make_pdf(&stream);
        let pages = reordered_pages(&pdf).expect("layout");
        assert!(pages[0].is_none(), "false gutter on single-column prose");
    }

    /// Kept at the unit level: the ordering itself, independent of any PDF.
    #[test]
    fn two_columns_are_emitted_one_after_the_other() {
        let mut rows = Vec::new();
        for i in 0..10 {
            rows.push((60.0, 100.0 + i as f64 * 12.0, "GAUCHE"));
            rows.push((320.0, 100.0 + i as f64 * 12.0, "DROITE"));
        }
        let refs: Vec<(f64, f64, &str)> = rows.clone();
        let text = page(612.0, &refs).to_text();
        let first_droite = text.find("DROITE").expect("right column missing");
        let last_gauche = text.rfind("GAUCHE").expect("left column missing");
        assert!(
            last_gauche < first_droite,
            "columns still interleaved:\n{text}"
        );
        assert_eq!(text.matches("GAUCHE").count(), 10);
        assert_eq!(text.matches("DROITE").count(), 10);
    }

    /// Ordinary prose must be left exactly alone — reordering a page that has no
    /// columns is how this optimisation would corrupt the common case.
    #[test]
    fn single_column_prose_is_not_reordered() {
        let rows: Vec<(f64, f64, &str)> = (0..10)
            .map(|i| {
                (
                    60.0,
                    100.0 + i as f64 * 12.0,
                    "une ligne de prose ordinaire",
                )
            })
            .collect();
        let p = page(612.0, &rows);
        assert!(p.gutter().is_none(), "false gutter on prose");
        let text = p.to_text();
        assert_eq!(text.lines().count(), 10);
    }

    /// A short page proves nothing, and a lone marginal note is not a column.
    #[test]
    fn weak_signals_never_trigger_a_split() {
        let short = page(612.0, &[(60.0, 100.0, "a"), (320.0, 100.0, "b")]);
        assert!(short.gutter().is_none(), "split on 1 line");

        let mut rows: Vec<(f64, f64, &str)> = (0..8)
            .map(|i| (60.0, 100.0 + i as f64 * 12.0, "corps du texte"))
            .collect();
        rows.push((560.0, 130.0, "*"));
        let p = page(612.0, &rows);
        assert!(p.gutter().is_none(), "margin note taken for a column");
    }

    /// The page shape that defeated the old page-global x projection: a
    /// full-width title over a two-column body. The title's own x span bridges
    /// the gutter, so projecting the WHOLE page finds no empty band and the
    /// page was declared single-column — MEASURED 2026-07-22 on page 1 of
    /// `pdf_mid.pdf`, where the projection covered 73.7 to 521.6 with no gap at
    /// all while the body band alone showed 22.7 pt of clear space at x=286.
    #[test]
    fn a_full_width_header_no_longer_hides_the_body_gutter() {
        let pdf = mixed_header_and_columns();
        let pages = reordered_pages(&pdf).expect("layout");
        let text = pages[0]
            .clone()
            .expect("mixed page has columns but was not reordered");
        let last_left = text.rfind("GAUCHE").expect("left column missing");
        let first_right = text.find("DROITE").expect("right column missing");
        assert!(last_left < first_right, "columns interleaved:\n{text}");
        assert!(text.contains("TITRE"), "header lost:\n{text}");
    }

    /// A horizontal cut alone must never license a rewrite. Ordinary prose has
    /// paragraph gaps, they split the page into bands, and none of that is
    /// evidence of columns — only a provable gutter is.
    #[test]
    fn a_paragraph_gap_alone_never_makes_a_page_columnar() {
        let mut stream = String::from("BT /F1 9 Tf\n");
        for block in 0..3 {
            for i in 0..6 {
                let y = 740 - block * 120 - i * 12;
                stream.push_str(&format!(
                    "1 0 0 1 60 {y} Tm (ligne de prose ordinaire numero {block}{i} sur la largeur) Tj\n"
                ));
            }
        }
        stream.push_str("ET\n");
        let pdf = crate::docs::tests::make_pdf(&stream);
        let pages = reordered_pages(&pdf).expect("layout");
        assert!(pages[0].is_none(), "single-column page was reordered");
    }

    /// The merge test has to recognise a row sweep, and — the half that matters
    /// far more — must not accuse a correct column-order reading.
    #[test]
    fn a_row_swept_rendering_is_recognised_and_a_correct_one_is_not() {
        let pdf = mixed_header_and_columns();
        let orders = page_orders(&pdf).expect("layout");
        let o = &orders[0];
        assert!(o.columnar, "page not seen as columnar");
        let swept: String = (1..=20)
            .map(|i| {
                format!(
                    "GAUCHE-{i:02} texte de la colonne de gauche DROITE-{i:02} texte de la colonne de droite\n"
                )
            })
            .collect();
        assert!(o.merged_in(&swept), "row sweep not recognised as merged");
        assert!(
            !o.merged_in(&o.text),
            "column-order reading wrongly flagged as merged:\n{}",
            o.text
        );
    }

    /// A title spanning the full text width, then a two-column body under it.
    fn mixed_header_and_columns() -> Vec<u8> {
        let mut stream = String::from("BT /F1 9 Tf\n");
        stream.push_str(
            "1 0 0 1 60 760 Tm (TITRE QUI TRAVERSE TOUTE LA LARGEUR DE LA PAGE ET LA GOUTTIERE) Tj\n",
        );
        for i in 1..=20 {
            let y = 700 - i * 12;
            stream.push_str(&format!(
                "1 0 0 1 60 {y} Tm (GAUCHE-{i:02} texte de la colonne de gauche) Tj\n\
                 1 0 0 1 320 {y} Tm (DROITE-{i:02} texte de la colonne de droite) Tj\n"
            ));
        }
        stream.push_str("ET\n");
        crate::docs::tests::make_pdf(&stream)
    }
}
