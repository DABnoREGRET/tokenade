//! **The elision marker: one spelling, one place.**
//!
//! `render_kept` WRITES it and `disclose::original_weight` READS IT BACK to
//! know how many lines a view represents. Both spelled it by hand,
//! so the day one changes a word the other stops counting, silently and
//! with no test speaking of the agreement.
//!
//! ## What the census found, and why it changed nothing
//!
//! MEASURED 2026-09-07 by `residue_census` on the 265 command captures:
//! `… [N lines elided by tokenade] …` is the **second heaviest shape
//! of the whole residue** — 223 lines for 8,466 bytes, and on `pacman -Q`
//! HALF of what is delivered (4,173 B, 110 markers). The 110 holes are
//! almost all 20 lines: that is not 110 facts, it is ONE fact said 110
//! times.
//!
//! Two shapes were built and measured to say it only once.
//! BOTH are withdrawn, and that is a result:
//!
//! | shape | delivered, 265 captures | e2e invariant |
//! |---|---|---|
//! | single banner at the top, markers removed | −3,569 B | **broken**: not a single weighed line left in the view |
//! | legend at the top + short marker `… [N] …` | **−2,656 B** | **broken**: recovery pointer lost |
//!
//! The first was withdrawn earlier without an explanation. Here it is, MEASURED on
//! 2026-09-08 by the `attach_drop_recovery` log on the recovery
//! fixture, the same input on both sides:
//!
//! ```text
//! BASE     display_bytes 16 116   dropped_tokens 2 104   earned true
//! LEGEND   display_bytes 20 832   dropped_tokens   528   earned false
//! ```
//!
//! **Folding MORE at stage 1 delivered 4,716 MORE bytes.** The shortened view
//! falls under the `disclose` threshold, which then stops summarising — and its
//! summary was worth more than the markers cost. The 2,656-byte gain
//! on the corpus is real; it is smaller than what a single output shape
//! loses on the other side, and the loss takes the recovery
//! pointer with it, which no number buys back.
//!
//! **A fold judged at stage 2 is not decided at stage 1.** The
//! lasting fix is to make `disclose` trigger on what a view
//! REPRESENTS — `original_weight` exists and returns exactly that number — rather
//! than on its literal size. It touches the threshold of the whole product and
//! needs its own measurement: it is an iteration on its own, not a clause
//! added in passing.

use crate::fold_count::{folded_ceiling, note_folded};

/// What a READ `(×N)` marker claims to be worth. The reader counterpart of the spelling
/// this module writes: it lived in `filter.rs`, three thousand lines away from
/// `marker()`, and `fold_count` already names it in its own comment as
/// one of the two places that read a count back.
pub fn run_weight(line: &str) -> usize {
    line.trim_end()
        .strip_suffix(')')
        .and_then(|l| l.rsplit_once("(×"))
        .and_then(|(_, n)| n.parse::<usize>().ok())
        .filter(|n| *n > 1)
        .unwrap_or(1)
}

/// The marker of an isolated hole.
pub fn marker(gap: usize) -> String {
    format!("… [{gap} lines elided by tokenade] …")
}

/// **A regular fact, said once.**
///
/// Regular sampling makes holes that are all equal, and [`marker`] pays for them
/// one by one: on `pacman -Q`, **96 of the 110 holes are exactly 20
/// lines**, hence 96 copies of the same sentence. That is not 96 facts, it is one.
///
/// The banner carries the total of the holes IT REPLACES — never the overall
/// total, otherwise the holes keeping their marker would be counted twice
/// — and [`weight_of`] reads it back unchanged, because the sentence keeps
/// the same hinge (`N lines elided by tokenade`). That was the condition:
/// a view whose next stage can no longer say what it represents loses
/// its summary and its recovery pointer.
pub fn uniform_banner(runs: usize, each: usize, total: usize) -> String {
    format!("… [{total} lines elided by tokenade, in {runs} runs of {each}] …")
}

/// **The marker of a hidden BODY**, in a skeleton view.
///
/// The other shape this product writes to say "N lines are missing here".
/// It was spelled THREE times in `compress.rs`, and nowhere here — so
/// [`weight_of`], which exists precisely to read this kind of line back, could
/// not read it. A stage-2 summary placed on a skeleton view
/// then underestimated the original magnitude, silently: an earlier measurement
/// showed what that gives on the other marker — "elided 148 lines" instead
/// of "standing for 471 line(s)".
///
/// The rendered text is unchanged, to the character: what changes is that it
/// now has a single author, and a reader that knows it.
pub fn body_marker(comment: &str, lines: usize) -> String {
    format!("    {comment} … {lines} lines …")
}

