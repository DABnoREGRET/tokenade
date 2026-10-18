//! **What a `.xls` cell's format says about its number** ([MS-XLS]).
//!
//! `calamine` returns the value and recognises dates, not the rest: a
//! percentage came out as `0.125`, an amount as `1234.5` without its currency,
//! where the same workbook as `.xlsx` says `12.5%` and `1234.5 €` (2026-10-04).
//! The format lives in the `Workbook` stream: FORMAT records (0x041E) name the
//! codes, XF records (0x00E0) file them by style index, and each numeric cell
//! (NUMBER 0x0203, RK 0x027E, MULRK 0x00BD, FORMULA 0x0006) carries its own.
//! Sheets are found through their BOUNDSHEET (0x0085), which gives NAME and
//! position.

use super::columns::{classify_format, NumFormat};
use std::collections::HashMap;
use std::io::{Cursor, Read};

fn u16_at(b: &[u8], o: usize) -> Option<usize> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?) as usize)
}

fn u32_at(b: &[u8], o: usize) -> Option<usize> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?) as usize)
}

/// The records starting at `at`: (type, body), up to EOF (0x000A).
fn records(stream: &[u8], mut at: usize) -> impl Iterator<Item = (usize, &[u8])> {
    std::iter::from_fn(move || {
        let (kind, len) = (u16_at(stream, at)?, u16_at(stream, at + 2)?);
        let body = stream.get(at + 4..at + 4 + len)?;
        at += 4 + len;
        Some((kind, body))
    })
}

/// A BIFF8 string of `cch` characters at `at` (one options byte: bit 0 =
/// UTF-16, otherwise one byte per character).
fn biff_string(b: &[u8], at: usize, cch: usize) -> Option<String> {
    let wide = b.get(at)? & 1 != 0;
    let raw = b.get(at + 1..at + 1 + cch * if wide { 2 } else { 1 })?;
    Some(if wide {
        char::decode_utf16(
            raw.chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]])),
        )
        .map(|c| c.unwrap_or('\u{FFFD}'))
        .collect()
    } else {
        raw.iter().map(|&c| c as char).collect()
    })
}

/// The `Workbook` stream (or `Book`, BIFF5) of a `.xls`.
fn workbook_stream(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut cf = cfb::CompoundFile::open(Cursor::new(bytes)).ok()?;
    let mut stream = Vec::new();
    let opened = cf
        .open_stream("/Workbook")
        .or_else(|_| cf.open_stream("/Book"));
    opened.and_then(|mut s| s.read_to_end(&mut stream)).ok()?;
    Some(stream)
}

/// By sheet NAME: (row, column) → what its format says, only for the cells
/// whose format changes the reading (percentage, currency).
pub(super) fn cell_formats(bytes: &[u8]) -> HashMap<String, HashMap<(usize, usize), NumFormat>> {
    let mut out = HashMap::new();
    let Some(stream) = workbook_stream(bytes) else {
        return out;
    };
    let mut codes: HashMap<usize, NumFormat> = HashMap::new();
    let mut xf_format: Vec<usize> = Vec::new();
    let mut sheets: Vec<(String, usize)> = Vec::new();
    for (kind, body) in records(&stream, 0) {
        match kind {
            0x041E => {
                let (Some(ifmt), Some(cch)) = (u16_at(body, 0), u16_at(body, 2)) else {
                    continue;
                };
                if let Some(k) = biff_string(body, 4, cch).and_then(|code| classify_format(&code)) {
                    codes.insert(ifmt, k);
                }
            }
            0x00E0 => xf_format.push(u16_at(body, 2).unwrap_or(0)),
            0x0085 => {
                let (Some(pos), Some(&cch)) = (u32_at(body, 0), body.get(6)) else {
                    continue;
                };
                if let Some(name) = biff_string(body, 7, cch as usize) {
                    sheets.push((name, pos));
                }
            }
            0x000A => break,
            _ => {}
        }
    }
    // 9 (`0%`) and 10 (`0.00%`) are the built-in percentages; dates are
    // already recognised by calamine.
    for id in [9, 10] {
        codes.entry(id).or_insert(NumFormat::Percent);
    }
    let format_of = |ixfe: usize| {
        let k = codes.get(xf_format.get(ixfe)?)?;
        (*k != NumFormat::Date).then(|| k.clone())
    };
    for (name, pos) in sheets {
        let mut cells = HashMap::new();
        for (kind, body) in records(&stream, pos) {
            match kind {
                0x0203 | 0x027E | 0x0006 => {
                    let (Some(r), Some(c), Some(x)) =
                        (u16_at(body, 0), u16_at(body, 2), u16_at(body, 4))
                    else {
                        continue;
                    };
                    if let Some(k) = format_of(x) {
                        cells.insert((r, c), k);
                    }
                }
                // MULRK: row, first column, then (ixfe, rk) × n, last column.
                0x00BD => {
                    let (Some(r), Some(first)) = (u16_at(body, 0), u16_at(body, 2)) else {
                        continue;
                    };
                    let n = body.len().saturating_sub(6) / 6;
                    for i in 0..n {
                        if let Some(k) = u16_at(body, 4 + i * 6).and_then(format_of) {
                            cells.insert((r, first + i), k);
                        }
                    }
                }
                0x000A => break,
                _ => {}
            }
        }
        if !cells.is_empty() {
            out.insert(name, cells);
        }
    }
    out
}

