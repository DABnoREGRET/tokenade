//! **One fold can DISARM the next.**
//!
//! Measured: a first-stage fold made the output GROW because it removed just
//! enough for the second stage to stop arming, and the recovery pointer
//! vanished with nothing to explain it. Every stage here therefore asks "if I
//! fold, is the hand-off still justified?" before folding.

use super::*;

/// Tokens a fold must have removed before its recovery pointer is worth a disk
/// write. Below this the agent can simply re-run; above it, the dropped bytes
/// are the reason `expand-ref` exists.
pub(crate) const RECOVERY_MIN_DROPPED_TOKENS: usize = 1_500;

/// The other half of the same question: how much of the output went away.
///
/// "The agent can simply re-run" is the whole justification for the absolute
/// floor, and it is FALSE for a snapshot of live machine state. Measured over
/// 25 real commands: `systemctl list-units` folds **90 %** of 5 244 bytes,
/// `df -h` 60 %, `lsblk` 57 %, `ip addr` 54 % — every one of them under the
/// absolute floor, every one of them with no pointer, and every one of them a
/// command whose re-run returns DIFFERENT bytes. Re-running is not recovery
/// when the thing measured has moved.
///
/// So a fold that removed more than HALF its input earns a pointer too, above
/// a small floor that keeps `free -m` (24 tokens dropped) and `uptime` from
/// buying a disk write for nothing.
pub(crate) const RECOVERY_MIN_DROPPED_SHARE_TOKENS: usize = 100;

/// Whether a fold's losses are worth keeping a recovery pointer for: a large
/// ABSOLUTE loss, or a large SHARE of the output. One declarer, so the two
/// halves cannot drift.
/// Fold the markers of a regular gap — **unless this fold is what would take
/// the view under the stash threshold.**
///
/// The two stages answer each other, and only this one sees both. The rule is
/// not "fold if it pays" but "don't be what disarms the next stage": a view
/// already under the threshold loses nothing by being folded, a view above it
/// must only stay folded if it is still above AFTERWARDS.
/// `ceiling` is what THIS PROCESS's passes actually removed. A marker we did
/// not write is content, and this fold would delete it while claiming its
/// count: so the decision is PURE and the ceiling comes from outside, rather
/// than being read from a global counter that a unit test can neither set
/// honestly nor observe.
pub(crate) fn fold_uniform_unless_it_disarms_the_stash(display: String, ceiling: usize) -> String {
    let folded = tokenade_core::elision::fold_uniform_markers(&display, ceiling);
    fold_unless_it_disarms_the_stash(display, folded, "uniform")
}

/// **Fold a repeated sentence — same conditions, and every value kept.**
///
/// `template_fold` had existed for months and had only ONE caller, `npm.rs`.
/// MEASURED 2026-09-08 with `residue_census --delivered`: **shapes seen more
/// than once weigh 172 335 of the 301 411 bytes of the corpus residue, 57 %**
/// — lines of `mount`, `top`, `tasklist`, `ps` that differ only by their
/// numbers. Post-pass #4 of `filter` answers the same shape but throws the
/// VALUES away, which earns it bars so high (4 000 characters, a quarter
/// removed) that it does not fire there.
///
/// Here and not in the first stage, for the reason above, re-measured: placed
/// in `shrink_only_post_passes`, this fold takes the delivered corpus from
/// 237 180 to **238 680 bytes** — it gains 1 752 on `tasklist`, 1 185 on
/// `mount` and 1 421 on `ps aux`, and loses **10 281 on `ls -la /usr/bin`
/// alone**, whose shortened view drops under the stash bar and stops being
/// summarised.
///
/// No "interaction handles" guard here, unlike post-pass #4: that one replaces
/// 500 `[ref=eN]` with a count and returns a tree on which `browser_click` has
/// nothing left to aim at. This fold re-reads EVERY value, so it cannot make a
/// handle disappear. A guard placed there could be reached by no fixture, and
/// an untestable guard is a guard you only think you have.
pub(crate) fn fold_templates_unless_it_disarms_the_stash(display: String) -> String {
    // `fold_if_it_pays` carries the profitability gate AND the legend that says
    // what `{}` means: rendering the template without it is the defect that
    // legend was written to close. A second copy of the gate, here, would have
    // forgotten the second half — which is what it did for an hour.
    let folded = tokenade_core::template_fold::fold_if_it_pays(&display);
    let changed = folded != display;
    fold_unless_it_disarms_the_stash(display, changed.then_some(folded), "template")
}

