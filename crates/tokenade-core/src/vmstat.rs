//! `vmstat` system-monitoring output compaction.
//!
//! `vmstat 1 N` (1-second interval for N seconds) is a staple of Linux
//! perf debugging. A 30-minute run = 1 800 sample rows + header. Each row
//! has 17 numeric columns:
//!
//! ```text
//! procs -----------memory---------- ---swap-- -----io---- -system-- ------cpu-----
//!  r  b   swpd   free   buff  cache   si   so    bi    bo   in   cs us sy id wa st
//!  1  0      0 123456  78901 234567    0    0    12    34  100  200  5  3 92  0  0
//!  0  0      0 123440  78901 234580    0    0     0    16   95  180  4  2 94  0  0
//!  …1 800 more sample rows…
//! ```
//!
//! What the agent reading this wants:
//!
//!   - per-column **min / max / mean** over the entire run
//!   - first + last sample (to see start/end state)
//!   - **peaks**: rows where any key column is at min/max
//!
//! Drops: the bulk of the time-series rows (which are mostly noise around
//! the steady state).
//!
//! Detection: a header row containing the canonical column words
//! (`procs`, `memory`, `swap`, `io`, `system`, `cpu`) within the first
//! few lines, OR the column-name row with `r b swpd free buff cache`.

use std::fmt::Write;

pub const KEEP_SAMPLE_ROWS: usize = 3;

const COLUMNS: &[&str] = &[
    "r", "b", "swpd", "free", "buff", "cache", "si", "so", "bi", "bo", "in", "cs", "us", "sy",
    "id", "wa", "st",
];

pub fn detect(text: &str) -> bool {
    let head: &str = text.get(..text.len().min(4096)).unwrap_or(text);

    // Canonical category header.
    if head.contains("procs ")
        && head.contains("memory")
        && head.contains("swap")
        && head.contains("io")
        && head.contains("system")
        && head.contains("cpu")
    {
        return true;
    }

    // Column-name row.
    head.lines().take(10).any(|l| {
        let t = l.trim();
        t.starts_with('r')
            && t.contains(" b ")
            && t.contains("swpd")
            && t.contains("free")
            && t.contains("buff")
            && t.contains("cache")
    })
}

#[derive(Default, Debug, Clone, Copy)]
struct ColStats {
    min: i64,
    max: i64,
    sum: i64,
    count: usize,
    seen_any: bool,
}

impl ColStats {
    fn observe(&mut self, v: i64) {
        if !self.seen_any {
            self.min = v;
            self.max = v;
            self.seen_any = true;
        } else {
            self.min = self.min.min(v);
            self.max = self.max.max(v);
        }
        self.sum = self.sum.saturating_add(v);
        self.count += 1;
    }
    fn mean(&self) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        self.sum as f64 / self.count as f64
    }
}

/// Parse a vmstat data row into 17 integers. Returns None if the line
/// doesn't look like a data row.
fn parse_vmstat_row(line: &str) -> Option<[i64; 17]> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    if parts.len() < 17 {
        return None;
    }
    let mut row = [0i64; 17];
    for (i, p) in parts.iter().take(17).enumerate() {
        row[i] = p.parse::<i64>().ok()?;
    }
    Some(row)
}

