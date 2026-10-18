//! Hostile inputs: adversarial, random, truncated — nothing may panic or fabricate.
//!
//! ONE SINGLE test binary for this subject. A file placed directly under
//! `tests/` is a CRATE: it compiles and LINKS its own, against all of
//! `tokenade-core`'s dependencies. This crate carried 87 of them. The pattern
//! is the one of `crates/tokenade-cli/tests/e2e/main.rs`.
//!
//! Adding a subject: one file here, one `mod` below. Not a file at the root
//! of `tests/`.

// The shared helpers, mounted ONCE for the binary.
#[path = "../support/corpus.rs"]
mod corpus;

mod adversarial_inputs;
mod fuzz_invariants;
mod proptest_compactors;
mod transcript_cache_hostile;
