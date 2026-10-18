//! **The instrument that asks whether our folds send the agent back for more.**
//!
//! The canary lives in `transcripts::canary`: it is here because an instrument
//! that no suite validates degrades silently, and this one has ALREADY lied
//! once — an overly lax attribution gave 58.8 % returns to a fold that
//! prints no hash.

use tokenade_bench::transcripts::{
    canary, canary_bypasses, canary_complaints, canary_exec_errors, canary_greps, canary_outcomes,
    canary_reruns, canary_sequences, canary_stash_sizes, grep_outcomes, mine, transcript_root,
};

#[test]
fn the_canary_still_reproduces_the_defect_it_names() {
    assert_eq!(
        canary(false),
        0,
        "the `transcripts` canary no longer reproduces \"one fold asked for, one silent \
         fold, one orphan\": the measurement under this instrument no longer proves anything"
    );
}

/// The rerun control has its own canary: two versions of this measurement
/// returned different numerators (3,843 versus 1,189 folded commands), so
/// it does not ship without an answer known in advance.
#[test]
fn the_rerun_control_is_exact_on_a_known_tree() {
    assert_eq!(
        canary_reruns(false),
        0,
        "the control canary no longer returns \"folded 1/2, control 1/4\""
    );
}

/// **Reach floor**: a measurement over zero transcripts would be green
/// and say nothing. The test abstains when the tree does not exist — a
/// build machine does not have this machine's sessions — but as soon as it
/// exists, it must carry something to measure.
#[test]
fn the_transcript_tree_is_actually_walked() {
    let root = transcript_root();
    if !root.exists() {
        return;
    }
    let m = mine(&root);
    assert!(
        m.files >= 1,
        "{} exists but yields no transcript",
        root.display()
    );
    // The named families are the ones the PRODUCT writes. If none of them
    // comes out any more while files are read, the banner has changed
    // shape and the instrument is measuring nothing.
    if m.files >= 50 {
        assert!(
            m.folds() > 0,
            "{} transcript(s) read and not a single fold recognised: the banner \
             changed shape without the instrument following",
            m.files
        );
    }
}

/// **A rate cannot exceed 100 %.**
///
/// The first table showed rates of 200 % and 300 %: it counted CALLS in the
/// numerator while the denominator counted folds, and `expand-ref` gets
/// rerun on the same hash with `--grep` then `--prompt`. Two different
/// quantities divided by one another.
#[test]
fn a_fold_asked_for_several_times_stays_a_single_fold() {
    let root = transcript_root();
    if !root.exists() {
        return;
    }
    let m = mine(&root);
    for (fam, t) in &m.by_family {
        assert!(
            t.recovered <= t.folds,
            "{fam}: {} fold(s) asked for out of {} announced — the numerator and \
             the denominator do not count the same thing",
            t.recovered,
            t.folds
        );
        assert!(
            t.calls >= t.recovered,
            "{fam}: {} call(s) for {} fold(s) asked for",
            t.calls,
            t.recovered
        );
    }
}

/// The `--grep` join has its own canary too: it crosses TWO sources (the
/// recovery command and the folded view that carried the hash), and a
/// wrong join answers without saying anything.
#[test]
fn the_grep_join_canary_is_exact() {
    assert_eq!(
        canary_greps(false),
        0,
        "the join canary no longer returns \"1 kept, 1 removed, 1 true negative\""
    );
}

/// **Join reach floor**: "zero false negatives" over
/// zero decidable cases is an empty sentence. As soon as the tree carries folds, the
/// join must decide at least one — otherwise the command's pattern has
/// changed shape and the measurement no longer speaks about anything.
#[test]
fn the_grep_join_actually_decides_cases() {
    let root = transcript_root();
    if !root.exists() {
        return;
    }
    // The join FIRST, the gate only when it decided nothing. Both walk
    // every transcript, and this file's tree has grown to 3.2 GB / 1,771
    // files (2026-10-02): two passes went past nextest's two-minute
    // terminate-after, a red suite for a test that has nothing to report.
    // The gate only matters when there is something to excuse.
    let g = grep_outcomes(&root);
    let decidables = g.removed + g.kept + g.zero_true + g.zero_false;
    if decidables >= 1 {
        return;
    }
    let m = mine(&root);
    if m.files < 50 || m.folds() == 0 {
        return;
    }
    assert!(
        decidables >= 1,
        "{} fold(s) announced and not a single decidable `--grep` recovery: \
         the command or the banner changed shape",
        m.folds()
    );
}

/// The outcomes canary distinguishes the four reasons why an
/// `expand-ref` can come back empty-handed. Three of them do NOT blame
/// the product (a fixture from our sources, a store redirected by a
/// lab session, an undecidable date gap): mixing them up with
/// the fourth would make the measurement say that a fold promised bytes it
/// had not written.
#[test]
fn the_outcomes_canary_separates_the_four_reasons() {
    assert_eq!(
        canary_outcomes(false),
        0,
        "the outcomes canary no longer separates \"inside the TTL\", \"after\", \
         \"fixture\" and \"redirected store\""
    );
}

/// The only direct witness on this axis — what the agent WRITES after a fold — has its
/// canary: a complaint after a fold, a silence after a fold, a complaint
/// after an intact result (the control), and a system reminder that reacts to
/// nothing. Without the last one, the measurement counted 24,765 "opportunities" that
/// were not.
#[test]
fn the_complaints_canary_is_exact() {
    assert_eq!(
        canary_complaints(false),
        0,
        "the complaints canary no longer returns \"folded 1/2, control 1/1\""
    );
}

/// The other half of the interaction cost — does the command that FOLLOWS a fold
/// fail? — has its canary, and it also lays down the STILL FRESH file
/// that must stay out of the corpus: a transcript being written
/// carries the output of the previous measurement.
#[test]
fn the_execution_canary_is_exact() {
    assert_eq!(
        canary_exec_errors(false),
        0,
        "the execution canary no longer returns \"after a fold 1/3, control 1/1\""
    );
}

/// The sequence of calls on the same hash distinguishes a CONVERSATION (two
/// `--prompt` questions in a row, which is intended) from a word-for-word
/// repetition (the first answer was of no use). Its canary lays down both, plus the
/// repeat after a size refusal.
#[test]
fn the_sequences_canary_is_exact() {
    assert_eq!(
        canary_sequences(false),
        0,
        "the sequences canary no longer returns \"3 once, 3 twice, 1 three times\""
    );
}

/// Bypassing is the only gesture in the corpus that EXPRESSES a refusal of the fold.
/// Its canary separates the three cases — after a fold of the same command, after
/// an intact output, and pre-emptive — because only the first would blame the
/// fold.
#[test]
fn the_bypasses_canary_is_exact() {
    assert_eq!(
        canary_bypasses(false),
        0,
        "the bypasses canary no longer returns \"2 after a fold, 1 after a \
         bare output, 1 never seen\""
    );
}

/// The ask-back rate by stored SIZE decides a setting: is stashing a
/// small output behind a pointer worth the risk of a round trip? Its
/// canary checks that the size is read from the whole banner (it is written
/// AFTER the path, hence after the first occurrence of the hash) and that a
/// BARE ask-back — everything requested again — is told apart from a question.
#[test]
fn the_sizes_canary_is_exact() {
    assert_eq!(
        canary_stash_sizes(false),
        0,
        "the sizes canary no longer returns \"< 1k → 1/2 (1 bare), 20k–100k → 1/1 (0 bare)\""
    );
}
