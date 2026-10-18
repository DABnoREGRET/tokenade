//! The bench's ratchets and guards, in ONE target.
//!
//! Every file placed directly under `tests/` is a crate: it compiles and
//! LINKS its own binary against the whole graph. The bench's three subjects
//! weighed **89 + 55 + 9 MB** of binaries for 21 tests, and three links on
//! every `cargo test`. It is the same pattern as the 87 targets of
//! `tokenade-core` grouped into 11: one binary per SUBJECT costs one link per
//! subject, and linking is this repository's memory peak.
//!
//! Adding a subject here is one `mod` line. A file that no `mod`
//! declares is not compiled and does not say so — `every_test_sibling_is_mounted`
//! rejects it.

mod arguments;
mod docs_recall_ratchet;
mod fold_recovery_ratchet;
