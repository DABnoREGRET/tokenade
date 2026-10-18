//! Telling a real table apart from a layout grid.
//!
//! An extractor emits pipes wherever text lined up in columns, so a two-column
//! page layout comes back as a table with no data in it. These score a
//! candidate block and demote the ones that are only alignment.

use super::*;

/// What a table cell holds. Numeric and Date score IDENTICALLY (see
/// [`column_purity`]) — the boundary between them is deliberately not load
/// bearing, so a `499 / 521` read as a date rather than a number changes
/// nothing. They are separate variants because a column that is uniformly
/// EITHER is evidence of a data table, and lumping them into "not text" would
/// let a column of mixed dates and numbers score as pure.
#[derive(PartialEq, Clone, Copy)]
pub(super) enum CellKind {
    Empty,
    Numeric,
    Date,
    ShortText,
    LongText,
}

pub(super) fn classify_cell(cell: &str) -> CellKind {
    // Layout analysis bolds cells it read as headers; the emphasis is markup we
    // added, so it must not decide the cell's type.
    let c = cell.trim().trim_matches('*').trim();
    if c.is_empty() {
        return CellKind::Empty;
    }
    let has_digit = c.chars().any(|ch| ch.is_ascii_digit());
    let has_alpha = c.chars().any(char::is_alphabetic);
    if has_digit && !has_alpha {
        // "0%", "-2.455%", "499 / 521 (95.77%)" — digits and punctuation only.
        return CellKind::Numeric;
    }
    if has_digit && is_datelike(c) {
        return CellKind::Date;
    }
    if c.split_whitespace().count() <= TABLE_LABEL_MAX_WORDS {
        CellKind::ShortText
    } else {
        CellKind::LongText
    }
}

/// A month name or an ISO-ish `d+[-/.]d+` run alongside a digit. Deliberately
/// shallow: see [`CellKind`] for why the numeric/date boundary carries no
/// weight in the score.
pub(super) fn is_datelike(c: &str) -> bool {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let lower = c.to_ascii_lowercase();
    if MONTHS.iter().any(|m| lower.contains(m)) {
        return true;
    }
    let b = c.as_bytes();
    b.windows(3).any(|w| {
        w[0].is_ascii_digit() && matches!(w[1], b'-' | b'/' | b'.') && w[2].is_ascii_digit()
    })
}

/// How much a column looks like ONE kind of thing.
///
/// A data table's columns are type-homogeneous; a paragraph or a title sliced
/// into cells is not. The asymmetry between the returned values is the point
/// and is not arbitrary: a column of uniform NUMBERS or DATES is positive
/// evidence of tabulation, because prose does not produce one by accident,
/// while a column of uniform SHORT TEXT is only weak evidence — a label column
/// and a chopped-up heading look identical at that resolution. That asymmetry
/// is what separates the real one-body-row table in `pdf_mid.pdf`
/// (`| benign sequences | 499 / 521 (95.77%) |`, one text column and one
/// numeric, 0.80) from the section title (`| 5 | THE EMBEDDING | ANTICHAIN |`,
/// three text columns, 0.60) — MEASURED 2026-07-22. Row count cannot separate
/// them: both hold a single body row.
pub(super) fn column_purity(kinds: &[CellKind]) -> f64 {
    let present: Vec<CellKind> = kinds
        .iter()
        .copied()
        .filter(|k| *k != CellKind::Empty)
        .collect();
    // A column with nothing in it is not a column. Scoring it 0 rather than
    // skipping it is what catches the ECMA-262 cover, whose leftmost "column"
    // is pure page margin.
    if present.is_empty() {
        return 0.0;
    }
    let dominant = [
        CellKind::Numeric,
        CellKind::Date,
        CellKind::ShortText,
        CellKind::LongText,
    ]
    .into_iter()
    .map(|k| (k, present.iter().filter(|c| **c == k).count()))
    .max_by_key(|(_, n)| *n)
    .expect("four kinds are always enumerated");
    let share = dominant.1 as f64 / present.len() as f64;
    let ceiling = match dominant.0 {
        CellKind::Numeric | CellKind::Date => 1.0,
        CellKind::ShortText => 0.6,
        // A column of sentences is a paragraph that acquired pipes.
        CellKind::LongText => 0.2,
        CellKind::Empty => 0.0,
    };
    share * ceiling
}

/// One candidate table, cells already split.
pub(super) struct TableBlock {
    header: Vec<String>,
    body: Vec<Vec<String>>,
}

