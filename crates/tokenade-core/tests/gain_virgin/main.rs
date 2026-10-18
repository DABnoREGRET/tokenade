//! **The ledger has a `OnceLock`: first set, first served.**
//!
//! `gain::set_root()` is the tests' isolation API, and it writes into a
//! `OnceLock`: the FIRST call of the process wins, later ones are ignored.
//! A test that checks what the ledger does when the root is NOT overridden
//! therefore cannot share its binary with a test that overrides it — it would
//! read its neighbour's verdict.
//!
//! The subject below has said so in its header for a long time and lived it by
//! being its own test crate; grouping the 87 targets put it in `stores`, with
//! two other ledger tests that DO call `set_root`. Green under nextest (one
//! process per test), **red under `cargo test --test-threads=1`**, which is
//! the release runner.
//!
//! Nothing else here: a subject that calls `set_root` would break this one.

mod gain_test_op_guard;
