//! Big spreadsheets rendered as a PROFILE, not as rows.
//!
//! A 40 000-row sheet dumped as a markdown table is the worst trade we make:
//! it costs six figures of tokens and answers no question anyone asks. What an
//! agent actually needs first is the SHAPE — which columns exist, what type
//! they hold, how many are empty, what the range is — and then the ability to
//! ask for the rows that matter. Rows are the detail; the profile is the map.
//!
//! So above [`PROFILE_MIN_ROWS`] we render one line per COLUMN instead of one
//! line per row, keep a small head sample so the agent can see the actual
//! shape of a record, and leave a pointer to the verbatim extraction. Below
//! that threshold nothing changes: a 20-row sheet IS its own summary, and a
//! profile of it would be longer than the data.
//!
//! Two rules carried over from every other compactor here:
//!   * the fold is DECLARED — row count, column count, and how to get the
//!     bytes back, never a silent truncation;
//!   * statistics are computed in one pass with bounded memory, so a sheet
//!     with a million rows costs the same working set as one with a thousand.
//!
//! Shape borrowed from `jdatamunch-mcp`'s `describe_dataset` (2026-08-01 scan):
//! name / type / cardinality / null share / range / sample values, and never
//! the raw rows by default.

use std::fmt::Write as _;

/// Sheets with at most this many data rows render as a plain table, unchanged.
pub const PROFILE_MIN_ROWS: usize = 60;

/// Data rows kept verbatim under the profile, so the agent sees a real record.
pub const SAMPLE_ROWS: usize = 5;

/// Distinct values tracked exactly before the count becomes a lower bound.
/// Beyond this the number is reported as `≥ N`, which is the honest reading.
const MAX_DISTINCT: usize = 2_048;

/// Sample values shown per column.
const SAMPLE_VALUES: usize = 3;

/// A text column with at most this many distinct values is a CATEGORY: every
/// value is listed with its count instead of three samples.
const CATEGORY_MAX: usize = 12;

/// What a column holds, inferred by a monotone rank that never walks back:
/// once a string is seen the column is a string, whatever follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
enum ColType {
    #[default]
    Empty,
    Integer,
    Float,
    /// ISO-8601 calendar dates, with or without a time: the range is the
    /// earliest and latest, which ISO ordering gives as text order.
    Date,
    /// `28/01/2024` or `28.01.2024`: a date whose day/month order the column
    /// as a whole must settle (see `Column::slash_order`).
    SlashDate,
    /// `08:07` or `08:07:00`, two-digit hour: text order is time order.
    Time,
    Text,
}

impl ColType {
    fn of(value: &str) -> ColType {
        let v = value.trim();
        if v.is_empty() {
            return ColType::Empty;
        }
        if v.parse::<i64>().is_ok() {
            return ColType::Integer;
        }
        if is_iso_date(v) {
            return ColType::Date;
        }
        if slash_date(v).is_some() {
            return ColType::SlashDate;
        }
        if is_time(v) {
            return ColType::Time;
        }
        // Accept the comma decimal separator: a French-locale export is not a
        // text column, and calling it one loses the range for a whole sheet.
        if v.parse::<f64>().is_ok() || v.replace(',', ".").parse::<f64>().is_ok() {
            return ColType::Float;
        }
        if formatted_number(v).is_some() {
            return ColType::Float;
        }
        ColType::Text
    }

    fn as_str(self) -> &'static str {
        match self {
            ColType::Empty => "empty",
            ColType::Integer => "integer",
            ColType::Float => "float",
            ColType::Date | ColType::SlashDate => "date",
            ColType::Time => "time",
            ColType::Text => "text",
        }
    }
}