/// Split a markdown table row into trimmed cells.
pub(super) fn split_row(row: &str) -> Vec<String> {
    row.trim()
        .trim_matches('|')
        .split('|')
        .map(|c| c.trim().to_string())
        .collect()
}

/// Is this the `|---|---|` structure row rather than data?
pub(super) fn is_separator_row(line: &str) -> bool {
    !line
        .chars()
        .any(|c| c != '|' && c != '-' && c != ' ' && c != ':')
}

/// Collect the `|`-prefixed lines of `md` into one candidate table.
pub(super) fn parse_table_block(md: &str) -> Option<TableBlock> {
    let rows: Vec<Vec<String>> = md
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with('|'))
        .filter(|l| !is_separator_row(l))
        .map(split_row)
        .collect();
    if rows.len() < TABLE_MIN_ROWS {
        return None;
    }
    let mut it = rows.into_iter();
    let header = it.next()?;
    Some(TableBlock {
        header,
        body: it.collect(),
    })
}

/// Does this candidate table hold DATA, or is it prose that layout analysis
/// sliced into cells?
///
/// Ported in spirit from marker's `processors/table_recon.py` (Apache-2.0),
/// which carries the attribution "Ported from the datalab pdftext_backfill
/// task". MODIFIED: marker's processor both reconstructs a grid and judges it;
/// pdf_oxide already reconstructs ours, so only the JUDGE is taken, and its
/// axes are re-weighted against the fixtures named below rather than marker's.
///
/// Four axes, weighted (weights chosen so that no single axis can by itself
/// condemn a table that the other three vouch for):
///
/// - **column type purity** (0.50) — see [`column_purity`]. Carries the most
///   weight because it is the only axis that fires on a table whose cells are
///   all populated and rectangular, which is exactly the failure we are here
///   for: a title chopped into columns is a perfectly well-formed grid.
/// - **fill ratio** (0.25) — share of body cells that are non-empty. Catches
///   the positioning grid, whose cells are mostly page margin.
/// - **header/row width agreement** (0.10) — share of body rows whose cell
///   count matches the header's. Low weight: pdf_oxide emits rectangular
///   markdown almost always, so this axis is usually a constant 1.0 and only
///   earns its keep on a torn table.
/// - **compound-cell penalty** (0.15) — share of cells that are NOT one cell's
///   worth of content. See [`compound_cell_share`].
///
/// MEASURED 2026-07-22 on the audit fixtures, every table block emitted:
///
/// | block                                            | score | verdict  |
/// |--------------------------------------------------|-------|----------|
/// | `pdf_mid` `\| 4 \| 22 \| 17 \|`                   | 1.000 | real     |
/// | `pdf_mid` `\| Cluster No \| 0 \| 1 \| 4 \|`       | 0.950 | real     |
/// | `pdf_mid` `\| benign sequences \| 499 / 521 \|`   | 0.900 | real     |
/// | `pdf_large` `\| in.yj \| in.ye & in.yt \|`        | 0.800 | SPURIOUS |
/// | `pdf_mid` `\| 5 \| THE EMBEDDING \| ANTICHAIN \|` | 0.717 | SPURIOUS |
/// | `pdf_large` `\|  \| L2 \|  \| L2 \|`              | 0.525 | SPURIOUS |
///
/// `pdf_small.pdf` emits no tables at all, so it constrains nothing here.
///
/// KNOWN MISS, stated rather than papered over: the `in.yj` block is a wiring
/// DIAGRAM's terminal labels and scores 0.80, above the threshold. It is a
/// full, rectangular, type-homogeneous 2-column grid of short text — which is
/// also an exact description of a legitimate two-column glossary. No weighting
/// of these four axes separates them, and inventing a fifth axis that happens
/// to split this one fixture would be fitting to the fixture. It is left
/// emitted as a table.
pub(super) fn table_confidence(block: &TableBlock) -> f64 {
    let width = block.header.len();
    if width == 0 || block.body.is_empty() {
        return 0.0;
    }
    // Purity is measured over the BODY only. The header row is text by
    // construction — column names are words — so including it drags every
    // numeric column toward "mixed" and scores real tables as false ones.
    let purity: f64 = (0..width)
        .map(|c| {
            let col: Vec<CellKind> = block
                .body
                .iter()
                .map(|r| classify_cell(r.get(c).map_or("", |s| s)))
                .collect();
            column_purity(&col)
        })
        .sum::<f64>()
        / width as f64;

    let cells: Vec<&String> = block.body.iter().flatten().collect();
    let fill = if cells.is_empty() {
        0.0
    } else {
        cells.iter().filter(|c| !c.trim().is_empty()).count() as f64 / cells.len() as f64
    };

    let agreement =
        block.body.iter().filter(|r| r.len() == width).count() as f64 / block.body.len() as f64;

    let compound = 1.0 - compound_cell_share(&cells);

    0.50 * purity + 0.25 * fill + 0.10 * agreement + 0.15 * compound
}

