//! **What a cell's FORMAT says about the number it holds.**
//!
//! A date, in a workbook, is a NUMBER — days since 30 December 1899 — and
//! what makes it a date lives elsewhere, in `styles.xml`. Without reading it, a
//! due date arrives as `46095`: an integer that no reader, human or agent,
//! reads back as 14 March 2026. This is not a loss of text, it is worse — the
//! value is there and it is unreadable. Same family for percentages: the
//! workbook stores `0.15` and displays `15 %`.
//!
//! And a column is described by its KIND and its ranges, not cell by cell: a
//! thousand rows of the same nature are one sentence, not a thousand.

use super::*;

/// What a cell format SAYS about the number it dresses.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum NumFormat {
    /// Days since 30 December 1899.
    Date,
    /// **A fraction displayed as a percentage.** The workbook stores `0.15` and
    /// shows `15%`; delivering `0.15` is off by a factor of a hundred for whoever
    /// reads "the margin is 0.15 %". Same family as the date: the number is intact
    /// and the FORMAT carries the meaning.
    Percent,
    /// **An amount in a currency.** `#,##0.00 €` stores `1234.5`: without the
    /// symbol, euro, dollar and yen read the same — the `.ods` of the same
    /// workbook says `1 234,50 €` (2026-10-04). The symbol, and whether it precedes.
    Currency { symbol: String, before: bool },
}

