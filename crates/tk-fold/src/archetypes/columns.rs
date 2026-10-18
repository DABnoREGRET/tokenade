//! **Constant columns, and trailing whitespace.**
//!
//! A column whose every row carries the same value says nothing N
//! times: it is pulled out once at the top. Trailing whitespace, for its part,
//! never says anything at all.

use super::*;

/// Fold decorative padding, SEGMENT by segment, and only return something
/// if at least one segment let itself be folded.
///
/// **Column decoration, by the BETTER of the two readings.**
///
/// There are two, of different strengths, and for a long time only one was the
/// universal rung:
///
/// * per SEGMENT separated by a blank line — stronger on a document carrying
///   several tables of different geometries, where the whole looks like
///   nothing;
/// * on the WHOLE DOCUMENT — stronger when a table is cut by
///   blank lines and no piece alone reaches the five column
///   rows required.
///
/// MEASURED 2026-09-09: the `netstat` compactor's output, 4,299 bytes,
/// qualified NO segment and qualified as a whole. It therefore went out as
/// is, and the NEXT pass recognised it as `AlignedBlock` and
/// returned it at **2,021** — the gain arrived one round too late, and the fixed point
/// was lost in between (`command_compaction_reaches_a_fixed_point`).
///
/// One single spelling of the question, which both routes go through: the
/// `auto` route, which recognises an output by its content, and the second stage of
/// delivery, which receives it routed by its command name.
/// **A column identical on every row is said ONCE.**
///
/// macOS `top -l1` returns thirty-six columns per process, and twelve of
/// them carry the same value on every row: `%CPU 0.0`,
/// `%CPU_ME 0.00000`, `INSTRS 0`, `#MREGS N/A`, `VSIZE N/A`… MEASURED 2026-09-09
/// on the corpus capture: **1,144 of the 5,032 bytes delivered (23 %)** are
/// twelve values copied twenty-two times.
///
/// Nothing disappears: the note names the column AND its value, so the row can be
/// read back. That is why the fold REQUIRES a header — without a column name the
/// note would say "the 27th is 0", which no reader can use.
///
/// Only applies to a table already separated by a SINGLE space: alignment
/// decoration goes first (see [`collapse_padding_best`]), and removing a
/// column from a still-aligned table would require re-spacing all of it —
/// a second mechanism, for a gain the first one already took.
pub fn fold_constant_columns(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let (start, count) = widest_single_spaced_table(&lines)?;
    let header: Vec<&str> = lines[start].split(' ').collect();
    let cols = header.len();
    let rows: Vec<Vec<&str>> = lines[start + 1..start + 1 + count]
        .iter()
        .map(|l| l.split(' ').collect())
        .collect();
    // The first column is the row's KEY (a PID, a name): even when
    // constant, removing it would leave rows that can no longer be told apart.
    // **Nothing proved that the first line IS a header**, and the
    // comment above already required it. On a `mount` listing without a
    // header, the note came out saying `on on · type type` — the name and the
    // value being the same cell of a DATA row. A familiar class of bug:
    // the promise was in the comment, not in the code.
    //
    // Two checks, and both are needed: a data row starts with
    // a value (a PID, a path), never with a column name; and a column's
    // name cannot be its own constant value.
    if header[0]
        .chars()
        .next()
        .is_none_or(|c| !c.is_ascii_alphabetic())
    {
        return None;
    }
    let constant: Vec<usize> = (1..cols)
        .filter(|&c| {
            rows.iter()
                .map(|r| r[c])
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                == 1
        })
        .collect();
    if constant.is_empty() || constant.iter().any(|&c| header[c] == rows[0][c]) {
        return None;
    }
    let note = format!(
        "[same on every row: {}]",
        constant
            .iter()
            .map(|&c| format!("{} {}", header[c], rows[0][c]))
            .collect::<Vec<_>>()
            .join(" · ")
    );
    // A MASK, not a list lookup: `constant.contains(i)` for
    // every cell of every row is quadratic in the number of columns, and
    // a six-thousand-column table carries six thousand constant ones.
    let mut drop_col = vec![false; cols];
    for &c in &constant {
        drop_col[c] = true;
    }
    let keep = |cells: &[&str]| {
        cells
            .iter()
            .enumerate()
            .filter(|(i, _)| !drop_col.get(*i).copied().unwrap_or(false))
            .map(|(_, v)| *v)
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut out = String::with_capacity(text.len());
    for (i, line) in lines.iter().enumerate() {
        if i == start {
            out.push_str(&note);
            out.push('\n');
            out.push_str(&keep(&header));
        } else if i > start && i <= start + count {
            out.push_str(&keep(&rows[i - start - 1]));
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    if text.ends_with('\n') {
        // `lines()` ate the last newline; we just put it back.
    } else {
        out.pop();
    }
    (out.len() < text.len()).then_some(out)
}

/// The longest single-spaced table: a header and its rows, all
/// split into the same number of cells.
pub(super) fn widest_single_spaced_table(lines: &[&str]) -> Option<(usize, usize)> {
    // Six columns at least: below that, a constant column weighs nothing and
    // the note costs more than it gives back. Four rows at least: "identical
    // everywhere" over three rows is a coincidence, not a property.
    const MIN_COLS: usize = 6;
    const MIN_ROWS: usize = 4;
    let cells = |l: &str| (!l.contains("  ") && !l.trim().is_empty()).then(|| l.split(' ').count());
    let mut best: Option<(usize, usize)> = None;
    let mut i = 0;
    while i < lines.len() {
        let Some(c) = cells(lines[i]).filter(|c| *c >= MIN_COLS) else {
            i += 1;
            continue;
        };
        let n = lines[i + 1..]
            .iter()
            .take_while(|l| cells(l) == Some(c))
            .count();
        if n >= MIN_ROWS && best.is_none_or(|(_, b)| n > b) {
            best = Some((i, n));
        }
        i += n + 1;
    }
    best
}

/// **The END of a line can carry nothing positional.**
///
/// The decoration fold handles INTERIOR space, and it has good reasons to
/// refuse often: an empty cell in the middle of a table says something
/// by its POSITION, a multi-level indent says nesting. Those two
/// refusals took the end-of-line trim down with them, which can be
/// concerned by neither: after the last character, there is no
/// next column left to shift nor level to lose.
///
/// MEASURED 2026-09-09: `Get-HotFix` is refused by `no_interior_cell_is_empty`
/// — one row has no `InstalledBy` — and goes out whole with its **100 bytes
/// of trailing whitespace out of 1,119**. It is not alone: **55,865 bytes** of the fixture
/// corpus are trailing whitespace, across 58 files, up to
/// **65 %** of a PowerShell output.
///
/// **Three spaces minimum, and that is deliberate.** Markdown gives meaning to
/// EXACTLY two trailing spaces — a hard line break — and it is the
/// only meaning trailing space carries in a format an agent may see
/// pass through here (`wrap cat README.md` comes through this gate). The guard costs
/// 910 bytes of the 55,865, i.e. 1.6 % of the deposit, so as never to rewrite
/// a document whose formatting depends on what is removed.
pub(super) fn trim_line_ends(text: &str) -> Option<String> {
    const MIN_TRAILING: usize = 3;
    let mut out = String::with_capacity(text.len());
    let mut changed = false;
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let trimmed = line.trim_end_matches([' ', '\t']);
        if line.len() - trimmed.len() >= MIN_TRAILING {
            changed = true;
            out.push_str(trimmed);
        } else {
            out.push_str(line);
        }
    }
    changed.then_some(out)
}

pub fn collapse_padding_best(text: &str) -> Option<String> {
    let by_segment = collapse_decorative_segments(text);
    let whole = padding_is_decorative(text).then(|| collapse_alignment_padding(text));
    // The end-of-line trim ALWAYS competes: it is the floor the other
    // two took down with them when they refused.
    let ends = trim_line_ends(text);
    let best = [by_segment, whole, ends]
        .into_iter()
        .flatten()
        .filter(|c| c.len() < text.len())
        .min_by_key(String::len);
    // Then the CONSTANT columns, on the table once de-aligned — the
    // two folds follow each other, and the second only gets a grip after the first.
    // A caller taking only one would have the second needlessly
    // missing, which is an earlier mistake made again.
    let after = best.clone().unwrap_or_else(|| text.to_string());
    match fold_constant_columns(&after) {
        Some(f) if f.len() < after.len() => Some(f),
        _ => best,
    }
}

/// A document can carry SEVERAL tables of different geometries:
/// `netstat -nr` writes `Routing tables`, then a five-column `Internet:`
/// table, then an `Internet6:` table whose columns fall elsewhere.
/// Judged as one block, it looks like nothing — the rows of the second table
/// measured against the columns of the first have holes everywhere, and the
/// fold was refused on a document of which **57 % of the bytes** are
/// padding.
///
/// A blank line separates: it is the only structure a document carries in
/// every language, and it is already what separates the records that
/// [`collapse_alignment_padding`] preserves.
///
/// Returning `Option` rather than an always-full `String` removes the DOUBLE
/// decision: there is no longer a predicate and a transformation that must
/// agree, but a single function that decides by acting.
pub fn collapse_decorative_segments(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut folded_any = false;
    let mut segment: Vec<&str> = Vec::new();

    // Segments are returned in order, separators included, so the
    // document keeps its shape: only the qualified blocks lose their
    // padding.
    let flush = |seg: &mut Vec<&str>, out: &mut String, folded_any: &mut bool| {
        if seg.is_empty() {
            return;
        }
        let body = seg.join("\n");
        if padding_is_decorative(&body) {
            out.push_str(&collapse_alignment_padding(&body));
            *folded_any = true;
        } else {
            out.push_str(&body);
        }
        out.push('\n');
        seg.clear();
    };

    for line in text.lines() {
        if line.trim().is_empty() {
            flush(&mut segment, &mut out, &mut folded_any);
            out.push('\n');
        } else {
            segment.push(line);
        }
    }
    flush(&mut segment, &mut out, &mut folded_any);

    folded_any.then(|| out.trim_end().to_string())
}