/// Share of cells holding what should have been SEVERAL cells.
///
/// Two signatures, both of which mean the same failure — a column boundary the
/// reconstruction missed:
///
/// - an internal run of 2+ spaces, i.e. the visual gutter survived INSIDE the
///   cell because no pipe was placed at it;
/// - a cell several times wider than the table's typical cell, which is how a
///   whole row collapses into its first column.
///
/// Both are measured against the block itself rather than an absolute width,
/// so a table of long labels is not penalised for being uniformly long.
pub(super) fn compound_cell_share(cells: &[&String]) -> f64 {
    if cells.is_empty() {
        return 0.0;
    }
    let words = |s: &str| s.split_whitespace().count();
    let mut widths: Vec<usize> = cells.iter().map(|c| words(c)).filter(|w| *w > 0).collect();
    if widths.is_empty() {
        return 0.0;
    }
    widths.sort_unstable();
    let median = widths[widths.len() / 2];
    // A cell is "several cells" only when the table has a typical width to be
    // several TIMES. Guard the multiplier so a table of one-word cells does not
    // brand every three-word cell compound.
    let outlier = (median * 4).max(6);
    cells
        .iter()
        .filter(|c| c.contains("  ") || words(c) >= outlier)
        .count() as f64
        / cells.len() as f64
}

/// Is this page's markdown a LAYOUT grid dressed as a table?
///
/// The PAGE-level policy over [`table_confidence`]: when a page's table
/// markdown as a whole fails the judge, the page is re-read with the plain
/// extractor instead. That remedy is why this stays a separate entry point
/// from [`demote_false_tables`] rather than being folded into it — it fires
/// EARLIER, while the engine's output for the page is still being chosen, and
/// its remedy is re-extraction of the whole page rather than demotion of one
/// block. Both ask the identical question and get it answered by the identical
/// scorer, so there is exactly one definition of "false table" in this file.
///
/// A title page comes back as a table whose cells are mostly empty. MEASURED
/// 2026-07-22: the ECMA-262 cover scores 0.628 against the eval corpus's real
/// tables at 0.90+, and falls back to plain text — which is what it produced
/// before layout analysis was switched on.
pub(super) fn looks_like_layout_grid(md: &str) -> bool {
    parse_table_block(md).is_some_and(|b| table_confidence(&b) < TABLE_MIN_CONFIDENCE)
}

/// Re-emit as ordinary text every table block on the page that fails the judge.
///
/// The BLOCK-level policy over [`table_confidence`]. It exists because
/// [`looks_like_layout_grid`] pools a whole page: MEASURED 2026-07-22 on
/// `pdf_large.pdf`, the half-empty diagram grid `|  | L2 |  | L2 |` scores 0.53
/// on its own but survives the page-level guard, because the page around it is
/// prose and two other tables, and pooling dilutes the block's emptiness far
/// below any threshold. Judging block by block is the only way that block is
/// reachable.
///
/// Demotion loses nothing: the cells' text is emitted in place, space-joined,
/// so a wrongly-demoted real table still carries every word it held. The rule
/// is "emit nothing rather than something false" — a false table tells an agent
/// that a heading is data, and that is a lie the text form cannot tell.
///
/// PDF ONLY, by construction of the caller: a `.docx` or `.xlsx` table comes
/// from actual table MARKUP in the file, so it is not a guess and must never be
/// second-guessed. Only layout analysis infers tables from geometry.
pub(super) fn demote_false_tables(page: &str) -> String {
    let lines: Vec<&str> = page.lines().collect();
    let mut out = String::with_capacity(page.len());
    let mut i = 0;
    while i < lines.len() {
        if !lines[i].trim_start().starts_with('|') {
            out.push_str(lines[i]);
            out.push('\n');
            i += 1;
            continue;
        }
        let start = i;
        while i < lines.len() && lines[i].trim_start().starts_with('|') {
            i += 1;
        }
        let block_lines = &lines[start..i];
        let block_md = block_lines.join("\n");
        let keep = parse_table_block(&block_md)
            .is_none_or(|b| table_confidence(&b) >= TABLE_MIN_CONFIDENCE);
        if keep {
            out.push_str(&block_md);
            out.push('\n');
        } else {
            for l in block_lines {
                if is_separator_row(l) {
                    continue;
                }
                let text = split_row(l).join(" ");
                let text = text.trim();
                if !text.is_empty() {
                    out.push_str(text);
                    out.push('\n');
                }
            }
        }
    }
    out
}

