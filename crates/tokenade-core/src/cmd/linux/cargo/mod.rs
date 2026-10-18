//! `cargo` — Rust toolchain.
//!
//! Per-subcommand compactors. After the L92/L94 refactor every
//! cargo subcommand lives here; `filter.rs` only keeps thin
//! delegating shims.
//!
//! Subcommands:
//!
//! - [`bench`]   — `cargo bench` — `test name ... bench: N ns/iter`
//! - [`build`]   — `cargo build` (+ `--quiet` variant) — streaming
//!   filter that keeps the first N `Compiling` lines as a sample,
//!   the full text of every `error/warning[…]:` diagnostic block,
//!   and the final `Finished` line
//! - [`fmt`]     — `cargo fmt` — silent on success, diff otherwise
//! - [`install`] — `cargo install` — `Installing <crate>` + summary
//! - [`run`]     — `cargo run` — `Compiling … Running … <binary>`
//! - [`search`]  — `cargo search <term>` — `name = "ver" # desc`
//! - [`test`]    — `cargo test` — `running N tests / test foo ... ok`
//! - [`tree`]    — `cargo tree` — ASCII dep tree
//! - [`update`]  — `cargo update` — `Updating <crate> from X to Y`

pub mod bench;
pub mod build;
pub mod fmt;
pub mod install;
pub mod run;
pub mod search;
pub mod test;
pub mod tree;
pub mod update;
