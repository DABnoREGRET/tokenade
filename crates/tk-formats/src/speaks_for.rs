//! **Does this summary STILL say what the input said?**
//!
//! A floor, and only once. Its documentation already said so: "a single
//! declarer because there are TWO doors". It was written in `filter`, which
//! is one of the two — the other being `auto::guarded`. The commands door
//! is one layer ABOVE the formats one, so the declarer made
//! every folded content climb up one notch. It now lives under both,
//! and both call it on the way down.
//!
//! It goes no lower than the formats: `structured_still_speaks_for`
//! queries `logs::template_of`, which is a format module.
//!
//! Why a module of its own and not an annex of `auto`: `auto.rs` is
//! the router, and the size ratchet said it bluntly — pouring these
//! 111 lines into it took it from 1,209 to 1,320 shipped lines. A fidelity
//! floor is not a route.

const SPEAKS_FOR_SAMPLE_LINES: usize = 500;
/// Share of the dominant line's vocabulary a faithful result has to carry.
const SPEAKS_FOR_MIN_SHARE: usize = 60;

/// How many words of the dominant line the share is measured over.
///
/// The line SET is already sampled (`SPEAKS_FOR_SAMPLE_LINES`); the words
/// inside the winning line were not, and each one is searched for in the
/// structured output — so the work is the input size times the output size.
/// MEASURED 2026-09-07 on HTML that is one long line (an entity-heavy page
/// arrives as exactly that): 83 ms, 324 ms, 1 296 ms, 5 160 ms as the input
/// doubles — 4x a doubling, quadratic — and `tokenade read` on a 10 MB page
/// spent **85 s** inside a check whose only consumer is one telemetry line.
///
/// 64 is far more than any real log line carries, so the verdict is unchanged
/// wherever the guard was ever meant to speak.
const SPEAKS_FOR_MAX_WORDS: usize = 64;

/// Does this tiny result still SAY what the input said?
///
/// The misclassification guard below rests on "a 99 % reduction means the
/// detector was wrong". That premise is false for input that repeats itself:
/// 20 000 lines of one shape legitimately fold to one line, and rejecting
/// that fold shipped 3 768 tokens where 30 were correct and strictly more
/// informative (the fold states the shape AND the count; the fallback is a
/// head slice that states neither).
///
/// Counting distinct LINES cannot separate the two — every line of a counter
/// log is textually unique. What separates them is whether the result still
/// carries the input's own words: a template fold echoes the shape it folded,
/// a misfired compactor emits a summary banner about something else.
pub fn structured_still_speaks_for(input: &str, structured: &str) -> bool {
    let mut counts: std::collections::HashMap<String, (usize, String)> =
        std::collections::HashMap::new();
    for line in input.lines().take(SPEAKS_FOR_SAMPLE_LINES) {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let e = counts
            .entry(crate::logs::template_of(t))
            .or_insert_with(|| (0, t.to_string()));
        e.0 += 1;
    }
    // Ties broken on the template text so the verdict can't depend on hash
    // order — same input, same answer, every process.
    let Some((_, (_, dominant))) = counts
        .iter()
        .max_by(|a, b| a.1 .0.cmp(&b.1 .0).then_with(|| b.0.cmp(a.0)))
    else {
        return false;
    };
    let words: Vec<&str> = dominant
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 3 && w.chars().any(|c| c.is_alphabetic()))
        .take(SPEAKS_FOR_MAX_WORDS)
        .collect();
    if words.is_empty() {
        return false;
    }
    let kept = words.iter().filter(|w| structured.contains(**w)).count();
    kept * 100 / words.len() >= SPEAKS_FOR_MIN_SHARE
}

/// A structured pass produced a summary so small it cannot be speaking for its
/// input any more: ≥100 tokens in, ≤30 out, ≥80 % gone, and the dominant
/// line's own words no longer appear in the result.
///
/// **One declarer because there are two doors.** This was inline in
/// `structured_or_generic`, which only the COMMAND path reaches; the content
/// path goes through `auto::compact` → `auto::guarded`, which has the empty
/// floor, the summary-of-nothing floor and the `after > before` floor,
/// and not this one. Measured 2026-08-27 on two shapes, each with a marker in
/// a field its own format defines:
///
/// | | command door | content door |
/// |---|---|---|
/// | `strace`, marker in an `openat` path | `+rejected`, paths kept | `40 syscalls (1 unique)`, **944 → 19** |
/// | Gradle build, 8 modules × 4 tasks | `+rejected`, module names kept | `Gradle BUILD SUCCESSFUL: 33 tasks (0 failed)`, **877 → 11** |
///
/// The command door already pays the cost of refusing both, so asking the same
/// question here removes a divergence rather than adding one.
pub fn reduction_is_pathological(
    input: &str,
    compacted: &str,
    before: usize,
    after: usize,
) -> bool {
    const MIN_INPUT: usize = 100;
    const MAX_OUTPUT: usize = 30;
    const SUSPICIOUS_PCT: usize = 80;
    before >= MIN_INPUT
        && after <= MAX_OUTPUT
        && (before - after) * 100 / before.max(1) >= SUSPICIOUS_PCT
        && !structured_still_speaks_for(input, compacted)
}

