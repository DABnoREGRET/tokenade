//! `log_tail` — delegates to [`crate::cmd::linux::log_tail`].
//!
//! The two files carried an identical set of logic lines. A second copy only
//! ever costs a fix: three modules drifted exactly this way and shipped two
//! platforms the pre-fix behaviour (2026-08-04).

pub fn detect(text: &str) -> bool {
    crate::cmd::linux::log_tail::detect(text)
}

pub fn try_compact(text: &str) -> Option<String> {
    crate::cmd::linux::log_tail::try_compact(text)
}

pub fn compact(text: &str) -> String {
    crate::cmd::linux::log_tail::compact(text)
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::cmd::linux::log_tail::compact_with_savings(text)
}

/// Re-exported: several windows log modules call this helper directly.
pub fn tail(
    text: &str,
    head_ctx: usize,
    tail_lines: usize,
    label: &str,
) -> Option<String> {
    crate::cmd::linux::log_tail::tail(text, head_ctx, tail_lines, label)
}
