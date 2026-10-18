//! How many original lines this process has folded — the ceiling of what
//! a view may CLAIM to represent.
//!
//! One counter, two readers, and the ceiling's reason to exist is written in
//! the comment below: a `(×N)` marker READ in content is
//! indistinguishable from a marker we WROTE, and a file carrying
//! `(×18446744073709551615)` overflowed the addition.
//!
//! Lived in `filter.rs`. `disclose` — the fold gate — therefore had to
//! climb into the commands layer to read a counter that `filter`
//! is merely the first to have incremented.

/// How many ORIGINAL lines one line of an already-collapsed view stands for.
///
/// A run-collapsing compactor (lsof, ps, table folds) writes `  (×47)` at the
/// end of a line that replaced 47 identical-shaped ones. When a second pass
/// then elides that line, the 47 go with it — and the footer, counting lines
/// of the view, said `1`.
///
/// MEASURED 2026-09-04 on `lsof -n -P` piped through `tokenade filter`:
/// 370 367 lines in, and the footer announced `folded 8147 line(s) total`.
/// Every one of those 8 147 was a collapsed run, so the real loss was two
/// orders of magnitude larger than the number the agent was given to judge it
/// by. Read here rather than threaded down from the top of the pipeline
/// because the view carries the fact already.
/// Total original lines this PROCESS's own passes are known to have folded
/// away. [`run_weight`] and `disclose::original_weight` read a count back out
/// of a rendered line — and a line that merely LOOKS like one of our markers
/// is indistinguishable from one we wrote. Measured 2026-09-07: a 9 000-line
/// file carrying the single content line `… [900000 lines elided by tokenade]
/// …` made the summary announce "standing for 908969 line(s) of the original
/// output", and a `(×18446744073709551615)` in content aborted the whole
/// command with an arithmetic overflow.
///
/// So a parsed count is never trusted on its own: it is capped by what our own
/// collapsing passes actually removed. Every emitter below records the TRUE
/// number of source lines it consumed, and no claim can exceed that. A process
/// that folded nothing can make no claim at all, which is the case content
/// forgery reaches.
static FOLDED_ORIGINAL_LINES: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Record `n` source lines collapsed into one rendered line. Called by every
/// `(×N)` emitter with a count it computed itself, never with one it parsed.
pub fn note_folded(n: usize) {
    if let Some(local) = ISOLATED.with(|c| c.get()) {
        ISOLATED.with(|c| c.set(Some(local.saturating_add(n))));
        return;
    }
    FOLDED_ORIGINAL_LINES.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
}

/// Upper bound on what any parsed marker may be believed to stand for.
pub fn folded_ceiling() -> usize {
    ISOLATED
        .with(|c| c.get())
        .unwrap_or_else(|| FOLDED_ORIGINAL_LINES.load(std::sync::atomic::Ordering::Relaxed))
}

