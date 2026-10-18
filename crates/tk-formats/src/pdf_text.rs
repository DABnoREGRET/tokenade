//! **`pdftotext -layout` output, before any other fold.**
//!
//! `-layout` keeps a table's columns by padding them with spaces — useful, the
//! columns stay readable — but it also prints the row's cell padding as a
//! BLANK LINE under every row: a 30-row table came out as 60 lines on a real
//! LibreOffice PDF. A blank line between two columnar lines is that padding,
//! not a paragraph break, and goes; one before or after prose stays.

/// A line laid out in columns: at least two gaps of three spaces or more
/// between non-blank text.
fn is_columnar(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty()
        && t.matches("   ").count() >= 2
        && t.split("   ").filter(|c| !c.trim().is_empty()).count() >= 3
}

/// `text` with the blank lines between two columnar lines removed, and how
/// many went.
pub fn tighten_layout(text: &str) -> (String, usize) {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<&str> = Vec::with_capacity(lines.len());
    let mut dropped = 0usize;
    for (i, line) in lines.iter().enumerate() {
        if line.trim().is_empty() && i > 0 {
            let prev = out.last().copied().unwrap_or("");
            let next = lines[i + 1..]
                .iter()
                .find(|l| !l.trim().is_empty())
                .copied()
                .unwrap_or("");
            // The WHOLE blank run decides: one line is padding, two are a break.
            let run = lines[..i]
                .iter()
                .rev()
                .take_while(|l| l.trim().is_empty())
                .count()
                + 1
                + lines[i + 1..]
                    .iter()
                    .take_while(|l| l.trim().is_empty())
                    .count();
            if is_columnar(prev) && is_columnar(next) && run == 1 {
                dropped += 1;
                continue;
            }
        }
        out.push(line);
    }
    let mut s = as_pipe_tables(&out).join("\n");
    if text.ends_with('\n') {
        s.push('\n');
    }
    (s, dropped)
}

/// Cells of a columnar line: split on runs of two spaces or more, so a
/// multi-word cell (`Île de France`) stays one cell.
fn cells(line: &str) -> Vec<String> {
    tk_base::regexes::re(r"\s{2,}")
        .split(line.trim())
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .collect()
}

/// A run of three or more consecutive columnar lines with the same number of
/// cells, as `| a | b |` rows. Space-aligned columns did not survive the
/// content route — runs of spaces were collapsed to one, and `Île de France
/// 12,000` no longer said where a cell ended — a pipe row says it in any
/// later fold.
fn as_pipe_tables(lines: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let n = if is_columnar(lines[i]) {
            cells(lines[i]).len()
        } else {
            0
        };
        let mut j = i;
        while n > 0 && j < lines.len() && is_columnar(lines[j]) && cells(lines[j]).len() == n {
            j += 1;
        }
        if j - i >= 3 {
            for (k, l) in lines[i..j].iter().enumerate() {
                out.push(format!(
                    "| {} |",
                    cells(l)
                        .iter()
                        .map(|c| c.replace('|', "\\|"))
                        .collect::<Vec<_>>()
                        .join(" | ")
                ));
                if k == 0 {
                    out.push(format!("|{}", " --- |".repeat(n)));
                }
            }
            i = j;
        } else {
            out.push(lines[i].to_string());
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_layout_table_loses_its_padding_rows_not_its_paragraph_breaks() {
        let text = "Rapport\n\nUne phrase.\n\n Région    T1      T2      T3\n\n Zone 1    1,000   2,000   3,000\n\n Zone 2    2,000   4,000   6,000\n\nFin du rapport.\n";
        let (out, n) = tighten_layout(text);
        assert_eq!(n, 2);
        assert_eq!(
            out,
            "Rapport\n\nUne phrase.\n\n| Région | T1 | T2 | T3 |\n| --- | --- | --- | --- |\n| Zone 1 | 1,000 | 2,000 | 3,000 |\n| Zone 2 | 2,000 | 4,000 | 6,000 |\n\nFin du rapport.\n"
        );
    }

    #[test]
    fn two_blank_lines_between_tables_are_a_break() {
        let text = " a    b    c\n\n\n d    e    f\n";
        assert_eq!(tighten_layout(text).1, 0);
    }

    #[test]
    fn a_multi_word_cell_stays_one_cell() {
        let text = " Région             T1        T2\n Île de France      12,000    3,000\n Grand Est          8,000     1,000\n";
        assert!(tighten_layout(text)
            .0
            .contains("| Île de France | 12,000 | 3,000 |"));
    }
}
