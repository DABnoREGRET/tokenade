//! Spreadsheets: the sheet grid, and the skeleton served instead of it.
//!
//! A workbook read cell by cell is mostly repetition — the same column kind
//! down thousands of rows. These read the grid (shared strings, merges, cell
//! types) and describe it by column and range instead of transcribing it.

use super::*;

/// Legacy Excel (.xls BIFF / .xlsb) via `calamine`.
///
/// **The same workbook, the same answer.** This reader wrote its lines by
/// hand: `## name` where the two other spellings say `[sheet: name]`, and
/// a table WITHOUT its `| --- |` separator line, so the header could not be told
/// apart from a data row. It did not go through
/// `rows_to_markdown` either, which escapes a cell's `|`: a cell containing
/// a vertical bar silently broke the table and shifted all the following
/// values by one column. It now goes through
/// `render_workbook`, like the `.xlsx` and like the `.ods` — which
/// also brings it the column profile of very large sheets, which it
/// used to dump line by line.
///
/// Hidden sheets are silenced, here through the metadata `calamine`
/// exposes (`SheetVisible`) rather than by reading XML. Third
/// spelling, same decision as for the other two.
///
/// Wrapped in `catch_unwind` for the same reason the PDF extractor is —
/// real-world files from twenty years of Excel versions hit parser edge cases,
/// and a panic in a hook kills the agent's tool call.
pub(super) fn extract_xls(bytes: &[u8]) -> String {
    use calamine::{Data, Reader, SheetVisible};
    let out = std::panic::catch_unwind(|| {
        let cursor = Cursor::new(bytes.to_vec());
        let mut wb = match calamine::open_workbook_auto_from_rs(cursor) {
            Ok(w) => w,
            Err(_) => return String::new(),
        };
        let hidden: std::collections::HashSet<String> = wb
            .sheets_metadata()
            .iter()
            .filter(|sh| !matches!(sh.visible, SheetVisible::Visible))
            .map(|sh| sh.name.clone())
            .collect();
        let (biff, biff_links) = if matches!(wb, calamine::Sheets::Xls(_)) {
            (
                biff_formats::cell_formats(bytes),
                biff_formats::cell_links(bytes),
            )
        } else {
            Default::default()
        };
        let mut sheets: Vec<(String, Vec<Vec<String>>)> = Vec::new();
        for name in wb.sheet_names().to_vec() {
            if hidden.contains(&name) {
                continue;
            }
            let Ok(range) = wb.worksheet_range(&name) else {
                continue;
            };
            if range.is_empty() {
                continue;
            }
            let (row0, col0) = range
                .start()
                .map_or((0, 0), |(r, c)| (r as usize, c as usize));
            // Percentage and currency: the cell's BIFF format, which
            // calamine does not return (`biff_formats`).
            let formats = biff.get(&name);
            let rows: Vec<Vec<String>> = range
                .rows()
                .enumerate()
                .map(|(i, row)| {
                    row.iter()
                        .enumerate()
                        .map(|(j, c)| {
                            let n = match c {
                                Data::Float(f) => Some(*f),
                                Data::Int(n) => Some(*n as f64),
                                _ => None,
                            };
                            let shown = n
                                .zip(formats.and_then(|f| f.get(&(row0 + i, col0 + j))))
                                .and_then(|(n, k)| {
                                    let raw = n.to_string();
                                    let value = at_excel_precision(&raw).unwrap_or(raw);
                                    match k {
                                        NumFormat::Percent => fraction_to_percent(n),
                                        NumFormat::Currency {
                                            symbol,
                                            before: true,
                                        } => Some(format!("{symbol}{value}")),
                                        NumFormat::Currency { symbol, .. } => {
                                            Some(format!("{value} {symbol}"))
                                        }
                                        NumFormat::Date => None,
                                    }
                                });
                            (c, shown)
                        })
                        .map(|(c, shown)| match c {
                            _ if shown.is_some() => shown.unwrap_or_default(),
                            Data::Empty => String::new(),
                            Data::String(v) => v.trim().to_string(),
                            // A date is a NUMBER, and without calamine's
                            // `dates` flag it arrives as such:
                            // indistinguishable from an amount, unreadable. Rendered by
                            // the SAME converter as the modern workbook,
                            // so that both spellings agree.
                            Data::DateTime(dt) => {
                                serial_to_date(dt.as_f64()).unwrap_or_else(|| dt.to_string())
                            }
                            Data::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
                            Data::Float(f) => {
                                let raw = f.to_string();
                                at_excel_precision(&raw).unwrap_or(raw)
                            }
                            other => other.to_string(),
                        })
                        .collect::<Vec<String>>()
                })
                .collect();
            // A merged title spreads over its columns, as the `.xlsx` reader
            // does (`Quarterly report` over A1:C1 read as one cell then two
            // empty ones). Calamine lists the regions for `.xls` only.
            let mut rows = rows;
            // A cell link is written `[text](url)`, as in `.xlsx`.
            for (&(r, c), url) in biff_links.get(&name).into_iter().flatten() {
                let (Some(i), Some(j)) = (r.checked_sub(row0), c.checked_sub(col0)) else {
                    continue;
                };
                if let Some(cell) = rows.get_mut(i).and_then(|row| row.get_mut(j)) {
                    let text = if cell.is_empty() {
                        url.clone()
                    } else {
                        cell.clone()
                    };
                    *cell = format!("[{text}]({url})");
                }
            }
            if let calamine::Sheets::Xls(xls) = &wb {
                if let Some(merges) = xls.worksheet_merge_cells(&name) {
                    let index: std::collections::HashMap<usize, usize> =
                        (0..rows.len()).map(|i| (row0 + i, i)).collect();
                    let regions = merges.iter().filter_map(|d| {
                        let (c1, c2) = (
                            (d.start.1 as usize).checked_sub(col0)?,
                            (d.end.1 as usize).checked_sub(col0)?,
                        );
                        Some((d.start.0 as usize, c1, d.end.0 as usize, c2))
                    });
                    fill_merges(&mut rows, regions, &index);
                }
            }
            // Skip fully blank rows — a sheet's unused grid is not content.
            let rows: Vec<Vec<String>> = rows
                .into_iter()
                .filter(|cells| !cells.iter().all(String::is_empty))
                .collect();
            if rows.is_empty() {
                continue;
            }
            sheets.push((name, rows));
        }
        format!(
            "{}{}",
            render_workbook(sheets),
            hidden_note("sheet", hidden.len(), &[])
        )
    });
    out.unwrap_or_default()
}

