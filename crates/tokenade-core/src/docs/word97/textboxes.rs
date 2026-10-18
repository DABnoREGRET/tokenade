//! **The text of a `.doc` text box** ([MS-DOC] 2.3.6, 2.8.25).
//!
//! A text box does not write its text in the body: it lives in a separate
//! story (`ccpTxbx`), split by the `PlcftxbxTxt` (pair 56), and the body keeps
//! only an anchor — the shape, listed with its position by the `PlcSpaMom`
//! (pair 40). Read without it, `SIDEBAR boxed note` disappeared from the `.doc`
//! while the `.docx` of the same document served it (2026-10-04). The box and
//! its shape recognise each other by their identifier: the box's `lid` is the
//! shape's `spid`.

use super::paragraphs::Mark;

fn u32_at(b: &[u8], o: usize) -> Option<usize> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?) as usize)
}

/// (anchor CP in the body, box text), in box order.
pub(super) fn read(spa_plc: &[u8], txbx_plc: &[u8], story: &[char]) -> Vec<(usize, Vec<char>)> {
    // PlcSpaMom: n + 1 CPs, then n FSPAs of 26 bytes, `spid` first.
    let shapes = spa_plc.len().saturating_sub(4) / 30;
    let anchor_of: std::collections::HashMap<usize, usize> = (0..shapes)
        .filter_map(|k| {
            Some((
                u32_at(spa_plc, 4 * (shapes + 1) + 26 * k)?,
                u32_at(spa_plc, 4 * k)?,
            ))
        })
        .collect();
    // PlcftxbxTxt: n + 1 CPs in the story, then n FTXBXS of 22 bytes; a REUSABLE
    // box (`fReusable`, byte 8) is a free slot: its `lid` (byte 14) may still
    // name the shape it used to serve.
    let boxes = txbx_plc.len().saturating_sub(4) / 26;
    (0..boxes)
        .filter_map(|k| {
            let at = 4 * (boxes + 1) + 22 * k;
            if txbx_plc.get(at + 8..at + 10)? != [0, 0] {
                return None;
            }
            let anchor = *anchor_of.get(&u32_at(txbx_plc, at + 14)?)?;
            let (from, to) = (u32_at(txbx_plc, 4 * k)?, u32_at(txbx_plc, 4 * (k + 1))?);
            let mut text = story.get(from..to)?.to_vec();
            while text
                .last()
                .is_some_and(|c| matches!(c, '\r' | '\u{7}' | ' '))
            {
                text.pop();
            }
            (!text.is_empty()).then_some((anchor, text))
        })
        .collect()
}

/// Each box becomes separate paragraphs, right after the paragraph carrying
/// its anchor — where the `.docx` serves it. The following paragraph marks are
/// shifted accordingly.
pub(super) fn insert(body: &mut Vec<char>, marks: &mut [Mark], mut boxes: Vec<(usize, Vec<char>)>) {
    boxes.sort_by_key(|(anchor, _)| std::cmp::Reverse(*anchor));
    for (anchor, mut text) in boxes {
        let at = body
            .get(anchor..)
            .and_then(|rest| rest.iter().position(|c| *c == '\r'))
            .map_or(body.len(), |i| anchor + i + 1);
        text.push('\r');
        let len = text.len();
        body.splice(at..at, text);
        for mark in marks.iter_mut().filter(|m| m.cp >= at) {
            mark.cp += len;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_lands_after_its_anchor_paragraph_and_a_spare_box_is_skipped() {
        let le = |v: u32| v.to_le_bytes();
        // Two shapes: spid 7 anchored at CP 2, spid 9 at CP 9.
        let mut spa: Vec<u8> = [2u32, 9, 20].iter().flat_map(|v| le(*v)).collect();
        for spid in [7u32, 9] {
            spa.extend(le(spid));
            spa.extend([0u8; 22]);
        }
        // Three boxes: "Box A\r" (lid 9), a free slot still holding the lid of a
        // live shape (7), "Box B\r" (lid 7).
        let story: Vec<char> = "Box A\rspare\rBox B\r".chars().collect();
        let mut txbx: Vec<u8> = [0u32, 6, 12, 18].iter().flat_map(|v| le(*v)).collect();
        for (reusable, lid) in [(0u16, 9i32), (1, 7), (0, 7)] {
            txbx.extend([0u8; 8]);
            txbx.extend(reusable.to_le_bytes());
            txbx.extend([0u8; 4]);
            txbx.extend(lid.to_le_bytes());
            txbx.extend([0u8; 4]);
        }
        let boxes = read(&spa, &txbx, &story);
        assert_eq!(boxes.len(), 2);
        let mut body: Vec<char> = "One\x08 x\rTwo\x08\rThree\r".chars().collect();
        let mut marks = vec![Mark {
            cp: 17,
            prefix: "# ".into(),
            in_table: false,
            row_edges: vec![],
        }];
        insert(&mut body, &mut marks, boxes);
        assert_eq!(
            body.iter().collect::<String>(),
            "One\x08 x\rBox B\rTwo\x08\rBox A\rThree\r"
        );
        assert_eq!(body[marks[0].cp], '\r');
        assert_eq!(marks[0].cp, 29);
    }
}
