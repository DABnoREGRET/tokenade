//! **What is NOT a target of the ranking.**
//!
//! A probe I ran five times on the same day, a gain BORROWED from another
//! path, a baseline that is a multiplier and not a reading, a stale
//! reading. Each of these cases puts a false name at the top of the ranking —
//! and the first one did. Setting aside is therefore a decision of the bench, never a
//! silence: each exclusion carries its PROOF, and `render_excluded` writes it.

use super::*;

/// Is this residue SPREAD usage, or a single day / a single session?
///
/// The function used to be called "is this my own exhaust" and that was true of
/// its original motivation — an iteration probe I had just written. What
/// it measures is broader and more honest: a CONCENTRATION. MEASURED while
/// writing this, `extract-docx` puts 97 % of its attributable residue on a
/// single session, and that session is called `compat`: it is not
/// mine. Setting it aside under a name that says "my probe" would be a claim
/// the measurement does not support. For a ranking that asks "where does an
/// improvement pay off on real usage", it is the concentration that
/// disqualifies, whoever the author.
///
/// Two forms, and the second nearly sent me to work on my own
/// measurement. The first: concentrated on one day AND little used — that is the
/// original rule, `a11y-compact` weighed 11.2 % of what was delivered for 5 calls on the same day.
///
/// The second: **almost all the residue on ONE day, whatever the number
/// of calls.** MEASURED 2026-09-07: `auto:html` came top of the per-compactor table
/// with 4,694,843 bytes still delivered over 409 uses — and **98 % of
/// those bytes came from the current session**, 391 rows written in the
/// preceding minutes by this very iteration's probes. The number
/// of calls protects from nothing: what we rank are BYTES, so
/// it is their concentration that must be looked at. A compactor really
/// used over 103 days does not put 98 % of its residue on a single one.
pub(crate) fn self_probe_reason(
    heaviest_day_share: f64,
    sess: SessionConcentration,
    count: u64,
) -> Option<String> {
    if heaviest_day_share >= 0.95 && count < 50 {
        return Some(format!(
            "a single day carries {:.0} % of the residue, over {count} uses",
            100.0 * heaviest_day_share
        ));
    }
    if heaviest_day_share >= 0.98 {
        return Some(format!(
            "a single day carries {:.0} % of the residue",
            100.0 * heaviest_day_share
        ));
    }
    // And the SESSION, which judges better than the day: a day can carry
    // real work next to a probe, a session that alone writes
    // almost all of a compactor's residue IS the probe. The bench already had it
    // at hand without using it — `--focus` printed "session 447cf2a1 ·
    // 83 %" and still gave its verdict on the day.
    //
    // The COVERAGE is in the rule, and that is what prevents
    // over-claiming: a rollup bucket carries no session's name,
    // so on `auto-compact` only 6.5 MB of the 18.5 MB still delivered are
    // attributable. Saying "it is my exhaust" of 18.5 MB because 83 % of
    // 6.5 is would be exactly the error this guard exists to prevent.
    if sess.share >= 0.80 && sess.coverage >= 0.5 && count < 5_000 {
        return Some(format!(
            "a single session carries {:.0} % of the attributable residue, which is \
             {:.0} % of the total",
            100.0 * sess.share,
            100.0 * sess.coverage
        ));
    }
    None
}

/// Heading of a row set aside because its residue is concentrated.
pub(crate) const CONCENTRATED: &str = "concentrated";
/// Heading of a row set aside because it never had an input.
pub(crate) const BORROWED: &str = "borrowed baseline";