/// `xl/sharedStrings.xml` → the `<si>` table, in order. A cell with `t="s"`
/// holds an INDEX into this, not a value.
pub(super) fn shared_strings(xml: &str) -> Vec<String> {
    if xml.is_empty() {
        return Vec::new();
    }
    // An <si> may hold one <t>, or several inside <r> runs (rich text) which
    // concatenate into ONE string — that concatenation is correct here.
    re(r"(?s)<si>(.*?)</si>")
        .captures_iter(xml)
        .map(|c| {
            let inner = c.get(1).map(|m| m.as_str()).unwrap_or("");
            let text: String = re(r"(?s)<t[^>]*>(.*?)</t>")
                .captures_iter(inner)
                .map(|t| t.get(1).map(|m| m.as_str()).unwrap_or(""))
                .collect();
            decode_entities(&text)
        })
        .collect()
}

/// Column index from a cell reference (`C5` → 2, `AA1` → 26). `None` when the
/// ref has no column letters.
pub(super) fn col_index(cell_ref: &str) -> Option<usize> {
    let letters: String = cell_ref
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    if letters.is_empty() {
        return None;
    }
    let mut n = 0usize;
    for c in letters.chars() {
        n = n * 26 + (c.to_ascii_uppercase() as usize - 'A' as usize + 1);
    }
    Some(n - 1)
}

/// One sheet → markdown table rows. Cell boundaries are preserved because we
/// parse `<c>` elements instead of stripping tags.
///
/// Cells are placed at the column their `r` reference names, NOT at their
/// position in the row. A spreadsheet omits empty cells, so real rows are
/// sparse: a row holding only `A2` and `C2` would otherwise render `| a2 | c2 |`
/// and put C's value under B's header — the same corruption as the fused
/// `257`+`3`, one layer up: the value survives but its MEANING is wrong, and an
/// agent has no way to tell.
/// Count the leading rows that are banner TITLES rather than data.
///
/// After [`apply_merges`], a caption merged across the sheet's full width is a
/// row whose every cell holds the same text — nothing else produces that shape,
/// because a header row with N identical column names is not a thing. Only
/// LEADING rows are considered: an identical run further down is data.
pub(super) fn leading_title_rows(rows: &[Vec<String>], n_cols: usize) -> usize {
    if n_cols < 2 {
        return 0; // a single column cannot be "merged across the width"
    }
    let mut n = 0;
    for row in rows {
        let first = row.first().map(|s| s.trim()).unwrap_or("");
        let full_width_same =
            !first.is_empty() && row.len() == n_cols && row.iter().all(|c| c.trim() == first);
        if !full_width_same {
            break;
        }
        n += 1;
    }
    // A sheet that is ONLY titles is not a sheet of titles — it is data we
    // failed to understand. Keep it rather than describe nothing.
    if n == rows.len() {
        0
    } else {
        n
    }
}

