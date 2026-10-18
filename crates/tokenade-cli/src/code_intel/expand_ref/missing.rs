//! **The hash cannot be found.**
//!
//! MEASURED (758 real transcripts): **41 recoveries out of 436** come back
//! empty-handed, 15 of them on a missing hash. A bare "not found" is
//! therefore a FREQUENT case, not an edge case.
//!
//! The report then says what it knows: how far back the stash keeps, what it
//! holds that comes close, and what remains possible. The horizon matters —
//! an evicted hash and a never-written hash call for two different moves.

use super::*;

/// What the store lets us ASSERT about a missing hash: how many entries it
/// holds, and the age of the oldest. A hash younger than that floor cannot
/// have been swept — expiry does not explain it, and saying so avoids sending
/// the reader hunting a retention problem they do not have.
pub(crate) fn stash_age_horizon() -> Option<(usize, u64)> {
    let dir = tokenade_core::disclose::stash_dir();
    let mut count = 0usize;
    let mut oldest: Option<std::time::SystemTime> = None;
    for e in std::fs::read_dir(&dir).ok()?.flatten() {
        if !e.path().is_file() {
            continue;
        }
        count += 1;
        if let Ok(m) = e.metadata().and_then(|m| m.modified()) {
            oldest = Some(match oldest {
                Some(o) if o <= m => o,
                _ => m,
            });
        }
    }
    if count == 0 {
        return None;
    }
    let days = oldest
        .and_then(|o| std::time::SystemTime::now().duration_since(o).ok())
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or(0);
    Some((count, days))
}

/// The message for a hash that returned nothing — composed HERE, printed by
/// [`report_missing_hash`]. Separate so a test can READ it: the message goes
/// to stderr, and a recovery promise that is checked nowhere is exactly what
/// was once found false elsewhere.
pub(crate) fn missing_hash_report(hash: &str) -> String {
    // A hash is printed as exactly 12 lowercase hex characters and
    // there is no prefix matching, so anything else could never
    // have resolved. Blaming expiry there sends the reader hunting
    // a retention problem they do not have; the real answer is that
    // this is not a hash.
    let well_formed = hash.len() == 12
        && hash
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c));
    if !well_formed {
        return format!(
            "expand-ref: '{hash}' is not a ref hash — those are exactly 12 \
             lowercase hex characters, copied from the banner a compactor \
             printed (`tokenade expand-ref <hash>`)."
        );
    }
    // **A failing recovery must say what to do next.** Measured over 758
    // transcripts: 41 recoveries out of 436 (9.4 %) come back empty-handed,
    // 15 of them on a missing hash — and this sentence named NO next step.
    // That lesson was already paid for: a recovery path is judged where the
    // agent reads, not in `--help`.
    let horizon = match stash_age_horizon() {
        // What the store CONTAINS, not what a rule would like it to contain:
        // the first version concluded "an older hash was swept", and the
        // machine answered "the oldest entry is 17 days old" for a TTL of 7.
        // A sentence deduced from a rule the bytes contradict is one more
        // fabrication.
        Some((n, days)) => {
            format!("  the stash holds {n} entr(y/ies), the oldest written {days} day(s) ago")
        }
        None => "  the stash is empty — nothing is recoverable by hash on this machine".to_string(),
    };
    format!(
        "expand-ref: hash '{hash}' not found in dedup cache or stash\n{horizon}\n  \
         the bytes are gone, so there is no second way back to THEM: re-run the command \
         that printed this hash (`tokenade raw <cmd>` re-runs it uncompacted)"
    )
}

/// The exit code for a hash that resolved to nothing, and why.
pub(crate) fn report_missing_hash(hash: &str) -> i32 {
    eprintln!("{}", missing_hash_report(hash));
    1
}
