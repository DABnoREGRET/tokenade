//! CSV / TSV compaction.
//!
//! Agents often receive multi-megabyte CSVs from `kubectl get -o csv`, `gh`
//! exports, or data-pipeline dumps. The signal is usually:
//!   - the column names,
//!   - a few representative rows,
//!   - the total row count.
//!
//! [`compact_csv`] / [`compact_tsv`] do exactly that:
//!   * drop columns that are 100% empty,
//!   * keep the header,
//!   * keep the first N data rows (default 30),
//!   * truncate any cell longer than 80 chars,
//!   * report the elided row count as a trailing comment line.
//!
//! Parsing is dependency-free: a small hand-rolled state machine handles
//! RFC 4180 quoting (`""` escape inside `"…"`), embedded newlines in quoted
//! fields, and the common CRLF/LF variants. Survives malformed input by
//! falling back to naive split — never panics.

use std::fmt::Write;

#[derive(Debug, Clone)]
pub struct CompactOptions {
    pub keep_rows: usize,
    pub cell_cap: usize,
    pub drop_empty_columns: bool,
    pub delimiter: u8,
}

impl Default for CompactOptions {
    fn default() -> Self {
        Self {
            keep_rows: 30,
            cell_cap: 80,
            drop_empty_columns: true,
            delimiter: b',',
        }
    }
}

pub fn compact_csv(input: &str) -> String {
    compact_with(
        input,
        &CompactOptions {
            delimiter: sniff_delimiter(input),
            ..Default::default()
        },
    )
}

/// `,` unless the HEADER says `;` — the separator of every spreadsheet export
/// in a locale whose decimal mark is the comma (fr, de, es, it…). Parsed with
/// `,`, `Client;Montant;Date` was one column and `10,50` split a number in two.
fn sniff_delimiter(input: &str) -> u8 {
    let header = input
        .trim_start_matches('\u{feff}')
        .lines()
        .next()
        .unwrap_or("");
    let (mut commas, mut semis, mut quoted) = (0usize, 0usize, false);
    for c in header.chars() {
        match c {
            '"' => quoted = !quoted,
            ',' if !quoted => commas += 1,
            ';' if !quoted => semis += 1,
            _ => {}
        }
    }
    if semis > commas {
        b';'
    } else {
        b','
    }
}

pub fn compact_tsv(input: &str) -> String {
    compact_with(
        input,
        &CompactOptions {
            delimiter: b'\t',
            ..Default::default()
        },
    )
}

pub fn compact_with(input: &str, opts: &CompactOptions) -> String {
    if input.is_empty() {
        return String::new();
    }
    let mut rows = parse(input, opts.delimiter);
    if rows.is_empty() {
        return input.to_string();
    }
    // Security: a cookie export (DevTools Application/Network panel, copied
    // as TSV or CSV) carries every cookie's raw VALUE. Detect the cookie
    // signature from the header and redact the `Value` column of the data
    // rows in place — header and all other columns are left untouched.
    // Gated by signature so a generic `Name,Value` metrics/config table is
    // NOT touched (would otherwise erase legitimate data).
    if let Some(vcol) = rows
        .first()
        .and_then(|h| crate::redact::cookie_value_col(h))
    {
        for row in rows.iter_mut().skip(1) {
            if let Some(cell) = row.get_mut(vcol) {
                let n = cell.chars().count();
                if n > 0 && !cell.starts_with("<redacted") {
                    *cell = format!("<redacted {n} chars>");
                }
            }
        }
    }
    // **A table too big to print is PROFILED, like the same table in a
    // workbook.** Its head and `# … N more rows elided` said nothing of the
    // rows not printed: 2 000 rows of amounts came back as 30 rows, where the
    // `.xlsx` of the same data reads `Montant | float | 10.50 … 19990.50 · mean
    // 10000.50` (2026-10-03). One profiler for both spellings. No stash here:
    // the layer that called this keeps the original and says where.
    if crate::table_profile::should_profile(&rows) && !tk_base::delivery::fits(input) {
        return crate::table_profile::render(&rows, &mut |_| {
            "the original rows, unchanged (see the recovery note)".to_string()
        });
    }
    let n_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);

    // Find which columns are entirely empty across data rows (header excluded).
    let mut drop_col = vec![false; n_cols];
    if opts.drop_empty_columns && rows.len() > 1 {
        for (col, slot) in drop_col.iter_mut().enumerate() {
            *slot = rows[1..]
                .iter()
                .all(|r| r.get(col).is_none_or(|c| c.trim().is_empty()));
        }
    }

    // Render header + first keep_rows data rows.
    let mut out = String::new();
    let delim = opts.delimiter as char;
    let total_data = rows.len().saturating_sub(1);

    let render_row = |out: &mut String, row: &[String]| {
        let mut first = true;
        for (i, cell) in row.iter().enumerate() {
            if drop_col.get(i).copied().unwrap_or(false) {
                continue;
            }
            if !first {
                out.push(delim);
            }
            first = false;
            out.push_str(&csv_encode(&truncate(cell, opts.cell_cap), opts.delimiter));
        }
        out.push('\n');
    };

    if let Some(header) = rows.first() {
        render_row(&mut out, header);
    }
    // Every row while the RENDERED table fits: a 60-row `gh issue list`
    // (6.3 KB, TSV) was cut to 30 and `# … 29 more rows elided` (2026-10-03).
    // The cap is for a table that does not.
    let mut whole = out.clone();
    for row in rows.iter().skip(1) {
        render_row(&mut whole, row);
    }
    let keep = if tk_base::delivery::fits(&whole) {
        total_data
    } else {
        opts.keep_rows.min(total_data)
    };
    for row in rows.iter().skip(1).take(keep) {
        render_row(&mut out, row);
    }
    if total_data > keep {
        let _ = writeln!(
            out,
            "# … {} more rows elided · {} total",
            total_data - keep,
            total_data
        );
    }
    // Name the dropped columns, don't just count them. The drop is decided on
    // the DATA rows (`rows[1..]`) but applied to the header too, so a column
    // that is blank in every row takes its HEADER LABEL with it — and a count
    // alone leaves the reader unable to tell that `notes` ever existed, let
    // alone ask for it. Naming them costs a few tokens and is the difference
    // between "something was removed" and "this was removed".
    let dropped: Vec<&str> = drop_col
        .iter()
        .enumerate()
        .filter(|(_, d)| **d)
        .map(|(i, _)| {
            rows.first()
                .and_then(|h| h.get(i))
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .unwrap_or("<unnamed>")
        })
        .collect();
    if !dropped.is_empty() {
        let _ = writeln!(
            out,
            "# {} empty column(s) dropped: {}",
            dropped.len(),
            dropped.join(", ")
        );
    }
    out
}

