//! **Automatic numbering in a `.doc`** ([MS-DOC] §2.9.147 and following).
//!
//! A list paragraph carries two properties: `sprmPIlfo` (which list, a 1-based
//! index into the `PlfLfo`) and `sprmPIlvl` (which level). The `LFO` names its
//! list by `lsid`; the `PlfLst` describes each list and its levels (`LVL`),
//! whose number format `nfc` says bullet (23) or number, and `iStartAt` where
//! to start counting. The `LVL`s follow the `PlfLst` in the table stream,
//! outside its declared length.

use super::super::markup_marks::{NumKind, NumberStyle};
use std::collections::HashMap;

fn u16_at(b: &[u8], o: usize) -> Option<usize> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?) as usize)
}

fn i32_at(b: &[u8], o: usize) -> Option<i32> {
    Some(i32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

/// A list level: numbered (otherwise bullet), its first number, and what its
/// number DISPLAYS — `nfc` format and `xst` text (`a)`, `iv.`), instead of the
/// `1.` it rendered for every list (2026-10-04).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Level {
    pub ordered: bool,
    pub start: i32,
    pub style: NumberStyle,
}

#[derive(Default)]
pub(super) struct Lists {
    /// `lsid` of each `LFO`, in order (`ilfo` 1 is the first).
    lfo: Vec<i32>,
    levels: HashMap<i32, Vec<Level>>,
}

impl Lists {
    /// `plf_lst` starts at the beginning of the `PlfLst` and runs to the end of
    /// the table stream (its `LVL`s follow it).
    pub(super) fn read(plf_lst: &[u8], plf_lfo: &[u8]) -> Lists {
        let mut lists = Lists::default();
        let count = u16_at(plf_lst, 0).unwrap_or(0);
        let mut at = 2 + count * 28;
        for k in 0..count {
            let Some(lsid) = i32_at(plf_lst, 2 + k * 28) else {
                break;
            };
            let simple = plf_lst.get(2 + k * 28 + 26).is_some_and(|f| f & 1 != 0);
            let mut levels = Vec::new();
            for _ in 0..if simple { 1 } else { 9 } {
                let Some(lvlf) = plf_lst.get(at..at + 28) else {
                    break;
                };
                let start = i32_at(lvlf, 0).unwrap_or(1);
                // nfc 23: bullet; 255: no number.
                let ordered = !matches!(lvlf.get(4), Some(23 | 255));
                let byte = |i: usize| lvlf.get(i).copied().unwrap_or(0) as usize;
                let (chpx, papx) = (byte(24), byte(25));
                let xst = at + 28 + papx + chpx;
                let cch = u16_at(plf_lst, xst).unwrap_or(0);
                let units: Vec<u16> = plf_lst
                    .get(xst + 2..xst + 2 + cch * 2)
                    .unwrap_or(&[])
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                at = xst + 2 + cch * 2;
                let level_index = levels.len() as u16;
                levels.push(Level {
                    ordered,
                    start,
                    style: number_style(lvlf.get(4).copied(), &units, level_index, start),
                });
            }
            lists.levels.insert(lsid, levels);
        }
        let lfos = i32_at(plf_lfo, 0).map_or(0, |n| n.max(0) as usize);
        lists.lfo = (0..lfos)
            .map_while(|k| i32_at(plf_lfo, 4 + k * 16))
            .collect();
        lists
    }

    /// The list (`lsid`) and level of a paragraph with the given `ilfo`/`ilvl`.
    pub(super) fn level(&self, ilfo: usize, ilvl: usize) -> Option<(i32, Level)> {
        let lsid = *self.lfo.get(ilfo.checked_sub(1)?)?;
        let levels = self.levels.get(&lsid)?;
        Some((lsid, levels.get(ilvl).or(levels.first())?.clone()))
    }
}

/// A level's format: `nfc` (0 decimal, 1 and 2 roman, 3 and 4 letters) and
/// its text, where THIS level's number is the character with code `ilvl`. A
/// text that cites several levels (`1.1.`) keeps the default form.
fn number_style(nfc: Option<u8>, xst: &[u16], ilvl: u16, start: i32) -> NumberStyle {
    let kind = match nfc {
        Some(1) => NumKind::UpperRoman,
        Some(2) => NumKind::LowerRoman,
        Some(3) => NumKind::UpperAlpha,
        Some(4) => NumKind::LowerAlpha,
        _ => NumKind::Decimal,
    };
    let placeholders = xst.iter().filter(|u| **u < 9).count();
    let (prefix, suffix) = match xst.iter().position(|u| *u == ilvl) {
        Some(at) if placeholders == 1 => (
            String::from_utf16_lossy(&xst[..at]),
            String::from_utf16_lossy(&xst[at + 1..]),
        ),
        _ => (String::new(), ".".into()),
    };
    NumberStyle {
        kind,
        prefix,
        suffix,
        start: start.max(0) as usize,
        ..Default::default()
    }
}

/// What a paragraph's properties say about its place: its list
/// (`sprmPIlfo`), its level (`sprmPIlvl`), and whether it is in a table
/// (`sprmPFInTable`).
#[derive(Debug, Default, PartialEq)]
pub(super) struct ParaSprms {
    pub ilfo: Option<usize>,
    pub ilvl: Option<usize>,
    pub in_table: bool,
    /// Right edges of a row's cells (`sprmTDefTable`, on the row's end mark): the
    /// grid of a merged cell.
    pub cell_edges: Vec<i64>,
}

pub(super) fn para_sprms(grpprl: &[u8]) -> ParaSprms {
    let mut found = ParaSprms::default();
    let mut at = 0;
    while let Some(sprm) = u16_at(grpprl, at) {
        let operand = at + 2;
        let size = match sprm >> 13 {
            0 | 1 => 1,
            2 | 4 | 5 => 2,
            3 => 4,
            7 => 3,
            // sprmPChgTabs can exceed 255: nothing safely readable beyond it.
            _ if sprm == 0xC615 => break,
            // sprmTDefTable: length on TWO bytes, counting the rest of the operand
            // "plus one" ([MS-DOC] TDefTableOperand).
            _ if sprm == 0xD608 => 1 + u16_at(grpprl, operand).unwrap_or(0),
            _ => 1 + grpprl.get(operand).copied().unwrap_or(0) as usize,
        };
        match sprm {
            0x460B => found.ilfo = u16_at(grpprl, operand),
            0x260A => found.ilvl = grpprl.get(operand).map(|v| *v as usize),
            0x2416 => found.in_table = grpprl.get(operand).is_some_and(|v| *v != 0),
            // cb (2), itcMac (1), then itcMac + 1 x-positions: the left edge, and the
            // right edge of each cell.
            0xD608 => {
                let count = grpprl.get(operand + 2).copied().unwrap_or(0) as usize;
                found.cell_edges = (1..=count)
                    .filter_map(|k| grpprl.get(operand + 3 + k * 2..operand + 5 + k * 2))
                    .map(|b| i16::from_le_bytes([b[0], b[1]]) as i64)
                    .collect();
            }
            _ => {}
        }
        at = operand + size;
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_paragraph_names_its_list_and_its_level() {
        // sprmPJc (1 byte), sprmPIlvl = 1, sprmPIlfo = 2, sprmPFInTable.
        let grpprl = [
            0x03, 0x24, 0x01, 0x0A, 0x26, 0x01, 0x0B, 0x46, 0x02, 0x00, 0x16, 0x24, 0x01,
        ];
        assert_eq!(
            para_sprms(&grpprl),
            ParaSprms {
                ilfo: Some(2),
                ilvl: Some(1),
                in_table: true,
                cell_edges: vec![]
            }
        );
    }

    #[test]
    fn a_row_end_names_its_cell_edges_and_the_sprm_after_it_is_still_read() {
        // sprmTDefTable: cb = 8 (7 following bytes, plus one), two cells, edges
        // -108 | 1440 | 4320; then sprmPFInTable.
        let mut grpprl = vec![0x08, 0xD6, 0x08, 0x00, 0x02];
        for x in [-108i16, 1440, 4320] {
            grpprl.extend(x.to_le_bytes());
        }
        grpprl.extend([0x16, 0x24, 0x01]);
        let got = para_sprms(&grpprl);
        assert_eq!(got.cell_edges, [1440, 4320]);
        assert!(got.in_table);
    }

    #[test]
    fn a_list_level_says_bullet_or_number_and_where_it_starts() {
        // A simple list (bullet), one with nine levels (the 2nd starts at 3).
        let mut lst = 2u16.to_le_bytes().to_vec();
        for (lsid, simple) in [(7i32, 1u8), (9, 0)] {
            let mut lstf = vec![0u8; 28];
            lstf[0..4].copy_from_slice(&lsid.to_le_bytes());
            lstf[26] = simple;
            lst.extend(lstf);
        }
        let lvl = |start: i32, nfc: u8| {
            let mut l = vec![0u8; 28];
            l[0..4].copy_from_slice(&start.to_le_bytes());
            l[4] = nfc;
            l.extend([1, 0, b'x', 0]);
            l
        };
        lst.extend(lvl(1, 23));
        for i in 0..9 {
            lst.extend(lvl(if i == 1 { 3 } else { 1 }, 0));
        }
        let mut lfo = 2u32.to_le_bytes().to_vec();
        for lsid in [9i32, 7] {
            let mut l = vec![0u8; 16];
            l[0..4].copy_from_slice(&lsid.to_le_bytes());
            lfo.extend(l);
        }
        let lists = Lists::read(&lst, &lfo);
        let brief = |l: Option<(i32, Level)>| l.map(|(id, l)| (id, l.ordered, l.start));
        assert_eq!(brief(lists.level(1, 1)), Some((9, true, 3)));
        assert_eq!(brief(lists.level(2, 0)), Some((7, false, 1)));
        assert_eq!(lists.level(0, 0), None);
    }

    #[test]
    fn a_level_text_gives_the_label_around_its_number() {
        let style = |nfc, xst: &str, ilvl| {
            let units: Vec<u16> = xst
                .chars()
                .map(|c| if c == '#' { ilvl } else { c as u16 })
                .collect();
            number_style(Some(nfc), &units, ilvl, 1)
        };
        assert_eq!(
            super::super::super::markup_marks::label(Some(&style(4, "#)", 0)), 2),
            "b) "
        );
        assert_eq!(
            super::super::super::markup_marks::label(Some(&style(2, "(#)", 1)), 4),
            "(iv) "
        );
        // `1.1.` cites two levels: default form.
        let two: Vec<u16> = vec![0, '.' as u16, 1, '.' as u16];
        assert_eq!(
            super::super::super::markup_marks::label(Some(&number_style(Some(0), &two, 1, 1)), 3),
            "3. "
        );
    }
}
