//! The PROBES: measurements, censuses, network routes. Everything here is either `#[ignore]` or gated on the environment or a feature — it is excluded by TARGET (`--test probes`) instead of relying on `--ignored`.
//!
//! ONE SINGLE test binary for this subject. A file placed directly under
//! `tests/` is a CRATE: it compiles and LINKS its own, against all of
//! `tokenade-core`'s dependencies. This crate carried 87 of them.
//!
//! Adding a subject: one file here, one `mod` below. Not a file at the root
//! of `tests/`.

// The shared helpers, mounted ONCE for the binary.
#[path = "../support/corpus.rs"]
mod corpus;

mod dom_dedup_timing;
mod estimator_gain_fidelity;
mod json_headroom;
mod markdown_composition;
mod net_infra_live;
mod repetitive_doc;
mod search_live;
mod web_headroom;
mod webfetch_gate_bench;

// The file carries `#![cfg(feature = "bpe")]`: the gate stays on the MODULE,
// it does not become a binary gate.
mod estimator_calibration_dump;