/// **A table INSIDE a cell is read inside its cell.** Splitting into tables,
/// rows and cells is done with non-greedy expressions: on a nested table, each
/// one stopped at the FIRST closing tag — the inner table's — and its rows
/// became rows of the outer table (`| In3 | In4 |` under
/// `| Outer A2 | In1 | In2 |`, docx and odt, 2026-10-04; the `.doc` of the same
/// file rendered `In1 In2 In3 In4` in the cell). Every table of depth ≥ 2
/// therefore loses its table, row and cell tags: its text stays, inline, in
/// the cell that contains it.
pub(super) fn flatten_nested_tables(xml: &str, schema: Schema) -> std::borrow::Cow<'_, str> {
    let (table, structure) = match schema {
        Schema::Ooxml => (
            r"<(/?)(?:w|a):tbl\b[^>]*?(/?)>",
            r"</?(?:w|a):(?:tbl|tr|tc)\b[^>]*>",
        ),
        // `\b` matches between `table` and `-row`: the name must end on a space, a
        // `/` or the `>`, otherwise every row counted as a table.
        Schema::Odf => (
            r"<(/?)table:table(?:\s[^>]*?)?(/?)>",
            r"</?table:(?:table|table-row|table-cell)(?:\s[^>]*)?/?>",
        ),
        Schema::Html => (
            r"(?i)<(/?)table\b[^>]*?(/?)>",
            r"(?i)</?(?:table|tr|td|th)\b[^>]*>",
        ),
    };
    let mut depth = 0usize;
    let mut nested: Vec<(usize, usize)> = Vec::new();
    let mut open_at = 0usize;
    for c in re(table).captures_iter(xml) {
        let (Some(m), closing, self_closing) = (c.get(0), &c[1] == "/", &c[2] == "/") else {
            continue;
        };
        if self_closing {
            continue;
        }
        if closing {
            if depth == 2 {
                nested.push((open_at, m.end()));
            }
            depth = depth.saturating_sub(1);
        } else {
            depth += 1;
            if depth == 2 {
                open_at = m.start();
            }
        }
    }
    if nested.is_empty() {
        return std::borrow::Cow::Borrowed(xml);
    }
    let mut out = String::with_capacity(xml.len());
    let mut last = 0;
    for (a, b) in nested {
        out.push_str(&xml[last..a]);
        out.push_str(&re(structure).replace_all(&xml[a..b], " "));
        last = b;
    }
    out.push_str(&xml[last..]);
    std::borrow::Cow::Owned(out)
}

/// **A cell merged across columns takes their places.** Each cell's right edge (RTF `\cellxN`, Word 97 `sprmTDefTable`); across the table those edges are the column grid. A
/// cell spanning two grid columns was ONE cell, so every cell after it moved
/// one column left — `TOTAL-NEEDLE` under `Q2` (2026-10-04, LibreOffice
/// export of a Word table merged mid-row). The span is followed by empty cells.
/// A row whose edges do not match its cells is left as it is.
pub(super) fn spread_merged_cells(rows: &mut [Vec<String>], edges: &[Vec<i64>]) {
    let mut grid: Vec<i64> = edges.iter().flatten().copied().collect();
    grid.sort_unstable();
    grid.dedup();
    for (row, row_edges) in rows.iter_mut().zip(edges) {
        if row_edges.len() != row.len() || row_edges.is_empty() {
            continue;
        }
        let mut spread = Vec::with_capacity(grid.len());
        let mut left = i64::MIN;
        for (cell, &right) in row.drain(..).zip(row_edges) {
            let span = grid
                .iter()
                .filter(|&&g| g > left && g <= right)
                .count()
                .max(1);
            spread.push(cell);
            spread.extend(std::iter::repeat_n(String::new(), span - 1));
            left = right;
        }
        *row = spread;
    }
}
