//! **The style of each paragraph of a `.doc`** ([MS-DOC] §2.4.6 and §2.9.271).
//!
//! A paragraph carries its properties in a PAPX, stored in 512-byte pages
//! (`PapxFkp`) that the table stream's `PlcBtePapx` indexes by FILE position
//! (FC), not character position: a paragraph's position is converted through
//! the piece table. The first two bytes of a PAPX name its style (`istd`); the
//! style sheet (`STSH`) says which one is a heading — by its built-in
//! identifier (`sti` 1–9 = Heading 1–9) or by its name (`Title`, a custom style
//! `Titre 2`…).

use super::super::{heading_level_from_style, md_hashes};
use super::lists::{para_sprms, Lists};
use std::collections::HashMap;

/// A piece of the text: its character positions `[cp, cp_end)`, the byte where
/// it starts in `WordDocument`, and whether it uses one byte per character.
#[derive(Clone, Copy)]
pub(super) struct Piece {
    pub cp: usize,
    pub cp_end: usize,
    pub fc: usize,
    pub narrow: bool,
}

fn u16_at(b: &[u8], o: usize) -> Option<usize> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?) as usize)
}

fn u32_at(b: &[u8], o: usize) -> Option<usize> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?) as usize)
}

/// What a style says about its paragraphs.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Style {
    /// Heading level (`None`: not a heading).
    pub heading: Option<usize>,
    /// List depth of the built-in styles `List Bullet 2`, `List Number 3`…:
    /// their list is at level 0, only its indent says it is deeper (the same
    /// rule as `.docx`).
    pub list_depth: Option<usize>,
}

/// Each style, indexed by `istd`.
pub(super) fn heading_styles(stsh: &[u8]) -> Vec<Style> {
    let mut levels = Vec::new();
    let (Some(cb_stshi), Some(cstd), Some(cb_base)) =
        (u16_at(stsh, 0), u16_at(stsh, 2), u16_at(stsh, 4))
    else {
        return levels;
    };
    let mut at = 2 + cb_stshi;
    for _ in 0..cstd {
        let Some(cb) = u16_at(stsh, at) else { break };
        let std = stsh.get(at + 2..at + 2 + cb).unwrap_or(&[]);
        at += 2 + cb;
        if std.is_empty() {
            levels.push(Style::default());
            continue;
        }
        let sti = u16_at(std, 0).unwrap_or(0) & 0x0FFF;
        let name = u16_at(std, cb_base).and_then(|n| {
            let raw = std.get(cb_base + 2..cb_base + 2 + n * 2)?;
            let units = raw
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]));
            Some(
                char::decode_utf16(units)
                    .map(|c| c.unwrap_or('\u{FFFD}'))
                    .collect::<String>(),
            )
        });
        let heading = if (1..=9).contains(&sti) {
            Some(sti)
        } else {
            name.as_deref().and_then(heading_level_from_style)
        };
        let list_depth = name.as_deref().and_then(|n| {
            let n = n
                .strip_prefix("List Bullet ")
                .or_else(|| n.strip_prefix("List Number "))?;
            n.parse::<usize>()
                .ok()
                .filter(|d| (2..=9).contains(d))
                .map(|d| d - 1)
        });
        levels.push(Style {
            heading,
            list_depth,
        });
    }
    levels
}

/// The properties of the paragraph whose end mark is at byte `fc`: its style
/// (`istd`) and the sequence of its other properties (`grpprl`).
fn papx_at<'a>(word: &'a [u8], plc: &[u8], fc: usize) -> Option<(usize, &'a [u8])> {
    let n = plc.len().checked_sub(4)? / 8;
    let k = (0..n).find(|&k| {
        u32_at(plc, k * 4).is_some_and(|a| a <= fc)
            && u32_at(plc, k * 4 + 4).is_some_and(|b| fc < b)
    })?;
    let pn = u32_at(plc, (n + 1) * 4 + k * 4)? & 0x003F_FFFF;
    let page = word.get(pn * 512..pn * 512 + 512)?;
    let crun = *page.get(511)? as usize;
    let run = (0..crun).find(|&i| {
        u32_at(page, i * 4).is_some_and(|a| a <= fc)
            && u32_at(page, i * 4 + 4).is_some_and(|b| fc < b)
    })?;
    let b_offset = *page.get(4 * (crun + 1) + run * 13)? as usize * 2;
    if b_offset == 0 {
        return Some((0, &[]));
    }
    // PapxInFkp: `cb` ≠ 0 ⇒ 2·cb − 1 bytes follow; `cb` = 0 ⇒ a second byte
    // `cb'` and 2·cb' bytes.
    let (start, len) = match *page.get(b_offset)? as usize {
        0 => (b_offset + 2, *page.get(b_offset + 1)? as usize * 2),
        cb => (b_offset + 1, cb * 2 - 1),
    };
    let props = page.get(start..start + len)?;
    Some((u16_at(props, 0)?, props.get(2..).unwrap_or(&[])))
}

