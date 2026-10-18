//! **The skeleton served instead of the grid.**
//!
//! A workbook read cell by cell is mostly repetition. What an agent wants to
//! know fits in three sentences: which sheets, which columns, and of what
//! kind — then the values that cannot be inferred.
//!
//! The read recipe is declared here because it is the same question asked
//! backwards: what one should have asked to directly get what was just
//! rebuilt.

use super::*;

/// Longest example value we quote per column.
const SKEL_EXAMPLE_CHARS: usize = 60;
/// Most columns we describe before folding the tail into a count.
const SKEL_MAX_COLS: usize = 60;

/// A map of a spreadsheet: sheets, columns, where the data actually is, one
/// example per column, and formula patterns — WITHOUT the data.
///
/// Retrieval is the wrong tool for a spreadsheet and the numbers say so: asked
/// "which people have a job title mentioning coach", prompt-scoped extraction
/// returned 5 of the 296 matching rows in 2206 tokens. That is not a tuning
/// problem — RAG answers "here are passages that resemble your question", while
/// a table question is "give me EVERY row where", and no 1500-token budget
/// returns 296 rows. The same file dumped whole is 6.9M tokens.
///
/// So do for a spreadsheet what `tokenade skeleton` does for code: hand over the
/// MAP, not the content, and let the agent pull exactly what it needs. The file
/// never enters the context; the answer is exact instead of approximate.
///
/// `data` is `(hash, stash_path)` of the already-extracted table, and it is what
/// makes the map ACTIONABLE — see [`read_recipe`]. Without it the map still
/// prints, minus the "how to read it" block.
///
/// Example values go through redaction: real spreadsheets are payroll exports
/// and scraped profiles, and a "helpful sample" is how a secret leaks into a
/// transcript.
pub fn spreadsheet_skeleton(
    bytes: &[u8],
    fmt: Format,
    path_hint: &str,
    data: Option<(&str, &str)>,
) -> Option<String> {
    let sheets = sheet_tables(bytes, fmt)?;
    if sheets.is_empty() {
        return None;
    }
    // Name the file in the banner: an agent juggling several documents needs to
    // know which map this is, and the recipe below points at a stash hash rather
    // than at a path it could recognise.
    let mut out =
        format!("[tokenade:spreadsheet skeleton — structure only, no data] {path_hint}\n");
    for (name, rows) in &sheets {
        let n_rows = rows.len();
        let n_cols = rows.iter().map(Vec::len).max().unwrap_or(0);
        if n_rows == 0 || n_cols == 0 {
            continue;
        }
        // Leading TITLE rows come first. A banner merged across the full width
        // ("Rapport trimestriel" over A1:C1) is a caption, not column names —
        // but after merge propagation it looks like a perfectly good header row,
        // and taking it as one pushed everything down by one: every column was
        // titled "Rapport trimestriel" while the REAL headers (Mois, Ventes,
        // Marge) were served as example DATA, types contradicting their own
        // examples. Skipping them costs one row and keeps the map truthful.
        let title_rows = leading_title_rows(rows, n_cols);
        let titles: Vec<&str> = rows[..title_rows]
            .iter()
            .filter_map(|r| r.first().map(|s| s.trim()))
            .collect();
        let body = &rows[title_rows..];
        // The first non-title row is a header when its cells are non-empty text;
        // otherwise the sheet is data from the start and columns are described
        // by letter alone.
        let has_header = body.first().is_some_and(|h| {
            h.iter().filter(|c| !c.trim().is_empty()).count() * 2 > n_cols
                && h.iter().all(|c| c.trim().parse::<f64>().is_err())
        });
        let header: &[String] = if has_header { &body[0] } else { &[] };
        let data_start = title_rows + usize::from(has_header);
        let _ = write!(
            out,
            "\nsheet {name:?}{}: {n_rows} rows x {n_cols} cols{}\n",
            if titles.is_empty() {
                String::new()
            } else {
                format!(" — {}", titles.join(" / "))
            },
            if has_header {
                format!(" (row {} = header)", data_start)
            } else {
                String::new()
            }
        );
        for col in 0..n_cols.min(SKEL_MAX_COLS) {
            let values: Vec<&str> = rows
                .iter()
                .skip(data_start)
                .filter_map(|r| r.get(col))
                .map(String::as_str)
                .filter(|v| !v.trim().is_empty())
                .collect();
            let ranges = fill_ranges(rows, col, data_start);
            let filled_pct = values.len() * 100 / (n_rows - data_start).max(1);
            let title = header
                .get(col)
                .map(|h| h.trim())
                .filter(|h| has_header && !h.is_empty())
                .unwrap_or("(no header)");
            let example = values
                .first()
                .map(|v| {
                    let red = crate::redact::redact(v);
                    let one = red.split_whitespace().collect::<Vec<_>>().join(" ");
                    if one.chars().count() > SKEL_EXAMPLE_CHARS {
                        format!(
                            "{}…",
                            one.chars().take(SKEL_EXAMPLE_CHARS).collect::<String>()
                        )
                    } else {
                        one
                    }
                })
                .unwrap_or_default();
            let widest = values.iter().map(|v| v.chars().count()).max().unwrap_or(0);
            let wide = if widest > 500 {
                format!(" · max {widest} chars — WIDE, select this column only if needed")
            } else {
                String::new()
            };
            let _ = writeln!(
                out,
                "  {:>3} {title:?}: {} · {filled_pct}% filled · {}{wide}",
                col_letter(col),
                column_kind(&values),
                fmt_ranges(&ranges, n_rows),
            );
            if !example.is_empty() {
                let _ = writeln!(out, "       e.g. {example:?}");
            }
        }
        if n_cols > SKEL_MAX_COLS {
            let _ = writeln!(out, "  … +{} more columns", n_cols - SKEL_MAX_COLS);
        }
    }
    if let Some(f) = formula_patterns(bytes, fmt) {
        out.push_str(&f);
    }
    if let Some((hash, stash)) = data {
        out.push_str(&read_recipe(hash, stash));
    }
    Some(out)
}

