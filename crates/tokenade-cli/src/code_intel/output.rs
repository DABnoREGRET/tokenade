//! Rendering a code-intelligence answer under a token budget.
//!
//! The body builders every command shares, so a truncated answer says it was
//! truncated in the same words wherever it came from.

use super::*;

/// Pure renderer for query's reduced output modes (Paths / Count /
/// Outline). `Full` is handled by the caller and never reaches here.
/// Kept side-effect-free so it can be unit-tested without an index.
/// Test-only convenience over [`output_mode_body_query_with_totals`] — the
/// logic lives there and IS live; this just supplies an empty `totals`.
#[cfg(test)]
pub(crate) fn output_mode_body_query(
    mode: OutputMode,
    results: &[(String, Vec<tokenade_core::index::Symbol>)],
    multi: bool,
) -> String {
    output_mode_body_query_with_totals(mode, results, multi, &[], None)
}

/// `totals[i]` is how many symbols matched query `i` BEFORE the cap. `count`
/// mode reported the capped list length — so a caller asking "how many?" was
/// handed the cap and could not tell it from the answer. That is the one mode
/// where truncation is invisible by construction.
pub(crate) fn output_mode_body_query_with_totals(
    mode: OutputMode,
    results: &[(String, Vec<tokenade_core::index::Symbol>)],
    multi: bool,
    totals: &[usize],
    // The index, when the caller has one — used ONLY to name the nearest
    // symbols on a miss. `None` keeps the bare refusal, which is what the unit
    // tests want: they assert the rendering, not the suggestion.
    idx: Option<&tokenade_core::index::Index>,
) -> String {
    let mut out = String::new();
    match mode {
        OutputMode::Paths => {
            let mut files: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
            for (_q, hits) in results {
                for s in hits {
                    files.insert(s.file.clone());
                }
            }
            for f in files {
                out.push_str(&f);
                out.push('\n');
            }
            let shown: usize = results.iter().map(|(_, h)| h.len()).sum();
            let total: usize = totals.iter().sum();
            if total > shown {
                out.push_str(&truncation_note(shown, total));
            }
        }
        OutputMode::Count => {
            for (i, (q, hits)) in results.iter().enumerate() {
                let n = totals.get(i).copied().unwrap_or(hits.len()).max(hits.len());
                if multi {
                    out.push_str(&format!("{q}: {n}\n"));
                } else {
                    out.push_str(&format!("{n}\n"));
                }
            }
        }
        OutputMode::Outline => {
            for (i, (q, hits)) in results.iter().enumerate() {
                if multi {
                    if i > 0 {
                        out.push('\n');
                    }
                    out.push_str(&format!("==> {q} <==\n"));
                }
                if hits.is_empty() {
                    out.push_str(&match idx {
                        Some(i) => crate::code_intel::miss_line(q, i),
                        None => crate::code_intel::miss_line_no_index(q),
                    });
                    out.push('\n');
                    continue;
                }
                for s in hits {
                    // The one declarer — signature-level, no body.
                    out.push_str(&s.location_line());
                    out.push('\n');
                }
                // The list IS the answer here, so a cut has to be said. Four
                // of query's five renderings already say it — `paths` above,
                // `full` and `--json` in the caller, and `count` reports the
                // pre-cap figure — and this one alone handed back exactly
                // `find_ranked_with_total`'s 30 as if that were all there
                // was. Measured on this repo, the cap binds on 33 of 80
                // sampled real function names.
                let total = totals.get(i).copied().unwrap_or(hits.len());
                if total > hits.len() {
                    out.push_str(&truncation_note(hits.len(), total));
                }
            }
        }
        OutputMode::Full => {}
    }
    out
}

/// The one sentence every capped `query` rendering says. Written twice
/// before (here and in the `full` arm of the caller) and missing from the
/// third, which is how the wordings drift and how one of them goes silent.
pub(crate) fn truncation_note(shown: usize, total: usize) -> String {
    format!("[{shown} of {total} matches shown — narrow the name to see the rest]\n")
}

