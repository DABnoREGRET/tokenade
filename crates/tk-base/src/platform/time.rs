//! **Time, without a dependency.**
//!
//! Enough to write an ISO 8601 date and read a modification date. Nothing
//! more: a whole date library for four functions would relink
//! the entire graph on every build.

/// **The civil calendar, only once.**
///
/// Howard Hinnant's algorithm (`civil_from_days`) lived in TWO
/// copies: `gain::civil_from_days`, which returns the triple, and
/// `webdash::utc_date_from_ts`, which returns the string. Two copies of a date
/// computation, each with its magic constants to re-check — and one of the two
/// pulled `redact`, a primitive, up into the dashboard layer.
///
/// `z` is a number of days since the Unix epoch.
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m as u32, d as u32)
}

/// `YYYY-MM-DD` in UTC for a Unix timestamp in SECONDS.
pub fn utc_date_from_ts(ts: u64) -> String {
    let (y, m, d) = crate::platform::civil_from_days((ts / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// A file's last-modification timestamp, in Unix seconds;
/// **0** if the file does not exist or cannot be read — which callers
/// read as "never done".
pub fn mtime_epoch(p: &std::path::Path) -> u64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// A Unix timestamp as ISO-8601 UTC. It lived in the Claude Code
// transcripts module, because that reader was the first to need
// it — and six others called it there, including a site reader, which
// thereby climbed from the web layer to the product layer to format a date.
/// Convert a Unix epoch second count to a `YYYY-MM-DDTHH:MM:SSZ`
/// string. Uses Howard Hinnant's civil-from-days algorithm (public
/// domain) — no external date crate needed. Valid for any epoch
/// second representable in `u64`.
pub fn epoch_to_iso8601_z(secs: u64) -> String {
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = (secs / 3600) % 24;
    let days_since_epoch = (secs / 86400) as i64;
    // civil_from_days — Hinnant, "chrono-Compatible Low-Level Date
    // Algorithms" (2013). z is days from civil 0000-03-01.
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!("{year:04}-{month:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}