/// How many lines of the original this line REPRESENTS, if it is one of our
/// markers.
///
/// `None` when it is not one: the caller then decides what the line
/// is worth. A content line that LOOKS like a marker goes through here like
/// ours — which is why the caller caps the sum by what its
/// own passes actually removed.
pub fn weight_of(line: &str) -> Option<usize> {
    let t = line.trim();
    // The SKELETON shape: `<comment> … N lines …`. It comes from a
    // different path than `render_kept`, and not reading it amounted to counting those
    // lines as one each.
    if let Some((_, rest)) = t.split_once("… ") {
        if let Some((n, tail)) = rest.split_once(" lines …") {
            if tail.trim().is_empty() {
                if let Ok(n) = n.parse::<usize>() {
                    return Some(n);
                }
            }
        }
    }
    let rest = t.strip_prefix("… [")?;
    if let Some((n, _)) = rest.split_once(" lines elided by tokenade") {
        return n.parse::<usize>().ok();
    }
    // The short shape, under a legend. Required whole (`N] …`) so that the start
    // of a sentence does not read as a number.
    rest.strip_suffix("] …")?.parse::<usize>().ok()
}

// ---------------------------------------------------------------------------
// `render_kept` — the one that WRITES the marker. This module's header already
// named it ("`filter::render_kept` WRITES it and `disclose::original_weight`
// READS IT BACK") but it lived in `filter.rs`, the command dispatcher,
// 3,000 lines away from the spelling it writes. Both halves of an agreement
// kept in two files is the defect this header describes.
// ---------------------------------------------------------------------------

/// **A hole that costs more than what it hides is not a fold.**
///
/// `render_kept` placed a marker on EVERY non-empty hole. The marker is
/// 34 bytes plus the count's digits; a `pacman -Q` line is
/// about twenty. So `… [1 lines elided by tokenade] …` replaced
/// `zziplib 0.13.80-1` with a LONGER sentence, and the reader lost the
/// package name in exchange for nothing. The delivered `pacman -Q` output
/// carried 15 of them, including a ONE-line hole.
///
/// It is the rule the whole rest of the product already applies
/// ([`crate::fold_gate::pays`]): a fold that gives back no bytes is refused.
/// It was missing exactly where the fold is finest.
///
/// Made PURE to be testable: the version living in `render_kept` reads a
/// process ceiling and counters, and a test calling it therefore measures
/// something other than its decision.
pub fn a_gap_pays_for_its_marker(hidden_bytes: usize, gap: usize) -> bool {
    // `+ 1`: the marker takes its own line, so it also costs its
    // newline. The hidden lines carry theirs, already counted by
    // the caller.
    crate::fold_gate::pays(hidden_bytes, 0, marker(gap).len() + 1)
}

