//! **What Word does not display in a `.doc`**: hidden text (`sprmCFVanish`)
//! and text DELETED under change tracking (`sprmCFRMarkDel`), the same rule as
//! `hidden.rs` for `.docx`/`.odt`.
//!
//! These are CHARACTER properties, stored like paragraph properties in 512-byte
//! pages (`ChpxFkp`, [MS-DOC] §2.9.33) indexed by the `PlcBteChpx` in file
//! positions (FC).

use super::paragraphs::Piece;

fn u16_at(b: &[u8], o: usize) -> Option<usize> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?) as usize)
}

fn u32_at(b: &[u8], o: usize) -> Option<usize> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?) as usize)
}

/// Does a run of character properties hide its text? The operand is 1 (yes),
/// 0 (no), or 0x80/0x81 ("as the style" / "the opposite"): only the explicit
/// yes counts.
fn hides(grpprl: &[u8]) -> bool {
    let mut at = 0;
    while let Some(sprm) = u16_at(grpprl, at) {
        let operand = at + 2;
        let size = match sprm >> 13 {
            0 | 1 => 1,
            2 | 4 | 5 => 2,
            3 => 4,
            7 => 3,
            _ => 1 + grpprl.get(operand).copied().unwrap_or(0) as usize,
        };
        // sprmCFVanish, sprmCFRMarkDel.
        if matches!(sprm, 0x083C | 0x0800) && grpprl.get(operand) == Some(&1) {
            return true;
        }
        at = operand + size;
    }
    false
}

/// The FILE ranges `[start, end)` whose text is not displayed.
fn hidden_fc_ranges(word: &[u8], plc: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let n = plc.len().saturating_sub(4) / 8;
    for k in 0..n {
        let Some(pn) = u32_at(plc, (n + 1) * 4 + k * 4).map(|v| v & 0x003F_FFFF) else {
            continue;
        };
        let Some(page) = word.get(pn * 512..pn * 512 + 512) else {
            continue;
        };
        let crun = page.get(511).copied().unwrap_or(0) as usize;
        for run in 0..crun {
            let (Some(from), Some(to)) = (u32_at(page, run * 4), u32_at(page, run * 4 + 4)) else {
                continue;
            };
            // One byte per run: the CHPX position, in words; 0 = no property.
            let at = page.get(4 * (crun + 1) + run).copied().unwrap_or(0) as usize * 2;
            if at == 0 {
                continue;
            }
            let cb = page.get(at).copied().unwrap_or(0) as usize;
            if page.get(at + 1..at + 1 + cb).is_some_and(hides) {
                out.push((from, to));
            }
        }
    }
    out
}

/// The body, each non-displayed character replaced by `\0` (which rendering
/// ignores): positions stay those of the document, on which paragraph marks
/// are computed. STRUCTURE characters — paragraph, cell, field, note call —
/// stay: removing them would weld two paragraphs or break a field.
pub(super) fn mask_hidden(word: &[u8], plc: &[u8], pieces: &[Piece], body: &[char]) -> Vec<char> {
    let ranges = hidden_fc_ranges(word, plc);
    let mut out = body.to_vec();
    if ranges.is_empty() {
        return out;
    }
    for p in pieces {
        let width = if p.narrow { 1 } else { 2 };
        for cp in p.cp..p.cp_end.min(out.len()) {
            let fc = p.fc + (cp - p.cp) * width;
            let shown = !ranges.iter().any(|&(a, b)| a <= fc && fc < b);
            if let Some(c) = out.get_mut(cp).filter(|c| {
                !shown
                    && !matches!(
                        **c,
                        '\r' | '\u{7}' | '\u{2}' | '\u{13}' | '\u{14}' | '\u{15}'
                    )
            }) {
                *c = '\0';
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_explicit_yes_hides() {
        assert!(hides(&[0x3C, 0x08, 0x01]));
        assert!(
            hides(&[0x4A, 0x43, 0x18, 0x00, 0x00, 0x08, 0x01]),
            "deleted, after a 2-byte sprm"
        );
        assert!(!hides(&[0x3C, 0x08, 0x00]));
        assert!(!hides(&[0x3C, 0x08, 0x81]));
        assert!(!hides(&[0x35, 0x08, 0x01]), "bold is not hidden");
    }

    /// A hidden run over a whole paragraph and its mark: the words go, the
    /// paragraph and cell marks stay (removing them would weld paragraphs).
    #[test]
    fn hidden_words_go_and_structure_stays() {
        let body: Vec<char> = "ab\rc\u{7}d\r".chars().collect();
        let mut word = vec![0u8; 1024];
        word[..7].copy_from_slice(b"ab\rc\x07d\r");
        let page = &mut word[512..];
        page[511] = 1; // crun
        page[0..4].copy_from_slice(&0u32.to_le_bytes());
        page[4..8].copy_from_slice(&7u32.to_le_bytes());
        page[8] = 250; // CHPX at byte 500
        page[500..504].copy_from_slice(&[3, 0x3C, 0x08, 0x01]);
        let plc: Vec<u8> = [0u32, 7, 1].iter().flat_map(|v| v.to_le_bytes()).collect();
        let pieces = [Piece {
            cp: 0,
            cp_end: 7,
            fc: 0,
            narrow: true,
        }];
        let masked: String = mask_hidden(&word, &plc, &pieces, &body)
            .into_iter()
            .collect();
        assert_eq!(masked, "\0\0\r\0\u{7}\0\r");
    }
}