/// Spread each merged region's value across every cell it covers.
///
/// A merged region stores its value ONCE, in the top-left anchor; the other
/// covered cells are empty in the XML even though the sheet shows the value
/// across all of them. Reading them literally produced three separate failures
/// on one merged `A1:C1` header (measured): the header row looked mostly empty
/// so header detection rejected it and every column came back `(no header)`;
/// the header TEXT was then served as column A's example value; and the column
/// typed as `number` while its own example was `"Trimestre 1"` — a map that
/// contradicts itself. Merged headers are ordinary in real workbooks.
///
/// Only empty cells are filled, so a merge never overwrites real data.
pub(super) fn apply_merges(
    rows: &mut [Vec<String>],
    xml: &str,
    index_of_row: &std::collections::HashMap<usize, usize>,
) {
    let merge_re = re(r#"(?s)<mergeCell\b[^>]*\bref="([A-Za-z]+)(\d+):([A-Za-z]+)(\d+)""#);
    let merges = merge_re.captures_iter(xml).filter_map(|m| {
        Some((
            m.get(2)?.as_str().parse::<usize>().ok()?,
            col_index(m.get(1)?.as_str())?,
            m.get(4)?.as_str().parse::<usize>().ok()?,
            col_index(m.get(3)?.as_str())?,
        ))
    });
    fill_merges(rows, merges, index_of_row);
}

/// Spread each merged region's value over its empty cells. A region is
/// `(first row, first column, last row, last column)` in the SHEET's row
/// numbering, mapped to `rows` by `index_of_row` (blank rows are not in
/// `rows`); columns are already `rows` columns. Shared by the `.xlsx` reader
/// (`<mergeCell>`) and the legacy `.xls` one (calamine's merge list).
pub(super) fn fill_merges(
    rows: &mut [Vec<String>],
    merges: impl Iterator<Item = (usize, usize, usize, usize)>,
    index_of_row: &std::collections::HashMap<usize, usize>,
) {
    for (r1, c1, r2, c2) in merges {
        let (Some(&i1), Some(&i2)) = (index_of_row.get(&r1), index_of_row.get(&r2)) else {
            continue;
        };
        let Some(anchor) = rows.get(i1).and_then(|r| r.get(c1)).cloned() else {
            continue;
        };
        if anchor.trim().is_empty() {
            continue;
        }
        for i in i1..=i2.min(rows.len().saturating_sub(1)) {
            let row = &mut rows[i];
            for c in c1..=c2 {
                if c >= row.len() {
                    row.resize(c + 1, String::new());
                }
                if row[c].trim().is_empty() {
                    row[c] = anchor.clone();
                }
            }
        }
    }
}

pub(super) fn sheet_rows(
    xml: &str,
    shared: &[String],
    formats: &std::collections::HashMap<usize, NumFormat>,
) -> Vec<Vec<String>> {
    let row_re = re(r"(?s)<row([^>]*)>(.*?)</row>");
    // An empty but STYLED cell is written `<c r="B1" s="1"/>`, and the
    // naive pair does not see it: it swallowed the next cell and published
    // its shared-string INDEX as the value. See `docs::element_pattern`.
    let cell_re = re(&super::element_pattern("c"));
    let v_re = re(r"(?s)<v[^>]*>(.*?)</v>");
    let is_re = re(r"(?s)<is>(.*?)</is>");
    let t_re = re(r"(?s)<t[^>]*>(.*?)</t>");
    let ref_re = re(r#"(?s)\br="([A-Za-z]+)\d*""#);
    let style_re = re(r#"(?s)\bs="(\d+)""#);
    let rownum_re = re(r#"(?s)\br="(\d+)""#);
    let f_re = re(r"(?s)<f\b[^>]*>(.+?)</f>");
    // Row number -> position in the returned Vec. Cells are placed by their `r`
    // reference, but ROWS are stored in document order, so a merge that names
    // row 7 cannot be applied by indexing with 7. Mapping through what each
    // <row> actually declares keeps the two coordinate systems from drifting.
    let mut index_of_row: std::collections::HashMap<usize, usize> =
        std::collections::HashMap::new();
    let mut rows: Vec<Vec<String>> = row_re
        .captures_iter(xml)
        .enumerate()
        .map(|(i, row)| {
            let attrs = row.get(1).map(|m| m.as_str()).unwrap_or("");
            if let Some(n) = rownum_re
                .captures(attrs)
                .and_then(|m| m.get(1)?.as_str().parse::<usize>().ok())
            {
                index_of_row.insert(n, i);
            }
            let body = row.get(2).map(|m| m.as_str()).unwrap_or("");
            let mut cells: Vec<String> = Vec::new();
            for (pos, c) in cell_re.captures_iter(body).enumerate() {
                let attrs = c.get(1).map(|m| m.as_str()).unwrap_or("");
                let inner = c.get(2).map(|m| m.as_str()).unwrap_or("");
                let idx = ref_re
                    .captures(attrs)
                    .and_then(|m| col_index(m.get(1)?.as_str()))
                    .unwrap_or(pos);
                let mut value = cell_value(attrs, inner, shared, v_re, is_re, t_re);
                // **A formula nobody computed is still said.** A workbook
                // written by a script (openpyxl, a pandas export with
                // formulas) stores `<f>B2+C2</f><v></v>`: Excel computes it on
                // opening, and we served an empty cell — the column read as
                // missing data. The formula is what the sheet holds.
                if value.is_empty() {
                    if let Some(f) = f_re.captures(inner) {
                        value = format!("={}", decode_entities(f[1].trim()));
                    }
                }
                // A date, a percentage: a NUMBER whose STYLE says what
                // it is. The style lives in another file of the workbook.
                if !formats.is_empty() && !attrs.contains(r#"t="s""#) {
                    let kind = style_re
                        .captures(attrs)
                        .and_then(|m| m.get(1)?.as_str().parse::<usize>().ok())
                        .and_then(|st| formats.get(&st).cloned());
                    if let Some((kind, n)) = kind.zip(value.trim().parse::<f64>().ok()) {
                        let rendered = match kind {
                            NumFormat::Date => serial_to_date(n),
                            NumFormat::Percent => fraction_to_percent(n),
                            NumFormat::Currency { symbol, before } if before => {
                                Some(format!("{symbol}{}", value.trim()))
                            }
                            NumFormat::Currency { symbol, .. } => {
                                Some(format!("{} {symbol}", value.trim()))
                            }
                        };
                        if let Some(rendered) = rendered {
                            value = rendered;
                        }
                    }
                }
                if cells.len() <= idx {
                    cells.resize(idx + 1, String::new());
                }
                cells[idx] = value;
            }
            cells
        })
        .collect();
    apply_merges(&mut rows, xml, &index_of_row);
    rows.into_iter().collect()
}

/// The text of one `<c>` element: shared-string index, inline string, or
/// literal value.
pub(super) fn cell_value(
    attrs: &str,
    inner: &str,
    shared: &[String],
    v_re: &regex::Regex,
    is_re: &regex::Regex,
    t_re: &regex::Regex,
) -> String {
    // t="s" → shared-string index; t="inlineStr" → <is><t>;
    // anything else (n, b, str, missing) → literal <v>.
    if attrs.contains(r#"t="s""#) {
        let idx = v_re
            .captures(inner)
            .and_then(|v| v.get(1))
            .and_then(|m| m.as_str().trim().parse::<usize>().ok());
        return idx.and_then(|i| shared.get(i).cloned()).unwrap_or_default();
    }
    if let Some(is) = is_re.captures(inner) {
        let s: String = t_re
            .captures_iter(is.get(1).map(|m| m.as_str()).unwrap_or(""))
            .map(|t| t.get(1).map(|m| m.as_str()).unwrap_or(""))
            .collect();
        return decode_entities(&s);
    }
    let v = v_re
        .captures(inner)
        .and_then(|v| v.get(1))
        .map(|m| decode_entities(m.as_str()))
        .unwrap_or_default();
    // t="b": a BOOLEAN stored as `1`/`0`, shown TRUE/FALSE. Served `1`, a
    // column of yes/no answers read as an integer column with a mean of 0.51.
    if attrs.contains(r#"t="b""#) {
        match v.trim() {
            "1" => return "TRUE".to_string(),
            "0" => return "FALSE".to_string(),
            _ => {}
        }
    }
    if attrs.contains(r#"t="str""#) || attrs.contains(r#"t="e""#) {
        return v;
    }
    at_excel_precision(&v).unwrap_or(v)
}

/// **A number is shown to the 15 significant digits Excel keeps, not to the
/// 17 its binary value spells.** Files store `0.07000000000000001` (openpyxl
/// writes the full repr; a cached `=A1*1.1` holds `1.1000000000000001`), and
/// Excel displays `0.07` / `1.1`. Served raw, the noise read as a precision
/// the sheet does not have — and a column of them as distinct values.
/// `None` when the value has 15 digits or fewer, or is not a plain number.
pub(super) fn at_excel_precision(v: &str) -> Option<String> {
    let digits = v
        .trim_start_matches('-')
        .split(['e', 'E'])
        .next()?
        .chars()
        .filter(char::is_ascii_digit)
        .skip_while(|c| *c == '0')
        .count();
    if digits <= 15 {
        return None;
    }
    let x: f64 = v.parse().ok()?;
    let rounded: f64 = format!("{x:.14e}").parse().ok()?;
    Some(rounded.to_string())
}

mod biff_formats;
mod columns;
mod links;
mod ods;
mod skeleton;
pub(super) use columns::*;
pub(super) use ods::*;
pub use skeleton::*;

pub(super) fn extract_xlsx(bytes: &[u8]) -> String {
    let Some(sheets) = sheet_tables(bytes, Format::Xlsx) else {
        return String::new();
    };
    let wb = ZipArchive::new(Cursor::new(bytes))
        .ok()
        .map(|mut z| raw_xml_entry(&mut z, "xl/workbook.xml"))
        .unwrap_or_default();
    let hidden = re(r#"(?s)<sheet\b[^>]*>"#)
        .find_iter(&wb)
        .filter(|t| {
            t.as_str().contains("state=\"hidden\"") || t.as_str().contains("state=\"veryHidden\"")
        })
        .count();
    format!(
        "{}{}",
        render_workbook(sheets),
        hidden_note("sheet", hidden, &[])
    )
}

/// **What the document hides is not served — and the absence is SAID.** A hidden
/// sheet or slide is left out (the author did not show it), but
/// leaving it out in silence made the workbook look complete: a formula can
/// read a sheet the agent was never told exists. A hidden SHEET is counted,
/// not named — its name is content the author hid too (`Brouillon`), a test
/// already held that line. A slide is named by its NUMBER, which says nothing.
pub(super) fn hidden_note(word: &str, count: usize, labels: &[String]) -> String {
    if count == 0 {
        return String::new();
    }
    let which = if labels.is_empty() {
        String::new()
    } else {
        format!(": {}", labels.join(", "))
    };
    format!(
        "\n[{count} hidden {word}{} not shown, as the document hides {}{which}]\n",
        if count == 1 { "" } else { "s" },
        if count == 1 { "it" } else { "them" },
    )
}

thread_local! {
    static VERBATIM_TABLES: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `f` with every sheet rendered row for row, never profiled: the text a
/// QUESTION is asked of. The profile keeps three sample rows; asked "what
/// does client Atelier Nord owe", retrieval over it returned the column
/// summary and five unrelated rows, and read like an answer (2026-10-03).
pub fn with_verbatim_tables<T>(f: impl FnOnce() -> T) -> T {
    let before = VERBATIM_TABLES.replace(true);
    let out = f();
    VERBATIM_TABLES.set(before);
    out
}

/// A workbook's rendering: each sheet under ITS name, the big ones folded into a
/// column profile.
///
/// Shared between the three spellings of the same object. It was not: the
/// `.xlsx` went through here, the `.ods` and the `.fods` through the generic tag
/// stripper, which knows neither a sheet's name nor whether it is
/// hidden. Two tables glued together with nothing saying they come from different
/// sheets, and the sheet the author hid served like the rest.
pub(super) fn render_workbook(sheets: Vec<(String, Vec<Vec<String>>)>) -> String {
    let mut out = String::new();
    for (name, rows) in sheets {
        // A big sheet answers no question as 40 000 markdown rows. Above the
        // threshold it becomes a column profile plus a head sample, with the
        // verbatim table one `expand-ref` away.
        let table = if !VERBATIM_TABLES.get() && crate::table_profile::should_profile(&rows) {
            let mut stash = |text: &str| match crate::disclose::stash_for_reference(
                text,
                &crate::disclose::SourceHint::web(),
            ) {
                Some((hash, _)) => format!("tokenade expand-ref {hash}"),
                None => "(original not stashed)".to_string(),
            };
            crate::table_profile::render(&rows, &mut stash)
        } else {
            crate::markdown::rows_to_markdown(&rows)
        };
        if table.trim().is_empty() {
            continue;
        }
        let _ = write!(out, "\n[sheet: {name}]\n{table}");
    }
    collapse_whitespace(&out)
}

#[cfg(test)]
mod skeleton_carries_cell_values {
    /// A merged title in a legacy `.xls` spreads over its columns, as in the
    /// same workbook saved as `.xlsx` (LibreOffice's export of an openpyxl
    /// sheet whose `A1:C1` holds `Quarterly report`).
    #[test]
    fn a_merged_title_of_a_legacy_workbook_spreads_over_its_columns() {
        let out = super::extract_xls(include_bytes!("../../tests/fixtures/docs/merged_title.xls"));
        assert!(
            out.contains("| Quarterly report | Quarterly report | Quarterly report |  |"),
            "{out}"
        );
        // Percentage and currency read from the BIFF format, as in `.xlsx`.
        assert!(
            out.contains("| EMEA | 1234.5 € | 12.5% | 2026-03-31 |"),
            "{out}"
        );
        assert!(out.contains("| APAC | 987 € | -4% | 2026-06-30 |"), "{out}");
    }

    /// A cell that links to the web says where, as in the `.xlsx` the same
    /// workbook came from: `site` read without its address.
    #[test]
    fn a_linked_cell_of_a_legacy_workbook_says_where_it_points() {
        let out = super::extract_xls(include_bytes!("../../tests/fixtures/docs/cell_link.xls"));
        assert!(
            out.contains("| Widget | line one line two | 10 | [site](https://example.com/w) |"),
            "{out}"
        );
        assert!(
            out.contains("| Gadget | pipe \\| inside | 20 | x |"),
            "{out}"
        );
    }

    use super::*;
    use std::io::{Cursor, Write};
    use zip::{write::SimpleFileOptions, ZipWriter};

    /// Multi-row XLSX: the skeleton needs real rows to map columns from.
    fn sheet_with(rows: &[Vec<&str>]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut w = ZipWriter::new(Cursor::new(&mut buf));
            let opts: SimpleFileOptions = Default::default();
            w.start_file("xl/worksheets/sheet1.xml", opts).unwrap();
            let body: String = rows
                .iter()
                .enumerate()
                .map(|(r, cells)| {
                    let cs: String = cells
                        .iter()
                        .enumerate()
                        .map(|(c, v)| {
                            let refr = format!("{}{}", (b'A' + c as u8) as char, r + 1);
                            format!(r#"<c r="{refr}" t="str"><v>{v}</v></c>"#)
                        })
                        .collect();
                    format!(r#"<row r="{}">{cs}</row>"#, r + 1)
                })
                .collect();
            w.write_all(
                format!(
                    r#"<?xml version="1.0"?><worksheet xmlns="x"><sheetData>{body}</sheetData></worksheet>"#
                )
                .as_bytes(),
            )
            .unwrap();
            w.finish().unwrap();
        }
        buf
    }

    /// MEASURED NEGATIVE: the skeleton is a MAP — sheet names, column
    /// headers, shapes — not cell values, so a credential sitting in a data
    /// row never reaches it. `read_pretool`'s spreadsheet branch was suspected
    /// of leaking and does not. Do not re-mine this.
    #[test]
    fn a_key_in_a_data_cell_does_not_reach_the_skeleton() {
        let key = format!("AKIA{}", "IOSFODNN7EXAMPLE");
        let mut rows: Vec<Vec<String>> =
            vec![vec!["service".into(), "region".into(), "credential".into()]];
        for i in 0..300 {
            rows.push(vec![
                format!("svc-{i}"),
                format!("eu-west-{i}"),
                format!("placeholder-{i}"),
            ]);
        }
        rows.push(vec!["billing".into(), "eu-west-1".into(), key.clone()]);
        let borrowed: Vec<Vec<&str>> = rows
            .iter()
            .map(|r| r.iter().map(|s| s.as_str()).collect())
            .collect();
        let bytes = sheet_with(&borrowed);
        let skel = spreadsheet_skeleton(&bytes, Format::Xlsx, "book.xlsx", None)
            .expect("300 rows produce a skeleton");
        assert!(
            !skel.contains(&key),
            "the skeleton is meant to be a map, not the cells:\n{skel}"
        );
        assert!(
            skel.contains("credential"),
            "the column NAME is what a map carries:\n{skel}"
        );
    }
}

/// **What the workbook declares, and what it does not show.**
///
/// `declared_sheets` is a PURE function: it is tested without building a
/// zip, so each rule has its own separating input rather than one workbook
/// that goes through them all.
#[cfg(test)]
mod a_workbook_says_itself_what_it_shows {
    use super::declared_sheets;

    const RELS: &str = r#"<?xml version="1.0"?><Relationships>
      <Relationship Id="rId1" Target="worksheets/sheet1.xml"/>
      <Relationship Id="rId2" Target="worksheets/sheet2.xml"/>
      <Relationship Id="rId3" Target="worksheets/sheet3.xml"/>
      </Relationships>"#;

    /// **The name follows the RELATIONSHIP, not the file number.**
    ///
    /// The number of `sheetN.xml` is the CREATION order, the order of
    /// `workbook.xml` is the order of the TABS. Moving a tab is enough to
    /// shift the two lists, and pairing by position then puts a
    /// table under the name of ANOTHER sheet — a fabrication, not a loss.
    /// Here the first tab lives in `sheet3.xml` and the second in
    /// `sheet1.xml`: by position, the two names would be swapped.
    #[test]
    fn the_name_follows_the_relationship_not_the_file_number() {
        let wb = r#"<workbook><sheets>
          <sheet name="Alpha" sheetId="7" r:id="rId3"/>
          <sheet name="Beta" sheetId="2" r:id="rId1"/>
          </sheets></workbook>"#;
        assert_eq!(
            declared_sheets(wb, RELS),
            vec![
                ("Alpha".to_string(), "xl/worksheets/sheet3.xml".to_string()),
                ("Beta".to_string(), "xl/worksheets/sheet1.xml".to_string()),
            ]
        );
    }

    /// `hidden` and `veryHidden`: the second is only reachable through a macro
    /// editor, so it is shown even less (write the LIST, not
    /// the single name encountered).
    #[test]
    fn a_sheet_the_workbook_does_not_show_does_not_come_out() {
        let wb = r#"<workbook><sheets>
          <sheet name="Visible" sheetId="1" r:id="rId1"/>
          <sheet name="Masquee" sheetId="2" r:id="rId2" state="hidden"/>
          <sheet name="TresMasquee" sheetId="3" r:id="rId3" state="veryHidden"/>
          </sheets></workbook>"#;
        assert_eq!(
            declared_sheets(wb, RELS),
            vec![(
                "Visible".to_string(),
                "xl/worksheets/sheet1.xml".to_string()
            )]
        );
    }

    /// A formula with no cached value (workbook written by a script) is
    /// stated; a cached value stays the value.
    #[test]
    fn a_never_computed_formula_is_stated() {
        let xml = r#"<worksheet><sheetData><row r="1"><c r="A1"><v>2</v></c><c r="B1"><f>A1*3</f><v></v></c><c r="C1"><f>A1+1</f><v>3</v></c><c r="D1"><f>A1&amp;"x"</f></c><c r="E1"><f t="shared" si="0"/></c></row></sheetData></worksheet>"#;
        let rows = crate::docs::sheet_rows(xml, &[], &std::collections::HashMap::new());
        assert_eq!(rows[0], vec!["2", "=A1*3", "3", "=A1&\"x\"", ""]);
    }

    /// A float's binary noise is not a precision of the workbook.
    #[test]
    fn a_number_is_rendered_at_excel_precision() {
        use crate::docs::{at_excel_precision, cell_value};
        assert_eq!(
            at_excel_precision("0.07000000000000001").as_deref(),
            Some("0.07")
        );
        assert_eq!(
            at_excel_precision("1.1000000000000001").as_deref(),
            Some("1.1")
        );
        assert_eq!(
            at_excel_precision("-0.30000000000000004").as_deref(),
            Some("-0.3")
        );
        // 15 digits or fewer: intact (a long identifier, an exact amount).
        assert_eq!(at_excel_precision("123456789012345"), None);
        assert_eq!(at_excel_precision("0.125"), None);
        // Leading zeros are not significant digits.
        assert_eq!(at_excel_precision("0.000123456789012345"), None);
        assert_eq!(at_excel_precision("abc"), None);
        // Sixteen true significant digits: rounded to fifteen, like Excel.
        assert_eq!(
            at_excel_precision("1234567890123456").as_deref(),
            Some("1234567890123460")
        );
        let v = regex::Regex::new(r"(?s)<v>(.*?)</v>").unwrap();
        let is = regex::Regex::new(r"(?s)<is>(.*?)</is>").unwrap();
        let t = regex::Regex::new(r"(?s)<t[^>]*>(.*?)</t>").unwrap();
        assert_eq!(
            cell_value(
                r#"r="F3" t="n""#,
                "<v>0.07000000000000001</v>",
                &[],
                &v,
                &is,
                &t
            ),
            "0.07"
        );
        assert_eq!(
            cell_value(r#"r="F3""#, "<v>0.07000000000000001</v>", &[], &v, &is, &t),
            "0.07"
        );
        assert_eq!(
            cell_value(
                r#"r="A1" t="str""#,
                "<v>0.07000000000000001</v>",
                &[],
                &v,
                &is,
                &t
            ),
            "0.07000000000000001"
        );
    }

    /// openpyxl writes `Type… Target… Id`: an `Id`-before-`Target` pattern
    /// read nothing, and the workbook fell back on walking the zip — which
    /// served the hidden sheet (2026-10-04).
    #[test]
    fn relationship_attributes_are_read_in_any_order() {
        let rels = r#"<Relationships>
          <Relationship Type="t" Target="/xl/worksheets/sheet1.xml" Id="rId1"/>
          <Relationship Type="t" Target="/xl/worksheets/sheet2.xml" Id="rId2"/>
          </Relationships>"#;
        let wb = r#"<workbook><sheets>
          <sheet name="Sales" sheetId="1" state="visible" r:id="rId1"/>
          <sheet name="Secret" sheetId="2" state="hidden" r:id="rId2"/>
          </sheets></workbook>"#;
        assert_eq!(
            declared_sheets(wb, rels),
            vec![("Sales".to_string(), "xl/worksheets/sheet1.xml".to_string())]
        );
    }

    /// The fallback (no readable relationship) also silences the hidden sheet: same
    /// decision through both paths.
    #[test]
    fn the_fallback_without_relationships_also_silences_the_hidden_sheet() {
        let sheet = |v: &str| {
            format!(
                r#"<worksheet><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>{v}</t></is></c></row></sheetData></worksheet>"#
            )
        };
        let zip = crate::docs::tests::make_zip(&[
            (
                "xl/workbook.xml",
                r#"<workbook><sheets><sheet name="Sales" sheetId="1" r:id="rId1"/><sheet name="Secret" sheetId="2" state="hidden" r:id="rId2"/></sheets></workbook>"#,
            ),
            ("xl/worksheets/sheet1.xml", &sheet("VISIBLE-DATA")),
            ("xl/worksheets/sheet2.xml", &sheet("SECRET-DATA")),
        ]);
        let out = crate::docs::extract_xlsx(&zip);
        assert!(
            out.contains("VISIBLE-DATA") && !out.contains("SECRET-DATA"),
            "{out}"
        );
        assert!(out.contains("[1 hidden sheet not shown"), "{out}");
    }

    /// **Anti-vacuity: a workbook that declares nothing usable returns an
    /// EMPTY list, and that is what makes the old walk resume.** Without
    /// it, a workbook without `_rels` would come out empty — a fix against
    /// fabrication that would become a total loss.
    #[test]
    fn without_relationships_the_list_is_empty_so_the_caller_falls_back() {
        let wb = r#"<workbook><sheets><sheet name="Alpha" r:id="rId1"/></sheets></workbook>"#;
        assert!(declared_sheets(wb, "").is_empty());
    }

    /// A target can be written absolute (`/xl/worksheets/…`) or prefixed
    /// `./`. Both forms exist in the wild; neither must produce
    /// a path the zip does not contain.
    #[test]
    fn both_target_spellings_give_the_same_path() {
        let rels = r#"<Relationships>
          <Relationship Id="rA" Target="/xl/worksheets/sheet9.xml"/>
          <Relationship Id="rB" Target="./worksheets/sheet8.xml"/>
          </Relationships>"#;
        let wb = r#"<workbook><sheets>
          <sheet name="A" r:id="rA"/><sheet name="B" r:id="rB"/></sheets></workbook>"#;
        assert_eq!(
            declared_sheets(wb, rels),
            vec![
                ("A".to_string(), "xl/worksheets/sheet9.xml".to_string()),
                ("B".to_string(), "xl/worksheets/sheet8.xml".to_string()),
            ]
        );
    }
}

/// **OpenDocument hides its sheets in a STYLE, not on the sheet.**
///
/// Hidden sheets of a Microsoft workbook had been silenced; the same
/// spreadsheet saved as OpenDocument still served them. The rule is tested on
/// the pure function rather than through a zip, and on inputs that SEPARATE
/// each clause.
#[cfg(test)]
mod opendocument_declares_its_hidden_sheets_in_a_style {
    use super::hidden_table_styles;

    #[test]
    fn only_a_table_family_style_declaring_false_counts() {
        let xml = r#"<office:automatic-styles>
          <style:style style:name="ta1" style:family="table">
            <style:table-properties table:display="true"/></style:style>
          <style:style style:name="ta2" style:family="table">
            <style:table-properties table:display="false"/></style:style>
          </office:automatic-styles>"#;
        let hidden = hidden_table_styles(xml);
        assert!(
            hidden.contains("ta2"),
            "the hidden sheet is not seen: {hidden:?}"
        );
        assert!(
            !hidden.contains("ta1"),
            "a visible sheet is declared hidden"
        );
    }

    /// **The bound is the clause that prevents a loss.** `table:display` lives
    /// also on COLUMN and ROW properties, where it talks about something
    /// else. Caught there, it would make a perfectly visible sheet
    /// disappear — the fabrication-for-loss trade already refused
    /// for an animation's `show="0"`.
    #[test]
    fn a_hidden_column_does_not_hide_its_sheet() {
        let xml = r#"<office:automatic-styles>
          <style:style style:name="ta1" style:family="table">
            <style:table-properties table:display="true"/></style:style>
          <style:style style:name="co1" style:family="table-column">
            <style:table-column-properties table:display="false"/></style:style>
          <style:style style:name="ro1" style:family="table-row">
            <style:table-row-properties table:display="false"/></style:style>
          </office:automatic-styles>"#;
        assert!(
            hidden_table_styles(xml).is_empty(),
            "a hidden column or row made the sheet disappear"
        );
    }

    /// A document without a hidden style produces nothing: the filter must not
    /// have any effect where it has nothing to decide.
    #[test]
    fn a_document_without_a_hidden_sheet_declares_nothing() {
        assert!(hidden_table_styles("<office:automatic-styles/>").is_empty());
        assert!(hidden_table_styles("").is_empty());
    }
}

/// **`calamine` versus our in-house reader, on the corpus's ten spreadsheets.**
///
/// The question « reuse a project's code » is at its most
/// precise here: `calamine` is ALREADY a dependency (MIT,
/// checked on 2026-09-11) and already used for legacy `.xls`, while
/// `.xlsx` and `.ods` go through our own zip+XML. Should they be routed to it?
///
/// The measurement costs no new line: `open_workbook_auto_from_rs` recognises
/// the format by itself, so `extract_xls` already reads an xlsx and an ods — only
/// the dispatch prevents it. The probe calls it on the corpus files and
/// compares, needle by needle, with what the product delivers today.
///
/// Ignored: it is a MEASUREMENT, not a ratchet. `--ignored --nocapture`.
#[cfg(test)]
mod calamine_versus_the_in_house_reader {
    use crate::docs::Format;

    #[test]
    #[ignore = "measurement probe, not a ratchet"]
    fn what_each_keeps_from_the_corpus_spreadsheets() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let corpus = root.join("testdata/docs-eval/corpus");
        let manifest: Vec<serde_json::Value> =
            serde_json::from_str(&std::fs::read_to_string(corpus.join("manifest.json")).unwrap())
                .unwrap();
        let norm = |s: &str| {
            s.chars()
                .filter(|c| !c.is_whitespace() && *c != '|')
                .collect::<String>()
        };
        let (mut ours_tot, mut theirs_tot, mut wanted) = (0usize, 0usize, 0usize);
        for e in &manifest {
            let file = e["file"].as_str().unwrap_or_default();
            let fmt = match e["format"].as_str() {
                Some("xlsx") => Format::Xlsx,
                Some("ods") => Format::Ods,
                _ => continue,
            };
            let Ok(bytes) = std::fs::read(corpus.join(file)) else {
                continue;
            };
            let needles: Vec<&str> = e["needles"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            let ours = crate::docs::extract(&bytes, fmt);
            let theirs = super::extract_xls(&bytes);
            let (on, tn) = (norm(&ours), norm(&theirs));
            let o = needles.iter().filter(|n| on.contains(&norm(n))).count();
            let t = needles.iter().filter(|n| tn.contains(&norm(n))).count();
            ours_tot += o;
            theirs_tot += t;
            wanted += needles.len();
            println!(
                "{file:<16} in-house {o}/{} ({} chars) · calamine {t}/{} ({} chars)",
                needles.len(),
                ours.chars().count(),
                needles.len(),
                theirs.chars().count()
            );
        }
        println!("\nTOTAL in-house {ours_tot}/{wanted} · calamine {theirs_tot}/{wanted}");
    }
}

#[cfg(test)]
mod a_date_is_a_number_the_style_names {
    use super::{cell_value, fraction_to_percent, numeric_formats, re, serial_to_date, NumFormat};

    /// The predefined formats appear nowhere in the file: 14 to 22
    /// and 45 to 47 are dates by the specification. Beyond 163 the
    /// workbook declares its own `formatCode`, and that is what must be read.
    #[test]
    fn both_sources_of_truth_are_read() {
        let styles = concat!(
            r#"<styleSheet><numFmts>"#,
            r#"<numFmt numFmtId="164" formatCode="DD/MM/YYYY"/>"#,
            "<numFmt numFmtId=\"165\" formatCode=\"#,##0.00\\ &quot;m&quot;\"/>",
            r#"</numFmts>"#,
            // `cellStyleXfs` has the SAME shape and is not what the cells
            // index: taking it would shift every index.
            r#"<cellStyleXfs count="1"><xf numFmtId="164"/></cellStyleXfs>"#,
            r#"<cellXfs count="4">"#,
            r#"<xf numFmtId="0"/>"#,   // 0: general
            r#"<xf numFmtId="14"/>"#,  // 1: predefined date
            r#"<xf numFmtId="164"/>"#, // 2: declared date
            r#"<xf numFmtId="165"/>"#, // 3: currency whose literal contains « m »
            r#"</cellXfs></styleSheet>"#,
        );
        let f = numeric_formats(styles);
        let d: std::collections::HashSet<usize> = f
            .iter()
            .filter_map(|(i, k)| (*k == NumFormat::Date).then_some(*i))
            .collect();
        assert!(!d.contains(&0), "the general format taken for a date");
        assert!(d.contains(&1), "predefined format 14 missed: {d:?}");
        assert!(d.contains(&2), "declared formatCode missed: {d:?}");
        assert!(
            !d.contains(&3),
            "a quoted « m » literal taken for a month: {d:?}"
        );
    }

    /// Without `cellXfs`, nothing is indexable: returning an empty set leaves the
    /// numbers as is rather than inventing dates.
    #[test]
    fn a_workbook_without_a_style_table_declares_no_date() {
        assert!(numeric_formats("<styleSheet/>").is_empty());
        assert!(numeric_formats("").is_empty());
    }

    /// `t="b"` stores `1`/`0` and displays TRUE/FALSE; a `t="n"` stays the number.
    #[test]
    fn a_boolean_cell_is_true_or_false() {
        let (v, is, t) = (
            re(r"(?s)<v[^>]*>(.*?)</v>"),
            re(r"(?s)<is>(.*?)</is>"),
            re(r"(?s)<t[^>]*>(.*?)</t>"),
        );
        assert_eq!(
            cell_value(r#"r="A1" t="b""#, "<v>1</v>", &[], v, is, t),
            "TRUE"
        );
        assert_eq!(
            cell_value(r#"r="A1" t="b""#, "<v>0</v>", &[], v, is, t),
            "FALSE"
        );
        assert_eq!(cell_value(r#"r="A1""#, "<v>1</v>", &[], v, is, t), "1");
    }

    /// The epoch is 30 December 1899, because Excel believes 1900 is a leap year.
    #[test]
    fn the_serial_number_becomes_an_iso_date() {
        assert_eq!(serial_to_date(46095.0).as_deref(), Some("2026-03-14"));
        assert_eq!(serial_to_date(1.0), None, "before the 1900 gap");
        assert_eq!(serial_to_date(61.0).as_deref(), Some("1900-03-01"));
        assert_eq!(serial_to_date(45658.0).as_deref(), Some("2025-01-01"));
        // A fraction of a day is a time, and it is rendered.
        assert_eq!(
            serial_to_date(45658.5).as_deref(),
            Some("2025-01-01T12:00:00")
        );
        assert_eq!(serial_to_date(f64::NAN), None);
        assert_eq!(serial_to_date(-5.0), None);
        // Less than a day: a time alone (`h:mm` format).
        assert_eq!(
            serial_to_date(0.3381944444444445).as_deref(),
            Some("08:07:00")
        );
        assert_eq!(serial_to_date(0.0).as_deref(), Some("00:00:00"));
    }

    /// **A percentage is stored as a fraction.** The workbook keeps
    /// `0.15` and shows `15%`; delivering `0.15` is wrong by a factor of a hundred for whoever
    /// reads « the margin is 0.15 % ». Same family as the date: the number is
    /// intact, the FORMAT carries the meaning.
    #[test]
    fn a_percentage_is_recognised_and_rendered_as_the_workbook_shows_it() {
        let styles = concat!(
            r#"<styleSheet><numFmts>"#,
            r#"<numFmt numFmtId="166" formatCode="0.0%"/>"#,
            r#"<numFmt numFmtId="167" formatCode="0.00&quot;%&quot;"/>"#,
            r#"</numFmts><cellXfs count="4">"#,
            r#"<xf numFmtId="0"/>"#,   // 0: general
            r#"<xf numFmtId="9"/>"#,   // 1: predefined percentage
            r#"<xf numFmtId="166"/>"#, // 2: declared percentage
            r#"<xf numFmtId="167"/>"#, // 3: a LITERAL « % », not a percentage
            r#"</cellXfs></styleSheet>"#,
        );
        let f = numeric_formats(styles);
        assert_eq!(f.get(&0), None, "the general format taken for a percentage");
        assert_eq!(f.get(&1), Some(&NumFormat::Percent), "predefined 9 missed");
        assert_eq!(f.get(&2), Some(&NumFormat::Percent), "formatCode missed");
        assert_eq!(f.get(&3), None, "a quoted % is a literal, not a percentage");
    }

    /// **An amount keeps its currency**, under the three spellings Excel and
    /// LibreOffice use, before or after the number; a literal that
    /// is not a currency (`"m"`) and the lone locale tag `[$-409]` are
    /// not.
    #[test]
    fn an_amount_keeps_its_currency() {
        use super::columns::currency_of;
        let cases: [(&str, Option<(&str, bool)>); 7] = [
            ("#,##0.00 €", Some(("€", false))),
            ("[$€-40C]#,##0.00", Some(("€", true))),
            (r##"#,##0.00\ "CHF""##, Some(("CHF", false))),
            ("$#,##0.00;[Red]-$#,##0.00", Some(("$", true))),
            (
                "_-* #,##0.00 [$£-809]_-;-* #,##0.00 [$£-809]_-",
                Some(("£", false)),
            ),
            (r##"#,##0.00\ "m""##, None),
            ("[$-409]#,##0.00", None),
        ];
        for (code, want) in cases {
            assert_eq!(
                currency_of(code),
                want.map(|(s, b)| (s.to_string(), b)),
                "{code}"
            );
        }
        let styles = concat!(
            r##"<styleSheet><numFmts><numFmt numFmtId="165" formatCode="#,##0.00 €"/></numFmts>"##,
            r##"<cellXfs><xf numFmtId="0"/><xf numFmtId="165"/></cellXfs></styleSheet>"##,
        );
        assert_eq!(
            numeric_formats(styles).get(&1),
            Some(&NumFormat::Currency {
                symbol: "€".into(),
                before: false
            })
        );
    }

    /// `0.15 * 100.0` is `15.000000000000002`: writing it as is would replace
    /// a right value with a wrong one at the twelfth decimal.
    #[test]
    fn the_fraction_becomes_a_percentage_without_binary_noise() {
        assert_eq!(fraction_to_percent(0.15).as_deref(), Some("15%"));
        // The three values that PROVE the rounding: without it, `0.07 * 100.0`
        // is 7.000000000000001 and `0.29 * 100.0` is 28.999999999999996 —
        // a right value replaced by a wrong one at the fifteenth decimal. A
        // mutant that removes the rounding survived the first series of cases,
        // all exact by chance.
        assert_eq!(fraction_to_percent(0.07).as_deref(), Some("7%"));
        assert_eq!(fraction_to_percent(0.29).as_deref(), Some("29%"));
        assert_eq!(fraction_to_percent(0.57).as_deref(), Some("57%"));
        assert_eq!(fraction_to_percent(0.1234).as_deref(), Some("12.34%"));
        assert_eq!(fraction_to_percent(1.0).as_deref(), Some("100%"));
        assert_eq!(fraction_to_percent(0.0).as_deref(), Some("0%"));
        assert_eq!(fraction_to_percent(-0.075).as_deref(), Some("-7.5%"));
        assert_eq!(fraction_to_percent(f64::INFINITY), None);
    }
}

#[cfg(test)]
mod hidden_parts_are_said {
    use super::hidden_note;

    #[test]
    fn a_count_always_and_labels_only_when_given() {
        assert_eq!(hidden_note("sheet", 0, &[]), "");
        assert_eq!(
            hidden_note("sheet", 2, &[]),
            "\n[2 hidden sheets not shown, as the document hides them]\n"
        );
        assert_eq!(
            hidden_note("slide", 1, &["4".to_string()]),
            "\n[1 hidden slide not shown, as the document hides it: 4]\n"
        );
    }
}