/// What a format code SAYS about the number, or nothing if it does not change
/// its reading. Shared by `.xlsx` (`styles.xml`) and `.xls` (BIFF).
pub(crate) fn classify_format(code: &str) -> Option<NumFormat> {
    let bare = re(r#"(?s)"[^"]*"|\[[^\]]*\]|\\."#).replace_all(code, " ");
    // Percentage first: a format does not mix the two, and `%` is a sign that
    // nothing else carries once literals are removed.
    if bare.contains('%') {
        Some(NumFormat::Percent)
    } else if bare
        .chars()
        .any(|ch| matches!(ch.to_ascii_lowercase(), 'y' | 'd' | 'h' | 's'))
    {
        Some(NumFormat::Date)
    } else {
        currency_of(code).map(|(symbol, before)| NumFormat::Currency { symbol, before })
    }
}

/// The currency symbol of a format code, and whether it precedes the number:
/// the locale tag `[$€-40C]` (symbol before the `-`), a literal `"€"`, or a
/// bare currency sign. The FIRST section only (before `;`): the following
/// ones dress the negatives and the zero of the same amount.
pub(crate) fn currency_of(code: &str) -> Option<(String, bool)> {
    let first = code.split(';').next().unwrap_or(code);
    let digit = first.find(['0', '#']);
    // Any OTHER bracket (`[$-409]`, `[Red]`, `[>100]`) is consumed without
    // yielding anything: without this branch, the `$` of `[$-409]` passed for a dollar.
    let found = re(r#"\[\$([^\]\-]+)(?:-[^\]]*)?\]|\[[^\]]*\]|"([^"]*)"|([€$£¥₹₩₽₺₪₫฿])"#)
        .captures_iter(first)
        .find_map(|c| {
            let at = c.get(0)?.start();
            let sym = c.get(1).or(c.get(2)).or(c.get(3))?.as_str().trim();
            let is_money = |s: &str| {
                s.chars().any(|ch| "€$£¥₹₩₽₺₪₫฿".contains(ch))
                    || (s.len() == 3 && s.chars().all(|ch| ch.is_ascii_uppercase()))
            };
            (!sym.is_empty() && is_money(sym))
                .then(|| (sym.to_string(), digit.is_some_and(|d| at < d)))
        });
    found
}

/// **The style indexes whose format is a DATE.**
///
/// A date, in an OOXML workbook, is a NUMBER: the number of days since
/// 30 December 1899. What makes it a date is the format the cell carries,
/// stored elsewhere, in `xl/styles.xml`. The reader did not read that file, so
/// a due date arrived as `46095` — an integer that no reader, human or agent,
/// can read back as 14 March 2026. This is not a loss of text, it is worse:
/// the value is there and it is unreadable.
///
/// Two sources of truth, and BOTH are needed. Formats 14 to 22 and 45 to 47
/// are predefined by the specification and appear nowhere in the file. Above
/// 163, the workbook declares its own `formatCode`, and that is what must be
/// read.
///
/// The test on the code: look for `y`, `d`, `h` or `s` OUTSIDE quoted literals
/// and condition brackets. `m` alone is deliberately left out — it means
/// "month" or "minute" depending on its neighbours, and a purely monetary
/// format such as `#,##0.00\ "m"` is not a date.
///
/// Style index → what its format declares, only for the formats that change
/// the reading of the number.
pub(crate) fn numeric_formats(styles_xml: &str) -> std::collections::HashMap<usize, NumFormat> {
    let mut kind: std::collections::HashMap<usize, NumFormat> = (14..=22)
        .chain(45..=47)
        .map(|i| (i, NumFormat::Date))
        .collect();
    // 9 (`0%`) and 10 (`0.00%`) are the built-in percentages.
    kind.insert(9, NumFormat::Percent);
    kind.insert(10, NumFormat::Percent);
    for c in re(r#"(?s)<numFmt\b[^>]*\bnumFmtId="(\d+)"[^>]*\bformatCode="([^"]*)""#)
        .captures_iter(styles_xml)
    {
        let (Some(id), Some(code)) = (c.get(1), c.get(2)) else {
            continue;
        };
        let Ok(id) = id.as_str().parse::<usize>() else {
            continue;
        };
        if let Some(k) = classify_format(&decode_entities(code.as_str())) {
            kind.insert(id, k);
        }
    }
    // `cellXfs` is the list that a cell's `s` attribute INDEXES, in order.
    // `cellStyleXfs`, just above, has the same shape and is not indexed by
    // cells: taking the wrong block shifts everything.
    let Some(block) = re(r"(?s)<cellXfs\b.*?</cellXfs>").find(styles_xml) else {
        return std::collections::HashMap::new();
    };
    re(r#"(?s)<xf\b[^>]*\bnumFmtId="(\d+)""#)
        .captures_iter(block.as_str())
        .enumerate()
        .filter_map(|(i, c)| {
            let id = c.get(1)?.as_str().parse::<usize>().ok()?;
            kind.get(&id).map(|k| (i, k.clone()))
        })
        .collect()
}

/// `0.15` → `15%`, without binary noise.
///
/// `0.15 * 100.0` is `15.000000000000002` in floating point; writing it as is
/// would replace a right value with a wrong one over twelve decimals. Round to
/// ten decimals and strip trailing zeros, so `0.1234` gives `12.34%` and
/// `0.15` gives `15%`.
pub(crate) fn fraction_to_percent(value: f64) -> Option<String> {
    if !value.is_finite() {
        return None;
    }
    let scaled = (value * 100.0 * 1e10).round() / 1e10;
    let mut text = format!("{scaled}");
    if text.contains('.') {
        text = text.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    Some(format!("{text}%"))
}

/// A workbook serial number → `YYYY-MM-DD`, time included when it is not
/// zero.
///
/// **The epoch is 30 December 1899, not the 31st.** Excel believes 1900 is a
/// leap year, for compatibility with a 1980s spreadsheet, so serial 60 denotes
/// a 29 February that does not exist. Shifting the origin by one day lands
/// right everywhere past March 1900, which covers everything a real workbook
/// contains; below that, rendering nothing is preferred over rendering a wrong
/// date.
///
/// The form is ISO because it is the only one that invents nothing.
/// OpenDocument writes the DISPLAYED text next to the value, and that is served
/// as is: it is what the document shows. OOXML carries only the number and a
/// format template; rendering `14/03/2026` rather than `03/14/2026` would
/// assume a culture the file does not declare.
///
/// **Less than a day is a TIME.** An `h:mm` cell carries `0.3382` for 08:07;
/// that serial falls before the 1900 gap, and the cell arrived as is — a
/// fraction nothing reads back as a time (2026-10-03).
pub(crate) fn serial_to_date(serial: f64) -> Option<String> {
    if serial.is_finite() && (0.0..1.0).contains(&serial) {
        let secs = ((serial * 86_400.0).round() as i64).min(86_399);
        return Some(format!(
            "{:02}:{:02}:{:02}",
            secs / 3600,
            (secs / 60) % 60,
            secs % 60
        ));
    }
    if !serial.is_finite() || !(61.0..=2_958_465.0).contains(&serial) {
        return None;
    }
    let days = serial.trunc() as i64;
    let frac = serial - serial.trunc();
    // Days since 1970-01-01 (the workbook epoch is 25569 days earlier).
    let z = days - 25569 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let date = format!("{y:04}-{m:02}-{d:02}");
    if frac < 1.0 / 86_400.0 {
        return Some(date);
    }
    let secs = (frac * 86_400.0).round() as i64;
    Some(format!(
        "{date}T{:02}:{:02}:{:02}",
        secs / 3600,
        (secs / 60) % 60,
        secs % 60
    ))
}

/// What a column holds, inferred from its values.
/// A date, not merely "has digits and separators". The loose version typed a
/// column of JSON blobs as `date` (a JSON payload is long, has digits and plenty
/// of `-`/`/`), and a wrong type sends a script author down the wrong path —
/// worse than saying `text`. Require the SHAPE: short, and made of digits and
/// separators only.
pub(crate) fn is_date_like(t: &str) -> bool {
    if t.len() < 6 || t.len() > 25 {
        return false;
    }
    let seps = t
        .chars()
        .filter(|c| matches!(c, '-' | '/' | ':' | ' ' | 'T' | '.'))
        .count();
    let digits = t.chars().filter(char::is_ascii_digit).count();
    // Everything is either a digit or a separator, and both are present.
    seps >= 2 && digits >= 4 && digits + seps == t.chars().count()
}

pub(crate) fn column_kind(values: &[&str]) -> &'static str {
    let mut num = 0usize;
    let mut url = 0usize;
    let mut date = 0usize;
    let mut json = 0usize;
    let n = values.len().max(1);
    for v in values {
        let t = v.trim();
        if t.starts_with("http://") || t.starts_with("https://") {
            url += 1;
        } else if t.parse::<f64>().is_ok() {
            num += 1;
        } else if is_date_like(t) {
            date += 1;
        } else if t.starts_with('{') || t.starts_with('[') {
            json += 1;
        }
    }
    // A column is only "a URL column" if it overwhelmingly is one — mixed
    // columns are text, and saying otherwise would mislead a script author.
    if url * 100 / n > 80 {
        "url"
    } else if num * 100 / n > 80 {
        "number"
    } else if date * 100 / n > 80 {
        "date"
    } else if json * 100 / n > 80 {
        // Says "parse me", which is the single most useful thing to know before
        // writing the script that reads this column.
        "json"
    } else {
        "text"
    }
}

/// Contiguous 1-based row ranges where `col` has a value.
///
/// Ranges, not a fill percentage, because WHERE the data sits is the thing a
/// sampler gets wrong: a column can be empty for 4000 rows and full afterwards,
/// so a head-sample reports "empty" and a script author concludes there is
/// nothing to read. Merged across gaps of 1 so a couple of blank rows don't
/// shatter the map into noise.
pub(crate) fn fill_ranges(
    rows: &[Vec<String>],
    col: usize,
    data_start: usize,
) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for (i, row) in rows.iter().enumerate().skip(data_start) {
        let filled = row.get(col).is_some_and(|c| !c.trim().is_empty());
        let line = i + 1;
        if !filled {
            continue;
        }
        match out.last_mut() {
            Some(last) if line <= last.1 + 2 => last.1 = line,
            _ => out.push((line, line)),
        }
    }
    out
}

pub(crate) fn fmt_ranges(ranges: &[(usize, usize)], total: usize) -> String {
    if ranges.is_empty() {
        return "EMPTY".into();
    }
    if ranges.len() == 1 && ranges[0].0 <= 2 && ranges[0].1 >= total {
        return "all".into();
    }
    let shown: Vec<String> = ranges
        .iter()
        .take(3)
        .map(|(a, b)| {
            if a == b {
                format!("{a}")
            } else {
                format!("{a}-{b}")
            }
        })
        .collect();
    let more = if ranges.len() > 3 {
        format!(" +{} more", ranges.len() - 3)
    } else {
        String::new()
    };
    format!("rows {}{more}", shown.join(","))
}

/// Column letter for an index (0 → A, 26 → AA).
pub(crate) fn col_letter(mut idx: usize) -> String {
    let mut s = String::new();
    loop {
        s.insert(0, (b'A' + (idx % 26) as u8) as char);
        if idx < 26 {
            break;
        }
        idx = idx / 26 - 1;
    }
    s
}
