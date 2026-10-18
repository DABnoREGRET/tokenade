//! **A cell that links somewhere says where.**
//!
//! A workbook's hyperlinks live beside its cells, not in them: the sheet lists
//! `<hyperlink ref="A7" r:id="rId1"/>` and the sheet's relationships file holds
//! `rId1 → https://…`. Read cell by cell, `Docs` came out as `Docs` and the
//! address was gone (2026-10-04, an openpyxl workbook) — while a `.docx` link
//! has long read `[text](url)`. Same spelling here. A link INSIDE the workbook
//! (`location="Sheet2!A1"`) points at cells already served, and is left alone.

use super::*;

/// The relationships part of a worksheet part: `xl/worksheets/sheet1.xml` →
/// `xl/worksheets/_rels/sheet1.xml.rels`.
pub(super) fn rels_path(sheet_path: &str) -> String {
    match sheet_path.rsplit_once('/') {
        Some((dir, file)) => format!("{dir}/_rels/{file}.rels"),
        None => format!("_rels/{sheet_path}.rels"),
    }
}

/// Rewrite each externally-linked cell of `rows` (one entry per `<row>` of
/// `sheet_xml`, in document order) as `[value](url)`.
pub(super) fn link_cells(
    mut rows: Vec<Vec<String>>,
    sheet_xml: &str,
    rels_xml: &str,
) -> Vec<Vec<String>> {
    let targets: std::collections::HashMap<String, String> = re(r"(?s)<Relationship\b[^>]*>")
        .find_iter(rels_xml)
        .filter(|r| r.as_str().contains(r#"TargetMode="External""#))
        .filter_map(|r| {
            let id = re(r#"\bId="([^"]*)""#)
                .captures(r.as_str())?
                .get(1)?
                .as_str()
                .to_string();
            let target = re(r#"\bTarget="([^"]*)""#)
                .captures(r.as_str())?
                .get(1)?
                .as_str();
            Some((id, decode_entities(target)))
        })
        .collect();
    if targets.is_empty() {
        return rows;
    }
    let row_of: std::collections::HashMap<usize, usize> = re(r#"(?s)<row\b[^>]*>"#)
        .find_iter(sheet_xml)
        .enumerate()
        .filter_map(|(i, r)| {
            Some((
                re(r#"\br="(\d+)""#).captures(r.as_str())?[1].parse().ok()?,
                i,
            ))
        })
        .collect();
    for link in re(r"(?s)<hyperlink\b[^>]*>").find_iter(sheet_xml) {
        let attr = |name: &str| {
            re(&format!(r#"\b{name}="([^"]*)""#))
                .captures(link.as_str())
                .map(|c| c[1].to_string())
        };
        let (Some(cell), Some(url)) = (
            attr("ref"),
            attr("r:id").and_then(|id| targets.get(&id).cloned()),
        ) else {
            continue;
        };
        // A range (`A7:B7`) is anchored on its first cell.
        let first = cell.split(':').next().unwrap_or("");
        let digits: String = first.chars().filter(char::is_ascii_digit).collect();
        let (Some(col), Some(&row)) = (
            col_index(first),
            digits.parse::<usize>().ok().and_then(|n| row_of.get(&n)),
        ) else {
            continue;
        };
        let Some(line) = rows.get_mut(row) else {
            continue;
        };
        if line.len() <= col {
            line.resize(col + 1, String::new());
        }
        let text = if line[col].is_empty() {
            url.clone()
        } else {
            line[col].clone()
        };
        line[col] = format!("[{text}]({url})");
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use zip::{write::SimpleFileOptions, ZipWriter};

    const SHEET: &str = r#"<worksheet><sheetData><row r="1"><c r="A1" t="str"><v>Item</v></c></row><row r="7"><c r="A7" t="str"><v>Docs</v></c><c r="C7" t="str"><v>Jump</v></c></row></sheetData><hyperlinks><hyperlink ref="A7" r:id="rId1"/><hyperlink ref="B7" r:id="rId2"/><hyperlink ref="C7" location="Other!A1"/></hyperlinks></worksheet>"#;
    const RELS: &str = r#"<Relationships><Relationship Id="rId1" Type="h" Target="https://example.com/sheet?a=1&amp;b=2" TargetMode="External"/><Relationship Id="rId2" Type="h" Target="https://example.com/empty" TargetMode="External"/></Relationships>"#;

    #[test]
    fn a_linked_cell_carries_its_address_by_its_row_number() {
        let rows = vec![
            vec!["Item".to_string()],
            vec!["Docs".to_string(), String::new(), "Jump".to_string()],
        ];
        let out = link_cells(rows, SHEET, RELS);
        assert_eq!(out[1][0], "[Docs](https://example.com/sheet?a=1&b=2)");
        assert_eq!(
            out[1][1], "[https://example.com/empty](https://example.com/empty)",
            "an empty linked cell shows its url"
        );
        assert_eq!(out[1][2], "Jump", "an in-workbook link is left alone");
        assert_eq!(out[0][0], "Item");
        assert_eq!(
            rels_path("xl/worksheets/sheet3.xml"),
            "xl/worksheets/_rels/sheet3.xml.rels"
        );
    }

    /// OpenDocument keeps the link IN the cell (`<text:a xlink:href>`); the
    /// spreadsheet route now renders it as the text route does.
    #[test]
    fn an_ods_cell_link_is_rendered_too() {
        let doc = r#"<?xml version="1.0"?><office:document xmlns:office="o" xmlns:table="t" xmlns:text="x" xmlns:xlink="l"><office:body><office:spreadsheet><table:table table:name="S"><table:table-row><table:table-cell><text:p>Item</text:p></table:table-cell></table:table-row><table:table-row><table:table-cell><text:p><text:a xlink:href="https://example.com/sheet" xlink:type="simple">Docs</text:a></text:p></table:table-cell></table:table-row></table:table></office:spreadsheet></office:body></office:document>"#;
        let out = crate::docs::extract(doc.as_bytes(), crate::docs::Format::FlatOdf);
        assert!(out.contains("[Docs](https://example.com/sheet)"), "{out}");
    }

    /// `table:number-columns-repeated` is that many cells: two empty cells
    /// written once moved the total under the wrong header.
    #[test]
    fn a_repeated_ods_cell_holds_its_columns() {
        let cell = |v: &str| format!("<table:table-cell><text:p>{v}</text:p></table:table-cell>");
        let rows = [
            format!("{}{}{}{}", cell("Item"), cell("Qty"), cell("Price"), cell("Total")),
            format!(r#"{}<table:table-cell table:number-columns-repeated="2"/>{}<table:table-cell table:number-columns-repeated="16379"/>"#, cell("Sum"), cell("47.5")),
            r#"<table:table-cell table:number-columns-repeated="5"/><table:table-cell table:number-columns-repeated="16379"/>"#.to_string(),
            format!(r#"<table:table-cell table:number-columns-repeated="2"><text:p>x</text:p></table:table-cell>{}"#, cell("y")),
        ];
        let body: String = rows
            .iter()
            .map(|r| format!("<table:table-row>{r}</table:table-row>"))
            .collect();
        let doc = format!(
            r#"<?xml version="1.0"?><office:document xmlns:office="o" xmlns:table="t" xmlns:text="x"><office:body><office:spreadsheet><table:table table:name="S">{body}</table:table></office:spreadsheet></office:body></office:document>"#
        );
        let out = crate::docs::extract(doc.as_bytes(), crate::docs::Format::FlatOdf);
        assert!(out.contains("| Sum |  |  | 47.5 |"), "{out}");
        assert!(
            out.contains("| x | x | y |"),
            "a repeated value is repeated: {out}"
        );
        assert!(
            !out.contains("|  |  |  |  |"),
            "an empty row or a trailing column appeared: {out}"
        );
    }

    /// The whole route: a workbook read by `extract` shows the link.
    #[test]
    fn an_xlsx_read_shows_its_links() {
        let mut buf = Vec::new();
        {
            let mut w = ZipWriter::new(Cursor::new(&mut buf));
            let opts: SimpleFileOptions = Default::default();
            for (name, body) in [
                ("xl/worksheets/sheet1.xml", SHEET),
                ("xl/worksheets/_rels/sheet1.xml.rels", RELS),
            ] {
                w.start_file(name, opts).unwrap();
                w.write_all(body.as_bytes()).unwrap();
            }
            w.finish().unwrap();
        }
        let out = crate::docs::extract(&buf, crate::docs::Format::Xlsx);
        assert!(
            out.contains("[Docs](https://example.com/sheet?a=1&b=2)"),
            "{out}"
        );
    }
}
