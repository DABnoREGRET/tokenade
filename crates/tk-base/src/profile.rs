//! Opt-in phase timing for the hook paths.
//!
//! Born from two latency investigations: `tokenade wrap 'echo hi'` costs ~46 ms
//! for a command that prints three bytes, and SessionStart costs ~1 s on a real
//! install against 5 ms on an empty one. Ten bisection experiments failed to
//! attribute either — every candidate (the two SQLite stores, the stash, the
//! 1.3 GB of transcripts, `read-pending`'s 12 k files, the ledgers, the hook
//! self-heal) was ruled out by measurement, and the cost still did not
//! reproduce from an assembled home. The next tool is a profiler, and this host
//! has neither `perf` nor `valgrind`.
//!
//! So the product carries its own. This is deliberately the smallest thing that
//! answers "which phase spent the wall time":
//!
//! * **off by default and free when off** — one `OnceLock<bool>` read, no
//!   allocation, no clock call. The hook path runs on every tool call; a
//!   profiler that costs something is a profiler nobody can leave in.
//! * **no aggregation, no tree** — each phase prints one line to stderr on
//!   drop. stderr, not stdout: a hook's stdout is the agent's context, and
//!   printing there would corrupt the very thing being measured.
//! * **nesting is the caller's business** — phases are named, not scoped.
//!
//! ```ignore
//! let _p = profile::phase("sessionstart:scaffold");
//! ```
//!
//! Enable with `TOKENADE_PROFILE=1`. Read the output with the phase names, not
//! the totals: two phases can overlap if the caller nests them.

use std::sync::OnceLock;
use std::time::Instant;

/// Is profiling on? Read once; an env lookup per phase would itself show up in
/// the numbers on a path that runs thousands of times a session.
fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        matches!(
            std::env::var("TOKENADE_PROFILE").as_deref(),
            Ok("1") | Ok("true") | Ok("yes")
        )
    })
}

/// A running phase. Prints `name` and its duration when dropped.
///
/// Holds no clock and prints nothing when profiling is off, so leaving these in
/// the hot path costs a branch.
pub struct Phase {
    name: &'static str,
    started: Option<Instant>,
}

impl Drop for Phase {
    fn drop(&mut self) {
        if let Some(t) = self.started {
            // µs, not ms: the phases worth finding here are the ones that add
            // up, and a 400 µs phase rounds to 0 ms while a hundred of them are
            // the whole problem.
            eprintln!(
                "[tokenade:profile] {:>9} µs  {}",
                t.elapsed().as_micros(),
                self.name
            );
        }
    }
}

/// Start a named phase. The returned guard reports on drop.
pub fn phase(name: &'static str) -> Phase {
    Phase {
        name,
        started: enabled().then(Instant::now),
    }
}

/// Time a closure and return its value. For expressions that are awkward to
/// wrap in a scope.
pub fn timed<T>(name: &'static str, f: impl FnOnce() -> T) -> T {
    let _p = phase(name);
    f()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The property that lets these stay in the hot path: with profiling off,
    /// a phase never reads the clock.
    #[test]
    fn a_phase_is_inert_when_profiling_is_off() {
        // `enabled()` is a process-wide OnceLock and the test binary does not
        // set TOKENADE_PROFILE, so this exercises the off path — the one that
        // ships.
        let p = phase("test:inert");
        assert!(
            p.started.is_none(),
            "profiling is off, so no clock should have been read"
        );
    }

    #[test]
    fn timed_returns_the_closure_value() {
        assert_eq!(timed("test:value", || 6 * 7), 42);
    }
}
