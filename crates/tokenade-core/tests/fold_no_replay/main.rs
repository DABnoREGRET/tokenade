//! **A subject that SETS an environment variable sets it for the whole
//! binary.**
//!
//! `no_fabricated_token` turns the replay memory off (`TOKENADE_NO_REPLAY=1`)
//! before measuring, and its safety comment says exactly under which condition
//! that is legal: "this test binary only contains tests that want this, and a
//! test binary is its own process". Grouping the 87 targets made the sentence
//! FALSE — it shared the `fold` binary with thirty-four other subjects, which
//! therefore ran without replay memory without asking for it, in alphabetical
//! order and under `cargo test` only.
//!
//! So it has its binary, and the sentence is true again. A subject added here
//! must want the same thing.

mod no_fabricated_token;