/// Renders the kept `indices` (sorted, into `lines`) as text, inserting
/// `… [N lines elided by tokenade] …` wherever two kept indices skip
/// lines. Shared by `filter::truncate_smart` and
/// `compress::truncate_middle`, so that both speak the SAME elision
/// dialect — that is the whole point of this module.
pub fn render_kept(lines: &[&str], indices: &[usize]) -> String {
    // Read the ceiling BEFORE this call adds to it. The markers in `lines`
    // were written by EARLIER passes, so those are the folds that can back a
    // claim; the gaps this call is about to make are `elided_total`, counted
    // separately. Taking the ceiling afterwards lets the call vouch for
    // itself — measured 2026-09-07, that doubled the cap and a 2 000-line
    // input still announced "standing for 3650".
    let ceiling_before = folded_ceiling();
    let mut out: Vec<String> = Vec::with_capacity(indices.len() + 4);
    let mut prev: Option<usize> = None;
    let mut elided_total: usize = 0;
    let mut represented_total: usize = 0;
    // It was measured that a regular sampling pays its banner at every
    // hole: on `pacman -Q`, 4,173 of the 8,296 bytes THIS compactor returns
    // (half) are 110 copies of the marker, 96 of which announce the same hole.
    // Saying it once gives back 3,569 bytes HERE — and a follow-up measured what it
    // costs THERE, on the path the agent really receives:
    //
    //   markers per hole : delivered 16 122 B, 2 213 tokens folded, pointer YES
    //   single banner    : delivered 20 397 B,   889 tokens folded, pointer NO
    //
    // The output GROWS. Removing 68 marker lines brings the document
    // under the NEXT stage's threshold, which then no longer cuts its middle:
    // one stage's fold cancels the next one's, and losing the recovery
    // pointer was only the symptom. It is the lesson "measure the
    // DELIVERY path, not `filter`" — learned, then relearned.
    //
    // Nothing is taken up again here until the measurement covers both
    // stages together; `residue_census` now says so at the top of its
    // report.
    // One rule for every gap, the two ENDS included. Only gaps between kept
    // lines used to be marked: a caller that kept neither the first nor the
    // last line lost a head or a tail with nothing said — measured
    // 2026-10-02 on `diff -y`, whose fold keeps changed rows wherever they
    // fall. The existing callers keep a head and a tail, so for them nothing
    // changes; the class is closed here rather than in each caller.
    let mut gap_lines = |range: std::ops::Range<usize>, out: &mut Vec<String>| {
        let gap = range.len();
        if gap == 0 {
            return;
        }
        // The bytes this hole REALLY hides, newlines included:
        // that is the only quantity to compare with the marker's price.
        let hidden_bytes: usize = lines[range.clone()]
            .iter()
            .map(|l| l.len() + 1)
            .fold(0usize, usize::saturating_add);
        if !a_gap_pays_for_its_marker(hidden_bytes, gap) {
            // The marker would cost more than the hole: keep the lines.
            // Shorter AND truer — nothing to recover, nothing to announce.
            for l in &lines[range] {
                out.push((*l).to_string());
            }
        } else {
            elided_total += gap;
            // Saturating, and capped below: both halves of this sum come
            // from text, and text can say anything.
            represented_total = represented_total.saturating_add(
                lines[range]
                    .iter()
                    .map(|l| run_weight(l))
                    .fold(0usize, usize::saturating_add),
            );
            // This marker is one WE write, with a count we computed, and
            // a later stage (`disclose::build_summary`) reads it back out
            // of the text. Record it, or that second stage cannot tell it
            // from a marker-shaped line of content and drops the claim.
            note_folded(gap);
            out.push(marker(gap));
        }
    };
    for &idx in indices {
        let start = prev.map_or(0, |p| p + 1);
        gap_lines(start..idx, &mut out);
        out.push(lines[idx].to_string());
        prev = Some(idx);
    }
    if let Some(p) = prev {
        gap_lines(p + 1..lines.len(), &mut out);
    }
    // Never lose content silently. When any middle was dropped, close
    // with one actionable footer telling the agent the full bytes are
    // recoverable. `render_kept` itself has no content hash (it is shared by
    // shell/Read/grep/webfetch paths), so the footer names which banner to
    // take it from, with `tokenade raw` / re-Read as the fallback when no
    // hash is in scope.
    if elided_total > 0 {
        // Text lives in `FOLD_FOOTER_HASH_PROMISE` because a SECOND place
        // rewrites it: `wrap::strip_unkept_hash_promise` replaces this exact
        // sentence when the stash could not be written, so a reader is never
        // sent to a hash nobody stored. That rewrite is an exact string match.
        // Editing the sentence here and nowhere else silently disables the
        // net — and its test held its own hardcoded copy, so it would have
        // stayed green while the real footer drifted. One const, two users.
        //
        // Says WHICH banner's hash, and includes `--raw`. Both were measured
        // wrong on 2026-08-04: a folded `git log` carries TWO hashes — the
        // fold store's (`full output: <hash>`, byte-identical to the source,
        // verified 359/359 lines) and the disclose stash's (`[stashed: …]`,
        // which returned 86 of those 359 lines with no marker saying so). This
        // footer said only "hash printed in this output's banner", and the
        // stash one is the more prominent of the two. It also omitted `--raw`,
        // so following it literally hits the size gate and returns a refusal
        // instead of bytes.
        // Say the ORIGINAL magnitude when the elided lines were themselves
        // collapsed runs, because that is the number an agent judges "how much
        // am I not seeing" by.
        // A claim is worth no more than the folding that backs it.
        let represented_total = represented_total.min(elided_total.saturating_add(ceiling_before));
        let stands_for = if represented_total > elided_total {
            format!(", standing for {represented_total} line(s) of the original output")
        } else {
            String::new()
        };
        out.push(format!(
            "[tokenade folded {elided_total} line(s) total{stands_for} — {}]",
            fold_footer_promise()
        ));
    }
    out.join("\n")
}

