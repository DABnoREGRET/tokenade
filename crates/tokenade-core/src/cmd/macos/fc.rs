//! `fc` — delegates to [`crate::cmd::linux::fc`].
//!
//! The two files were byte-identical apart from comments. A second copy of the
//! same logic only ever costs a fix: three modules drifted exactly this way and
//! shipped two platforms the pre-fix behaviour (2026-08-04). Delegation means
//! the next correction lands on both by construction.

crate::cmd::families::delegates_to!(crate::cmd::linux::fc);