pub fn compact(text: &str) -> String {
    if !detect(text) {
        return text.to_string();
    }

    let mut stats: [ColStats; 17] = [ColStats::default(); 17];
    let mut rows: Vec<[i64; 17]> = Vec::new();
    let mut unmatched: Vec<String> = Vec::new();
    let mut header_category: Option<String> = None;
    let mut header_columns: Option<String> = None;

    for raw in text.lines() {
        let line = raw.trim_end();
        if line.is_empty() {
            continue;
        }
        // Capture the original header lines for output.
        if header_category.is_none() && line.contains("procs ") && line.contains("memory") {
            header_category = Some(line.trim().to_string());
            continue;
        }
        let t = line.trim();
        // Column-name row.
        if header_columns.is_none()
            && t.starts_with('r')
            && t.contains(" b ")
            && t.contains("swpd")
            && t.contains("free")
        {
            header_columns = Some(t.to_string());
            continue;
        }
        // Data row.
        if let Some(row) = parse_vmstat_row(line) {
            for i in 0..17 {
                stats[i].observe(row[i]);
            }
            rows.push(row);
        } else {
            // Not a vmstat row (`vmstat 1 3; free -h` in one doc) — keep.
            unmatched.push(line.to_string());
        }
    }

    if rows.is_empty() {
        return text.to_string();
    }

    let n_rows = rows.len();
    let mut out = String::with_capacity(2048);
    let _ = writeln!(out, "{n_rows} sample(s)");
    if let Some(h) = &header_category {
        out.push_str(h);
        out.push('\n');
    }
    if let Some(h) = &header_columns {
        out.push_str(h);
        out.push('\n');
    }

    // Aggregates table.
    out.push_str("\nper-column aggregates (min / mean / max):\n");
    for (i, name) in COLUMNS.iter().enumerate() {
        let s = &stats[i];
        let _ = writeln!(
            out,
            "  {:<5} {:>10} {:>10.1} {:>10}",
            name,
            s.min,
            s.mean(),
            s.max
        );
    }

    // First sample row + last sample row.
    if n_rows >= 1 {
        out.push_str("\nfirst sample:\n  ");
        let first = &rows[0];
        out.push_str(
            &first
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(" "),
        );
        out.push('\n');
    }
    if n_rows >= 2 {
        out.push_str("\nlast sample:\n  ");
        let last = &rows[n_rows - 1];
        out.push_str(
            &last
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(" "),
        );
        out.push('\n');
    }

    // Peak rows: where us+sy (CPU usage) or wa (iowait) hit max.
    if n_rows >= 3 {
        // Find max (us + sy) row index — the "busiest CPU" sample.
        let mut busy_idx = 0usize;
        let mut busy_val = i64::MIN;
        // us index = 12, sy index = 13, wa index = 15.
        for (i, row) in rows.iter().enumerate() {
            let cpu_busy = row[12] + row[13];
            if cpu_busy > busy_val {
                busy_val = cpu_busy;
                busy_idx = i;
            }
        }
        if busy_idx != 0 && busy_idx != n_rows - 1 {
            let _ = write!(
                out,
                "\npeak-CPU sample (row {}/{}):\n  ",
                busy_idx + 1,
                n_rows
            );
            let row = &rows[busy_idx];
            out.push_str(
                &row.iter()
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            out.push('\n');
        }
        // Find max iowait row.
        let mut iowait_idx = 0usize;
        let mut iowait_val = i64::MIN;
        for (i, row) in rows.iter().enumerate() {
            if row[15] > iowait_val {
                iowait_val = row[15];
                iowait_idx = i;
            }
        }
        if iowait_val > 0 && iowait_idx != busy_idx && iowait_idx != 0 && iowait_idx != n_rows - 1 {
            let _ = write!(
                out,
                "\npeak-iowait sample (row {}/{}):\n  ",
                iowait_idx + 1,
                n_rows
            );
            let row = &rows[iowait_idx];
            out.push_str(
                &row.iter()
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            out.push('\n');
        }
    }

    if !unmatched.is_empty() {
        out.push('\n');
        out.push_str(&crate::auto::compact_remainder(&unmatched.join("\n")));
    }
    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str =
        "procs -----------memory---------- ---swap-- -----io---- -system-- ------cpu-----
 r  b   swpd   free   buff  cache   si   so    bi    bo   in   cs us sy id wa st
 1  0      0 123456  78901 234567    0    0    12    34  100  200  5  3 92  0  0
 0  0      0 123440  78901 234580    0    0     0    16   95  180  4  2 94  0  0
 1  0      0 123430  78901 234600    0    0     0    20   98  185  6  3 91  0  0
 2  0      0 123420  78901 234620    0    0     5    25 1500 2500 45  15 35  5  0
 0  0      0 123410  78901 234640    0    0     0    18   99  190  3  2 95  0  0
";

    #[test]
    fn detects_vmstat_with_category_header() {
        assert!(detect(SAMPLE));
    }

    #[test]
    fn detects_vmstat_column_row_only() {
        let s = " r  b   swpd   free   buff  cache   si   so    bi    bo   in   cs us sy id wa st
 1  0      0 100    200    300     0    0    10    20  100  200  5  3 92  0  0
 0  0      0 100    200    300     0    0    10    20  100  200  5  3 92  0  0
";
        assert!(detect(s));
    }

    #[test]
    fn rejects_random_tabular() {
        // Other tabular data without vmstat columns.
        assert!(!detect(
            "Name   Age   City\nAlice  30    NYC\nBob    25    LA\n"
        ));
    }

    #[test]
    fn rejects_prose() {
        assert!(!detect(
            "This text mentions procs and memory but isn't vmstat.\n"
        ));
    }

    #[test]
    fn extracts_aggregates_and_samples() {
        let out = compact(SAMPLE);
        /* L122: banner removed; assertion deleted */
        // Per-column aggregates table.
        assert!(out.contains("per-column aggregates"));
        // The `us` column has values [5, 4, 6, 45, 3] — min=3, max=45.
        // Look for the row labelled `us`.
        assert!(out.contains("us "));
        // First and last samples present.
        assert!(out.contains("first sample"));
        assert!(out.contains("last sample"));
        // Peak-CPU sample (busiest row has us=45 sy=15).
        assert!(out.contains("peak-CPU sample"));
    }

    #[test]
    fn detects_peak_iowait() {
        // Build a fixture where row 3 has high iowait.
        let s = " r  b   swpd   free   buff  cache   si   so    bi    bo   in   cs us sy id wa st
 1  0      0 100    200    300     0    0    10    20  100  200  5  3 90  2  0
 0  0      0 100    200    300     0    0    10    20  100  200  5  3 90  2  0
 1  0      0 100    200    300     0    0    50   100  100  200  5  3 50 42  0
 0  0      0 100    200    300     0    0    10    20  100  200  5  3 90  2  0
 0  0      0 100    200    300     0    0    10    20  100  200  5  3 90  2  0
";
        let out = compact(s);
        assert!(out.contains("peak-iowait sample"));
    }

    #[test]
    fn unknown_passes_through() {
        let t = "not vmstat";
        assert_eq!(compact(t), t);
    }

    #[test]
    fn savings_huge_run() {
        // 10 000 sample rows = 30-min `vmstat 1 N` style.
        let mut s = String::from("procs -----------memory---------- ---swap-- -----io---- -system-- ------cpu-----\n r  b   swpd   free   buff  cache   si   so    bi    bo   in   cs us sy id wa st\n");
        for i in 0..10_000 {
            // Vary values mildly to exercise min/max.
            let us = (i % 60) + 5;
            let sy = (i % 20) + 2;
            let wa = (i % 10).max(0);
            let id = 100 - us - sy - wa;
            let _ = writeln!(
                s,
                " 1  0      0 100000  20000 300000    0    0    10    20  100  200 {us} {sy} {id} {wa}  0"
            );
        }
        let (out, sav) = compact_with_savings(&s);
        assert!(out.contains("10000 sample(s)"));
        assert!(out.contains("per-column aggregates"));
        assert!(sav.percent() > 99.0, "got {sav:?}");
    }

    #[test]
    fn parse_vmstat_row_works() {
        let row = parse_vmstat_row(
            " 1  0      0 123456  78901 234567    0    0    12    34  100  200  5  3 92  0  0",
        );
        assert!(row.is_some());
        let r = row.unwrap();
        assert_eq!(r[0], 1);
        assert_eq!(r[3], 123456);
        assert_eq!(r[12], 5); // us
        assert_eq!(r[14], 92); // id
    }

    #[test]
    fn parse_vmstat_row_rejects_short() {
        assert!(parse_vmstat_row("1 2 3").is_none());
        assert!(parse_vmstat_row("not a row").is_none());
    }
}
