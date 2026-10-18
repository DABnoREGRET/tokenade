//! **A `.ppt` link** ([MS-PPT] 2.10.13 ExHyperlinkContainer, 2.5.12
//! InteractiveInfoAtom, 2.9.70 TextInteractiveInfoInstance).
//!
//! The address lives in the document's list of external objects
//! (`ExHyperlinkContainer`: identifier, then the target in the `CString`
//! of instance 1); the text only carries a range and a reference to that
//! identifier, in pairs that follow the text atom — `InteractiveInfo`
//! (0x0FF2, reference at +4 of its atom 0x0FF3) then `TxInteractiveInfoAtom`
//! (0x0FDF: start, end). Read without them, `Visit our site today.` lost
//! the address that the `.pptx` and the `.odp` of the same slideshow serve (2026-10-04).

use super::{children, descendants, u32_at, Rec, TEXT_BYTES, TEXT_CHARS, TEXT_HEADER};
use std::collections::HashMap;

const EX_HYPERLINK: u16 = 0x0FD7;
const EX_HYPERLINK_ATOM: u16 = 0x0FD3;
const CSTRING: u16 = 0x0FBA;
const INTERACTIVE_INFO: u16 = 0x0FF2;
const INTERACTIVE_INFO_ATOM: u16 = 0x0FF3;
const TX_INTERACTIVE_INFO: u16 = 0x0FDF;

/// Identifier → address, for only the links that lead somewhere.
pub(super) fn targets(s: &[u8], doc: Rec) -> HashMap<u32, String> {
    let mut all = Vec::new();
    descendants(s, doc, &mut all, 0);
    all.iter()
        .filter(|r| r.kind == EX_HYPERLINK && r.container)
        .filter_map(|link| {
            let kids = children(s, *link);
            let id = u32_at(s, kids.iter().find(|k| k.kind == EX_HYPERLINK_ATOM)?.start)?;
            let target = kids.iter().find(|k| k.kind == CSTRING && k.instance == 1)?;
            let units = s
                .get(target.start..target.end)?
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]));
            let url: String = char::decode_utf16(units).filter_map(Result::ok).collect();
            super::super::markup_marks::has_scheme(&url).then_some((id, url))
        })
        .collect()
}

/// The linked ranges of the text atom `kids[text]`: (start, end, address),
/// in characters of the text.
pub(super) fn ranges(
    s: &[u8],
    kids: &[Rec],
    text: usize,
    links: &HashMap<u32, String>,
) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    let mut pending: Option<String> = None;
    for k in kids.iter().skip(text + 1) {
        match k.kind {
            TEXT_CHARS | TEXT_BYTES | TEXT_HEADER => break,
            INTERACTIVE_INFO => {
                pending = children(s, *k)
                    .iter()
                    .find(|a| a.kind == INTERACTIVE_INFO_ATOM)
                    .and_then(|a| u32_at(s, a.start + 4))
                    .and_then(|id| links.get(&id).cloned());
            }
            TX_INTERACTIVE_INFO => {
                if let (Some(url), Some(from), Some(to)) =
                    (pending.take(), u32_at(s, k.start), u32_at(s, k.start + 4))
                {
                    out.push((from as usize, to as usize, url));
                }
            }
            _ => {}
        }
    }
    out
}

/// `text` with each range written `[…](address)`. A paragraph break inside
/// the range closes it before itself: a markdown link does not cross the line.
pub(super) fn apply(text: &str, mut ranges: Vec<(usize, usize, String)>) -> String {
    if ranges.is_empty() {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().collect();
    ranges.sort_by_key(|(from, ..)| std::cmp::Reverse(*from));
    for (from, to, url) in ranges {
        let to = chars
            .iter()
            .skip(from)
            .position(|c| *c == '\n')
            .map_or(to, |nl| to.min(from + nl))
            .min(chars.len());
        if from >= to || chars[from..to].iter().all(|c| c.is_whitespace()) {
            continue;
        }
        chars.splice(to..to, format!("]({url})").chars());
        chars.insert(from, '[');
    }
    chars.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_range_becomes_a_link_and_stops_at_the_line_end() {
        let out = apply(
            "Visit our site today.\nNext",
            vec![
                (6, 14, "https://e.x/".into()),
                (15, 30, "https://f.x/".into()),
            ],
        );
        assert_eq!(
            out,
            "Visit [our site](https://e.x/) [today.](https://f.x/)\nNext"
        );
        assert_eq!(apply("a  b", vec![(1, 3, "https://e.x/".into())]), "a  b");
    }
}
