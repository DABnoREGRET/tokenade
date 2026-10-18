//! GENERATED parity harness — the needle table below is generated; the test
//! itself is hand-maintained.
//!
//! Proves each [`crate::cmd::spec_table_folds::FOLD_SPECS`] row reproduces
//! the stub module it replaces, byte for byte: on input that fires the gate
//! and folds, on input that fires but is too short, on tabular and
//! non-tabular shapes, and on input that must not fire at all.
//!
//! Compares against [`crate::cmd::dispatch_auto`] — the route dispatch really
//! takes — and NOT against a per-key list of module paths. It used to hold one
//! such list, one entry per key, and a command with several per-OS copies got
//! exactly one of them: `sysctl` was checked against the macOS copy while
//! `dispatch_auto` routes `sysctl` to the Linux one. Five rows (`cp`, `grep`,
//! `rm`, `strings`, `sysctl`) were proven against a module no caller reaches,
//! and shadowed a different, live one. Its doc claimed "a key appears once per
//! OS directory that holds it, and all copies must agree with the row" — the
//! list was one entry per key, and collecting it into a map would have dropped
//! the duplicates anyway.
#[cfg(test)]
include!("spec_fold_parity_tests.rs");