#[derive(Debug, Default)]
struct Column {
    name: String,
    kind: ColType,
    nulls: usize,
    total: usize,
    distinct: std::collections::HashSet<String>,
    distinct_overflow: bool,
    numeric: Vec<f64>,
    samples: Vec<String>,
    /// Smallest and largest value as text, kept for EVERY row: the distinct
    /// set stops growing at `MAX_DISTINCT`, a range must not.
    lo: Option<String>,
    hi: Option<String>,
    /// Count per value while the column could still be a category.
    counts: std::collections::HashMap<String, usize>,
    /// The currency or `%` every value carries, if they all carry the same.
    unit: Option<String>,
    unit_mixed: bool,
    /// For `SlashDate`: has a first / a second field above 12 been seen, and
    /// the (sort key, original) of the earliest and latest under each order —
    /// [0] day first, [1] month first.
    over_twelve: [bool; 2],
    slash_lo: [Option<(String, String)>; 2],
    slash_hi: [Option<(String, String)>; 2],
}

impl Column {
    fn observe(&mut self, value: &str) {
        self.total += 1;
        let v = value.trim();
        if v.is_empty() {
            self.nulls += 1;
            return;
        }
        // Monotone: Empty < Integer < Float < Text, and the rank only rises.
        // A column of integers with one "n/a" is a text column, which is the
        // truth the agent needs before it writes a SUM over it.
        self.kind = merge(self.kind, ColType::of(v));
        if self.lo.as_deref().is_none_or(|lo| v < lo) {
            self.lo = Some(v.to_string());
        }
        if self.hi.as_deref().is_none_or(|hi| v > hi) {
            self.hi = Some(v.to_string());
        }
        if let Some((a, b, y)) = slash_date(v) {
            self.over_twelve[0] |= a > 12;
            self.over_twelve[1] |= b > 12;
            for (i, (m, d)) in [(b, a), (a, b)].into_iter().enumerate() {
                let key = format!("{y:04}-{m:02}-{d:02}");
                if self.slash_lo[i].as_ref().is_none_or(|(k, _)| key < *k) {
                    self.slash_lo[i] = Some((key.clone(), v.to_string()));
                }
                if self.slash_hi[i].as_ref().is_none_or(|(k, _)| key > *k) {
                    self.slash_hi[i] = Some((key, v.to_string()));
                }
            }
        }
        if self.distinct.len() < MAX_DISTINCT {
            self.distinct.insert(v.to_string());
        } else if !self.distinct.contains(v) {
            self.distinct_overflow = true;
        }
        let parsed = v
            .replace(',', ".")
            .parse::<f64>()
            .ok()
            .map(|n| (n, None))
            .or_else(|| formatted_number(v));
        if let Some((n, unit)) = parsed {
            if n.is_finite() {
                self.numeric.push(n);
                // The unit is shown only when EVERY value carries the same one.
                match (&self.unit, unit) {
                    (None, u) if self.numeric.len() == 1 => self.unit = u.map(str::to_string),
                    (Some(have), Some(u)) if have == u => {}
                    _ => self.unit_mixed = true,
                }
            }
        }
        if self.counts.len() <= CATEGORY_MAX || self.counts.contains_key(v) {
            *self.counts.entry(v.to_string()).or_default() += 1;
        }
        if self.samples.len() < SAMPLE_VALUES && !self.samples.iter().any(|s| s == v) {
            self.samples.push(v.to_string());
        }
    }

    fn distinct_label(&self) -> String {
        if self.distinct_overflow {
            format!("≥{}", self.distinct.len())
        } else {
            self.distinct.len().to_string()
        }
    }

