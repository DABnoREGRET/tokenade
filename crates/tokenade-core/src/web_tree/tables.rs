//! **Tables.**
//!
//! A LAYOUT table is not a data table, and rendering it as one produces
//! columns that mean nothing. A cell's `|` is escaped rather than replaced: a
//! substituted character is a fabrication too.

use super::*;

// ---------------------------------------------------------------------------
// Tables — colspan/rowspan aware (P0.1). trafilatura's phantom-cell algorithm:
// a rowspan reserves its column(s) for N following rows; a colspan appends
// filler cells so later rows stay aligned.
// ---------------------------------------------------------------------------

pub(super) fn span_of(cell: NodeRef<Node>, key: &str) -> usize {
    attr(cell, key)
        .and_then(|v| v.trim().parse::<usize>().ok())
        .map(|n| n.clamp(1, 100))
        .unwrap_or(1)
}

/// Rows belonging to THIS table (descends through thead/tbody/tfoot, but not
/// into a nested table or into a cell's own content).
/// A table's caption, if it has one and it says something.
///
/// `<caption>` must be the FIRST child of the `<table>` per the
/// specification; we therefore do not descend further, and a nested table's
/// caption stays with its own table.
pub(super) fn table_caption(table: NodeRef<Node>) -> Option<String> {
    let cap = table.children().find(|c| el_name(*c) == Some("caption"))?;
    let text = one_line(inline(cap).trim());
    (!text.is_empty()).then_some(text)
}

pub(super) fn table_rows(table: NodeRef<Node>) -> Vec<NodeRef<Node>> {
    let mut rows = Vec::new();
    fn rec<'a>(n: NodeRef<'a, Node>, rows: &mut Vec<NodeRef<'a, Node>>) {
        for c in n.children() {
            match el_name(c) {
                Some("table") => {}         // nested table — not our rows
                Some("tr") => rows.push(c), // do NOT descend (its cells may nest tables)
                Some(_) => rec(c, rows),    // thead/tbody/tfoot/etc.
                None => {}
            }
        }
    }
    rec(table, &mut rows);
    rows
}

pub(super) fn row_cells(tr: NodeRef<Node>) -> Vec<NodeRef<Node>> {
    tr.children()
        .filter(|c| matches!(el_name(*c), Some("td") | Some("th")))
        .collect()
}

/// A LAYOUT `<table>`: no rows of its own, or else no `<th>` and never more
/// than ONE cell per row.
///
/// The old web builds its pages out of nested tables, and a single-column
/// table expresses no relation: there is no second column to relate the
/// first to. Rendering it as a markdown table costs two things, measured on
/// `pages/hn_thread`: `inline(cell)` FLATTENS the cell — the thread's two
/// comments, from two different people, arrived welded on a 1,029-character
/// line without a boundary between them — and the reader receives a
/// `| --- |` that ASSERTS a table where the page put none.
///
/// Both conditions matter, and the bound is the number of columns. A `<th>`
/// is a DECLARATION: the page itself says its columns mean something. And a
/// table without `<th>` with two or more columns is the ordinary case of a
/// real table written without a header — measured on this corpus,
/// `sportingnews_art` (8 promo rows) and `10000__natiga_qalubiaedu_org` (7
/// subjects and their grades) are exactly that, and they stay tables. A
/// broader rule was tried first — "a cell carries block content" — and the
/// ratchet REFUSED it: it also unfolds HN's comment tables, whose content
/// then falls under scenery sorts a cell never went through
/// (`is_boilerplate` does not see `inline`), and the page's structural
/// fidelity went from 0.891 to 0.796.
///
/// A table WITHOUT rows of its own is the same fact in another form: its
/// rows are in a nested table, and `table_to_md` would render
/// `inline(table)`, which flattens the whole subtree — the table was only a
/// frame.
pub(super) fn is_a_layout_table(table: NodeRef<Node>) -> bool {
    let rows = table_rows(table);
    if rows.is_empty() {
        // No rows of OUR OWN: `table_to_md` would render `inline(table)`,
        // which flattens the whole subtree — including the nested tables that
        // do carry the rows. The word "table" then describes nothing.
        return true;
    }
    let cells: Vec<NodeRef<Node>> = rows.iter().flat_map(|tr| row_cells(*tr)).collect();
    if cells.iter().any(|c| el_name(*c) == Some("th")) {
        return false;
    }
    rows.iter().all(|tr| row_cells(*tr).len() <= 1)
}