/// Does this row compare its rate to something the machine read?
///
/// `gain --json` answers `measured`, `borrowed` or `mixed`.
/// A `borrowed` row has no input at all — `structure_map` BUILDS
/// its answer, it folds nothing — so its `percent` is the published ratio and
/// its "residue" is the whole answer. Ranking it among fourteen measured
/// rates is what sent me looking for a fold in a repo map.
///
/// `mixed` is set aside too, and it is the REAL case on this recording:
/// none of the four rows reads `borrowed`, because the
/// `ci_heuristic` stamp arrived after part of the history and an earlier
/// row does not carry it. `structure_map` is 93 % borrowed,
/// `symbol_find` 93 %, `semantic_search` 49 %. A half-fabricated rate
/// is not a rate: keeping only `borrowed` would have left in the table
/// exactly the row that made me lose the iteration.
///
/// `skeleton` stays, and that is the distinction that matters: it tokenises the
/// file it is handed, its baseline is measured, its borrowed share is zero.
/// The terminal table, for its part, partitions ALL the code-intel ops and
/// therefore loses the measured half of the bucket — the biggest, 73 %.
pub(crate) fn borrowed_reason(
    baseline: Option<&str>,
    op: &str,
    borrowed_saved: u64,
    saved: u64,
    rest: u64,
    count: u64,
) -> Option<Excluded> {
    if matches!(baseline, Some("measured") | None) || borrowed_saved == 0 {
        return None;
    }
    let share = if saved > 0 {
        100.0 * borrowed_saved as f64 / saved as f64
    } else {
        100.0
    };
    Some(Excluded {
        kind: BORROWED,
        why: format!(
            "{share:.0} % of its savings is compared {}, not to an input read: nothing \
             was folded, the {} bytes over {count} calls ARE the answer",
            fabricated_from(op),
            thousands(rest)
        ),
    })
}

/// **The reason DISPLAYED must be the one that decided.** The first version named the
/// code-intel ratio for every row set aside, and the first row the
/// rule gained by widening — `web-search` — was rendered under that reason
/// although its baseline is an entirely different native multiplier. An instrument that
/// explains its decision badly is rightly disobeyed.
pub(crate) fn fabricated_from(op: &str) -> &'static str {
    if tokenade_core::code_intel_heuristic::is_code_intel(op) {
        "to the published ratio of `code_intel_heuristic` (57 %, CodeGraph 0.9.4)"
    } else if tokenade_core::coefficients::ALWAYS_COUNTERFACTUAL_OPS.contains(&op) {
        "to the native multiplier of `coefficients` (×3, the SERP the model would have ingested)"
    } else {
        "to a coefficient of `coefficients` (native multiplier or flat turn credit)"
    }
}

/// **Why this row is NOT a target — the single spelling of the
/// question.**
///
/// Four rules answer, and they arrived one by one: the
/// concentration, the fabricated baseline, the absence of a
/// named piece and the freshness. Each was written at the moment
/// an iteration went off on a target that was not one, and each
/// should have been asked at the three places that ask the question — the two
/// tables and `--focus`. The first was only at one, and the list of
/// set-aside rows RECOMPUTED its sentence with an `unwrap_or("reason lost")`.
///
/// The order matters: we return the reason that DECIDED, and the most decisive
/// first — a fabricated baseline says there is nothing to fold at all, whereas a
/// dead row could come back to life tomorrow.
pub(crate) fn not_a_target(
    op: &str,
    baseline: Option<&str>,
    borrowed_saved: u64,
    saved: u64,
    rest: u64,
    count: u64,
    sess: SessionConcentration,
    day_share: f64,
    has_a_named_compactor: bool,
    today: i64,
    window_days: u64,
) -> Option<Excluded> {
    borrowed_reason(baseline, op, borrowed_saved, saved, rest, count)
        .or_else(|| {
            self_probe_reason(day_share, sess, count).map(|why| Excluded {
                kind: CONCENTRATED,
                why,
            })
        })
        .or_else(|| stale_reason(sess, today, window_days))
        .or_else(|| history_reason(sess, rest, today))
        .or_else(|| {
            (!has_a_named_compactor).then(|| Excluded {
                kind: HEADLESS,
                why: format!(
                    "all its compactors are set aside: its residue is in rollup buckets \
                     that name nobody, so there is no piece to open{}",
                    if sess.rolled_last_day > 0 {
                        format!(
                            " — these buckets date from {} at the latest, i.e. {} day(s), so \
                             it is HISTORY and not a blind spot of today",
                            iso_day(sess.rolled_last_day),
                            today - sess.rolled_last_day
                        )
                    } else {
                        String::new()
                    }
                ),
            })
        })
}

/// Heading of a row that no longer has any named compactor.
pub(crate) const HEADLESS: &str = "no named piece";

/// Heading of a row ranked on its history.
pub(crate) const HISTORY: &str = "history";

/// Beyond this share of residue in old rollup buckets, the row is
/// ranked on what it used to deliver, not on what it delivers.
pub(crate) const HISTORY_SHARE: f64 = 0.9;