    /// Range + central tendency for a numeric column, samples otherwise.
    fn detail(&self) -> String {
        if matches!(self.kind, ColType::Integer | ColType::Float) && !self.numeric.is_empty() {
            let mut v = self.numeric.clone();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let min = v[0];
            let max = v[v.len() - 1];
            let mean = v.iter().sum::<f64>() / v.len() as f64;
            let median = v[v.len() / 2];
            let unit = match (&self.unit, self.unit_mixed) {
                (Some(u), false) => format!(" ({u})"),
                _ => String::new(),
            };
            return format!(
                "{} … {} · mean {} · median {}{unit}",
                num(min),
                num(max),
                num(mean),
                num(median)
            );
        }
        // A date column answers "from when to when", like a number's range.
        // Dates were TEXT: `"2026-02-11", "2026-03-12", …` — three samples and
        // no earliest or latest, where the same workbook column of amounts
        // read `10.50 … 19990.50` (2026-10-03).
        // A column of times was TEXT, three samples: no first or last hour
        // (2026-10-03). Fixed-width `HH:MM[:SS]` sorts as text.
        if matches!(self.kind, ColType::Date | ColType::Time) {
            if let (Some(lo), Some(hi)) = (&self.lo, &self.hi) {
                return format!("{lo} … {hi}");
            }
        }
        // `28/01/2024` was TEXT, three samples. The order is the column's: a
        // first field above 12 anywhere makes it day-first, a second one
        // month-first. Neither (every value ≤ 12/12) or both: no range is
        // given rather than one that may be months wrong.
        if self.kind == ColType::SlashDate {
            let order = match self.over_twelve {
                [true, false] => Some((0, "day first")),
                [false, true] => Some((1, "month first")),
                _ => None,
            };
            if let Some((i, label)) = order {
                if let (Some((_, lo)), Some((_, hi))) = (&self.slash_lo[i], &self.slash_hi[i]) {
                    return format!("{lo} … {hi} ({label})");
                }
            }
        }
        // **A category is listed WHOLE, with its counts.** Three samples of a
        // four-value `status` column never named the fourth (`cancelled`), and
        // said nothing of how the rows split — the first question asked of a
        // status column (2026-10-03).
        if self.kind == ColType::Text
            && !self.distinct_overflow
            && self.counts.len() <= CATEGORY_MAX
        {
            let mut by_count: Vec<(&String, &usize)> = self.counts.iter().collect();
            by_count.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
            return by_count
                .iter()
                .map(|(v, n)| format!("\"{}\" {n}", truncate(v, 24)))
                .collect::<Vec<_>>()
                .join(" · ");
        }
        if self.samples.is_empty() {
            return String::new();
        }
        self.samples
            .iter()
            .map(|s| format!("\"{}\"", truncate(s, 24)))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// A number as a spreadsheet EXPORT writes it: `1 234,50 €`, `$1,234.50`,
/// `12,5 %`. Every one of these was a TEXT column, so the range of a column of
/// amounts was three samples (2026-10-03). Returns the value and its unit.
///
/// Strict on purpose: a thousands separator is accepted only between groups
/// of exactly three digits, so `06 12 34 56 78` (a phone number) and
/// `2026-03-31` stay what they are. The decimal mark is the LAST separator
/// when there are two kinds, and a lone `,` or `.` followed by exactly three
/// digits is a thousands mark only when nothing else could be (`1,234`).
fn formatted_number(v: &str) -> Option<(f64, Option<&'static str>)> {
    fn sign(t: &str) -> Option<(bool, &str)> {
        t.strip_prefix(['-', '\u{2212}'])
            .map(|r| (true, r))
            .or_else(|| t.strip_prefix('+').map(|r| (false, r)))
    }
    const UNITS: [&str; 5] = ["€", "$", "£", "¥", "%"];
    let mut t = v.trim();
    // A sign may stand outside the unit (`-$5.00`, `−12,50 €` with the
    // typographic minus U+2212 that French exports write), and accounting
    // writes a loss in parentheses: `(1,234.50)`. All three were TEXT.
    let mut neg = false;
    if let Some(r) = t.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
        (t, neg) = (r.trim(), true);
    }
    if let Some((n, r)) = sign(t) {
        (t, neg) = (r.trim_start(), neg ^ n);
    }
    let mut unit = None;
    for u in UNITS {
        if let Some(r) = t.strip_suffix(u) {
            (t, unit) = (r.trim_end(), Some(u));
            break;
        }
        if let Some(r) = t.strip_prefix(u) {
            (t, unit) = (r.trim_start(), Some(u));
            break;
        }
    }
    if let Some((n, r)) = sign(t) {
        (t, neg) = (r, neg ^ n);
    }
    if t.is_empty() || !t.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let is_sep = |c: char| matches!(c, ' ' | '\u{a0}' | '\u{202f}' | ',' | '.');
    if !t.chars().all(|c| c.is_ascii_digit() || is_sep(c)) {
        return None;
    }
    // Split off the decimal part: the last `,`/`.` when it is NOT a 3-digit
    // thousands group, or when the other mark also appears.
    let last = t.rfind([',', '.']);
    let (int, frac) = match last {
        Some(i) => {
            let tail = &t[i + 1..];
            let mark = t.as_bytes()[i] as char;
            let other = if mark == ',' { '.' } else { ',' };
            let thousands = tail.len() == 3 && !t[..i].contains(other) && unit != Some("%");
            if !thousands {
                (&t[..i], Some(tail))
            } else {
                (t, None)
            }
        }
        None => (t, None),
    };
    // The integer part: digit groups, the first 1-3 digits, every other
    // exactly 3, joined by ONE kind of separator.
    let groups: Vec<&str> = int.split(is_sep).collect();
    let seps: std::collections::HashSet<char> = int.chars().filter(|c| is_sep(*c)).collect();
    if seps.len() > 1
        || groups
            .iter()
            .any(|g| g.is_empty() || !g.bytes().all(|b| b.is_ascii_digit()))
        || groups.len() > 1 && (groups[0].len() > 3 || groups[1..].iter().any(|g| g.len() != 3))
    {
        return None;
    }
    if frac.is_some_and(|f| f.is_empty() || !f.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    let mut digits: String = groups.concat();
    if let Some(f) = frac {
        digits.push('.');
        digits.push_str(f);
    }
    let n: f64 = digits.parse().ok()?;
    // A plain number with no formatting at all was already read upstream.
    (unit.is_some() || int.contains(is_sep) || frac.is_some())
        .then_some((if neg { -n } else { n }, unit))
}

/// Two observations of one column. Numbers rise along `Integer < Float`; a
/// date mixed with anything but emptiness is text — `max` alone would have
/// called a column of integers and dates a date column.
fn merge(a: ColType, b: ColType) -> ColType {
    use ColType::*;
    match (a, b) {
        (Empty, x) | (x, Empty) => x,
        (Date, Date) => Date,
        (SlashDate, SlashDate) => SlashDate,
        (Time, Time) => Time,
        (Date | SlashDate | Time, _) | (_, Date | SlashDate | Time) => Text,
        (x, y) => x.max(y),
    }
}

/// `2026-03-31`, `2026-03-31T14:05`, `2026-03-31 14:05:09`, an optional `Z`
/// or offset.
fn is_iso_date(v: &str) -> bool {
    let b = v.as_bytes();
    let digits =
        |r: std::ops::Range<usize>| b.get(r).is_some_and(|s| s.iter().all(u8::is_ascii_digit));
    if b.len() < 10
        || !digits(0..4)
        || b[4] != b'-'
        || !digits(5..7)
        || b[7] != b'-'
        || !digits(8..10)
    {
        return false;
    }
    let (m, d) = (
        (b[5] - b'0') * 10 + (b[6] - b'0'),
        (b[8] - b'0') * 10 + (b[9] - b'0'),
    );
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return false;
    }
    let rest = &v[10..];
    rest.is_empty()
        || ((rest.starts_with('T') || rest.starts_with(' '))
            && rest[1..]
                .chars()
                .all(|c| c.is_ascii_digit() || matches!(c, ':' | '.' | 'Z' | '+' | '-')))
}

/// `08:07`, `08:07:00`, `08:07:00.250`: hour 00-23, minutes and seconds
/// 00-59. A one-digit hour is refused: `8:07` would sort after `10:00`.
fn is_time(v: &str) -> bool {
    let (hms, frac) = v.split_once('.').unwrap_or((v, "0"));
    let parts: Vec<&str> = hms.split(':').collect();
    let two = |p: &str, max: u8| p.len() == 2 && p.parse::<u8>().is_ok_and(|n| n <= max);
    (2..=3).contains(&parts.len())
        && two(parts[0], 23)
        && parts[1..].iter().all(|p| two(p, 59))
        && (frac == "0" || parts.len() == 3)
        && !frac.is_empty()
        && frac.bytes().all(|b| b.is_ascii_digit())
}

/// `28/01/2024`, `1/2/2024`, `28.01.2024`: two 1-2 digit fields and a
/// four-digit year, one separator. Returns the two fields as written and the
/// year; which is the day is the column's to decide.
fn slash_date(v: &str) -> Option<(u8, u8, u16)> {
    let sep = if v.contains('/') { '/' } else { '.' };
    let mut it = v.split(sep);
    let (a, b, y) = (it.next()?, it.next()?, it.next()?);
    let field = |f: &str| (1..=2).contains(&f.len()) && f.bytes().all(|c| c.is_ascii_digit());
    if it.next().is_some()
        || !field(a)
        || !field(b)
        || y.len() != 4
        || !y.bytes().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    let (a, b): (u8, u8) = (a.parse().ok()?, b.parse().ok()?);
    ((1..=31).contains(&a) && (1..=31).contains(&b) && (a <= 12 || b <= 12))
        .then(|| (a, b, y.parse().unwrap_or(0)))
}

/// Format a number without a trailing `.0` on integers.
fn num(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n:.2}")
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

/// Is this sheet big enough that a profile beats the rows?
pub fn should_profile(rows: &[Vec<String>]) -> bool {
    rows.len().saturating_sub(1) > PROFILE_MIN_ROWS
}

/// Render `rows` (first row treated as the header) as a column profile plus a
/// small head sample.
///
/// `stash` receives the verbatim table and returns the recovery pointer. Pure
/// apart from that closure, so this is testable without disk.
pub fn render(rows: &[Vec<String>], stash: &mut impl FnMut(&str) -> String) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let width = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let header = &rows[0];
    let data = &rows[1..];

