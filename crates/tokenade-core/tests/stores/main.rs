//! The ledger and the logs: what is written to disk, what is redacted there, and what holds under concurrent writers.
//!
//! ONE SINGLE test binary for this subject. A file placed directly under
//! `tests/` is a CRATE: it compiles and LINKS its own, against all of
//! `tokenade-core`'s dependencies. This crate carried 87 of them. The pattern
//! is the one of `crates/tokenade-cli/tests/e2e/main.rs`.
//!
//! Adding a subject: one file here, one `mod` below. Not a file at the root
//! of `tests/`.

/// The gain ledger is process-wide state: `HOME`, `TOKENADE_FORCE_GAIN` and
/// `gain::set_root` (a `OnceLock`). Each ledger test was written as the only
/// test of its own binary; grouped here they ran in parallel, and one would
/// write while another moved `HOME`, set the root, or removed the opt-in —
/// one of them failed at random. Held for the whole test.
pub(crate) fn ledger_lock() -> std::sync::MutexGuard<'static, ()> {
    static L: std::sync::Mutex<()> = std::sync::Mutex::new(());
    L.lock().unwrap_or_else(|e| e.into_inner())
}

mod atomic_600_writers;
mod debug_log_redacts_every_string;
mod gain_prune_noise;
mod gain_record_redacts_label;
mod gain_rows_fit_pipe_buf;
mod lockable_appends_are_readable;
mod no_unbounded_file_locks;
