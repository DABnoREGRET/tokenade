//! Properties held over the WHOLE corpus: idempotence, determinism, survival of error lines, richness, no quality regression.
//!
//! ONE test binary for this subject. A file placed directly under
//! `tests/` is a CRATE: it compiles and LINKS its own, against all of
//! `tokenade-core`'s dependencies. This crate used to carry 87. The pattern is
//! the one of `crates/tokenade-cli/tests/e2e/main.rs`.
//!
//! Adding a subject: a file here, a `mod` below. Not a file at
//! the root of `tests/`.

// The shared helpers, mounted ONCE for the binary.
#[path = "../support/corpus.rs"]
mod corpus;

mod build_log_corpus;
mod corpus_error_survival;
mod determinism;
mod error_survival;
mod failure_survives;
mod idempotence;
mod non_ascii_roundtrip;
mod quality_contracts;
mod richness_corpus;