    let mut cols: Vec<Column> = (0..width)
        .map(|i| Column {
            name: header
                .get(i)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("col{}", i + 1)),
            ..Default::default()
        })
        .collect();
    for row in data {
        for (i, c) in cols.iter_mut().enumerate() {
            c.observe(row.get(i).map(String::as_str).unwrap_or(""));
        }
    }

    let mut out = String::new();
    let _ = writeln!(out, "{} rows × {} columns", data.len(), width);
    out.push_str("| column | type | nulls | distinct | range / sample |\n");
    out.push_str("|---|---|---|---|---|\n");
    for c in &cols {
        let null_pct = if c.total == 0 {
            0.0
        } else {
            c.nulls as f64 * 100.0 / c.total as f64
        };
        let _ = writeln!(
            out,
            "| {} | {} | {:.0}% | {} | {} |",
            c.name,
            c.kind.as_str(),
            null_pct,
            c.distinct_label(),
            c.detail()
        );
    }

    let sample = data.len().min(SAMPLE_ROWS);
    if sample > 0 {
        let _ = writeln!(out, "\nfirst {sample} row(s):");
        out.push_str(&crate::markdown::rows_to_markdown(
            &rows[..=sample.min(rows.len() - 1)],
        ));
    }
    let verbatim = crate::markdown::rows_to_markdown(rows);
    let ptr = stash(&verbatim);
    let _ = write!(
        out,
        "\n[tokenade:sheet profiled — {} row(s) not printed; verbatim: {ptr}]",
        data.len().saturating_sub(sample)
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet(n: usize) -> Vec<Vec<String>> {
        let mut rows = vec![vec![
            "id".into(),
            "name".into(),
            "amount".into(),
            "note".into(),
        ]];
        for i in 0..n {
            rows.push(vec![
                i.to_string(),
                format!("client {}", i % 7),
                format!("{}.5", i * 3),
                if i % 10 == 0 {
                    String::new()
                } else {
                    "ok".into()
                },
            ]);
        }
        rows
    }

    fn stash() -> impl FnMut(&str) -> String {
        |_: &str| "tokenade expand-ref 0123456789ab".to_string()
    }

    #[test]
    fn a_small_sheet_is_not_profiled() {
        assert!(!should_profile(&sheet(10)));
        assert!(!should_profile(&sheet(PROFILE_MIN_ROWS)));
        assert!(should_profile(&sheet(PROFILE_MIN_ROWS + 1)));
    }

    #[test]
    fn the_profile_states_shape_types_and_recovery() {
        let out = render(&sheet(500), &mut stash());
        assert!(out.contains("500 rows × 4 columns"));
        assert!(out.contains("| id | integer |"), "got:\n{out}");
        assert!(out.contains("| amount | float |"), "got:\n{out}");
        assert!(out.contains("| name | text |"), "got:\n{out}");
        assert!(out.contains("tokenade expand-ref"), "recovery pointer");
        assert!(out.contains("row(s) not printed"), "the fold is declared");
    }

    #[test]
    fn cardinality_and_nulls_are_reported() {
        let out = render(&sheet(500), &mut stash());
        // `name` cycles over 7 values; `note` is empty every 10th row.
        let name_line = out.lines().find(|l| l.starts_with("| name ")).unwrap();
        assert!(name_line.contains("| 7 |"), "distinct: {name_line}");
        let note_line = out.lines().find(|l| l.starts_with("| note ")).unwrap();
        assert!(note_line.contains("10%"), "null share: {note_line}");
    }

    #[test]
    fn a_numeric_column_reports_its_range_not_its_values() {
        let out = render(&sheet(500), &mut stash());
        let line = out.lines().find(|l| l.starts_with("| id ")).unwrap();
        assert!(line.contains("0 … 499"), "got {line}");
        assert!(line.contains("median"), "got {line}");
    }

    #[test]
    fn one_stray_string_makes_the_whole_column_text() {
        // The agent must not write a SUM over a column with an "n/a" in it.
        let mut rows = sheet(100);
        rows[50][0] = "n/a".into();
        let out = render(&rows, &mut stash());
        let line = out.lines().find(|l| l.starts_with("| id ")).unwrap();
        assert!(line.contains("| text |"), "monotone type rank: {line}");
    }

    #[test]
    fn a_head_sample_shows_a_real_record() {
        let out = render(&sheet(500), &mut stash());
        assert!(out.contains("first 5 row(s):"));
        assert!(out.contains("client 1"), "sample rows are verbatim:\n{out}");
    }

    #[test]
    fn a_comma_decimal_column_is_still_numeric() {
        let rows = vec![
            vec!["montant".to_string()],
            vec!["12,5".to_string()],
            vec!["3,25".to_string()],
        ];
        let out = render(&rows, &mut stash());
        assert!(out.contains("| montant | float |"), "got:\n{out}");
    }

    #[test]
    fn ragged_rows_do_not_panic_and_widen_the_profile() {
        let rows = vec![
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
            vec!["1".to_string()],
            vec!["1".to_string(), "2".to_string()],
        ];
        let out = render(&rows, &mut stash());
        assert!(out.contains("2 rows × 3 columns"));
    }

    #[test]
    fn an_unnamed_header_cell_gets_a_positional_name() {
        let rows = vec![
            vec!["".to_string(), "b".to_string()],
            vec!["1".to_string(), "2".to_string()],
        ];
        let out = render(&rows, &mut stash());
        assert!(out.contains("| col1 |"), "got:\n{out}");
    }

    /// A date column reads "from when to when", over EVERY row — not text
    /// samples, and not only the first values the distinct set tracked.
    #[test]
    fn a_date_column_reports_its_range() {
        let mut rows = vec![vec!["id".to_string(), "when".to_string()]];
        for i in 0..3000 {
            let day = format!(
                "{}-{:02}-{:02}",
                2020 + i / 365,
                1 + (i / 28) % 12,
                1 + i % 28
            );
            rows.push(vec![i.to_string(), day]);
        }
        rows.push(vec!["x".into(), "2031-12-31T23:59:00Z".into()]);
        let out = render(&rows, &mut stash());
        assert!(out.contains("| when | date |"), "{out}");
        assert!(out.contains("2020-01-01 … 2031-12-31T23:59:00Z"), "{out}");
        // A date mixed with a number is text, not a date.
        let mixed = vec![
            vec!["c".to_string()],
            vec!["2026-01-01".into()],
            vec!["42".into()],
        ];
        assert!(render(&mixed, &mut stash()).contains("| c | text |"));
    }

    /// Every value of a category, with its count — the fourth status was never
    /// named among three samples.
    #[test]
    fn a_category_lists_every_value_with_its_count() {
        let mut rows = vec![vec!["status".to_string()]];
        for (v, n) in [
            ("open", 120),
            ("closed", 70),
            ("pending", 15),
            ("cancelled", 5),
        ] {
            rows.extend(std::iter::repeat_n(vec![v.to_string()], n));
        }
        let out = render(&rows, &mut stash());
        assert!(
            out.contains("\"open\" 120 · \"closed\" 70 · \"pending\" 15 · \"cancelled\" 5"),
            "{out}"
        );
        // Above the cap it is no category: samples, as before.
        let mut many = vec![vec!["name".to_string()]];
        many.extend((0..100).map(|i| vec![format!("person {i}")]));
        assert!(!render(&many, &mut stash()).contains("\"person 0\" 1"));
    }

    #[test]
    fn an_exported_amount_column_gets_a_range_and_its_unit() {
        let mut rows = vec![vec![
            "eur".to_string(),
            "usd".to_string(),
            "rate".to_string(),
            "phone".to_string(),
        ]];
        for (e, u, r) in [
            ("1 234,50 €", "$1,234.50", "12,5 %"),
            ("9,90 €", "$9.90", "100 %"),
            ("12 000,00 €", "$12,000.00", "0,5 %"),
        ] {
            rows.push(vec![e.into(), u.into(), r.into(), "06 12 34 56 78".into()]);
        }
        rows.extend(std::iter::repeat_n(rows[1].clone(), 30));
        let out = render(&rows, &mut stash());
        assert!(
            out.contains("| eur | float |") && out.contains("9.90 … 12000"),
            "{out}"
        );
        assert!(
            out.contains("(€) |") && out.contains("($) |") && out.contains("0.50 … 100"),
            "{out}"
        );
        // Groups of two digits are no thousands groups: a phone number stays text.
        assert!(out.contains("| phone | text |"), "{out}");
    }

    #[test]
    fn a_slash_date_column_reports_its_range_in_the_order_it_proves() {
        let col = |vals: &[&str]| {
            let mut rows = vec![vec!["d".to_string()]];
            for _ in 0..20 {
                rows.extend(vals.iter().map(|v| vec![v.to_string()]));
            }
            render(&rows, &mut stash())
        };
        // 28 can only be a day: day first, so 05/11 is November, the latest,
        // where month first would have put 28/01 last.
        let out = col(&["28/01/2024", "05/11/2024", "03/02/2024"]);
        assert!(
            out.contains("| d | date |") && out.contains("28/01/2024 … 05/11/2024 (day first)"),
            "{out}"
        );
        let out = col(&["01/28/2024", "03/02/2026", "12/15/2025"]);
        assert!(
            out.contains("01/28/2024 … 03/02/2026 (month first)"),
            "{out}"
        );
        // Nothing above 12: no order proven, no range.
        assert!(!col(&["01/02/2024", "03/04/2026"]).contains(" … "));
    }

    #[test]
    fn a_time_column_reports_its_first_and_last_hour() {
        let mut rows = vec![vec!["t".to_string()]];
        for v in ["08:07:00", "20:24:00", "00:57:00", "13:00:00"].repeat(10) {
            rows.push(vec![v.to_string()]);
        }
        let out = render(&rows, &mut stash());
        assert!(
            out.contains("| t | time |") && out.contains("00:57:00 … 20:24:00"),
            "{out}"
        );
        // A one-digit hour does not sort: no time type.
        assert!(!is_time("8:07") && !is_time("24:00") && !is_time("12:60") && is_time("23:59"));
    }

    #[test]
    fn a_negative_amount_is_a_number_in_every_spelling() {
        for (v, n) in [
            ("-$5.00", -5.0),
            ("\u{2212}12,50 €", -12.5),
            ("(1,234.50)", -1234.5),
            ("$-5.00", -5.0),
            ("+3 %", 3.0),
        ] {
            assert_eq!(formatted_number(v).map(|(x, _)| x), Some(n), "{v}");
        }
        assert_eq!(formatted_number("(note)"), None);
    }
}
