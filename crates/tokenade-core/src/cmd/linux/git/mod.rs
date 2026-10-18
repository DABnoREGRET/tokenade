//! `git` — per-subcommand compactors.
//!
//! Each subcommand owns its own file. After the L92 refactor the
//! entire git compactor surface is here; `filter.rs` only keeps thin
//! delegating shims for backward source-compatibility.
//!
//! - [`log`]         — `git log` (all `--pretty` / `--oneline` /
//!   `--graph` / `--stat` / `-p` variants)
//! - [`status`]      — `git status` (porcelain v1/v2/full)
//! - [`diff`]        — `git diff` (`--stat` / `--name-only` /
//!   `--word-diff` / `--check`)
//! - [`branch`]      — `git branch -a/-r/--show-current/--merged`
//! - [`stash`]       — `git stash list/show/pop/apply/drop`
//! - [`tag`]         — `git tag --list/--sort/--annotate`
//! - [`remote`]      — `git remote -v/show/add/rm/set-url`
//! - [`show`]        — `git show <ref>`
//! - [`blame`]       — `git blame -L/--root/--reverse`
//! - [`config`]      — `git config --list/--global`
//! - [`reflog`]      — `git reflog`
//! - [`rebase_like`] — `git rebase/cherry-pick/revert/merge`
//! - [`net`]         — `git fetch/pull/push/clone`

pub mod blame;
pub mod branch;
pub mod config;
pub mod diff;
pub mod log;
pub mod net;
pub mod rebase_like;
pub mod reflog;
pub mod remote;
pub mod show;
pub mod stash;
pub mod status;
pub mod tag;
