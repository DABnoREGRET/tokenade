//! **Search INSIDE the folded bytes** rather than asking for everything again.
//!
//! Most recoveries do not want the blob: they want the error line, the JSON
//! field, the name of the test that failed. Returning 400 KB for three lines
//! refunds the agent for a fold we had just made it pay for.
//!
//! And a `--grep` that finds NOTHING must say so usefully: without a sample
//! of what is there, the agent cannot tell "wrong pattern" from "wrong hash",
//! and it calls again without the pattern — that is, everything.

use super::*;

/// The per-pattern header, printed only when more than one pattern is grouped.
/// Shared verbatim by the stash path and the in-memory path — they used to
/// carry two copies of this line and of the footer below.
pub(crate) fn print_grep_head(group: bool, pat: &str, matched: usize, total: usize, capped: &str) {
    if group {
        outln!(
            "=== grep {pat:?} — {matched} match(es) of {total} line(s) held under this \
             hash{capped} ==="
        );
    }
}

/// The count on stderr, printed only when a single pattern went unheaded.
///
/// `of {total} total` said "of the whole output", which is a claim this
/// command cannot make: a fold-store hash IS the source, while a disclose
/// stash is often a folded summary. The number is the same; what it counts
/// is named.
pub(crate) fn print_grep_tail(group: bool, matched: usize, total: usize, capped: &str) {
    if !group {
        eprintln!(
            "[tokenade:expand-ref] {matched} matching line(s) of {total} held under \
             this hash{capped}"
        );
    }
}

/// Does `line` hold `pat`, folding case on BOTH sides when asked?
///
/// One predicate for the count and for the listing that follows it: they read
/// the same lines, and a count that disagrees with its own list is worse than
/// either being wrong alone. Named rather than inlined so a test can reach it
/// — the path that uses it is served only when a ref comes from the dedup
/// cache instead of the stash, which an end-to-end fixture cannot arrange.
use tokenade_core::disclose::GrepMatcher as Matcher;

/// Kept as the spelling the unit tests read; the behaviour lives in
/// [`tokenade_core::disclose::GrepMatcher`], shared with the stash path.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn line_matches(line: &str, pat: &str, ignore_case: bool) -> bool {
    Matcher::new(pat, ignore_case).is_match(line)
}

/// A pattern that matched nothing has two very different meanings, and both
/// used to print the same line.
///
/// `--grep alpha` over a blob whose every line says `Alpha` answered
/// "0 matching line(s) of 3000 total" — byte-identical to a term the content
/// genuinely does not hold. On the RECOVERY command, which is only reached
/// after a fold already took the bytes away, that reads as an answer: the
/// caller concludes the content is not there and stops looking.
/// How many lines of orientation a zero-match grep gets.
///
/// A miss with no sample costs a whole turn. MEASURED 2026-09-05 across this
/// machine's transcripts: of 38 `--grep` answers, **22 matched nothing**, and
/// in every case sampled the agent's very next call was another `expand-ref`
/// on the SAME hash — `| tail -26`, or `--grep "."` — purely to find out what
/// was in there. Six of the 22 held fewer than 60 lines in total, so the
/// content it was refusing to show was smaller than the round trip it forced.
///
/// The lines go to STDERR, beside the count, because they are orientation and
/// not the grep's result: `expand-ref --grep X | wc -l` must still answer 0.
pub(crate) const GREP_MISS_SAMPLE_LINES: usize = 12;

/// A few lines of what the hash actually holds, so the pattern can be fixed
/// without spending a turn to look.
pub(crate) fn print_grep_miss_sample(hash: &str, total: usize) {
    let Some((body, _)) = tokenade_core::disclose::read_stash_by_hash(hash) else {
        return;
    };
    let lines: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return;
    }
    let n = lines.len().min(GREP_MISS_SAMPLE_LINES);
    let shown: Vec<String> = lines[..n]
        .iter()
        .map(|l| {
            let t = l.trim_end();
            if t.chars().count() > 160 {
                format!("{}…", t.chars().take(160).collect::<String>())
            } else {
                t.to_string()
            }
        })
        .collect();
    eprintln!(
        "[tokenade:expand-ref] first {n} of {total} line(s), so the pattern can be \
         refined without another round trip (or ask instead: `--prompt \"q1, q2\"`):"
    );
    for l in shown {
        eprintln!("  {l}");
    }
}

pub(crate) fn print_grep_miss(pat: &str, ignore_case: bool, would_match_ignoring_case: usize) {
    if !ignore_case && would_match_ignoring_case > 0 {
        eprintln!(
            "[tokenade:expand-ref] nothing matches {pat:?} exactly, but \
             {would_match_ignoring_case} line(s) match ignoring case — repeat with \
             `--ignore-case`."
        );
        return;
    }
    // A THIRD meaning of an empty result, after "genuinely absent" and "present
    // in another case". A stash is not always the complete output: a fold-store
    // hash IS the source byte for byte, while a disclose stash is often a
    // folded summary — its own banner says `omits middle lines`. Measured
    // 2026-09-01 on `grep -rn 'fn ' filter.rs`: 280 lines out, 207 kept under
    // the hash, and a symbol living in the 73 that were not kept answered
    // `nothing matches …, ignoring case either` — which reads as "not in the
    // output" and ends the search.
    //
    // Naming the scope costs one clause and is true whichever kind of hash this
    // is; claiming which kind it is would need stored provenance, which this
    // command does not have. The COUNT is not repeated
    // here: the line above already carries it, and it now says `held under this
    // hash` where it used to say `total`, which was the same overclaim in the
    // one place the number was printed.
    eprintln!(
        "[tokenade:expand-ref] nothing matches {pat:?} in what this hash holds, \
         ignoring case either — and a stash can be a folded summary, so a line \
         the run printed may not be among them."
    );
}