/// By sheet NAME: (row, column) → the WEB address of the link set on the
/// cell. A workbook link lives in a HLINK (0x01B8) next to the cells
/// ([MS-XLS] 2.4.140, `Hyperlink` object of [MS-OSHARED] 2.3.7.1): read cell
/// by cell, `site` came out without its address, where the `.xlsx` of the
/// same workbook says `[site](https://…)` (2026-10-04). Only the external link
/// is rendered, as for `.xlsx`: an internal reference (`Feuil2!A1`) points at
/// cells already served, a local file has no address to follow.
pub(super) fn cell_links(bytes: &[u8]) -> HashMap<String, HashMap<(usize, usize), String>> {
    let mut out = HashMap::new();
    let Some(stream) = workbook_stream(bytes) else {
        return out;
    };
    let sheets: Vec<(String, usize)> = records(&stream, 0)
        .take_while(|(kind, _)| *kind != 0x000A)
        .filter(|(kind, _)| *kind == 0x0085)
        .filter_map(|(_, body)| {
            Some((
                biff_string(body, 7, *body.get(6)? as usize)?,
                u32_at(body, 0)?,
            ))
        })
        .collect();
    for (name, pos) in sheets {
        let links: HashMap<(usize, usize), String> = records(&stream, pos)
            .take_while(|(kind, _)| *kind != 0x000A)
            .filter(|(kind, _)| *kind == 0x01B8)
            .filter_map(|(_, body)| {
                Some((
                    (u16_at(body, 0)?, u16_at(body, 4)?),
                    hyperlink_url(body.get(24..)?)?,
                ))
            })
            .collect();
        if !links.is_empty() {
            out.insert(name, links);
        }
    }
    out
}

/// The URL of a `Hyperlink` object, if it is a URL moniker.
fn hyperlink_url(h: &[u8]) -> Option<String> {
    const URL_MONIKER: [u8; 16] = [
        0xE0, 0xC9, 0xEA, 0x79, 0xF9, 0xBA, 0xCE, 0x11, 0x8C, 0x82, 0x00, 0xAA, 0x00, 0x4B, 0xA9,
        0x0B,
    ];
    let flags = u32_at(h, 4)?;
    let mut at = 8;
    // A `HyperlinkString`: character count (trailing NUL included), UTF-16.
    let string = |at: &mut usize| -> Option<String> {
        let n = u32_at(h, *at)?;
        let raw = h.get(*at + 4..*at + 4 + n * 2)?;
        *at += 4 + n * 2;
        Some(utf16(raw))
    };
    for flag in [0x10, 0x80] {
        // Display name, then target frame: skipped.
        if flags & flag != 0 {
            string(&mut at)?;
        }
    }
    if flags & 0x01 == 0 {
        return None;
    }
    let url = if flags & 0x100 != 0 {
        string(&mut at)?
    } else {
        (h.get(at..at + 16)? == URL_MONIKER).then_some(())?;
        let len = u32_at(h, at + 16)?;
        utf16(h.get(at + 20..at + 20 + len)?)
    };
    let url = url.trim_end_matches('\0').trim().to_string();
    (url.contains("://") || url.starts_with("mailto:")).then_some(url)
}

fn utf16(raw: &[u8]) -> String {
    char::decode_utf16(
        raw.chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]])),
    )
    .map(|c| c.unwrap_or('\u{FFFD}'))
    .collect::<String>()
    .split('\0')
    .next()
    .unwrap_or("")
    .to_string()
}
