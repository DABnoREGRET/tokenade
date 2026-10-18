//! The cross-cutting subjects that belong to no layer: transcript reader equivalence, VM fixtures, snapshots, search quality.
//!
//! ONE SINGLE test binary for this subject. A file placed directly under
//! `tests/` is a CRATE: it compiles and LINKS its own, against all of
//! `tokenade-core`'s dependencies. This crate carried 87 of them.
//!
//! Adding a subject: one file here, one `mod` below. Not a file at the root
//! of `tests/`.

// The shared helpers, mounted ONCE for the binary.
#[path = "../support/ab.rs"]
mod ab;

mod a_format_is_reached_by_its_router;
mod search_quality;
mod snapshot_compactors;
mod transcript_readers_equivalence;
mod vm_fixtures;