/// A cell's vertical bar, escaped the markdown way.
///
/// A bare bar would open a column: `| 0|1 | q |` is a three-column row under
/// a two-column header, and the row stops describing its table. Replacing it
/// with `¦` (U+00A6) kept the columns, but DELIVERED a character the page
/// does not contain — measured on `pages/hn_thread`, 17 times — and the bar
/// is precisely what a documentation page puts in a cell: `string | number`,
/// `cmd1 | cmd2`, `a|b`. An agent that copies `string ¦ number` writes wrong
/// code.
///
/// `\|` is the form markdown declares for this, including inside a cell's
/// code span, and it costs the same two bytes as `¦`.
pub(crate) fn escape_pipes(text: &str) -> String {
    if !text.contains('|') {
        return text.to_string();
    }
    text.replace('|', "\\|")
}

/// The mark that says what this figure carries — or nothing at all.
///
/// `<svg>` counts as an image: that is how a page draws a diagram. A video or
/// a sound is named for what it IS: two figures of `pages/wikipedia_art` are
/// `<video>`s (a talk, a build with Cargo) and announcing them as "image"
/// named the wrong thing, while saying nothing would leave their caption
/// floating with nothing explaining what it is the caption of. `video_link`
/// does not cover them: it only knows the `<iframe>`s of seven hosts.
///
/// `None` for everything else, and that is the heart of the rule: `<figure>`
/// is the container of content DETACHED from the text, not of an
/// illustration — a code listing, a quote, a table, a list.
pub(super) fn figure_marker(figure: NodeRef<Node>) -> Option<&'static str> {
    let mut found = None;
    for d in figure.descendants() {
        match el_name(d) {
            Some("img" | "picture" | "svg") => return Some("[image]"),
            Some("video") => found = found.or(Some("[video]")),
            Some("audio") => found = found.or(Some("[audio]")),
            _ => {}
        }
    }
    found
}

/// A cell's text, its own LINES kept: a `<br>` directly in the cell (`1 Main
/// St<br>Springfield`) and the items of a list in it come back joined by
/// `<br>`, the way a markdown table writes a line break in a cell — joined by
/// a space they read as one line (`a b`, 2026-10-04). Everything else is the
/// shared `inline` rendering.
fn cell_text(cell: NodeRef<Node>) -> String {
    let mut lines: Vec<String> = vec![String::new()];
    for c in cell.children() {
        match el_name(c) {
            Some("br") => lines.push(String::new()),
            Some("ul" | "ol") => {
                for li in c.children().filter(|l| el_name(*l) == Some("li")) {
                    lines.push(inline(li));
                }
                lines.push(String::new());
            }
            _ => {
                if let Some(line) = lines.last_mut() {
                    inline_into(c, line);
                }
            }
        }
    }
    lines
        .iter()
        .map(|l| l.trim().replace('\n', " "))
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("<br>")
}