/// A body paragraph, by the position of its end mark (`\r`): what precedes
/// it, and whether it is in a table cell.
pub(super) struct Mark {
    pub cp: usize,
    pub prefix: String,
    pub in_table: bool,
    /// On a ROW end mark (`\x07`): the edges of its cells.
    pub row_edges: Vec<i64>,
}

/// The `#`s of a heading, or the indent and marker of a list item (`- `,
/// `3. `), numbered the way Word counts — per list and per level, a deeper
/// level restarting under each new item.
pub(super) fn paragraph_marks(
    word: &[u8],
    plc: &[u8],
    styles: &[Style],
    lists: &Lists,
    pieces: &[Piece],
    chars: &[char],
) -> Vec<Mark> {
    let mut out = Vec::new();
    let mut counters: HashMap<i32, Vec<i32>> = HashMap::new();
    for (cp, &mark) in chars
        .iter()
        .enumerate()
        .filter(|(_, c)| matches!(**c, '\r' | '\u{7}'))
    {
        let Some(p) = pieces.iter().find(|p| p.cp <= cp && cp < p.cp_end) else {
            continue;
        };
        let fc = p.fc + (cp - p.cp) * if p.narrow { 1 } else { 2 };
        let Some((istd, grpprl)) = papx_at(word, plc, fc) else {
            continue;
        };
        let sprms = para_sprms(grpprl);
        // A cell or row mark: only a row's grid counts.
        if mark == '\u{7}' {
            if !sprms.cell_edges.is_empty() {
                out.push(Mark {
                    cp,
                    prefix: String::new(),
                    in_table: true,
                    row_edges: sprms.cell_edges,
                });
            }
            continue;
        }
        let style = styles.get(istd).copied().unwrap_or_default();
        let in_table = sprms.in_table;
        if let Some(level) = style.heading {
            out.push(Mark {
                cp,
                prefix: md_hashes(level),
                in_table,
                row_edges: Vec::new(),
            });
            continue;
        }
        let Some(ilfo) = sprms.ilfo else {
            if in_table {
                out.push(Mark {
                    cp,
                    prefix: String::new(),
                    in_table,
                    row_edges: Vec::new(),
                });
            }
            continue;
        };
        // The level in ITS list gives the format and the count; the indent is never
        // shallower than what the style name shows: LibreOffice writes `ilvl 0` on a
        // `List Bullet 2`.
        let ilvl = sprms.ilvl.unwrap_or(0).min(8);
        let shown = ilvl.max(style.list_depth.unwrap_or(0));
        let Some((lsid, level)) = lists.level(ilfo, ilvl) else {
            continue;
        };
        let count = counters.entry(lsid).or_insert_with(|| vec![0; 9]);
        count.iter_mut().skip(ilvl + 1).for_each(|c| *c = 0);
        let n = match count.get_mut(ilvl) {
            Some(c) if *c == 0 => {
                *c = level.start;
                *c
            }
            Some(c) => {
                *c += 1;
                *c
            }
            None => level.start,
        };
        let marker = if level.ordered {
            super::super::markup_marks::label(Some(&level.style), n.max(0) as usize)
        } else {
            "- ".to_string()
        };
        out.push(Mark {
            cp,
            prefix: format!("{}{marker}", "  ".repeat(shown)),
            in_table,
            row_edges: Vec::new(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One STD: its `sti` (built-in identity) and its name, after a 10-byte base.
    fn std(sti: u16, name: &str) -> Vec<u8> {
        let mut b = vec![0u8; 10];
        b[0..2].copy_from_slice(&sti.to_le_bytes());
        b.extend((name.encode_utf16().count() as u16).to_le_bytes());
        b.extend(name.encode_utf16().flat_map(u16::to_le_bytes));
        b.extend([0, 0]);
        let mut out = (b.len() as u16).to_le_bytes().to_vec();
        out.extend(b);
        out
    }

    /// The built-in identity says "heading" whatever the name is written in;
    /// a custom style is judged by its name; an empty slot keeps its index.
    #[test]
    fn a_style_is_a_heading_by_its_identity_or_its_name() {
        let mut stsh = vec![4, 0, 4, 0, 10, 0];
        stsh.extend(std(0, "Normal"));
        stsh.extend(std(2, "Kapitel 2"));
        stsh.extend([0, 0]);
        stsh.extend(std(0x0FFE, "Title"));
        stsh.extend(std(0x0FFE, "List Bullet 2"));
        stsh[2] = 5;
        let heading: Vec<_> = heading_styles(&stsh).iter().map(|s| s.heading).collect();
        assert_eq!(heading, [None, Some(2), None, Some(1), None]);
        assert_eq!(heading_styles(&stsh)[4].list_depth, Some(1));
    }
}
