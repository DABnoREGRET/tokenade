//! Rarity-weighted retention — how much of the INFORMATION in a raw output
//! survives compaction.
//!
//! A byte-ratio ("we cut 90%") says nothing about whether the 10% kept is the
//! part that mattered: dropping 900 identical log lines and dropping the one
//! stack frame naming the failing function score the same. This metric weights
//! every distinct atom by its self-information `-log2(count/total)`, so a
//! token that appears once in the input is worth far more than one that
//! appears a thousand times, and reports the fraction of that weight the
//! compacted text still contains.
//!
//! There is no universal threshold and the corpus proves it: 77% of fixtures
//! round-trip at 1.00, while `tasklist` sits at 0.29 and `sc_query` at 0.10 —
//! discarding per-process noise IS their job. Any floor loose enough to admit
//! those admits a compactor that has started eating the payload.
//!
//! So the gate built on this is a per-fixture RATCHET, not a threshold:
//! `tests/richness_corpus.rs` records what each compactor retains today and
//! fails when it retains less. It runs by default, like any other test.
//!
//! Measured on the day it was written: capping `tasklist` at 8 rows instead of
//! 30 silently drops 22 processes; the entire workspace suite (6417 + 409 + 49
//! tests) stays green, and the savings number IMPROVES from −75% to −92%. That
//! is the failure mode this metric exists for — the ordinary signal moves the
//! wrong way, so only a retention measure can see it.
//!
//! Idea taken from a competitor scan (2026-07-29, tokf's
//! `richness.rs`); the atom rule and the "opt-in floor, never a gate" framing
//! are theirs, the implementation is ours.

use std::collections::HashMap;

/// Shortest run of atom characters worth weighing. Below this almost
/// everything is punctuation, shell noise, or English glue whose loss carries
/// no signal — counting it would drown the rare identifiers we actually care
/// about.
const MIN_ATOM_LEN: usize = 6;

/// Split text into atoms: maximal runs of alphanumeric / `_` / `-` / `.` of at
/// least [`MIN_ATOM_LEN`] characters, lowercased.
///
/// `-` and `.` are IN because the identifiers worth protecting are shaped like
/// `tokenade-core`, `api.example.com`, `some_module.rs:412` — splitting those
/// apart would turn one rare atom into several common ones and understate the
/// loss.
fn atoms(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() || ch == '_' || ch == '-' || ch == '.' {
            cur.push(ch.to_ascii_lowercase());
        } else {
            if cur.chars().count() >= MIN_ATOM_LEN {
                out.push(std::mem::take(&mut cur));
            } else {
                cur.clear();
            }
        }
    }
    if cur.chars().count() >= MIN_ATOM_LEN {
        out.push(cur);
    }
    out
}

/// Fraction of `original`'s rarity-weighted information present in
/// `compacted`, in `0.0..=1.0`.
///
/// Returns 1.0 when the original carries no atoms at all (nothing could be
/// lost) so callers can assert a floor without special-casing tiny inputs.
pub fn richness(original: &str, compacted: &str) -> f64 {
    let orig_atoms = atoms(original);
    if orig_atoms.is_empty() {
        return 1.0;
    }
    let total = orig_atoms.len() as f64;
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for a in &orig_atoms {
        *counts.entry(a.as_str()).or_insert(0) += 1;
    }
    // The kept set is membership-based, not count-based: folding 900 copies of
    // a line down to one plus a `(×900)` marker keeps the information, and
    // must not be scored as a 99.9% loss.
    let kept: std::collections::HashSet<String> = atoms(compacted).into_iter().collect();

    let mut total_weight = 0.0f64;
    let mut kept_weight = 0.0f64;
    for (atom, count) in &counts {
        // Self-information of the atom under the input's own distribution.
        // A hapax in a 10k-atom output weighs ~13.3; a token that IS half the
        // output weighs 1.0.
        let weight = -((*count as f64 / total).log2());
        total_weight += weight;
        if kept.contains(*atom) {
            kept_weight += weight;
        }
    }
    if total_weight <= 0.0 {
        // Degenerate: a single atom repeated, so p = 1 and every weight is 0.
        // Fall back to plain membership — either it survived or it didn't.
        return if counts.keys().all(|a| kept.contains(*a)) {
            1.0
        } else {
            0.0
        };
    }
    (kept_weight / total_weight).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_total_retention() {
        let raw = "error in module_alpha at line 42\nwarning in module_beta\n";
        assert_eq!(richness(raw, raw), 1.0);
    }

    #[test]
    fn empty_compaction_retains_nothing() {
        let raw = "connection refused to database_primary";
        assert_eq!(richness(raw, ""), 0.0);
    }

    #[test]
    fn atomless_input_cannot_lose_information() {
        // No run of 6+ atom chars anywhere — nothing to weigh.
        assert_eq!(richness("a b c ok 42 -", ""), 1.0);
    }

    #[test]
    fn folding_repeats_costs_nothing() {
        // The shape every compactor produces: N identical lines → one line
        // plus a count marker. The information is intact, so must the score be.
        let raw = "retrying connection\n".repeat(50);
        let folded = "retrying connection (×50)";
        assert!(
            richness(&raw, folded) > 0.99,
            "repeat-folding must not read as loss, got {}",
            richness(&raw, folded)
        );
    }

    #[test]
    fn dropping_the_rare_line_costs_more_than_dropping_a_common_one() {
        // 40 copies of a boilerplate line + one unique failure. Cutting the
        // failure must score strictly worse than cutting the boilerplate, even
        // though the byte savings are the opposite way round.
        let mut raw = "compiling package_common v1.0\n".repeat(40);
        raw.push_str("thread panicked at unique_symbol_xyz in crashing_module\n");
        let kept_failure = "compiling package_common v1.0 (×40)\n\
                            thread panicked at unique_symbol_xyz in crashing_module";
        let kept_boilerplate = "compiling package_common v1.0 (×40)";
        let with = richness(&raw, kept_failure);
        let without = richness(&raw, kept_boilerplate);
        assert!(
            with > without,
            "keeping the rare failure ({with}) must outscore dropping it ({without})"
        );
        assert!(with > 0.99, "nothing was lost, got {with}");
    }

    #[test]
    fn compound_identifiers_stay_whole() {
        // `-` and `.` are atom characters: a path or a crate name is ONE rare
        // atom, not several common ones.
        let raw = "failed to open crates/tokenade-core/src/richness.rs";
        assert!(atoms(raw).iter().any(|a| a.contains("tokenade-core")));
    }

    #[test]
    fn case_is_not_information() {
        let raw = "ConnectionTimeout in NetworkModule";
        let lowered = "connectiontimeout in networkmodule";
        assert_eq!(richness(raw, lowered), 1.0);
    }
}
