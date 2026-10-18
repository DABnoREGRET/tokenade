//! **Time**: a civil date, an ISO timestamp, and the day tokenade
//! was installed.
//!
//! The day arithmetic is Hinnant's, written here rather than
//! borrowed from a dependency: this module only handles integers, and
//! a date crate would cost more than the twenty lines it replaces.
//!
//! The INSTALL date is a separate and more fragile question: it is
//! inferred from three clues that can all lie, hence the order of preference
//! and the clamping. An aberrant timestamp would announce savings since
//! 1970.

use super::*;

/// Days from the Unix epoch for a civil (proleptic Gregorian) date.
/// Hinnant's `days_from_civil` — the inverse of the algorithm in
/// [`epoch_to_iso8601_z`].
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = ((m as i64) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + (d as i64) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Parse a `YYYY-MM-DDTHH:MM:SS[.fff]Z` timestamp to epoch millis. Returns
/// `None` on any shape it can't read (never panics). Used to place a
/// transcript message on the same millisecond axis as a gain row's `ts`.
pub fn iso8601_to_epoch_ms(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    // Require the date/time skeleton; the separator at [10] may be 'T' or a
    // space (both are seen across agents). Everything past the seconds is
    // optional (fractional seconds and/or a timezone).
    if b.len() < 19
        || b[4] != b'-'
        || b[7] != b'-'
        || (b[10] != b'T' && b[10] != b' ')
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let num = |a: usize, z: usize| -> Option<i64> {
        let sl = b.get(a..z)?;
        if !sl.iter().all(|c| c.is_ascii_digit()) {
            return None;
        }
        std::str::from_utf8(sl).ok()?.parse().ok()
    };
    let y = num(0, 4)?;
    let mo = num(5, 7)? as u32;
    let d = num(8, 10)? as u32;
    let hh = num(11, 13)?;
    let mm = num(14, 16)?;
    let ss = num(17, 19)?;
    // Allow a leap second (ss==60) by clamping; reject the rest.
    if !(1..=12).contains(&mo)
        || !(1..=31).contains(&d)
        || !(0..24).contains(&hh)
        || !(0..60).contains(&mm)
        || !(0..=60).contains(&ss)
    {
        return None;
    }
    let mut secs = days_from_civil(y, mo, d) * 86_400 + hh * 3600 + mm * 60 + ss.min(59);
    // Fractional seconds (up to 3 digits used).
    let mut ms = 0i64;
    let mut i = 19usize;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let mut place = 100i64;
        while i < b.len() && b[i].is_ascii_digit() {
            if place >= 1 {
                ms += (b[i] - b'0') as i64 * place;
                place /= 10;
            }
            i += 1;
        }
    }
    // Timezone: 'Z' (or absent) = UTC; ±HH:MM shifts to UTC. A local naive
    // stamp (no zone) is treated as UTC — the same convention every caller
    // uses, so spans and event ts stay on one axis.
    if let Some(&c) = b.get(i) {
        if c == b'+' || c == b'-' {
            let oh = num(i + 1, i + 3).unwrap_or(0);
            // minutes may be preceded by ':' (i+3) or not.
            let om_at = if b.get(i + 3) == Some(&b':') {
                i + 4
            } else {
                i + 3
            };
            let om = num(om_at, om_at + 2).unwrap_or(0);
            let off = oh * 3600 + om * 60;
            if c == b'+' {
                secs -= off;
            } else {
                secs += off;
            }
        }
    }
    if secs < 0 {
        return None;
    }
    Some((secs * 1000 + ms) as u64)
}

