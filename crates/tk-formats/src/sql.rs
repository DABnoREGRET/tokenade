//! SQL query result compaction — psql, mysql, sqlite3 output formats.
//!
//! Database CLIs print results as ASCII-art tables:
//!
//! ```text
//!  id |   name    |     created_at
//! ----+-----------+---------------------
//!   1 | alice     | 2024-01-01 12:34:56
//!   2 | bob       | 2024-01-02 09:00:00
//! (2 rows)
//! ```
//!
//! For an agent the **values** are the signal; the column-aligning whitespace,
//! the `----+----+----` separator lines, and the `(N rows)` footer are noise.
//! The compactor emits the rows as `value | value | value` (one space around
//! pipes), preserves the header, drops separators, caps very long result sets.

use std::fmt::Write;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavour {
    /// `+---+---+` borders on EVERY row separator (MySQL CLI default).
    Mysql,
    /// `---+---+---` only between header and rows; rows have no top/bottom
    /// border. (psql, sqlite3 with `.headers on`.)
    Psql,
    Unknown,
}

/// Cap rendered rows beyond this count; the remainder is summarised.
pub const ROW_CAP: usize = 50;

pub fn detect(text: &str) -> Flavour {
    let head: &str = text.get(..text.len().min(4096)).unwrap_or(text);
    let lines: Vec<&str> = head.lines().take(20).collect();
    // MySQL: most rows are bordered with `|` at column 0 AND there are
    // `+---+` border-only lines.
    let has_plus_border = lines.iter().any(|l| {
        let t = l.trim();
        t.starts_with('+') && t.ends_with('+') && t.chars().all(|c| matches!(c, '+' | '-'))
    });
    let row_pipe_count = lines
        .iter()
        .filter(|l| {
            let t = l.trim_start();
            t.starts_with('|') && l.matches('|').nth(2).is_some()
        })
        .count();
    if has_plus_border && row_pipe_count >= 2 {
        return Flavour::Mysql;
    }
    // Psql: header line with ` | ` separators AND a `---+---+---` separator
    // (only dashes and `+`).
    let psql_sep = lines.iter().any(|l| {
        let t = l.trim();
        !t.is_empty()
            && t.chars().all(|c| matches!(c, '-' | '+' | ' '))
            && t.contains('-')
            && t.contains('+')
    });
    let header_pipes = lines
        .iter()
        .any(|l| l.contains(" | ") && !l.trim_start().starts_with('|'));
    if psql_sep && header_pipes {
        return Flavour::Psql;
    }
    Flavour::Unknown
}