/// `Some(rc)` when the stash itself answered the grep, `None` when nothing
/// matched and the shared in-memory path below must run.
pub(crate) fn serve_stash_grep(hash: &str, grep_pats: &[String], ignore_case: bool) -> Option<i32> {
    let group = grep_pats.len() > 1;
    let mut any = false;
    // Whether anything was actually RETRIEVED. A grep that matched nothing
    // served the stash and recovered none of it, so it spared no re-run and
    // is credited nothing — crediting the whole blob for a zero-line answer
    // is how this lever produced its largest field rows.
    let mut retrieved = false;
    let mut blob_toks = 0usize;
    // What this round actually PUTS in the context, accumulated as it is
    // printed. The credit nets it out, so a grep that answers with four lines
    // is not credited as though it had re-shipped the whole blob.
    let mut delivered = String::new();
    for pat in grep_pats {
        if let Some((hits, matched, total, bt)) =
            tokenade_core::disclose::grep_stash_by_hash(hash, pat, ignore_case)
        {
            any = true;
            blob_toks = bt;
            // Never let a capped result read as exhaustive. A pattern
            // that overflows the cap is also the moment to suggest the
            // better verb: hundreds of literal hits usually mean the
            // agent wants an ANSWER, not a line list.
            let capped = if hits.len() < matched {
                tokenade_core::gain::record_nudge(
                    "grep-capped",
                    "fire",
                    tokenade_core::gain::NudgeMetric::Items(matched),
                );
                format!(
                    " — showing {} of {matched}; refine the pattern, or ask instead: \
                             `--prompt \"<what you need>\"`",
                    hits.len()
                )
            } else {
                String::new()
            };
            print_grep_head(group, pat, matched, total, &capped);
            retrieved |= !hits.is_empty();
            for (n, l) in &hits {
                outln!("{n}:{l}");
                delivered.push_str(l);
                delivered.push('\n');
            }
            print_grep_tail(group, matched, total, &capped);
            if matched == 0 {
                // Only on a miss, and only then does the second read happen.
                let folded = tokenade_core::disclose::grep_stash_by_hash(hash, pat, true)
                    .map_or(0, |(_, m, _, _)| m);
                print_grep_miss(pat, ignore_case, folded);
                // Only when the case-fold hint did NOT fire: there the caller
                // already has its next move, and a sample would bury it.
                if ignore_case || folded == 0 {
                    print_grep_miss_sample(hash, total);
                }
            }
        }
    }
    if !any {
        return None;
    }
    // The alternative is RE-RUNNING the command, whose output the harness
    // would truncate at the delivery cap — that bound is applied by
    // `gain::realistic_of_row`. The hits are what this round put in the
    // context window, and that is a cost this fold caused, so it is netted
    // out here rather than left at zero.
    if retrieved {
        let delivered_toks = tokenade_core::tokenizer::estimate(&delivered);
        record_recovery_credit(hash, blob_toks, delivered_toks);
    }
    Some(0)
}

pub(crate) fn print_grep_over_text(red: &str, grep_pats: &[String], ignore_case: bool) {
    let group = grep_pats.len() > 1;
    let total = red.lines().count();
    for pat in grep_pats {
        // Compiled ONCE per pattern, not once per line.
        let m = Matcher::new(pat, ignore_case);
        let hit = |l: &str| -> bool { m.is_match(l) };
        // Same caps as the stash path: a single-line file must
        // not dump itself through a "matching line".
        let matched = red.lines().filter(|l| hit(l)).count();
        let shown = matched.min(tokenade_core::disclose::GREP_MAX_HITS);
        let capped = if shown < matched {
            format!(" — showing {shown} of {matched}, refine the pattern for the rest")
        } else {
            String::new()
        };
        print_grep_head(group, pat, matched, total, &capped);
        for (i, l) in red
            .lines()
            .enumerate()
            .filter(|(_, l)| hit(l))
            .take(tokenade_core::disclose::GREP_MAX_HITS)
        {
            outln!(
                "{}:{}",
                i + 1,
                tokenade_core::disclose::clip_match_line(l, pat)
            );
        }
        print_grep_tail(group, matched, total, &capped);
        if matched == 0 {
            let folded = red
                .lines()
                .filter(|l| l.to_lowercase().contains(&pat.to_lowercase()))
                .count();
            print_grep_miss(pat, ignore_case, folded);
        }
    }
}
