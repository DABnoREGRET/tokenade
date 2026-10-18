//! **Where does this residue come from, and over which window?**
//!
//! A session share means nothing without its denominator: "90 % in
//! a single session" is a verdict when the residue is attributed, and an
//! artifact when it is not. The two therefore travel together, and
//! `SessionConcentration` has only ONE constructor — the two views that
//! filled it field by field got the same field wrong.

// No `use super::*;`: this module asks NOTHING of the trunk. It is the
// proof that the concentration can be computed without knowing the ranking.

/// What can be said about a compactor's sessions: the share carried by the
/// biggest one, and the share of the residue this first share is computed on.
/// Both together, because the first alone lies as soon as the second is
/// small.
#[derive(Clone, Copy, Default)]
pub(crate) struct SessionConcentration {
    pub(crate) share: f64,
    pub(crate) coverage: f64,
    /// The LAST day covered by a ROLLUP bucket of this `op`, or 0.
    ///
    /// A rollup bucket names no compactor, and that is what marks a
    /// family "no named piece" (✖). Knowing WHEN these buckets were
    /// written flips the reading: `html-to-markdown` carries 40 % of the pool
    /// in six anonymous buckets, and they date from JULY — the blindness is
    /// history, not a live defect. Naming it without dating it makes one look for
    /// a fix where there is nothing to fix.
    pub(crate) rolled_last_day: i64,
    /// The day of the LAST event of this `op`, in days since the epoch.
    ///
    /// A ranking row carries `span.days` days of history (104
    /// measured on 2026-09-08), so a defect
    /// fixed last week still appears there with its previous rate and
    /// presents itself as a target. Measured 2026-09-08: `read_code_strip_lite`
    /// was 7th at −43 % and 9,742 bytes per call, and its last event
    /// predated the fix to the read-view ladder. An earlier iteration
    /// put it in `--focus`, which is only consulted AFTER choosing.
    pub(crate) last_day: i64,
    /// The share of this `op`'s residue that sleeps in ROLLUP buckets.
    ///
    /// A LIVE row can be ranked on its history: measured
    /// 2026-10-02, `sysinfo` was 2nd with 1,576,872 bytes, of which 1,573,894
    /// in May–July buckets — 2,978 bytes delivered since. `last_day`
    /// said it was live (an event today), so `stale_reason` could
    /// not see it.
    pub(crate) rolled_share: f64,
}

/// The residue of each `op`, per session, read from the RAW ledger.
///
/// The main table comes from `gain --json`, which aggregates and carries only
/// `heaviest_day_share`; sessions are only visible row by row.
/// Aggregated per `op` EXACTLY as `gain --json` aggregates — and that is what
/// answers the question left open earlier: comparing this number to the one
/// from `--focus` compared two different quantities, one per `op`, the other
/// by NAME matching over four fields. The disagreement was not one.
pub(crate) fn session_shares() -> std::collections::BTreeMap<String, SessionConcentration> {
    let Some(home) = tokenade_core::platform::home_dir() else {
        return std::collections::BTreeMap::new();
    };
    let path = home.join(".tokenade").join("gain.jsonl");
    shares_from(&tokenade_core::gain::ledger_with_rollup(&path))
}

/// The pass itself, over a TEXT.
///
/// Extracted to be checkable: as long as it read the disk, the mutation
/// that stopped dating the rollup buckets per `op` survived — no test could
/// present it with a ledger.
pub(crate) fn shares_from(text: &str) -> std::collections::BTreeMap<String, SessionConcentration> {
    let mut per_op: std::collections::BTreeMap<
        String,
        (u64, std::collections::BTreeMap<String, u64>, i64, i64, u64),
    > = std::collections::BTreeMap::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(op) = v["op"].as_str() else { continue };
        let e = per_op.entry(op.to_string()).or_default();
        e.0 += tokenade_core::gain::delivered_of(&v);
        if let Some(sess) = residue_owner(&v) {
            *e.1.entry(sess.to_string()).or_default() += tokenade_core::gain::delivered_of(&v);
        }
        let day = v["ts"].as_i64().unwrap_or(0) / 86_400;
        e.2 = e.2.max(day);
        if v["rollup"].as_bool().unwrap_or(false) {
            e.3 = e.3.max(day);
            e.4 += tokenade_core::gain::delivered_of(&v);
        }
    }
    per_op
        .into_iter()
        .map(
            |(op, (total, by_sess, last_day, rolled_last_day, rolled))| {
                let attributable: u64 = by_sess.values().sum();
                (
                    op,
                    SessionConcentration {
                        share: top_share(&by_sess),
                        coverage: coverage_of(attributable, total),
                        last_day,
                        rolled_last_day,
                        rolled_share: if total == 0 {
                            0.0
                        } else {
                            rolled as f64 / total as f64
                        },
                    },
                )
            },
        )
        .collect()
}

