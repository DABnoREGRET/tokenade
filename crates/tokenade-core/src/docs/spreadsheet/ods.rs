//! **OpenDocument**: the declared sheets, and what the document HIDES.
//!
//! A `.ods` carries its styles differently from a `.xlsx`, and above all it
//! carries table and page styles that HIDE whole sheets. Delivering them
//! without saying so would put into the reading what nobody sees when opening
//! the file; dropping them without saying so would make real data disappear.
//!
//! Formula patterns are here for the same reason: a computed column is
//! summarised by its formula, not by its thousand results.

use super::*;

/// `(sheet name, rows)` per sheet, for the formats that are tables end-to-end.
/// **What the workbook DECLARES: which sheets, under which name, in which
/// file — and which ones it does not show.**
///
/// Two defects in a single read, both from the "fabrication" family rather
/// than "loss".
///
/// **The name was paired by POSITION.** Paths were sorted by the `sheetN.xml`
/// number and matched by index to the names read in `workbook.xml`. But that
/// number is the CREATION order and the `workbook.xml` order is the TAB order:
/// moving a tab is enough to shift the two lists, and a table then comes out
/// under another sheet's name. The map exists and is the one already used for
/// speaker notes: `<sheet r:id="rId3">` and `xl/_rels/workbook.xml.rels` link
/// the name to the file. The relationship, not the number — third time on this
/// axis.
///
/// **A HIDDEN sheet was served.** `state="hidden"` (and `veryHidden`, which
/// only a macro editor brings back) says the workbook does not show it.
/// Serving it amounts to presenting as the document what no reader sees — the
/// same reasoning as for Word's invisible text, applied at sheet scale.
///
/// Returns an EMPTY list when the workbook declares nothing usable, so that the
/// caller falls back to walking the entries: a workbook without `rels` must
/// come out imperfect, not empty.
/// **The style names OpenDocument declares as NOT DISPLAYED.**
///
/// Where the Microsoft workbook writes `state="hidden"` on the sheet itself,
/// OpenDocument stores it in its STYLE: the table carries a
/// `table:style-name="ta2"`, and it is the `ta2` style that declares
/// `<style:table-properties table:display="false"/>`. So styles must be read
/// first and tables afterwards — a single pass over the table cannot answer.
///
/// The original defect is not a forgotten format, it is an ASYMMETRY: a
/// Microsoft spreadsheet hid its hidden sheets while the same spreadsheet saved
/// as OpenDocument served them. The same document, two answers.
///
/// The search is bounded to the `<style:style … style:family="table">` block,
/// not the document: `table:display` also exists on COLUMN and ROW properties,
/// where it means something else, and a `false` caught there would make a
/// perfectly visible sheet disappear.
pub(crate) fn hidden_table_styles(content_xml: &str) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for m in re(r"(?s)<style:style\b[^>]*>.*?</style:style>").find_iter(content_xml) {
        let block = m.as_str();
        let Some(head_end) = block.find('>') else {
            continue;
        };
        let head = &block[..head_end];
        // **The element NAME already carries the family**, and a guard on
        // `style:family="table"` on top of it decides nothing: a mutant removing it
        // survived, on a test that does set a hidden column AND row. They do not
        // pass because they are written `<style:table-column-properties>` and
        // `<style:table-row-properties>`, not because a family rules them out.
        // Removed rather than backed by an artificial entry OpenDocument never
        // writes (a surviving mutant sometimes reveals a dead clause, not a test gap).
        let hidden =
            re(r#"(?s)<style:table-properties\b[^>]*\btable:display="false""#).is_match(block);
        if !hidden {
            continue;
        }
        if let Some(name) = re(r#"(?s)\bstyle:name="([^"]*)""#)
            .captures(head)
            .and_then(|c| c.get(1))
        {
            out.insert(name.as_str().to_string());
        }
    }
    out
}

/// **The style names an OpenDocument presentation declares NOT DISPLAYED.**
///
/// Same mechanism as for a hidden sheet, and not by coincidence: OpenDocument
/// stores this kind of decision in the STYLE, not on the object. The page
/// carries `draw:style-name="dp3"` and it is `dp3` that declares
/// `<style:drawing-page-properties presentation:visibility="hidden"/>`.
///
/// **The spelling is read from real bytes, not guessed.** A previous iteration
/// SET ASIDE this fix for lack of a specimen, and that was the right decision
/// at the time. The specimen can be built: a hand-written `.fodp` converted by
/// LibreOffice yields an `.odp` whose `content.xml` carries exactly this form,
/// and the DTD shipped with LibreOffice (`officedocument/1_0/drawing.mod`)
/// declares `presentation:visibility (visible|hidden)`.
///
/// The search is bounded to `<style:drawing-page-properties>`: nothing else
/// carries this attribute, and a bound set now costs less than a page
/// disappearance to explain later.
pub(crate) fn hidden_page_styles(content_xml: &str) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for m in re(r"(?s)<style:style\b[^>]*>.*?</style:style>").find_iter(content_xml) {
        let block = m.as_str();
        let Some(head_end) = block.find('>') else {
            continue;
        };
        if !re(r#"(?s)<style:drawing-page-properties\b[^>]*\bpresentation:visibility="hidden""#)
            .is_match(block)
        {
            continue;
        }
        if let Some(name) = re(r#"(?s)\bstyle:name="([^"]*)""#)
            .captures(&block[..head_end])
            .and_then(|c| c.get(1))
        {
            out.insert(name.as_str().to_string());
        }
    }
    out
}

pub(crate) fn declared_sheets(workbook_xml: &str, rels_xml: &str) -> Vec<(String, String)> {
    // Attributes in ANY order: openpyxl writes `Type… Target… Id`, and an
    // `Id`-before-`Target` pattern read nothing — the workbook then fell back to
    // walking the zip, where a hidden sheet was served (2026-10-04).
    let id = re(r#"\bId="([^"]*)""#);
    let target = re(r#"\bTarget="([^"]*)""#);
    let targets: std::collections::HashMap<String, String> = re(r"(?s)<Relationship\b[^>]*>")
        .find_iter(rels_xml)
        .filter_map(|m| {
            let t = m.as_str();
            Some((
                id.captures(t)?[1].to_string(),
                target.captures(t)?[1].to_string(),
            ))
        })
        .collect();
    if targets.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for m in re(r"(?s)<sheet\b[^>]*/?>").find_iter(workbook_xml) {
        let tag = m.as_str();
        let attr = |k: &str| {
            re(&format!(r#"(?s)\b{k}="([^"]*)""#))
                .captures(tag)
                .and_then(|c| c.get(1).map(|v| v.as_str().to_string()))
        };
        // `veryHidden` is only reachable through a macro editor; it is shown even
        // less than `hidden`, so written here rather than discovered later.
        if attr("state").is_some_and(|st| st == "hidden" || st == "veryHidden") {
            continue;
        }
        let (Some(name), Some(rid)) = (attr("name"), attr("r:id").or_else(|| attr("id"))) else {
            continue;
        };
        let Some(target) = targets.get(&rid) else {
            continue;
        };
        // Targets are relative to the `workbook.xml` folder, i.e. `xl/`. An
        // absolute target starts with `/`.
        let path = match target.strip_prefix('/') {
            Some(abs) => abs.to_string(),
            None => format!("xl/{}", target.trim_start_matches("./")),
        };
        out.push((decode_entities(&name), path));
    }
    out
}

/// The sheets of an OpenDocument workbook, read from `content.xml` ALONE.
///
/// Pure and zip-free, because the same XML arrives by two routes: the
/// `content.xml` of a `.ods` and the body of a flat `.fods`, which has no
/// container at all. One reader for both, otherwise the same workbook saved two
/// ways gives two answers — an asymmetry already paid for several times.
///
/// The hidden-sheet filter lives here, not in the caller, because a general
/// mechanism only serves through its callers, and this one had only ONE.
/// Two arguments rather than one: in a zipped `.ods`, styles and tables live
/// together in `content.xml`, but in a flat document the styles sit ABOVE
/// `<office:body>`. Passing only the body made `hidden_table_styles` blind, and
/// the hidden sheet came back — fixed before shipping, on the specimen that
/// showed it.
/// How many sheets `ods_tables` leaves out because the document hides them —
/// said, not served (see `hidden_note`).
pub(crate) fn ods_hidden_sheet_count(styles_from: &str, tables_from: &str) -> usize {
    let hidden = hidden_table_styles(styles_from);
    re(r#"(?s)<table:table\b[^>]*>"#)
        .find_iter(tables_from)
        .filter(|t| {
            re(r#"\btable:style-name="([^"]*)""#)
                .captures(t.as_str())
                .is_some_and(|c| hidden.contains(&c[1]))
        })
        .count()
}

pub(crate) fn ods_tables(styles_from: &str, tables_from: &str) -> Vec<(String, Vec<Vec<String>>)> {
    // The workbook route has its own cell reader and so did not go through the
    // filter the text route goes through. A review annotation lives INSIDE
    // `table:table-cell`, so it did not merely get added to the document: it
    // REPLACED the cell value, and `42` arrived as
    // `RELECTEUR-NOMME2026-…ANNOTATION-… 42`. A general mechanism wired to only
    // one of its two callers, again.
    let tables_from = &super::strip_non_prose_elements(tables_from, super::Schema::Odf);
    // Same lesson for links: `<text:a xlink:href>` lives in the cell, and the
    // text route renders it `[text](url)` while this one served `Docs` without
    // its address (2026-10-04, same workbook as `.xlsx`: already linked).
    let tables_from = &crate::docs::markup_marks::mark_links(
        tables_from,
        super::Schema::Odf,
        &Default::default(),
    );
    // The same move as for the Microsoft workbook, in OpenDocument's spelling.
    let hidden = hidden_table_styles(styles_from);
    let style_re = re(r#"(?s)<table:table\b[^>]*\btable:style-name="([^"]*)""#);
    let name_re = re(r#"(?s)<table:table\b[^>]*\btable:name="([^"]*)""#);
    let tbl_re = re(r"(?s)<table:table\b.*?</table:table>");
    let row_re = re(r"(?s)<table:table-row\b[^>]*>(.*?)</table:table-row>");
    // `<table:table-cell/>` is OpenDocument's empty cell; the naive pair dropped
    // it and everything after it shifted column.
    let cell_re = re(&super::element_pattern("table:table-cell"));
    tbl_re
        .find_iter(tables_from)
        .filter(|m| {
            !style_re
                .captures(m.as_str())
                .and_then(|c| c.get(1))
                .is_some_and(|st| hidden.contains(st.as_str()))
        })
        .map(|m| {
            let name = name_re
                .captures(m.as_str())
                .and_then(|c| c.get(1))
                .map(|x| decode_entities(x.as_str()))
                .unwrap_or_else(|| "sheet".into());
            (name, xml_table_rows(m.as_str(), row_re, cell_re, true))
        })
        .collect()
}

pub(crate) fn sheet_tables(bytes: &[u8], fmt: Format) -> Option<Vec<(String, Vec<Vec<String>>)>> {
    let mut zip = ZipArchive::new(Cursor::new(bytes)).ok()?;
    match fmt {
        Format::Xlsx => {
            let shared = shared_strings(&raw_xml_entry(&mut zip, "xl/sharedStrings.xml"));
            let formats = numeric_formats(&raw_xml_entry(&mut zip, "xl/styles.xml"));
            // Real sheet names live in workbook.xml; the entry path
            // (`sheet1.xml`) is an internal detail nobody asked about.
            let wb = raw_xml_entry(&mut zip, "xl/workbook.xml");
            let rels = raw_xml_entry(&mut zip, "xl/_rels/workbook.xml.rels");
            let declared = declared_sheets(&wb, &rels);
            let pairs: Vec<(String, String)> = if declared.is_empty() {
                // The workbook DECLARES nothing usable (no rels, or a form we cannot
                // read): fall back to walking the entries rather than returning an empty
                // workbook. A fallback that loses everything would be worse than the
                // approximate pairing it replaces.
                // Each declared sheet with whether it is hidden: the pairing is
                // by POSITION, so a hidden one keeps its slot and is dropped
                // after — the same decision as the declared path.
                let names: Vec<(String, bool)> = re(r"(?s)<sheet\b[^>]*>")
                    .find_iter(&wb)
                    .filter_map(|m| {
                        let t = m.as_str();
                        let name = re(r#"\bname="([^"]*)""#).captures(t)?[1].to_string();
                        let hidden =
                            t.contains("state=\"hidden\"") || t.contains("state=\"veryHidden\"");
                        Some((decode_entities(&name), hidden))
                    })
                    .collect();
                let mut paths: Vec<String> = (0..zip.len())
                    .filter_map(|i| zip.by_index(i).ok().map(|f| f.name().to_string()))
                    .filter(|n| n.starts_with("xl/worksheets/sheet") && n.ends_with(".xml"))
                    .collect();
                paths.sort_by_key(|n| (slide_number(n), n.clone()));
                paths
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| !names.get(*i).is_some_and(|(_, hidden)| *hidden))
                    .map(|(i, p)| {
                        (
                            names
                                .get(i)
                                .map(|(n, _)| n.clone())
                                .unwrap_or_else(|| p.clone()),
                            p.clone(),
                        )
                    })
                    .collect()
            } else {
                declared
            };
            Some(
                pairs
                    .iter()
                    .map(|(name, path)| {
                        (name.clone(), {
                            let xml = raw_xml_entry(&mut zip, path);
                            let rels = raw_xml_entry(&mut zip, &super::links::rels_path(path));
                            super::links::link_cells(
                                sheet_rows(&xml, &shared, &formats),
                                &xml,
                                &rels,
                            )
                        })
                    })
                    .collect(),
            )
        }
        Format::Ods => {
            let xml = raw_xml_entry(&mut zip, "content.xml");
            Some(ods_tables(&xml, &xml))
        }
        _ => None,
    }
}

/// Formula PATTERNS, folded per column: `H2:H2574 = =B*C` rather than 2573
/// copies of the same formula. The shape is what a reader needs; the repetition
/// is what blows the budget.
pub(crate) fn formula_patterns(bytes: &[u8], fmt: Format) -> Option<String> {
    if fmt != Format::Xlsx {
        return None;
    }
    let mut zip = ZipArchive::new(Cursor::new(bytes)).ok()?;
    let paths: Vec<String> = (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok().map(|f| f.name().to_string()))
        .filter(|n| n.starts_with("xl/worksheets/sheet") && n.ends_with(".xml"))
        .collect();
    let cell_re = re(r#"(?s)<c\b([^>]*)>(.*?)</c>"#);
    let f_re = re(r"(?s)<f[^>]*>(.*?)</f>");
    let ref_re = re(r#"(?s)\br="([A-Za-z]+)(\d+)""#);
    // column letter → (first row, last row, first formula seen)
    let mut per_col: std::collections::BTreeMap<String, (u32, u32, String)> = Default::default();
    for p in &paths {
        let xml = raw_xml_entry(&mut zip, p);
        for c in cell_re.captures_iter(&xml) {
            let attrs = c.get(1).map(|m| m.as_str()).unwrap_or("");
            let inner = c.get(2).map(|m| m.as_str()).unwrap_or("");
            let Some(f) = f_re.captures(inner).and_then(|m| m.get(1)) else {
                continue;
            };
            let formula = decode_entities(f.as_str());
            if formula.trim().is_empty() {
                continue;
            }
            let Some(rf) = ref_re.captures(attrs) else {
                continue;
            };
            let (Some(letters), Some(row)) = (rf.get(1), rf.get(2)) else {
                continue;
            };
            let row: u32 = row.as_str().parse().unwrap_or(0);
            per_col
                .entry(letters.as_str().to_string())
                .and_modify(|e| {
                    e.0 = e.0.min(row);
                    e.1 = e.1.max(row);
                })
                .or_insert((row, row, formula));
        }
    }
    if per_col.is_empty() {
        return None;
    }
    let mut out = String::from("\nformulas (folded per column):\n");
    for (col, (a, b, f)) in per_col.iter().take(30) {
        let _ = writeln!(out, "  {col}{a}:{col}{b} = {f}");
    }
    Some(out)
}
