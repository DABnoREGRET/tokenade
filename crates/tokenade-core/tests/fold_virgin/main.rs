//! **The tests whose validity REQUIRES a fresh process.**
//!
//! The truthfulness ceiling of the regular fold (`fold_count::folded_ceiling`)
//! is a PROCESS counter: it rises when a pass really folds. A test that checks
//! what the fold does when the counter is ZERO therefore cannot share its
//! binary with a test that folds — execution order decides the outcome.
//!
//! The subject below already carried that condition written in its header, and
//! lived it by being ALONE in its file — which was enough when each file of
//! `tests/` was its own crate. Grouping the 87 targets into 8 put
//! `uniform_holes_are_declared_once` (which folds for real) in the SAME
//! binary, and alphabetical order runs it first: green under nextest, which
//! gives one process per test, **red under `cargo test --test-threads=1`,
//! which is the release runner**. The 1.1.13 release stopped on it, exactly
//! like the 1.1.12 one before it.
//!
//! Hence: a target of its own, and nothing else in it. Adding a subject here is
//! only right if it requires the same thing, and if it folds nothing itself.

mod uniform_holes_never_claim_the_pages_own;