/// **What does this row deliver TODAY?**
///
/// `stale_reason` judges on the LAST event; a row that still receives
/// some but whose weight is almost all in rollup buckets older than
/// [`STALE_AFTER_DAYS`] escapes it. Measured 2026-10-02: `sysinfo` 2nd in the
/// ranking for 2,978 bytes delivered since July.
pub(crate) fn history_reason(
    sess: SessionConcentration,
    rest: u64,
    today: i64,
) -> Option<Excluded> {
    let old = sess.rolled_last_day > 0 && today - sess.rolled_last_day > STALE_AFTER_DAYS;
    (old && sess.rolled_share >= HISTORY_SHARE).then(|| {
        let live = (rest as f64 * (1.0 - sess.rolled_share)).round() as u64;
        Excluded {
            kind: HISTORY,
            why: format!(
                "{:.0} % of its residue sleeps in rollup buckets from {} at the latest ({} day(s)); \
                 since then, it has delivered only {live} bytes",
                sess.rolled_share * 100.0,
                iso_day(sess.rolled_last_day),
                today - sess.rolled_last_day
            ),
        }
    })
}

/// Heading of a row nothing arrives at any more.
pub(crate) const STALE: &str = "dead";

/// How many days without a single event before a row stops being a
/// target. Three days: a fix shipped yesterday still leaves its previous rate
/// in the `span.days` window (104 measured on 2026-09-08), and that is exactly
/// what sent an earlier iteration onto `read_code_strip_lite`.
pub(crate) const STALE_AFTER_DAYS: i64 = 3;

/// The ranking's window, as `gain --json` measures it (`span.days`).
///
/// Measured 104 on 2026-09-08, 115 on 2026-09-19. A constant here becomes the
/// defect again: the document then shows two windows, the true header and the
/// frozen prose.
pub(crate) fn ranking_covers(window_days: u64) -> String {
    format!("the ranking covers {window_days} days")
}

/// **Does this row still receive anything?**
///
/// The window is `span.days`, not a constant. A compactor fixed last
/// week appears there with its BEFORE rate and presents itself as a
/// target: measured 2026-09-08, `read_code_strip_lite` was 7th at −43 % for
/// 9,742 bytes per call, and its last event dated from 2026-09-02,
/// before the fix to the read-view ladder. An earlier iteration put the date
/// in `--focus`, which is only consulted after choosing; here it is
/// in the table.
pub(crate) fn stale_reason(
    sess: SessionConcentration,
    today: i64,
    window_days: u64,
) -> Option<Excluded> {
    let idle = today - sess.last_day;
    (sess.last_day > 0 && idle > STALE_AFTER_DAYS).then(|| Excluded {
        kind: STALE,
        why: format!(
            "nothing for {idle} day(s) — last event on {}; {}, so a \
             pre-fix rate still presents itself there as a target",
            iso_day(sess.last_day),
            ranking_covers(window_days)
        ),
    })
}

pub(crate) fn is_self_probe(
    heaviest_day_share: f64,
    sess: SessionConcentration,
    count: u64,
) -> bool {
    self_probe_reason(heaviest_day_share, sess, count).is_some()
}

/// "Set aside, and WHY." The reason is built once, at read time, and
/// carried all the way to rendering.
///
/// It was not: the table filtered on a boolean and the list of
/// set-aside rows RECOMPUTED the sentence, with an `unwrap_or("reason lost — the
/// rule changed under the list")` that itself said what could happen.
/// A second exclusion rule (a borrowed comparison baseline) would have had to
/// be spelled at both places, like the first.
#[derive(Clone)]
pub(crate) struct Excluded {
    /// Under which heading this row is rendered.
    pub(crate) kind: &'static str,
    /// The proof, with the numbers the rule looked at.
    pub(crate) why: String,
}

/// "Set aside, and on what proof." A SILENT removal is half of the
/// defect: it was recorded for the rollup buckets, then for the
/// compactor table, and the list of `op`s remained 125 bare names in the
/// document that steers every iteration. A single rendering for both, otherwise
/// the third list will start again without proof like the first two.
pub(crate) fn render_excluded(title: &str, rows: &[String]) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let mut md = format!("<details><summary>{title} ({})</summary>\n\n", rows.len());
    for r in rows {
        md.push_str(&format!("- {r}\n"));
    }
    md.push_str("\n</details>\n\n");
    md
}