/// **A sentence repeated in the MIDDLE of a line, said once.**
///
/// The three neighbouring folds catch repetition by the LINE: the template
/// wants the whole shape, dedup wants identical lines, path grouping wants the
/// repetition at the head. A sentence that recurs in the middle of otherwise
/// different lines meets none of them. MEASURED 2026-09-09 on the delivery
/// path: `lspci` carries `Advanced Micro Devices, Inc. [AMD]` on 23 lines,
/// **770 of the 3 228 delivered bytes**.
///
/// Goes through the same guard as the template fold — a stage-1 fold that
/// takes the view under the `disclose` threshold deprives it of its summary,
/// and the first stage cannot know that.
pub(crate) fn fold_phrases_unless_it_disarms_the_stash(display: String) -> String {
    let folded = tokenade_core::phrase_fold::fold_if_it_pays(&display);
    fold_unless_it_disarms_the_stash(display, folded, "phrase")
}

/// **Column padding goes away on the COMMAND path too.**
///
/// `collapse_decorative_segments` — removing a column table's decoration
/// without reading a word — had only ONE caller: the `auto` lane, which decides
/// by CONTENT. An output routed by its command name never met it. MEASURED
/// 2026-09-09 with `residue_census --delivered`: `ps -eo` came back **whole,
/// 2 718 bytes of which 29 % spaces**, and it is not alone — six corpus
/// captures are returned at 100 %.
///
/// Same family as `template_fold` wired on `npm` and `[paths from …]` wired on
/// `grep`: a general mechanism, a single lane.
pub(crate) fn strip_padding_unless_it_disarms_the_stash(display: String) -> String {
    // A help table's padding IS its structure: collapsed, `  cmd00000     does…`
    // becomes ` cmd00000 does…`, and the stash summary can no longer index the
    // names (`help_index`). Found by the `a_long_help_text` shape of
    // `hostile_scale`: a 300-command help came back head-and-tail.
    if tokenade_core::disclose::looks_like_a_help_text(&display) {
        return display;
    }
    // The BETTER of the two readings, like the `auto` lane: per segment and over
    // the whole document. The per-segment reading alone let `netstat` output
    // through — none of its pieces reaches five column lines, the whole does.
    let candidate = tokenade_core::archetypes::collapse_padding_best(&display);
    fold_unless_it_disarms_the_stash(display, candidate, "padding")
}

/// **The common rule: a fold must never be what disarms the next stage.**
///
/// The two stages answer each other, and only this one sees both. The rule is
/// not "fold if it pays" but "don't be what takes the line downward": a view
/// already under the threshold loses nothing by being folded, a view above it
/// must only stay folded if it is still above AFTERWARDS. Written ONCE: the
/// elision markers paid for it, then the repeated sentences, and a second copy
/// would have drifted from the first.
pub(super) fn fold_unless_it_disarms_the_stash(
    display: String,
    folded: Option<String>,
    which: &'static str,
) -> String {
    let Some(folded) = folded else {
        return display;
    };
    let threshold = tokenade_core::disclose::threshold_tokens();
    let was_above = tokenade_core::tokenizer::estimate(&display) >= threshold;
    let stays_above = tokenade_core::tokenizer::estimate(&folded) >= threshold;
    if was_above && !stays_above {
        tokenade_core::debug_log::log(
            "wrap",
            "fold_refused",
            serde_json::json!({
                "which": which,
                "before_bytes": display.len(),
                "after_bytes": folded.len(),
                "why": "the fold would take the view under the stash threshold",
            }),
        );
        return display;
    }
    folded
}

/// What the SECOND delivery stage produces, plus what the caller needs to
/// credit what it just did.
pub(crate) struct SecondStage {
    pub display: String,
    /// The size BEFORE the stash, when it could bite. `None` means "too short
    /// for the question to arise", and that is what keeps the estimate off the
    /// hot path of small commands.
    pub pre_stash_tokens: Option<usize>,
}