/// Return the Unix epoch second when Tokenade was first installed on
/// this machine. Order of preference:
///
/// 1. `~/.tokenade/install.ts` — written on first call; stable.
/// 2. First record's `ts` in `~/.tokenade/gain.jsonl` (backfilled
///    into install.ts if found).
/// 3. mtime of `~/.tokenade/` itself (last-resort backfill).
///
/// Returns `None` only if `~/.tokenade` doesn't exist or all three
/// methods fail (e.g. corrupted FS metadata). The install.ts file is
/// (re-)created idempotently any time a fresh backfill succeeds, so
/// subsequent calls become deterministic.
pub fn install_timestamp_epoch(home_dir: &Path) -> Option<u64> {
    let tdir = home_dir.join(".tokenade");
    let install_path = tdir.join("install.ts");

    // Existing marker — preferred. We still sanity-clamp so a hand-
    // edited install.ts can't shift the cutoff to 1970 or year 3000.
    if let Ok(s) = fs::read_to_string(&install_path) {
        if let Ok(t) = s.trim().parse::<u64>() {
            return Some(clamp_install_ts(t));
        }
    }

    // Backfill order:
    //   1. Earliest record in gain.jsonl — most accurate.
    //   2. mtime of the `.tokenade/` dir itself — proxy for create time.
    //   3. NOW — if neither source exists, this is a brand-new install
    //      and the right cutoff is "from this moment forward". Without
    //      this fallback we'd return None, the dashboard would skip the
    //      cutoff, and a fresh user would see months of pre-install
    //      Claude usage labelled as "bypassed Tokenade" — misleading.
    let backfill = first_ts_in_gainlog(&tdir.join("gain.jsonl"))
        .or_else(|| dir_mtime_epoch(&tdir))
        .or_else(now_epoch)
        .map(clamp_install_ts);

    if let Some(t) = backfill {
        // Best-effort persist. We don't propagate the write error: even
        // if the file can't be written, the caller can keep using the
        // computed value for this run.
        let _ = fs::create_dir_all(&tdir);
        let _ = fs::write(&install_path, t.to_string());
        return Some(t);
    }

    None
}

/// Wall-clock NOW as Unix epoch seconds, or `None` if the system
/// clock is set before 1970 (effectively never on real machines).
pub(crate) fn now_epoch() -> Option<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

/// Clamp a candidate install timestamp into a plausible window.
/// - Floor at 2024-01-01 (`1_704_067_200`): Tokenade didn't exist
///   before then, and a restored backup of `~/.tokenade/` with an
///   ancient mtime would otherwise hide every Claude usage record.
/// - Ceiling at 2099-12-31 (`4_102_444_800`): defends against bogus
///   clock-skew futures from VM restores or hand-edited install.ts.
pub(crate) fn clamp_install_ts(t: u64) -> u64 {
    const FLOOR: u64 = 1_704_067_200; // 2024-01-01T00:00:00Z
    const CEIL: u64 = 4_102_444_800; // 2099-12-31T00:00:00Z
    t.clamp(FLOOR, CEIL)
}

/// Convenience: install timestamp formatted as the same ISO-8601
/// string layout the transcripts use, so the dashboard can pass it
/// straight into [`aggregate_since`].
pub fn install_timestamp_iso(home_dir: &Path) -> Option<String> {
    install_timestamp_epoch(home_dir).map(crate::platform::epoch_to_iso8601_z)
}

/// Read the first valid `ts` field from a JSON-lines file. We trust
/// records are append-only so "first record" = "earliest record".
pub(crate) fn first_ts_in_gainlog(path: &Path) -> Option<u64> {
    let f = fs::File::open(path).ok()?;
    let mut earliest: Option<u64> = None;
    for line in BufReader::new(f).lines().map_while(Result::ok).take(50) {
        // Bound the scan: append-only ledgers have their oldest record
        // first, but we read up to 50 lines so a corrupted prefix
        // doesn't prevent finding ANY usable ts.
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
            if let Some(t) = v["ts"].as_u64() {
                earliest = Some(earliest.map_or(t, |e| e.min(t)));
            }
        }
    }
    earliest
}

pub(crate) fn dir_mtime_epoch(path: &Path) -> Option<u64> {
    let meta = fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?;
    mtime
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}
