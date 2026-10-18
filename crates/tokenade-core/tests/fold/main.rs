//! Folds: what a compactor removes, what it says about it, and what can be
//! recovered from it.
//!
//! ONE SINGLE test binary for the whole subject, and that is the point. Each
//! file placed directly under `tests/` is a CRATE: it compiles and LINKS its
//! own, against the whole dependency set of `tokenade-core` (16 tree-sitter
//! grammars, rusqlite, quickjs, model2vec, scraper). This crate carried 87 of
//! them, for 192 executables in `target/debug/deps`, the largest weighing
//! 194 MB. `nextest` measures 101 s of EXECUTION for 9,193 tests: the hours
//! were in the link, not in the tests.
//!
//! The pattern is the one of `crates/tokenade-cli/tests/e2e/main.rs`, which
//! holds 12 subjects in one target and already says why — "extra files under
//! `tests/` would each build and link their own". `tokenade-core` had never
//! received it.
//!
//! Adding a subject here: one file in this directory, one `mod` below.
//! Do NOT put one back at the root of `tests/`.

// The shared corpus, mounted ONCE for the whole binary. Subjects take it
// through `crate::corpus` instead of each re-mounting it through `#[path]`.
#[path = "../support/corpus.rs"]
mod corpus;

mod a_fold_is_reached_by_the_route_that_needs_it;
mod a_hand_rolled_fold_declares_itself;
mod clamped_tail_starts_a_line;
mod comment_stripping;
mod folded_lines_are_recoverable;
mod language_free_fold;
mod outline_grammar;
mod recovery_counters;
mod unanswerable;
mod uniform_holes_are_declared_once;