/// **The second delivery stage, written once.**
///
/// `filter_command` is the first: it folds according to the command. What
/// follows is what the agent REALLY receives — the stash of what is still too
/// big, then the recovery pointer to what the fold threw away.
///
/// The two stages interact, and that interaction cost two iterations: one
/// folded 3 569 bytes in the first stage and the next measured that the
/// DELIVERED output grew from 16 122 to 20 397 bytes, because removing lines
/// takes the document under the second stage's threshold, which then stops
/// cutting. A bench that cannot call this block cannot see that effect, and
/// `residue_census --delivered` had been measuring a single stage without
/// anything saying so.
///
/// Extracted for that reason: a single copy, called by `wrap` (which RUNS the
/// command) and by `filter --deliver` (which receives already-captured
/// output). Two compositions of the same chain would drift, and that is
/// exactly what the delivered output does not forgive.
/// The stash threshold for what this command printed: a Read's when it read
/// ONE source-code file (see the comment at the call site), the incidental
/// one otherwise.
pub(crate) fn stash_threshold_for(read_target: Option<&std::path::Path>) -> usize {
    match read_target {
        Some(path) if tokenade_core::disclose::earns_the_read_threshold(path) => {
            tokenade_core::disclose::read_threshold_tokens()
        }
        _ => tokenade_core::disclose::threshold_tokens(),
    }
}

pub(crate) fn second_stage(
    display: String,
    raw: &str,
    shell_cmd: &str,
    dropped_tokens: usize,
    before_tokens: usize,
    // The compactor that folded, under the name the ledger records. See
    // `nudges::fold_banner_head`: without it, 99.9 % of folds come out under a
    // generic label and "which fold sends the agent looking" has no answer.
    producer: &str,
) -> SecondStage {
    // L64 — disclose: if the compact output is STILL > 5k tokens, stash it and
    // emit a summary + path. Shell hint → extension sniffed from the content.
    //
    // #1051 — when the wrapped command is a plain single-file read
    // (`cat`/`nl`/`head`/`tail`/… <file>), promote the hint to `File(path)` so
    // the disclose footer can redirect EXACT-match lookups to grep/Read the
    // original file directly instead of the head+tail stash preview. Caught by
    // the `log-needle` benchmark: a folded `cat <log>` previously sent the agent
    // into 8× `search_stash` (BM25) + grep-on-stash detours (~14 wasted turns)
    // before it grepped the source. Conservative — any pipe/redirect/multi-file
    // → Shell.
    // **The fold that can only be decided HERE.** Regular sampling pays its
    // marker at every gap: on `pacman -Q`, 96 of the 110 gaps are twenty lines,
    // and the 110 markers weigh half of what the first stage returns. Saying it
    // once is worth 3 590 DELIVERED bytes on that capture.
    //
    // Decided here and not at write time, because the first stage cannot know
    // what the second will do. MEASURED 2026-09-08 by placing it there: the 265
    // captures go from 240 776 to 247 330 delivered bytes — worse — and the
    // whole loss sits in `ls -la /usr/bin`, 3 585 → 13 729, because removing
    // 113 markers takes the view UNDER the stash threshold, which then stops
    // summarising it. One stage's fold disarms the next.
    //
    // So: fold only if the folded view stays above the threshold, or if it was
    // already below it before. The fold must never be what takes the line
    // downward.
    let display = fold_uniform_unless_it_disarms_the_stash(
        display,
        tokenade_core::fold_count::folded_ceiling(),
    );
    let display = strip_padding_unless_it_disarms_the_stash(display);
    // **Code read is never rewritten into templates.** Both folds gather lines
    // BY SHAPE: on `sed -n 276,400p replay.rs` (tests) the template fold took
    // `let repo = a_hand_built_repo({});` out of four test functions into one
    // `(×4)` line plus its values — code that no longer reads in order and
    // cannot be copied back. A source read is a deliberate read (same rule as
    // its threshold below); its bytes stay in their order.
    let read_target = tokenade_core::disclose::single_file_read_target(shell_cmd);
    let several_reads = tokenade_core::disclose::reads_only_source_files(shell_cmd);
    let source_read = several_reads
        || read_target
            .as_deref()
            .is_some_and(tokenade_core::disclose::earns_the_read_threshold);
    let display = if source_read {
        display
    } else {
        fold_templates_unless_it_disarms_the_stash(display)
    };
    let display = if source_read {
        display
    } else {
        fold_phrases_unless_it_disarms_the_stash(display)
    };
    // **Reading ONE code file means wanting it.** `sed -n '1,400p' x.ts` is a
    // deliberate read, like a Read, and the Read has its threshold
    // (`read_threshold_tokens`, 25 k) for that very reason. Here it fell under
    // the incidental-output threshold, lowered by session pressure: found in
    // transcripts, 173 lines (1 849 tokens) of an `access-boundary.service.ts`
    // delivered head+tail, then `expand-ref --raw`; reproduced under
    // `TOKENADE_PRESSURE=0.3` (167 lines ⇒ 31). Code only: a `cat` of a log
    // stays incidental output. Several code files read at once
    // (`for f in …; do sed -n …; done`) are just as deliberate: same threshold.
    let threshold = if several_reads {
        tokenade_core::disclose::read_threshold_tokens()
    } else {
        stash_threshold_for(read_target.as_deref())
    };
    let stash_hint = match read_target {
        Some(path) => tokenade_core::disclose::SourceHint::File(path),
        None => tokenade_core::disclose::SourceHint::Shell,
    };
    // Measured BEFORE the stash so the fallback fold can be credited by the
    // caller. The byte floor is exact, not a heuristic: a token is at least one
    // byte, so a string shorter than the threshold can never exceed it in
    // tokens and `maybe_stash` cannot fire. That keeps the extra estimate off
    // every small command's hot path — this channel's latency was a project of
    // its own.
    let stash_floor = threshold;
    let pre_stash_tokens = if display.len() >= stash_floor {
        Some(tokenade_core::tokenizer::estimate(&display))
    } else {
        None
    };
    let display = tokenade_core::profile::timed("wrap:maybe_stash", || {
        tokenade_core::disclose::maybe_stash_with_threshold(display, stash_hint.clone(), threshold)
    });
    // …and recovery for what the fold DROPPED. `maybe_stash` above fires on the
    // size of what we are about to SHOW; it says nothing about what we removed.
    // A `grep -r` over a tree folds 14 000 matches into a couple of hundred
    // lines — small on the way out, and until now with no pointer back to the
    // other 13 800. Re-running is not a recovery path: the command may be slow,
    // and its output may not be reproducible.
    let display = attach_drop_recovery(
        display,
        raw,
        dropped_tokens,
        before_tokens,
        &stash_hint,
        producer,
    );
    SecondStage {
        display,
        pre_stash_tokens,
    }
}