/// The "how to read it" half of a spreadsheet skeleton. A map with no way to
/// act on it sends the agent straight back to reading the whole file, so this
/// block decides whether the skeleton helps or merely delays the damage.
///
/// It points at the ALREADY-EXTRACTED table (stashed by the caller), not at the
/// original workbook, for three reasons that each cost an agent a wrong answer:
///
/// 1. **The obvious advice does not run.** The first version of this block said
///    `openpyxl.load_workbook(...)`. Measured on the machine that emitted it:
///    openpyxl is absent, and pandas cannot read xlsx without it — `read_excel`
///    raises `ImportError`. An agent that copies a broken recipe falls back to
///    reading the 6.9M-token file, so the nudge would cause the harm it exists
///    to prevent. The stash is a text file: stdlib `open()` always works, on
///    every OS, with no install and no network.
/// 2. **A naive reader is silently wrong.** xlsx keeps cell text in a shared
///    string table (`t="s"` holds an INDEX), and omits empty cells, so a
///    hand-rolled `zipfile`+`ElementTree` script yields integers where names
///    belong and shifts values under the wrong header. Those are exactly the
///    bugs already fixed in this module — the stash holds the parsed result.
/// 3. **Filtering must be exhaustive.** `expand-ref --grep` returns EVERY
///    matching row (the 296, not the 5 retrieval offered) and filters
///    server-side, so only the matches are billed.
///
/// `.active`/"first sheet" defaults are deliberately absent: this workbook may
/// have several sheets, and quietly picking one is how a confident wrong answer
/// gets produced.
pub(crate) fn read_recipe(hash: &str, stash_path: &str) -> String {
    format!(
        "\n[the sheet is parsed and cached — pull only what you need, it stays out of context]\n\
         • find rows (every match, filtered before it reaches you; --grep repeats \
         for several patterns, results grouped per pattern):\n\
         \x20   tokenade expand-ref {hash} --grep \"<what you're looking for>\"\n\
         • ask it a question instead (several at once, comma-separated):\n\
         \x20   tokenade expand-ref {hash} --prompt \"q1, q2\"\n\
         • count / sum / group (plain stdlib, only stdout comes back):\n\
         \x20   tokenade exec --lang python --script 'rows=[l.strip().strip(\"|\").split(\"|\") \
         for l in open({stash_path:?}) if \"|\" in l]; print(len(rows))'\n\
         • read it whole only if you truly need every row: tokenade expand-ref {hash} --raw\n"
    )
}