pub fn compact(text: &str) -> String {
    match detect(text) {
        Flavour::Mysql => compact_mysql(text),
        Flavour::Psql => compact_psql(text),
        Flavour::Unknown => text.to_string(),
    }
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

/// `+---+---+` rows are pure border noise — drop them. Each remaining row
/// starts and ends with `|`; split on `|` and re-emit as compact `v | v | v`.
fn compact_mysql(text: &str) -> String {
    let mut emitted = 0usize;
    let mut header_done = false;
    let mut total_rows = 0usize;
    let mut out = String::with_capacity(text.len() / 3);

    for raw in text.lines() {
        let t = raw.trim();
        if t.is_empty() {
            continue;
        }
        if is_border(t) {
            continue;
        }
        // Footer like `2 rows in set (0.00 sec)` — keep verbatim.
        if !t.starts_with('|') {
            out.push_str(raw);
            out.push('\n');
            continue;
        }
        total_rows += 1;
        if header_done && emitted >= ROW_CAP {
            continue;
        }
        let row = trim_pipes(t);
        out.push_str(&row);
        out.push('\n');
        emitted += 1;
        if !header_done {
            // First non-border `|...|` row is the header.
            header_done = true;
            emitted = 0;
        }
    }
    if total_rows > 0 && total_rows.saturating_sub(1) > emitted {
        let _ = writeln!(
            out,
            "… {} more row(s) elided",
            total_rows.saturating_sub(1) - emitted
        );
    }
    let header = format!(
        "[tokenade:sql] mysql — borders dropped, {} row(s) emitted of {}\n",
        emitted,
        total_rows.saturating_sub(1)
    );
    format!("{header}{}", out.trim_end())
}

/// psql / sqlite3 use ` col | col | col ` rows with a single `---+---+---`
/// separator after the header. We treat the line of dashes-and-plus as the
/// boundary; everything between header and the next blank/footer is data.
fn compact_psql(text: &str) -> String {
    let mut emitted_rows = 0usize;
    let mut total_rows = 0usize;
    let mut past_separator = false;
    let mut header: Option<String> = None;
    let mut out = String::with_capacity(text.len() / 2);

    let mut table_done = false;
    for raw in text.lines() {
        let t = raw.trim();
        // A blank line after the data ends the table; whatever follows
        // (another section of a mixed script output) passes VERBATIM —
        // it used to be counted as "rows" and elided past ROW_CAP.
        if table_done {
            out.push_str(raw);
            out.push('\n');
            continue;
        }
        if t.is_empty() {
            if past_separator {
                table_done = true;
            }
            continue;
        }
        // Detect the `---+---+---` separator line. `+` required: psql and
        // sqlite always join columns with it, while a plain `-----` rule in
        // mixed pre-table content used to flip `past_separator` and count
        // the following prose as capped data rows. (Single-column psql
        // output has no `+` — accept a dashes-only line ONLY as the first
        // non-header line, the position psql prints it in.)
        let dashes_only = t.chars().all(|c| matches!(c, '-' | '+' | ' ')) && t.contains('-');
        if dashes_only
            && (t.contains('+') || (header.is_some() && !past_separator && total_rows == 0))
        {
            past_separator = true;
            continue;
        }
        // Footer: `(N rows)` (psql) or `2 rows in set` (mysql-ish in sqlite).
        if t.starts_with('(') && t.ends_with(')') && t.contains("row") {
            out.push_str(t);
            out.push('\n');
            table_done = true;
            continue;
        }
        if !past_separator {
            if header.is_none() {
                // Header line.
                header = Some(squeeze_pipes(t));
                out.push_str(header.as_deref().unwrap());
                out.push('\n');
            } else {
                // Extra pre-separator content (a second header would be
                // unusual; a mixed section is not) — keep it verbatim
                // instead of silently dropping.
                out.push_str(raw);
                out.push('\n');
            }
            continue;
        }
        // Data rows.
        total_rows += 1;
        if emitted_rows < ROW_CAP {
            out.push_str(&squeeze_pipes(t));
            out.push('\n');
            emitted_rows += 1;
        }
    }
    if total_rows > emitted_rows {
        let _ = writeln!(out, "… {} more row(s) elided", total_rows - emitted_rows);
    }
    let header_line = format!(
        "[tokenade:sql] psql — separator dropped, {} row(s) emitted of {}\n",
        emitted_rows, total_rows
    );
    format!("{header_line}{}", out.trim_end())
}

fn is_border(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty()
        && t.starts_with('+')
        && t.ends_with('+')
        && t.chars().all(|c| matches!(c, '+' | '-'))
}

/// `| a | b | c |` → `a | b | c` (trim leading/trailing pipe, normalise inner).
fn trim_pipes(line: &str) -> String {
    let inner = line.trim().trim_start_matches('|').trim_end_matches('|');
    inner
        .split('|')
        .map(|s| s.trim())
        .collect::<Vec<_>>()
        .join(" | ")
}

/// `  id |   name    | created_at` → `id | name | created_at` — collapses
/// runs of whitespace around `|` to a single space.
fn squeeze_pipes(line: &str) -> String {
    line.split('|')
        .map(|s| s.trim())
        .collect::<Vec<_>>()
        .join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_psql() {
        let t = " id | name\n----+------\n  1 | alice\n  2 | bob\n(2 rows)\n";
        assert_eq!(detect(t), Flavour::Psql);
    }

    #[test]
    fn detects_mysql() {
        let t = "+----+-------+\n| id | name  |\n+----+-------+\n|  1 | alice |\n|  2 | bob   |\n+----+-------+\n2 rows in set\n";
        assert_eq!(detect(t), Flavour::Mysql);
    }

    #[test]
    fn psql_compacts() {
        let t = " id |   name   | created_at\n----+----------+---------------------\n  1 | alice    | 2024-01-01 12:34:56\n  2 | bob      | 2024-01-02 09:00:00\n(2 rows)\n";
        let out = compact(t);
        assert!(out.contains("id | name | created_at"));
        assert!(out.contains("1 | alice | 2024-01-01 12:34:56"));
        assert!(out.contains("(2 rows)"));
        // Separator line gone.
        assert!(!out.contains("----+----------+"));
    }

    #[test]
    fn mysql_drops_borders() {
        let t = "+----+-------+\n| id | name  |\n+----+-------+\n|  1 | alice |\n|  2 | bob   |\n+----+-------+\n2 rows in set (0.01 sec)\n";
        let out = compact(t);
        assert!(out.contains("id | name"));
        assert!(out.contains("1 | alice"));
        assert!(out.contains("2 rows in set"));
        assert!(!out.contains("+----+"));
    }

    #[test]
    fn caps_huge_psql() {
        let mut t = String::from(" id | n\n----+---\n");
        for i in 0..200 {
            let _ = writeln!(t, " {i:3} | x");
        }
        t.push_str("(200 rows)\n");
        let out = compact(&t);
        assert!(out.contains("more row(s) elided"));
        // First N rendered, later rows dropped.
        assert!(out.contains("0 | x"));
        assert!(!out.contains("150 | x"));
    }

    #[test]
    fn unknown_passes_through() {
        let t = "not a sql result at all\njust prose\n";
        assert_eq!(compact(t), t);
    }

    #[test]
    fn savings_real_shape() {
        // 200-row psql result table.
        let mut t = String::from(
            " id |     name     | created_at\n----+--------------+---------------------\n",
        );
        for i in 0..200 {
            let _ = writeln!(t, " {i:3} | user_{i:04} | 2024-01-01 12:34:56");
        }
        t.push_str("(200 rows)\n");
        let (out, sav) = compact_with_savings(&t);
        assert!(out.contains("id | name | created_at"));
        // Borders + padding + 150 dropped rows mean serious savings.
        assert!(sav.percent() > 70.0, "got {:?}", sav);
    }
}