pub(crate) fn fold_loss_earns_recovery(dropped_tokens: usize, before_tokens: usize) -> bool {
    if dropped_tokens >= RECOVERY_MIN_DROPPED_TOKENS {
        return true;
    }
    // **A fold that took the output UNDER the stash threshold owes the pointer
    // the stash would have given.** Above the line the output would have been
    // stashed WITH its hash; the fold made it small enough to skip that, so it
    // is the only way back. Measured 2026-10-02 on a daily review command:
    // 5 115 tokens, `generic` dropped 43 scattered lines (conversation headers
    // and replies among them) to land at ~3 300, 1 108 tokens — under both
    // floors above — and the note under it sent the agent to a `full output:`
    // banner that was not there.
    let threshold = tokenade_core::disclose::threshold_tokens();
    if dropped_tokens >= RECOVERY_MIN_DROPPED_SHARE_TOKENS
        && before_tokens >= threshold
        && before_tokens - dropped_tokens.min(before_tokens) < threshold
    {
        return true;
    }
    dropped_tokens >= RECOVERY_MIN_DROPPED_SHARE_TOKENS && dropped_tokens * 2 > before_tokens
}

#[cfg(test)]
mod recovery_belt {

    /// **The gate must say what it decides.** A real gain was once withdrawn
    /// because the recovery pointer vanished with nothing explaining why; the
    /// trace answered in one shot:
    ///
    /// | | delivered | tokens folded | pointer |
    /// |---|---|---|---|
    /// | markers per gap | 16 122 B | 2 213 | yes |
    /// | single banner | 20 397 B | 889 | no |
    ///
    /// The output GREW — a first-stage fold disarmed the second.
    /// This test pins the gate's arithmetic on those two measured points, so
    /// that a future threshold tweak does not silently carry them away.
    #[test]
    fn the_measured_two_stage_pair_falls_on_either_side_of_the_gate() {
        // The case that KEEPS its pointer: 2 213 out of 4 252, the share misses
        // by only 174 tokens.
        assert!(
            fold_loss_earns_recovery(2_213, 4_252),
            "the measured fold must be covered"
        );
        assert!(
            !fold_loss_earns_recovery(889, 4_252),
            "889 out of 4 252 covers nothing"
        );
        // And it is the ABSOLUTE bound that decides between the two, not the
        // share: 889 fails both (889 < 1 500, and 1 778 ≤ 4 252) while 2 213
        // passes the first. Written after a wrong assertion — I had pinned the
        // share, and 2 126 already passed through the absolute bound.
        assert!(fold_loss_earns_recovery(RECOVERY_MIN_DROPPED_TOKENS, 4_252));
        assert!(!fold_loss_earns_recovery(
            RECOVERY_MIN_DROPPED_TOKENS - 1,
            4_252
        ));
        // Under the absolute bound, only a majority share saves: 889 out of
        // 1 700 yes, 889 out of 4 252 no — same fold, two outputs of different
        // sizes.
        assert!(fold_loss_earns_recovery(889, 1_700));
    }
    use super::*;

