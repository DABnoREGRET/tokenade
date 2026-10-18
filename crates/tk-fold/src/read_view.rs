//! **Which READ view does this file get: the decision, in a single
//! place.**
//!
//! `hooks::read_diffs::try_code_strip` TAKES it and `read_view_headroom`
//! REPLAYS it to measure what it costs. Both used to spell it out by hand: the
//! bench carried its own `ladder()` "reproduced identically" plus a separate
//! `delivered_under_the_rule()`, so the day the product moves a bar the
//! bench keeps pricing the old one — and the bench is what picks the target
//! of the next iteration.
//!
//! ## The ladder
//!
//! comments (≥ [`COMMENTS_MIN_PCT`] %) → lite (≥ [`LITE_MIN_PCT`] %) →
//! skeleton (≥ [`SKELETON_MIN_PCT`] %, and size ≥ [`SKELETON_MIN_SIZE`]).
//!
//! With, AHEAD of everything else, one rule: the skeleton goes first when
//! it is worth less than half of the better of the two ordinary modes.
//! Without it, a mode that has just cleared its bar wins and the skeleton —
//! which exists precisely for big files — is never reached.

/// The size above which a SKELETON view is offered: below it, the detail
/// of the bodies is what the reader came for.
pub const SKELETON_MIN_SIZE: u64 = 6_000;

/// Bar for comments mode.
pub const COMMENTS_MIN_PCT: usize = 10;

/// Bar for lite mode. Lower: folding blank lines only shifts the spacing
/// between blocks.
pub const LITE_MIN_PCT: usize = 5;

/// Bar for the skeleton as a LAST resort, when no ordinary mode has bitten.
pub const SKELETON_MIN_PCT: usize = 10;

/// The view the ladder keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Comments,
    Lite,
    Skeleton,
}

impl View {
    pub fn as_str(self) -> &'static str {
        match self {
            View::Comments => "comments",
            View::Lite => "lite",
            View::Skeleton => "skeleton",
        }
    }
}

/// A mode's gain, in percent, saturated and without division by zero.
pub fn gain_pct(before: usize, after: usize) -> usize {
    before
        .saturating_sub(after)
        .checked_mul(100)
        .and_then(|n| n.checked_div(before))
        .unwrap_or(0)
}

/// **A mode that has cleared its bar must not hide a fold twice as
/// good.**
///
/// MEASURED 2026-09-08 on **50,043 real code files over 6 KB**
/// (270,123,474 tokens): the ladder delivered 246,364,851 of them (−8 %), and
/// **14,526 files returned MORE THAN TWICE what the skeleton would have
/// left**, i.e. 54,366,909 tokens too many.
///
/// The factor of two is not a comfort setting: the skeleton omits the
/// BODIES, which the other two modes do not, so it only takes the place
/// of a working fold when it wins by a wide margin.
pub fn a_skeleton_that_wins_by_far(skeleton_tokens: usize, chosen_tokens: usize) -> bool {
    skeleton_tokens.saturating_mul(2) < chosen_tokens
}

/// **Whom must the skeleton beat?** The BETTER of the two ordinary modes,
/// not the first one to clear its bar — otherwise a mediocre comments mode
/// is enough for the skeleton to jump ahead of a lite mode that was doing
/// better.
pub fn the_better_ordinary_mode(comments_tokens: usize, lite_tokens: usize) -> usize {
    comments_tokens.min(lite_tokens)
}

/// **The decision, in tokens only** — no body is built here.
///
/// `skeleton` is `None` when the file is under [`SKELETON_MIN_SIZE`] or
/// when a skeleton would make no sense on this content. `None` as a result
/// means "no view bites": the reader gets the file as is.
pub fn choose(
    before: usize,
    comments: usize,
    lite: usize,
    skeleton: Option<usize>,
) -> Option<(View, usize)> {
    if let Some(sk) = skeleton {
        if a_skeleton_that_wins_by_far(sk, the_better_ordinary_mode(comments, lite)) {
            return Some((View::Skeleton, sk));
        }
    }
    if gain_pct(before, comments) >= COMMENTS_MIN_PCT {
        return Some((View::Comments, comments));
    }
    if gain_pct(before, lite) >= LITE_MIN_PCT {
        return Some((View::Lite, lite));
    }
    let sk = skeleton?;
    (gain_pct(before, sk) >= SKELETON_MIN_PCT).then_some((View::Skeleton, sk))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The case that motivated the skeleton-first rule: comments clear their bar
    /// on an 18 KB file, and the skeleton is worth a sixth of it.
    #[test]
    fn a_skeleton_worth_far_less_goes_first() {
        assert_eq!(
            choose(1_000, 880, 950, Some(140)),
            Some((View::Skeleton, 140))
        );
        // …but it compares against the BETTER of the two ordinary ones, not the first.
        // Here lite gives 260: the skeleton at 140 no longer wins by a wide margin.
        assert_eq!(
            choose(1_000, 880, 260, Some(140)),
            Some((View::Comments, 880))
        );
    }

    /// Without a skeleton, the ordinary ladder stays what it was.
    #[test]
    fn the_ordinary_ladder_is_unchanged_without_a_skeleton() {
        assert_eq!(choose(1_000, 880, 950, None), Some((View::Comments, 880)));
        // comments at 9 %: under the bar, lite at 6 % takes it.
        assert_eq!(choose(1_000, 910, 940, None), Some((View::Lite, 940)));
        // both under their bar: nothing bites.
        assert_eq!(choose(1_000, 960, 960, None), None);
    }

    /// The last-resort skeleton has ITS OWN bar: a skeleton that saves almost
    /// nothing is not a view, it is a loss of bodies for nothing.
    #[test]
    fn a_skeleton_of_last_resort_still_has_to_earn_it() {
        assert_eq!(
            choose(1_000, 960, 960, Some(870)),
            Some((View::Skeleton, 870))
        );
        assert_eq!(choose(1_000, 960, 960, Some(910)), None);
    }

    /// The bounds cannot be bypassed through an overflow.
    #[test]
    fn nothing_overflows_into_a_win() {
        assert!(!a_skeleton_that_wins_by_far(usize::MAX / 2 + 1, 10));
        assert_eq!(gain_pct(0, 0), 0);
    }

    /// **The gain is relative to the INPUT.** Dividing by the output always gives
    /// a bigger number, so the error only shows just under a
    /// bar: at 905 tokens returned out of 1,000, the true gain is 9 % and the false
    /// one exactly 10 %. A `before` → `after` mutation survived the
    /// first version of these tests, which used values where both
    /// denominators fell on the same side.
    #[test]
    fn the_gain_is_measured_against_the_input() {
        assert_eq!(gain_pct(1_000, 905), 9);
        assert_eq!(gain_pct(1_000, 900), 10);
        // And the bar applies to THAT number: nothing bites here.
        assert_eq!(choose(1_000, 905, 1_000, None), None);
    }
}