/// **To whom is this row's residue ATTRIBUTABLE?**
///
/// `None` means "to nobody": a rollup bucket carries the sum of a
/// day and no session, and a row without a `sess` field names none either.
/// Both are unknown, not an owner.
///
/// Written once because it was written THREE times, and the third copy
/// lied: `focus_scan` did `v["sess"].as_str().unwrap_or("?")`, so it
/// **invented a session named `?`** that collected all the rows carrying
/// none — then concluded "a single session carries 90 % of the residue,
/// set aside". MEASURED 2026-09-07: `web-search` 90 % in `?`, `auto:code-strip`
/// 100 %. The view refused two targets on what is exactly the ABSENCE of
/// proof, and contradicted the table where those two rows appear.
pub(crate) fn residue_owner(v: &serde_json::Value) -> Option<&str> {
    if v["rollup"].as_bool().unwrap_or(false) {
        return None;
    }
    v["sess"].as_str()
}

pub(crate) fn coverage_of(attributable: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        attributable as f64 / total as f64
    }
}

impl SessionConcentration {
    /// **The only path** to a `SessionConcentration`.
    ///
    /// The two views built it field by field, and both were wrong
    /// on the same field: `coverage: 1.0`, on the belief that the session map
    /// carries all the residue. True of the ROWS it contains, false of the op —
    /// MEASURED: `webfetch_compact` has 489,873 of its 491,215 bytes in
    /// rollup buckets, and the probe verdict fell on the remaining 1,342,
    /// i.e. 0.3 % of the bytes.
    ///
    /// Fixing both sites left the class open: a third
    /// reader would have rewritten the literal. Here the site no longer NAMES the field,
    /// it must pass what no session carries.
    /// The `--focus` rows, split AND measured in one go.
    ///
    /// Two halves of one decision — "a row without a session goes into its
    /// own bucket" and "that bucket enters the denominator" — lived in two
    /// places of the rendering, and two mutations survived there because a caller
    /// could keep one while forgetting the other. The site no longer builds a
    /// denominator: it receives the split and the measurement together.
    pub(crate) fn of_focus_rows(
        rows: &[(u64, u64, i64, Option<String>)],
        rolled_after: u64,
    ) -> (std::collections::BTreeMap<String, u64>, u64, Self) {
        let mut by_sess: std::collections::BTreeMap<String, u64> = Default::default();
        let mut sessionless = 0u64;
        for (_, after, _, sess) in rows {
            match sess {
                Some(s) => *by_sess.entry(s.clone()).or_default() += after,
                None => sessionless += after,
            }
        }
        let c = Self::measured(&by_sess, rolled_after + sessionless);
        (by_sess, sessionless, c)
    }

    pub(crate) fn measured(
        per_sess: &std::collections::BTreeMap<String, u64>,
        unattributed: u64,
    ) -> Self {
        let attributable: u64 = per_sess.values().sum();
        SessionConcentration {
            share: top_share(per_sess),
            coverage: coverage_of(attributable, attributable + unattributed),
            // This view builds its concentration from the rows it
            // already holds; the dates come to it from its own sweep.
            last_day: 0,
            rolled_last_day: 0,
            rolled_share: 0.0,
        }
    }
}

/// **"From when to when, and is it over?"** — a single spelling.
///
/// Three places asked the question: a row's window, its worst
/// events, and the rollup buckets. The third did NOT ask it, and that is
/// what cost the middle of an iteration: `html-to-markdown` carries 21,212,031
/// bytes — 40 % of the pool — in six buckets that name no compactor,
/// and nothing said whether they were from yesterday or from July. They are from July:
/// the blindness is history, not a live defect, and the conclusion
/// flips depending on the answer.
pub(crate) fn day_span(days: impl IntoIterator<Item = i64>, today: i64) -> String {
    let mut first = i64::MAX;
    let mut last = 0i64;
    for d in days {
        if d <= 0 {
            continue;
        }
        first = first.min(d);
        last = last.max(d);
    }
    if last == 0 {
        return "undated".to_string();
    }
    let idle = today - last;
    format!(
        "from {} to {}{}",
        iso_day(first),
        iso_day(last),
        if idle > 0 {
            format!(" — NOTHING for {idle} day(s)")
        } else {
            String::new()
        }
    )
}

/// The current UTC day, in the unit of `by_day` (days since the epoch).
pub(crate) fn today_day() -> i64 {
    (tokenade_core::platform::now_unix_secs() / 86_400) as i64
}

/// A day number rendered `YYYY-MM-DD`. Civil computation from the epoch, without
/// a dependency: the table only carries integers.
pub(crate) fn iso_day(day: i64) -> String {
    // Howard Hinnant's algorithm (civil_from_days), public domain.
    let z = day + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{:02}-{:02}", if m <= 2 { y + 1 } else { y }, m, d)
}

/// The share carried by the biggest contributor of a split. A single
/// spelling for the three places that asked the question — the `op`
/// table, the compactor table and the `--focus` view — each with its own
/// sum and its own division until now.
pub(crate) fn top_share<K: Ord>(by: &std::collections::BTreeMap<K, u64>) -> f64 {
    let total: u64 = by.values().sum();
    let top = by.values().copied().max().unwrap_or(0);
    if total == 0 {
        0.0
    } else {
        top as f64 / total as f64
    }
}
