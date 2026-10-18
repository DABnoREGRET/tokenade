//! `git_listing` — delegates to [`crate::cmd::linux::git_listing`].
//!
//! This module used to carry its OWN copy of the fold, and the copies drifted
//! in a way that was not harmless: the linux one routes `head` through
//! `read_mode::effective_head`, selects the kept lines with
//! `read_mode::partition_indices`, and stamps `read_mode::banner_tag()` into
//! the banner. This copy did none of the three, so on macOS and Windows the
//! read-mode head budget was IGNORED for every git listing and the banner
//! never carried its tag.
//!
//! What made the gap easy to miss: this same file ALREADY delegated
//! `path_census` to linux. A module that delegates part of its work reads as
//! aligned.
//!
//! Nothing in this fold is platform-specific — it counts lines and groups path
//! prefixes. Delegation, not duplication: the next fix lands everywhere by
//! construction. Same reasoning, and the same accident, as `docker_table` and
//! `log_tail` (2026-08-04).

crate::cmd::families::delegates_listing_to!(crate::cmd::linux::git_listing);