fn truncate(s: &str, cap: usize) -> String {
    if s.chars().count() <= cap {
        return s.to_string();
    }
    let head: String = s.chars().take(cap).collect();
    format!("{head}…")
}

/// Encode a single CSV cell, quoting when the delimiter, quote, CR, or LF is
/// present, and doubling embedded quotes per RFC 4180.
fn csv_encode(cell: &str, delim: u8) -> String {
    let dc = delim as char;
    if cell.contains(dc) || cell.contains('"') || cell.contains('\n') || cell.contains('\r') {
        let escaped = cell.replace('"', "\"\"");
        format!("\"{escaped}\"")
    } else {
        cell.to_string()
    }
}

/// Minimal RFC 4180 parser — `"a,""b"",c"` → `a,"b",c`. Streaming-friendly,
/// allocation-aware, and never panics on malformed input (best-effort).
fn parse(input: &str, delim: u8) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut cell = String::new();
    let mut in_quotes = false;
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if matches!(chars.peek(), Some('"')) {
                    chars.next();
                    cell.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                cell.push(c);
            }
        } else if c == '"' && cell.is_empty() {
            in_quotes = true;
        } else if c as u32 == delim as u32 {
            row.push(std::mem::take(&mut cell));
        } else if c == '\n' {
            row.push(std::mem::take(&mut cell));
            rows.push(std::mem::take(&mut row));
        } else if c == '\r' {
            // swallow CR; LF below ends the row
        } else {
            cell.push(c);
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_empty_columns_and_names_them() {
        let raw = "a,b,c\n1,,3\n4,,6\n7,,9\n";
        let out = compact_csv(raw);
        assert!(out.contains("a,c"));
        // `b` is gone from the TABLE — the header line no longer carries it.
        assert_eq!(out.lines().next(), Some("a,c"));
        // …but the note says which column left, not merely how many. A count
        // alone cannot be acted on: the reader does not learn that `b` existed.
        assert!(
            out.contains("1 empty column(s) dropped: b"),
            "dropped column must be named: {out}"
        );
    }

    #[test]
    fn truncates_long_cells() {
        let big = "x".repeat(500);
        let raw = format!("name\n{big}\n");
        let out = compact_csv(&raw);
        assert!(out.contains('…'));
        assert!(out.len() < raw.len() / 2);
    }

    /// Fitting is judged on the RENDERED table: an all-empty column is not
    /// delivered, so it does not count against the budget.
    #[test]
    fn fitting_is_judged_on_the_rendered_table() {
        let pad = ",".repeat(40);
        let mut t = format!("id,name{pad}\n");
        for i in 0..60 {
            t.push_str(&format!("{i},issue number {i}{pad}\n"));
        }
        let (raw, dense) = (
            crate::tokenizer::estimate(&t),
            crate::tokenizer::estimate(&compact_csv(&t)),
        );
        assert!(dense < raw, "{dense} vs {raw}");
        let _g = tk_base::delivery::threshold_on_this_thread((raw + dense) / 2);
        let out = compact_csv(&t);
        assert!(!out.contains("elided"), "{out}");
    }

    /// A table that fits keeps every row.
    #[test]
    fn a_table_that_fits_keeps_every_row() {
        let mut t = String::from("id,name,state\n");
        for i in 0..60 {
            t.push_str(&format!("{i},issue number {i},open\n"));
        }
        let out = compact_csv(&t);
        assert!(!out.contains("elided"), "{out}");
        assert!(out.contains("\n59,issue number 59,open"), "{out}");
    }

    #[test]
    fn caps_rows_with_count() {
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        let mut raw = String::from("a\n");
        for i in 0..200 {
            let _ = writeln!(raw, "{i}");
        }
        let out = compact_csv(&raw);
        // Too big to print: profiled, like the same table in a workbook, and
        // the rows not printed are COUNTED.
        assert!(out.contains("200 rows × 1 columns"), "{out}");
        assert!(out.contains("row(s) not printed"), "{out}");
        assert!(!out.contains("\n100\n"), "{out}");
    }

    /// `;` is the separator of every spreadsheet export where the decimal mark
    /// is a comma: read with `,`, the header was one column.
    #[test]
    fn a_semicolon_header_is_split_on_semicolons() {
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        let mut raw = String::from("\u{feff}Client;Montant;Date\n");
        for i in 0..200 {
            let _ = writeln!(raw, "Société {i};{i},50;2026-01-0{}", 1 + i % 9);
        }
        let out = compact_csv(&raw);
        assert!(out.contains("200 rows × 3 columns"), "{out}");
        assert!(out.contains("| Montant |"), "{out}");
    }

    #[test]
    fn handles_quoted_fields_with_commas() {
        let raw = "a,b\n\"hello, world\",2\n";
        let out = compact_csv(raw);
        assert!(out.contains("hello, world"));
    }

    #[test]
    fn malformed_does_not_panic() {
        let raw = "a,b\n\"unclosed quote forever\n more data";
        // No panic; output is best-effort.
        let _ = compact_csv(raw);
    }

    #[test]
    fn tsv_uses_tab() {
        let raw = "a\tb\tc\n1\t\t3\n4\t\t6\n";
        let out = compact_tsv(raw);
        assert!(out.contains("a\tc"));
    }

    #[test]
    fn redacts_devtools_cookie_tsv_value_column() {
        // Realistic DevTools "copy" of the Application → Cookies table (tab
        // separated). The Value column must be redacted; every other column
        // and the header stay intact.
        let raw = "Name\tValue\tDomain\tPath\tExpires\tSize\tHttpOnly\tSameSite\n\
                   sid\tSECRETSESSION123\t.example.com\t/\t2027-01-01\t18\ttrue\tLax\n\
                   csrf\tTOP_SECRET_TOKEN\t.example.com\t/\t2027-01-01\t20\tfalse\tStrict\n";
        let out = compact_tsv(raw);
        // Raw cookie values gone.
        assert!(!out.contains("SECRETSESSION123"), "sid value leaked: {out}");
        assert!(
            !out.contains("TOP_SECRET_TOKEN"),
            "csrf value leaked: {out}"
        );
        assert!(out.contains("<redacted"), "no redaction marker: {out}");
        // Header + other columns preserved.
        assert!(out.contains("Name\tValue\tDomain"), "header changed: {out}");
        assert!(out.contains("sid\t"), "cookie name lost: {out}");
        assert!(out.contains(".example.com"), "domain lost: {out}");
        assert!(out.contains("Lax"), "samesite lost: {out}");
    }

    #[test]
    fn redacts_cookie_csv_value_column() {
        // Comma-separated cookie export must be covered too.
        let raw = "Name,Value,Domain,HttpOnly\nauth,LEAKME_PLEASE,x.com,true\n";
        let out = compact_csv(raw);
        assert!(!out.contains("LEAKME_PLEASE"), "value leaked: {out}");
        assert!(out.contains("auth"), "name lost: {out}");
        assert!(out.contains("x.com"), "domain lost: {out}");
    }

    #[test]
    fn generic_name_value_table_not_touched() {
        // FALSE-POSITIVE GUARD: a plain two-column metrics/config table with
        // `Name`/`Value` headers and NO cookie attribute must keep its values.
        let raw = "Name\tValue\nrequests_total\t42815\nerror_rate\t0.03\n";
        let out = compact_tsv(raw);
        assert!(
            out.contains("42815"),
            "metrics value wrongly redacted: {out}"
        );
        assert!(
            out.contains("0.03"),
            "metrics value wrongly redacted: {out}"
        );
        assert!(!out.contains("<redacted"), "no redaction expected: {out}");
    }

    #[test]
    fn cookie_value_redaction_is_idempotent() {
        let raw = "Name\tValue\tDomain\tHttpOnly\nsid\tSECRET\t.x.com\ttrue\n";
        let once = compact_tsv(raw);
        // Feed the already-redacted output back through: the redacted cell
        // (`<redacted N chars>`) must not be re-wrapped.
        let twice = compact_tsv(&once);
        assert_eq!(once, twice, "cookie redaction not idempotent: {once}");
    }
}