// ---------------------------------------------------------------------------
// The fold FOOTER's promise: one sentence, one place. It lived in
// `filter.rs` while `wrap` rewrites it literally and the web pipeline
// replaces half of it — two readers outside the commands layer for a
// string that belongs to the marker.
// ---------------------------------------------------------------------------

/// The recovery sentence in the fold footer, minus the trailing command.
///
/// Public because `wrap::strip_unkept_hash_promise` rewrites this exact text
/// (to [`FOLD_FOOTER_NO_HASH`]) when the stash could not be written. Both
/// sides MUST read the same const: the rewrite is a literal string match, and
/// a footer edited here alone would leave the safety net matching nothing.
pub const FOLD_FOOTER_HASH_PROMISE: &str =
    "recover the verbatim bytes with `tokenade expand-ref <hash> --raw`, taking <hash> from \
     the `full output:` banner below; a `[stashed: …]` pointer is a preview, not the source. \
     No such banner? re-run the source command via";

/// Replacement for [`FOLD_FOOTER_HASH_PROMISE`] when no hash was stored.
pub const FOLD_FOOTER_NO_HASH: &str = "recover the verbatim bytes by re-running the source \
                                       command via";

/// The WHOLE promise, fallback command included.
///
/// Spelled once because a reader must be able to replace it in one block:
/// it talks about the world of COMMANDS ("re-run the source command via
/// `tokenade raw <cmd>`") and a page has no command. The web pipeline
/// rewrites it — see `web_pipeline` — and a rewrite that knew only
/// half of the sentence would leave the other half in place.
pub fn fold_footer_promise() -> String {
    format!("{FOLD_FOOTER_HASH_PROMISE} `tokenade raw <cmd>`")
}

#[cfg(test)]
mod fold_footer_promise_tests {
    /// A hole at the HEAD or TAIL is stated like a hole in the middle. Only the
    /// holes between kept lines used to be marked (2026-10-02, `diff -y`).
    #[test]
    fn render_kept_marks_the_gaps_at_both_ends() {
        let owned: Vec<String> = (0..40)
            .map(|i| format!("line number {i} with some words on it"))
            .collect();
        let lines: Vec<&str> = owned.iter().map(String::as_str).collect();
        let out = super::render_kept(&lines, &[18, 19, 20]);
        let rows: Vec<&str> = out.lines().collect();
        assert!(rows[0].contains("18 lines elided by tokenade"), "{out}");
        assert!(
            rows.iter()
                .any(|l| l.contains("19 lines elided by tokenade")),
            "{out}"
        );
        assert!(rows.contains(&"line number 18 with some words on it"));
        // Kept at both ends: nothing more than before.
        let same = super::render_kept(&lines, &[0, 1, 38, 39]);
        assert!(
            same.lines().next() == Some("line number 0 with some words on it"),
            "{same}"
        );
    }

    use crate::compress::truncate_middle;