/// Pure renderer for semantic's reduced output modes. Semantic hits
/// are file-level (no symbol signature is available), so `Outline`
/// drops the matched-terms explainability and keeps `score  file`.
/// `retained` is how many candidates survived ranking, which is what `count`
/// owes the caller: the previous answer was the size of the caller's own
/// window, identical for every query ever asked. `None` keeps the old
/// behaviour for callers that have no total to give.
/// One line saying the ranking stopped at the depth it was given, or nothing
/// when it did not.
///
/// `-o count` has always marked a pool-filling count with a `+` — "a count
/// that fills the pool is a FLOOR, not a total … raise it with `--limit`" —
/// and the three modes that LIST rows said nothing at all. Measured
/// 2026-09-04 on this repository: `semantic "dedup" -o count` answered `40+`
/// while the same query returned exactly ten rows in every listing mode, with
/// no sign that a further screen existed. The help promises the opposite for
/// the neighbouring flag: "the footer says how many results it withheld, so a
/// truncated answer never looks complete".
///
/// Empty unless the ranking actually filled the requested depth: an answer
/// that came back short was not cut, and a footer there would invent a
/// horizon that is not real.
pub(crate) fn semantic_depth_footer(
    hits_shown: usize,
    limit: usize,
    retained: Option<(usize, bool)>,
) -> String {
    if limit == 0 || hits_shown < limit {
        return String::new();
    }
    let (total, floor) = match retained {
        Some((n, f)) if n > hits_shown => (n, f),
        _ => return String::new(),
    };
    let plus = if floor { "+" } else { "" };
    format!(
        "[tokenade:depth] {hits_shown} shown — the ranking considered {total}{plus}. \
         Raise with --limit {} to look past the first screen.\n",
        limit.saturating_mul(2)
    )
}

pub(crate) fn output_mode_body_semantic_with_total(
    mode: OutputMode,
    hits: &[tokenade_core::semantic::Hit],
    retained: Option<(usize, bool)>,
) -> String {
    let mut out = String::new();
    match mode {
        OutputMode::Paths => {
            let mut files: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
            for h in hits {
                files.insert(h.file.clone());
            }
            for f in files {
                out.push_str(&f);
                out.push('\n');
            }
        }
        OutputMode::Count => {
            match retained {
                // Ranking scores a bounded pool of candidates, so a count that
                // fills the pool is a FLOOR, not a total. Marking it keeps the
                // number from being read as "this is all there is" — raise it
                // with `--limit`.
                Some((n, true)) => out.push_str(&format!("{n}+\n")),
                Some((n, false)) => out.push_str(&format!("{n}\n")),
                None => out.push_str(&format!("{}\n", hits.len())),
            }
        }
        OutputMode::Outline => {
            for h in hits {
                out.push_str(&format!("{:5.2}  {}\n", h.score, h.file));
            }
        }
        OutputMode::Full => {}
    }
    out
}

/// Test-only convenience over [`output_mode_body_semantic_with_total`].
#[cfg(test)]
pub(crate) fn output_mode_body_semantic(
    mode: OutputMode,
    hits: &[tokenade_core::semantic::Hit],
) -> String {
    output_mode_body_semantic_with_total(mode, hits, None)
}

/// Count of signature entries in a skeleton (non-empty lines). Used by
/// `skeleton --output-mode count`. Approximate for multi-line
/// signatures, but a faithful "how many things does this file
/// declare" figure for the common case.
pub(crate) fn skeleton_signature_count(sk: &str) -> usize {
    sk.lines().filter(|l| !l.trim().is_empty()).count()
}

pub(super) fn push_dependents(b: &mut String, deps: &[String], indent: &str) {
    for d in deps.iter().take(IMPACT_MAX_DEPENDENTS) {
        b.push_str(indent);
        b.push_str(d);
        b.push('\n');
    }
    if deps.len() > IMPACT_MAX_DEPENDENTS {
        b.push_str(indent);
        b.push_str(&format!(
            "[{} of {} dependents shown — highest impact first; --json for the full list]\n",
            IMPACT_MAX_DEPENDENTS,
            deps.len()
        ));
    }
}

#[cfg(test)]
mod depth_footer_tests {
    use super::semantic_depth_footer;

    /// The footer appears exactly when the ranking filled the depth it was
    /// given, and says how deep it looked.
    #[test]
    fn a_listing_says_when_it_stopped_at_the_depth_it_was_given() {
        // Ten asked for, ten returned, forty considered and the pool filled.
        let f = semantic_depth_footer(10, 10, Some((40, true)));
        assert!(f.contains("10 shown"), "{f}");
        assert!(f.contains("40+"), "the floor marker is lost: {f}");
        assert!(f.contains("--limit 20"), "no way out is offered: {f}");

        // Same, but the pool did NOT fill: the total is exact, no `+`.
        let f = semantic_depth_footer(10, 10, Some((40, false)));
        assert!(f.contains("40.") || f.contains("40 "), "{f}");
        assert!(
            !f.contains("40+"),
            "an exact total was marked as a floor: {f}"
        );

        // An answer that came back SHORT was not cut by the depth.
        assert_eq!(semantic_depth_footer(4, 10, Some((4, false))), "");
        // Short EVEN THOUGH many candidates were considered: relevance cut
        // this answer, not the depth, and telling the reader to raise
        // `--limit` would send them down a path that changes nothing.
        assert_eq!(semantic_depth_footer(4, 10, Some((40, true))), "");
        // Nor was one where nothing beyond the shown rows was ranked.
        assert_eq!(semantic_depth_footer(10, 10, Some((10, false))), "");
        // And with no total to speak of there is nothing honest to say.
        assert_eq!(semantic_depth_footer(10, 10, None), "");
    }
}