    /// A big absolute loss still earns a pointer, as it always did.
    #[test]
    fn a_large_absolute_loss_earns_a_pointer() {
        assert!(fold_loss_earns_recovery(
            RECOVERY_MIN_DROPPED_TOKENS,
            1_000_000
        ));
        assert!(!fold_loss_earns_recovery(
            RECOVERY_MIN_DROPPED_TOKENS - 1,
            1_000_000
        ));
    }

    /// The new half: `systemctl list-units` folds 88 % of a 5 KB output, well
    /// under the absolute floor, and re-running it returns different bytes.
    #[test]
    fn a_large_share_earns_a_pointer_under_the_absolute_floor() {
        // ~1 200 of ~1 300 tokens — the measured shape.
        assert!(fold_loss_earns_recovery(1_200, 1_300));
        // Exactly half is not more than half: the belt is strict.
        assert!(!fold_loss_earns_recovery(500, 1_000));
        assert!(fold_loss_earns_recovery(501, 1_000));
    }

    /// `free -m` drops 46 % of 211 bytes — about 24 tokens. Re-running it is
    /// free, and a disk write for that is the cost the absolute floor exists
    /// to avoid. The share floor is what keeps it out.
    #[test]
    fn a_trivial_loss_buys_no_disk_write_however_large_its_share() {
        assert!(!fold_loss_earns_recovery(24, 50));
        assert!(!fold_loss_earns_recovery(
            RECOVERY_MIN_DROPPED_SHARE_TOKENS - 1,
            100
        ));
        assert!(fold_loss_earns_recovery(
            RECOVERY_MIN_DROPPED_SHARE_TOKENS,
            150
        ));
    }

    /// Nothing dropped, nothing to recover.
    #[test]
    fn a_fold_that_dropped_nothing_earns_nothing() {
        assert!(!fold_loss_earns_recovery(0, 0));
        assert!(!fold_loss_earns_recovery(0, 100_000));
    }
}

#[cfg(test)]
mod a_help_table_keeps_its_columns {
    /// `hostile_scale::a_long_help_text`: the whitespace fold crushed
    /// `  cmd00000     does…` into ` cmd00000 does…`, and the index of the
    /// stashed help could no longer find its names.
    #[test]
    fn padding_is_not_collapsed_in_a_help_text() {
        let mut help = String::from("A tool.\n\nUsage: tool [OPTIONS] <COMMAND>\n\nCommands:\n");
        for i in 0..400 {
            help.push_str(&format!(
                "  cmd{i:05}     {}\n",
                "does a long and detailed thing ".repeat(1 + i % 4)
            ));
        }
        assert_eq!(
            super::strip_padding_unless_it_disarms_the_stash(help.clone()),
            help
        );
    }
}

#[cfg(test)]
mod a_fold_that_disarms_owes_its_pointer {
    use super::fold_loss_earns_recovery;

    /// A daily review command: 5 115 tokens, 1 108 removed by `generic` —
    /// under both floors — and the output fell under the threshold with no
    /// pointer, the stash that would have given one disarmed.
    #[test]
    fn crossing_the_threshold_downward_earns_a_pointer() {
        let t = tokenade_core::disclose::threshold_tokens();
        assert!(
            fold_loss_earns_recovery(1_108, t + 115),
            "crossed the threshold downward"
        );
        // Already under the threshold before the fold: nothing was disarmed.
        assert!(!fold_loss_earns_recovery(1_108, t - 1));
        // Still above after the fold: the stash will give its own.
        assert!(!fold_loss_earns_recovery(1_108, t + 1_200));
        // A speck that crosses the line does not buy a disk write.
        assert!(!fold_loss_earns_recovery(50, t + 10));
    }
}