thread_local! {
    static ISOLATED: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

/// Test seam: a ceiling of this thread's own, starting at zero, until the
/// guard drops. Resetting the shared counter could not give a test a known floor:
/// the counter is process-wide, and any test folding on another thread raised
/// it between the reset and the call under test — `a_run_count_taken_from_
/// content_does_not_reach_the_footer` failed on every full `--lib` run and
/// passed alone. Nothing in production calls it.
///
/// `pub` without `#[cfg(test)]`: `tk-fold` is compiled WITHOUT `cfg(test)`
/// when `tokenade-core`'s tests link it (same lesson as `platform::ENV_LOCK`).
pub fn isolate_folded_ceiling() -> IsolatedCeiling {
    ISOLATED.with(|c| c.set(Some(0)));
    IsolatedCeiling(())
}

/// Ends [`isolate_folded_ceiling`] on drop, so a test-runner thread reused by
/// the next test (`--test-threads=1`) goes back to the shared counter.
pub struct IsolatedCeiling(());

impl Drop for IsolatedCeiling {
    fn drop(&mut self) {
        ISOLATED.with(|c| c.set(None));
    }
}

// `named_if_it_folded` — keep the named compactor only if it actually
// folded. Fold accounting, like the ceiling above.

/// The `<prog>_<sub>` spec for a two-token command, when one exists and folds.
///
/// A compactor named for a subcommand (`go test` → `go_test`, `systemctl
/// list-dependencies` → `systemctl_list_dependencies`) is keyed `prog_sub`,
/// while the bare dispatch above only ever looks up `prog`. The generic path
/// has retried the composite key since `go_test` was found unreachable — but it
/// is the LAST arm of the match, so every program with an arm of its own
/// returns before reaching it, and its subcommand specs are unreachable in
/// exactly the way that fix was written to prevent.
///
/// Measured 2026-09-01: `systemctl list-dependencies` folded nothing (916 →
/// 916 bytes) with `systemctl_list_dependencies` sitting in the spec table, and
/// `gh` returned unconditionally with 58 `gh_*` specs behind it. Extracted here
/// so an arm can ask the same question the generic path asks, instead of the
/// answer depending on whether the program was important enough to get an arm.
///
/// Content-defensive, like the retry it replaces: every `try_compact` starts
/// with `if !detect(text) { return None }`, so a wrong guess folds nothing and
/// the caller falls through unchanged. A fold that does not shrink the payload
/// is refused.
/// The program's compactor, if it FOLDED something; otherwise the other one.
///
/// A command module may return its input UNCHANGED — that is what
/// `ps.rs` does on a macOS `ps aux` output. Where that result was returned
/// as is, the format detector never ran: MEASURED 2026-09-09 on
/// the corpus capture, **4,387 bytes returned versus 2,016** that `auto`
/// produces, and 3,012 versus 1,686 on the full delivery path.
///
/// **The comparison is against the INPUT, not the other candidate**, and that is the
/// lesson of the first attempt: "the shorter of the two wins" broke
/// `a_machine_full_of_zombies_still_gets_a_short_answer`, because the
/// `ps` compactor promotes the zombies then COUNTS them — 30 lines that
/// answer — where the generic one returns 48 lines slightly lighter in bytes.
/// A module that folds carries a judgement the byte count does not express.
/// One that folds nothing carries none.
///
/// It is also the guard the generic path ALREADY applied (`after <
/// before`) and that three families lacked.
pub fn named_if_it_folded(fallback: String, named: Option<String>, original: &str) -> String {
    match named {
        Some(c) if c.len() < original.len() => c,
        _ => fallback,
    }
}

// ─────────────────────────────────────────────────────────────────────
// When the compactor INFLATED, keep the raw output
// ─────────────────────────────────────────────────────────────────────
//
// `pick_compact_or_raw` lived in `process_util`, the primitive that launches
// subprocesses, because the command wrapper was the first to need
// it. Yet it is not process launching: it is fold
// ACCOUNTING — the only place in the base layer that named
// `Savings`, and hence the only thing preventing `process_util` from moving
// into the `tk-base` crate.

/// Pick the smaller of (raw, compacted) by token count. Returns the
/// chosen body + the truthful Savings to log.
///
/// When `compacted_tokens > raw_tokens` (the per-command compactor's
/// banner overhead exceeded its content savings — typical on tiny
/// outputs like `cargo build … | tail -3`), the agent would receive
/// a BIGGER payload than the original. Fall back to raw + record
/// honest 0% savings.
///
/// Used by 3 layers: cmd_wrap, MCP exec_compact, sandbox::execute.
/// Centralised so the behaviour can't drift between them.
///
/// `raw_tokens_estimate` is a closure that returns the raw byte's
/// token count — only called when inflation is detected, so we don't
/// pay the tokenizer cost on the happy path.
pub fn pick_compact_or_raw<F: FnOnce() -> usize>(
    raw: String,
    compacted: String,
    sav: crate::Savings,
    raw_tokens_estimate: F,
) -> (String, crate::Savings) {
    if sav.tokens_after > sav.tokens_before {
        let n = raw_tokens_estimate();
        (raw, crate::Savings::new(n, n))
    } else {
        (compacted, sav)
    }
}

#[cfg(test)]
include!("fold_count_tests.rs");
