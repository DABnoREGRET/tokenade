//! `find` — delegates to [`crate::cmd::linux::find`].
//!
//! This was a literal copy of the linux module. MEASURED 2026-08-04, the only
//! differences were a `HEAD` constant left at the scaffolded default of 60
//! instead of the tuned 200, and a generic `"line(s)"` where linux names what
//! it counted (`"find path(s)"`). The output shape is identical on both platforms, so the
//! copy bought nothing and silently withheld two corrections.
//!
//! Delegation, not duplication: the next tuning lands on both by construction.

crate::cmd::families::delegates_to!(crate::cmd::linux::find);
