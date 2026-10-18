//! One process-wide async runtime, built lazily EXACTLY once.
//!
//! `reqwest` is async; the rest of tokenade is synchronous and runs from plain
//! `std::thread`s (the search fan-out spawns one per engine). Containment is a
//! single `OnceLock<Runtime>` with `block_on` at the transport boundary, so a
//! synchronous caller never sees a future.
//!
//! Risk #1 in the plan is "runtime-per-call" — a silent perf regression that
//! would leave every test green while each fetch paid a fresh thread-pool
//! spin-up. [`runtimes_built`] counts real constructions so a test can assert
//! the count stays 1 no matter how many threads call [`block_on`].
//!
//! `block_on` must be called from OUTSIDE the runtime (our synchronous callers
//! and the search fan-out threads are). In-process batch fan-out (D11) instead
//! runs its concurrency INSIDE one `block_on(async { … })`, never nesting.

use std::future::Future;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;

static BUILDS: AtomicU32 = AtomicU32::new(0);

/// The shared multi-thread runtime. Built once; every later call returns the
/// same handle.
fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        BUILDS.fetch_add(1, Ordering::Relaxed);
        tokio::runtime::Builder::new_multi_thread()
            // Small fixed pool: our concurrency is a handful of engines / URLs,
            // not thousands of connections. Bounded so a machine with 128 cores
            // does not spawn 128 idle workers for one fetch.
            .worker_threads(4)
            .enable_all()
            .thread_name("tokenade-net")
            .build()
            .expect("tokio runtime build")
    })
}

/// Drive `f` to completion on the shared runtime. Safe from any synchronous
/// caller and from the search fan-out threads (all outside the runtime).
pub fn block_on<F: Future>(f: F) -> F::Output {
    runtime().block_on(f)
}

/// How many runtimes have actually been constructed this process. The invariant
/// is 1 — see the module docs and the risk register.
pub fn runtimes_built() -> u32 {
    BUILDS.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Risk #1: the runtime must be built once even under concurrent first use
    /// from many threads — otherwise the migration is a silent perf regression.
    #[test]
    fn the_runtime_is_built_at_most_once_across_threads() {
        let handles: Vec<_> = (0..8)
            .map(|i| std::thread::spawn(move || block_on(async move { i * 2 })))
            .collect();
        let sum: i32 = handles.into_iter().map(|h| h.join().unwrap()).sum();
        let expected: i32 = (0..8).map(|i| i * 2).sum();
        assert_eq!(sum, expected);
        assert_eq!(
            runtimes_built(),
            1,
            "the runtime must be constructed exactly once"
        );
    }
}