pub(super) fn table_to_md(table: NodeRef<Node>) -> String {
    let rows = table_rows(table);
    if rows.is_empty() {
        return inline(table).trim().to_string();
    }
    let mut grid: Vec<Vec<String>> = Vec::new();
    // carry[col] = (rows still owed by an earlier rowspan, the text owed).
    //
    // **A spanned cell is READ in every row and column it covers.** Left
    // empty, `postgres` (rowspan 2) labelled its first row only: the second,
    // `| | 6 | 25 |`, belonged to no backend (crawl4ai, 2026-10-03). A short
    // label is repeated where it spans — what a spreadsheet's merge reads as;
    // a long cell (a note across rows) keeps the old empty phantom. A colspan
    // repeats only in a multi-row HEADER, where it names every column below.
    const SPREAD_MAX: usize = 40;
    let mut carry: Vec<(usize, String)> = Vec::new();
    let is_header_row = |tr: NodeRef<Node>| {
        let cells = row_cells(tr);
        !cells.is_empty() && cells.iter().all(|c| el_name(*c) == Some("th"))
    };
    // Repeating a header colspan only helps when header rows MERGE below.
    let lead = rows.iter().take_while(|tr| is_header_row(**tr)).count();
    for (ri, tr) in rows.iter().enumerate() {
        let header_row = lead >= 2 && ri < lead;
        let cells = row_cells(*tr);
        let mut row: Vec<String> = Vec::new();
        let mut col = 0usize;
        let mut ci = 0usize;
        loop {
            // Flush phantom cells owed to this column by a prior rowspan.
            while col < carry.len() && carry[col].0 > 0 {
                carry[col].0 -= 1;
                row.push(carry[col].1.clone());
                col += 1;
            }
            if ci >= cells.len() {
                break;
            }
            let cell = cells[ci];
            ci += 1;
            let cs = span_of(cell, "colspan");
            let rs = span_of(cell, "rowspan");
            let text = escape_pipes(&cell_text(cell));
            let short = text.chars().count() <= SPREAD_MAX;
            for k in 0..cs {
                while carry.len() <= col {
                    carry.push((0, String::new()));
                }
                let here = if k == 0 || (header_row && short) {
                    text.clone()
                } else {
                    String::new()
                };
                if rs > 1 {
                    carry[col] = (rs - 1, if short { here.clone() } else { String::new() });
                }
                row.push(here);
                col += 1;
            }
        }
        // Columns to the right still owed a phantom.
        while col < carry.len() {
            let owed = if carry[col].0 > 0 {
                carry[col].0 -= 1;
                carry[col].1.clone()
            } else {
                String::new()
            };
            row.push(owed);
            col += 1;
        }
        grid.push(row);
    }
    let ncols = grid.iter().map(|r| r.len()).max().unwrap_or(0);
    if ncols == 0 {
        return String::new();
    }
    for r in &mut grid {
        while r.len() < ncols {
            r.push(String::new());
        }
    }
    // Per-column alignment from the header row's `align=`/`text-align:` markup,
    // expanded by colspan — essence's four separator variants, same detector the
    // regex path uses (`web::cell_align_marker`), so the two converters agree.
    let aligns: Vec<&'static str> = {
        let mut a = Vec::new();
        for cell in row_cells(rows[0]) {
            let attrs = format!(
                r#"align="{}" style="{}""#,
                attr(cell, "align").unwrap_or(""),
                attr(cell, "style").unwrap_or(""),
            );
            let marker = crate::web::cell_align_marker(&attrs);
            for _ in 0..span_of(cell, "colspan") {
                a.push(marker);
            }
        }
        while a.len() < ncols {
            a.push("---");
        }
        a.truncate(ncols);
        a
    };
    // **Several header rows are ONE header.** Markdown has a single header
    // row; the second `<th>` row (`p50 | p99` under `Latency`) used to land in
    // the body, and the columns were never named. The leading all-`<th>` rows
    // merge column by column, each distinct label once: `Latency p50`.
    let lead = lead.min(grid.len());
    if lead >= 2 {
        let merged: Vec<String> = (0..ncols)
            .map(|c| {
                let mut parts: Vec<&str> = Vec::new();
                for r in &grid[..lead] {
                    let v = r[c].trim();
                    if !v.is_empty() && parts.last() != Some(&v) {
                        parts.push(v);
                    }
                }
                parts.join(" ")
            })
            .collect();
        grid.splice(..lead, [merged]);
    }
    // First row is the header (matches the regex converter's convention).
    let mut md = String::new();
    for (i, r) in grid.iter().enumerate() {
        md.push_str("| ");
        md.push_str(&r.join(" | "));
        md.push_str(" |\n");
        if i == 0 {
            md.push_str("| ");
            md.push_str(&aligns.join(" | "));
            md.push_str(" |\n");
        }
    }
    md
}