    /// The sentence ends with "re-run the source command via": without the
    /// command that follows, the fold names ZERO remedies and the sentence does not end.
    ///
    /// Nothing held that half. Measured: the mutation that cut
    /// the command from `fold_footer_promise` survived the whole suite,
    /// including the web path test — which compares the output with this same
    /// function and therefore shortens on both sides.
    #[test]
    fn the_emitted_footer_names_both_recoveries() {
        let plain: String = (0..400).map(|i| format!("plain line {i}\n")).collect();
        let out = truncate_middle(&plain, 20, 10);
        let footer = out
            .lines()
            .find(|l| l.starts_with("[tokenade folded "))
            .expect("a fold this size must carry a footer");
        for named in ["tokenade expand-ref <hash> --raw", "`tokenade raw <cmd>`"] {
            assert!(
                footer.contains(named),
                "the fold no longer names \"{named}\": {footer}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writing and reading agree, by construction and not by
    /// convention: that is the whole point of this module.
    #[test]
    fn what_we_write_is_what_we_read_back() {
        assert_eq!(weight_of(&marker(20)), Some(20));
        // The uniform banner: what it announces is what it weighs. Without this
        // read, a more heavily folded view loses its stage-2 summary — that is
        // the measured mechanism, and the reason why writing and
        // reading live in the same module.
        assert_eq!(weight_of(&uniform_banner(96, 20, 1_920)), Some(1_920));
        assert_eq!(weight_of(&uniform_banner(2, 3, 6)), Some(6));
        // The skeleton shape, written elsewhere and so far unreadable here.
        assert_eq!(weight_of(&body_marker("//", 5)), Some(5));
        assert_eq!(weight_of(&body_marker("#", 150)), Some(150));
        // …and nothing else. A CONTENT line that starts like the
        // marker weighs nothing: without this requirement, `// … 5 lines … of
        // hand-written prose` would count for five lines of the original.
        assert_eq!(weight_of("    // … 5 lines … of prose"), None);
        assert_eq!(weight_of("    // … five lines …"), None);
        assert_eq!(weight_of(&marker(0)), Some(0));
        // And the regular shape returns its TOTAL, not the size of one hole.
    }

    /// What is not a marker is not one.
    #[test]
    fn a_line_that_is_not_ours_weighs_nothing_here() {
        assert_eq!(weight_of("ordinary content"), None);
        assert_eq!(
            weight_of("… [not a number lines elided by tokenade] …"),
            None
        );
        assert_eq!(weight_of("[tokenade:diff] 1610 line(s)"), None);
    }

    /// **A single author per marker.**
    ///
    /// The skeleton shape was spelled three times in `compress.rs` and
    /// zero times here, so [`weight_of`] — written precisely to read these
    /// lines back — did not know it. A SOURCE guard, because it is
    /// the SPELLING that must stay unique: a fourth site would write it without
    /// any behaviour test noticing.
    #[test]
    fn no_module_spells_a_marker_of_its_own() {
        // The walk is written ONCE: three guards rewrote it, and
        // a mutation removing the descent survived in two of
        // them. `rust_sources` refuses a result that saw no
        // subdirectory, so the clause below no longer has to think about it.
        let files = crate::source_guard::all_rust_sources();
        let mut checked = 0usize;
        for (p, code) in &files {
            if p.file_name().is_some_and(|f| f == "elision.rs") {
                continue;
            }
            checked += 1;
            for (n, line) in code.lines().enumerate() {
                assert!(
                    !(line.contains("format!") && line.contains("lines …")),
                    "{}:{} writes its own elision marker — `elision` is \
                     the only author, otherwise `weight_of` cannot read it back",
                    p.display(),
                    n + 1
                );
            }
        }
        assert!(checked > 100, "only {checked} file(s) swept");
    }
}

/// **Fold the markers of a REGULAR hole, on an already rendered text.**
///
/// `None` when there is nothing to take. Otherwise, the text where the dominant hole
/// is announced ONCE at the top and not repeated, the others keeping their
/// marker — the reader knows "one line in K, except where marked".
///
/// **Why on the finished text, and not where the markers are written.** This
/// fold was built earlier inside `render_kept`, measured, and it is NEGATIVE:
/// over the 265 captures, what the agent receives goes from 240,776 to 247,330
/// bytes. The cause lies in a single capture — `ls -la /usr/bin` goes from
/// 3,585 to 13,729 — because removing 113 markers brings the view under the
/// NEXT stage's threshold, which then stops summarising it. One stage's fold
/// defuses the next, and the first cannot know it.
///
/// So the decision belongs to the stage that sees it: the caller compares the
/// folded text with the `disclose` threshold and REFUSES the fold that would cross the
/// line downwards. Here we only return the folded shape.
///
/// ## `ceiling`: what THIS PROCESS actually folded
///
/// **A marker nobody folded is content.** This fold PARSES
/// lines that look like our markers, REMOVES them, and announces their sum
/// — three things one does not do to text one did not write. MEASURED
/// 2026-09-08 on a 1,139-byte log quoting fourteen times
/// `… [20 lines elided by tokenade] …` (a saved transcript, a CI log
/// of a tokenade session, this repo's documentation): the fourteen lines
/// disappeared and the output asserted **"280 lines elided by tokenade"**
/// where this process had folded NONE — and [`weight_of`] reads that 280 back,
/// so the next stage's summary was inflated by as much.
///
/// It is the incident `fold_count::folded_ceiling` exists to prevent, in a
/// place that did not consult it. The rule is its own: *no
/// claim may exceed what our own passes removed*, and a
/// process that folded nothing can claim nothing at all.
pub fn fold_uniform_markers(text: &str, ceiling: usize) -> Option<String> {
    let mut counts: std::collections::BTreeMap<usize, usize> = Default::default();
    for line in text.lines() {
        if let Some(gap) = gap_of_marker(line) {
            *counts.entry(gap).or_default() += 1;
        }
    }
    let total: usize = counts.values().sum();
    let (&each, &runs) = counts.iter().max_by_key(|(_, n)| **n)?;
    // The hole must DOMINATE: below two thirds, the legend would describe a
    // minority, and an earlier attempt wrote an "all holes equal" rule that never
    // fired because it had been read off an example line whose
    // digits the instrument erases.
    if runs * 3 < total * 2 {
        return None;
    }
    // **A SAMPLE, not a holed text.** "One line in K" reads when
    // the holes are LONGER than what is kept between them:
    // `pacman -Q`, twenty lines removed for one kept, is a sampled
    // list. Blocks of four lines separated by a ONE-line hole
    // read like a whole text, and the marker is the only sign
    // of what is missing at THAT place: measured 2026-10-02 on a CLI's
    // conversation-review listing, one line in two removed in the middle — conversation
    // headers and replies — and 43 one-line holes merged into a header
    // banner: one could no longer tell which line of a conversation
    // was missing. At a tie (one in two), the markers stay in place.
    if each <= median_kept_span(text) {
        return None;
    }
    // What we are about to remove and claim. Above the ceiling,
    // these lines do not come from us: they are content.
    let claimed = runs.checked_mul(each)?;
    if claimed > ceiling {
        return None;
    }
    let banner = uniform_banner(runs, each, claimed);
    // And the fold must pay for its own label, like the template fold: two
    // markers replaced by a longer banner make the output GROW.
    if !crate::fold_gate::pays(marker(each).len() * runs, 0, banner.len()) {
        return None;
    }
    let mut out = String::with_capacity(text.len());
    out.push_str(&banner);
    out.push('\n');
    for line in text.lines() {
        if gap_of_marker(line) == Some(each) {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if !text.ends_with('\n') {
        out.pop();
    }
    Some(out)
}

/// [`fold_uniform_markers`], **unless this fold is what would make a
/// threshold be crossed downwards.**
///
/// Both halves of the decision in the same place, and both PURE: the
/// truthfulness ceiling and the thresholds come from outside. A decision reading
/// `fold_count::folded_ceiling()` or `disclose::threshold_tokens()` itself would
/// not be testable from a unit test — this was paid for once on the
/// first of these two counters.
///
/// The threshold rule is not "fold if it pays" but "do not be
/// what defuses the next stage": stashing and the replay
/// memory are decided on the SIZE of this view, further on.
pub fn fold_uniform_unless_it_crosses(
    text: &str,
    ceiling: usize,
    thresholds: &[usize],
) -> Option<String> {
    let folded = fold_uniform_markers(text, ceiling)?;
    crate::fold_gate::crosses_no_threshold_downwards(text, &folded, thresholds).then_some(folded)
}

/// The median length of the runs of kept lines BETWEEN two markers (the
/// edges, head and tail kept on purpose, are not runs).
fn median_kept_span(text: &str) -> usize {
    let mut spans = Vec::new();
    let mut run: Option<usize> = None;
    for line in text.lines() {
        if gap_of_marker(line).is_some() {
            if let Some(n) = run {
                spans.push(n);
            }
            run = Some(0);
        } else if let Some(n) = run.as_mut() {
            *n += 1;
        }
    }
    spans.sort_unstable();
    spans.get(spans.len() / 2).copied().unwrap_or(0)
}

/// The size of the hole if this line is EXACTLY a [`marker`] marker.
///
/// Deliberately stricter than [`weight_of`], which accepts several shapes:
/// we only remove what we wrote ourselves, down to the spelling.
pub(crate) fn gap_of_marker(line: &str) -> Option<usize> {
    let t = line.trim();
    let rest = t.strip_prefix("… [")?;
    let (n, tail) = rest.split_once(" lines elided by tokenade")?;
    tail.eq(" ] …")
        .then_some(())
        .or_else(|| tail.eq("] …").then_some(()))?;
    n.parse::<usize>().ok()
}

#[cfg(test)]
mod uniform_tests {
    use super::*;

    fn doc(gaps: &[usize]) -> String {
        let mut s = String::new();
        for (i, g) in gaps.iter().enumerate() {
            s.push_str(&format!("kept line {i}\n"));
            s.push_str(&marker(*g));
            s.push('\n');
        }
        s.push_str("last line\n");
        s
    }

    /// A conversation-review listing: blocks of four lines, a ONE-line hole
    /// between them — a holed text, not a sample. Every
    /// marker stays in place; the sample (holes ≥ blocks) folds.
    #[test]
    fn a_holed_text_keeps_its_markers_in_place() {
        let mut s = String::from("head\n");
        for i in 0..20 {
            for j in 0..4 {
                s.push_str(&format!("conversation {i} line {j}\n"));
            }
            s.push_str(&marker(1));
            s.push('\n');
        }
        assert!(
            fold_uniform_markers(&s, 1_000).is_none(),
            "holes of one line between blocks of four"
        );
        // One line in two (hole 1, chunk 1): the real measured shape.
        let alt: String = (0..40)
            .map(|i| format!("line {i}\n{}\n", marker(1)))
            .collect();
        assert!(
            fold_uniform_markers(&format!("head\n{alt}"), 1_000).is_none(),
            "one line in two"
        );
        assert!(
            fold_uniform_markers(&doc(&[20; 12]), 240).is_some(),
            "a sample still folds"
        );
    }

    /// The dominant hole is said once, the others keep their marker, and
    /// the announced total is that of the REPLACED holes — never the overall
    /// total, otherwise the outliers would be counted twice.
    #[test]
    fn the_dominant_gap_is_said_once_and_the_others_survive() {
        let mut gaps = vec![20usize; 12];
        gaps.push(37);
        let folded =
            fold_uniform_markers(&doc(&gaps), gaps.iter().sum()).expect("one hole dominates");
        assert_eq!(
            folded.matches(&marker(20)).count(),
            0,
            "the dominant hole is repeated"
        );
        assert_eq!(
            folded.matches(&marker(37)).count(),
            1,
            "the outlier lost its marker"
        );
        assert!(folded.starts_with(&uniform_banner(12, 20, 240)), "{folded}");
        // And what the banner announces is readable by the next stage: without
        // it, the view loses its summary AND its recovery pointer.
        assert_eq!(weight_of(folded.lines().next().unwrap()), Some(240));
        // The total count is preserved: 12×20 under the banner, 37 in clear.
        let seen: usize = folded.lines().filter_map(weight_of).sum();
        assert_eq!(seen, 240 + 37);
    }

    /// Without dominance, nothing: a legend describing a minority lies.
    #[test]
    fn a_gap_that_does_not_dominate_is_left_alone() {
        let gaps = vec![20, 20, 20, 7, 8, 9, 10, 11, 12];
        assert!(fold_uniform_markers(&doc(&gaps), gaps.iter().sum()).is_none());
    }

    /// And a fold that does not pay for its banner is refused: ONE 37-byte
    /// marker replaced by a 54-byte banner makes the output grow. With two it
    /// already pays (74 versus 54), and the gate says so — the measurement
    /// decides, not my intuition, which put the tipping point at three.
    #[test]
    fn a_fold_that_does_not_pay_for_its_banner_is_refused() {
        assert!(
            fold_uniform_markers(&doc(&[20]), 20).is_none(),
            "a single marker"
        );
        assert!(
            fold_uniform_markers(&doc(&[20, 20]), 40).is_some(),
            "two already pay"
        );
        assert!(fold_uniform_markers(&doc(&[20; 12]), 240).is_some());
    }

    /// A text without markers is not touched.
    #[test]
    fn a_text_without_markers_is_left_alone() {
        assert!(fold_uniform_markers("une ligne\nune autre\n", 1_000).is_none());
        // A CONTENT line that looks like a marker without being one is
        // not removed: we only remove what we wrote.
        assert!(fold_uniform_markers("… [20 lines elided by someone else] …\n", 1_000).is_none());
    }
}

#[cfg(test)]
mod forged_marker_tests {
    use super::*;

    /// **A marker nobody folded is CONTENT.**
    ///
    /// A log quoting our marker fourteen times — a saved
    /// transcript, a CI log of a tokenade session, this repo's
    /// documentation — saw its fourteen lines disappear and the output assert
    /// "280 lines elided by tokenade" on a process that had folded
    /// none. MEASURED 2026-09-08 on 1,139 bytes.
    #[test]
    fn markers_this_process_never_wrote_are_content() {
        let forged: String = (0..14)
            .map(|i| {
                format!(
                    "2026-09-08 agent a lu le fichier numero {i}\n{}\n",
                    marker(20)
                )
            })
            .collect();
        // A process that folded NOTHING can claim nothing.
        assert!(
            fold_uniform_markers(&forged, 0).is_none(),
            "markers we did not write were removed and claimed"
        );
        // And a PARTIAL claim does not pass either: if our passes
        // only removed 100 lines, we cannot announce 280.
        assert!(fold_uniform_markers(&forged, 100).is_none());
        // Up to what we actually folded, the fold is legitimate.
        assert!(fold_uniform_markers(&forged, 280).is_some());
    }

    /// **The seam: whoever REMOVES a marker must be handed the ceiling.**
    ///
    /// The mistake is not having forgotten a guard, it is that no guard
    /// recalled that one was needed. `disclose::original_weight` has been capping
    /// for a while; `fold_uniform_markers`, written a week later
    /// in ANOTHER module, did not know it. Every public function here
    /// that recognises a marker to remove it takes `ceiling` — a future
    /// sibling that forgets it fails here, wherever it is called.
    #[test]
    fn whoever_removes_a_marker_is_handed_the_ceiling() {
        // "Which half of this file ships?" has ONE answer in this
        // repo: `srcsweep`, which tracks braces. The first version of
        // this guard cut at the first `#[cfg(test)]` — line 115 for a
        // function at line 218 — and answered "0 found" on a file that
        // carried one. The same truncation was measured in the shared
        // seam: 1,521,151 bytes of production invisible over 127
        // files.
        let files = crate::source_guard::all_rust_sources();
        let (_, src) = files
            .iter()
            .find(|(p, _)| p.file_name().and_then(|f| f.to_str()) == Some("elision.rs"))
            .expect("the module is swept");
        let mut removers = 0usize;
        for (i, _) in src.match_indices("\npub fn ") {
            let head = &src[i + 1..];
            let sig_end = head.find(" {").unwrap_or(head.len());
            let name = head[7..].split('(').next().unwrap_or("");
            let body = &head[..head.find("\n}\n").unwrap_or(head.len())];
            if !body.contains("gap_of_marker(") {
                continue;
            }
            removers += 1;
            assert!(
                head[..sig_end].contains("ceiling: usize"),
                "`{name}` removes markers without being handed what this process \
                 actually folded: a marker nobody folded is \
                 CONTENT, and removing it while claiming its count is the incident \
                 `fold_count::folded_ceiling` exists to prevent"
            );
        }
        assert_eq!(removers, 1, "the guard no longer finds what it checks");
    }
}

#[cfg(test)]
mod uniform_threshold_tests {
    use super::*;

    /// A sampled view: `keep` kept lines, a `gap` hole between
    /// each. The exact shape the hard cap produces.
    fn sampled(keep: usize, gap: usize) -> String {
        (0..keep)
            .map(|i| {
                format!(
                    "| {i} | une ligne de tableau qui porte du texte |\n{}\n",
                    marker(gap)
                )
            })
            .collect()
    }

    fn est(s: &str) -> usize {
        crate::tokenizer::estimate(s)
    }

    #[test]
    fn a_fold_that_stays_above_every_threshold_is_kept() {
        let view = sampled(40, 26);
        let folded = fold_uniform_markers(&view, 40 * 26).expect("the regular fold fires");
        // Threshold DERIVED from what the estimator answers, never guessed.
        let t = est(&folded) - 1;
        assert!(
            fold_uniform_unless_it_crosses(&view, 40 * 26, &[t]).is_some(),
            "a fold that stays above the threshold was refused"
        );
    }

    /// The other side of the bound, the one no test held: the fold brings
    /// the view UNDER a threshold, so the next stage stops arming and the
    /// delivered output grows. Without this case, the guard can be removed entirely
    /// without a single line turning red.
    #[test]
    fn a_fold_that_would_cross_a_threshold_downwards_is_refused() {
        let view = sampled(40, 26);
        let folded = fold_uniform_markers(&view, 40 * 26).expect("the regular fold fires");
        let t = est(&folded) + 1;
        assert!(
            est(&view) >= t && est(&folded) < t,
            "the fixture does not bracket the threshold: {} / {} / {t}",
            est(&view),
            est(&folded)
        );
        assert!(
            fold_uniform_unless_it_crosses(&view, 40 * 26, &[t]).is_none(),
            "the fold defused the next stage"
        );
    }

    /// And the truthfulness ceiling stays held by the same call: a marker
    /// we did not write is content.
    #[test]
    fn nothing_is_claimed_above_the_ceiling() {
        let view = sampled(40, 26);
        assert!(fold_uniform_unless_it_crosses(&view, 0, &[]).is_none());
    }
}
