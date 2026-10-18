//! **A `.ppt` table**: a GROUP of shapes ([MS-PPT] §2.13 « Table »).
//!
//! The group's shape carries the `tableProperties` property (0x03A0); each
//! cell is a rectangle placed by its anchor in the group
//! (`OfficeArtChildAnchor`), the borders are segments (type 20). Read like
//! the other text boxes, the cells came out one per line — `Region`,
//! `Q1`, `Q2`, `EMEA`… — and which was worth what was no longer said.

use super::{atom_text, children, Rec, TEXT_BYTES, TEXT_CHARS};

const GROUP_SHAPE: u16 = 0xF004;
const SHAPE_HEADER: u16 = 0xF00A;
const CHILD_ANCHOR: u16 = 0xF00F;
const TEXTBOX: u16 = 0xF00D;
const LINE: u16 = 20;

fn i32_at(s: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_le_bytes(s.get(at..at + 4)?.try_into().ok()?))
}

/// Does the shape carry `tableProperties` in one of its property
/// tables (`OfficeArtFOPT` 0xF00B, tertiary 0xF122)?
fn is_table_frame(s: &[u8], shape: Rec) -> bool {
    children(s, shape)
        .iter()
        .filter(|c| matches!(c.kind, 0xF00B | 0xF122))
        .any(|opt| {
            (0..opt.instance as usize).any(|i| {
                s.get(opt.start + i * 6..opt.start + i * 6 + 2)
                    .and_then(|b| <[u8; 2]>::try_from(b).ok())
                    .is_some_and(|b| u16::from_le_bytes(b) & 0x3FFF == 0x03A0)
            })
        })
}

/// The table of a group as `|` rows, or `None` if the group is not one.
pub(super) fn table_text(s: &[u8], group: Rec) -> Option<String> {
    let shapes: Vec<Rec> = children(s, group)
        .into_iter()
        .filter(|c| c.kind == GROUP_SHAPE)
        .collect();
    if !is_table_frame(s, *shapes.first()?) {
        return None;
    }
    let mut cells: Vec<((i32, i32), String)> = Vec::new();
    for shape in shapes.iter().skip(1) {
        let kids = children(s, *shape);
        if kids
            .iter()
            .any(|k| k.kind == SHAPE_HEADER && k.instance == LINE)
        {
            continue;
        }
        let Some(anchor) = kids.iter().find(|k| k.kind == CHILD_ANCHOR) else {
            continue;
        };
        let (Some(left), Some(top)) = (i32_at(s, anchor.start), i32_at(s, anchor.start + 4)) else {
            continue;
        };
        let text = kids
            .iter()
            .filter(|k| k.kind == TEXTBOX)
            .flat_map(|tb| children(s, *tb))
            .filter(|k| matches!(k.kind, TEXT_CHARS | TEXT_BYTES))
            .map(|k| {
                atom_text(s, k)
                    .replace(['\n', '\u{2028}'], " ")
                    .trim()
                    .replace('|', "\\|")
            })
            .collect::<Vec<_>>()
            .join(" ");
        cells.push(((top, left), text));
    }
    let mut tops: Vec<i32> = cells.iter().map(|((t, _), _)| *t).collect();
    let mut lefts: Vec<i32> = cells.iter().map(|((_, l), _)| *l).collect();
    for v in [&mut tops, &mut lefts] {
        v.sort_unstable();
        v.dedup();
    }
    if tops.is_empty() || lefts.is_empty() {
        return None;
    }
    // A merged cell spans several columns: the others stay empty.
    let mut grid = vec![vec![String::new(); lefts.len()]; tops.len()];
    for ((top, left), text) in cells {
        if let (Ok(r), Ok(c)) = (tops.binary_search(&top), lefts.binary_search(&left)) {
            if let Some(cell) = grid.get_mut(r).and_then(|row| row.get_mut(c)) {
                *cell = text;
            }
        }
    }
    let mut out = String::new();
    for (i, row) in grid.iter().enumerate() {
        out.push_str(&format!("| {} |\n", row.join(" | ")));
        if i == 0 {
            out.push_str(&format!("|{}\n", " --- |".repeat(lefts.len())));
        }
    }
    Some(out.trim_end().to_string())
}