#[cfg(test)]
mod tests {
    // Came from `filter_tests.rs` with their subject: `structured_still_speaks_for`
    // and the floor it serves live here since the declarer moved down
    // to the formats router. A test file that stays behind its
    // function is exactly what made `compress_tests.rs` the test file
    // of `skeleton`.
    #[test]
    fn the_dominant_line_is_sampled_by_words_not_read_whole() {
        // The share is measured over at most SPEAKS_FOR_MAX_WORDS words, the
        // same way the line set is capped at SPEAKS_FOR_SAMPLE_LINES. Sixty-four
        // words all present, then well over a hundred that are absent: with the
        // cap the verdict is "still speaks for it", without it the share falls
        // to about a third and the verdict flips.
        let present: Vec<String> = (0..super::SPEAKS_FOR_MAX_WORDS)
            .map(|i| format!("alpha{i:03}"))
            .collect();
        let absent: Vec<String> = (0..136).map(|i| format!("zeta{i:03}")).collect();
        let structured = present.join(" ");
        let dominant = format!("{} {}", structured, absent.join(" "));
        assert!(
            super::structured_still_speaks_for(&dominant, &structured),
            "the first {} words are all present; the share must be measured over them",
            super::SPEAKS_FOR_MAX_WORDS
        );

        // The cap must not be so small that it decides on one word. A real
        // dominant line is short, and the share is meant to be measured over
        // it: six of these ten survive, which is exactly the minimum share, so
        // the verdict is yes — but only if more than the first word is looked
        // at, because the first word is one of the four that are gone.
        let ten: Vec<String> = (0..10).map(|i| format!("word{i:03}")).collect();
        let kept_six = ten[4..].join(" ");
        assert!(
            super::structured_still_speaks_for(&ten.join(" "), &kept_six),
            "six of ten words kept is the minimum share; a one-word sample cannot see it"
        );

        // And the cap does not invent agreement: when the sampled words are
        // the ones MISSING, the verdict is still no.
        let flipped = format!("{} {}", absent.join(" "), structured);
        assert!(
            !super::structured_still_speaks_for(&flipped, &structured),
            "words absent from the structured output must not count as kept"
        );
    }

    /// One long line must not cost the input size TIMES the output size.
    ///
    /// MEASURED 2026-09-07 before the word cap, on `&amp;` repeated until the
    /// line is 1.28 MB — HTML with many entities arrives as exactly one long
    /// line: this call took 1 296 ms in RELEASE and quadrupled on every
    /// doubling, so `tokenade read` on a 10 MB page spent 85 s inside a check
    /// whose only consumer is one telemetry line.
    ///
    /// The deadline is deliberately loose. It is not a benchmark — it fails
    /// only for work that grows with the square of the input, which at this
    /// size is tens of seconds even before the debug profile multiplies it.
    #[test]
    fn one_long_line_does_not_cost_the_square_of_the_input() {
        let input = "&amp;".repeat(256_000);
        let structured = "&".repeat(256_000);
        let started = std::time::Instant::now();
        let _ = super::structured_still_speaks_for(&input, &structured);
        let took = started.elapsed();
        assert!(
            took < std::time::Duration::from_secs(5),
            "checking one {}-byte line took {took:?} — the word sample is unbounded again",
            input.len()
        );
    }

    #[test]
    fn a_banner_about_something_else_does_not_speak_for_the_input() {
        // The shape the guard exists for: the compactor emitted a summary
        // that shares none of the input's vocabulary. Whatever it is, it is
        // not this input in fewer bytes.
        let input: String = (0..120)
            .map(|i| format!("INFO line {i}: some random log line about npm packages\n"))
            .collect();
        assert!(!super::structured_still_speaks_for(
            &input,
            "Installed 0 packages, 0 upgraded, 0 removed."
        ));
        assert!(super::structured_still_speaks_for(
            &input,
            "INFO line <N>: some random log line about npm packages  [×120 lines]"
        ));
    }
}
