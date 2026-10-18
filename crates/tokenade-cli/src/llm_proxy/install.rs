//! Putting the proxy in an agent's path, and getting it back out.
//!
//! Three constraints, all of them operator decisions rather than engineering
//! taste, and each one shapes the code below:
//!
//! 1. **Config files only.** No environment variable is written, anywhere. The
//!    env route would need a shell rc file, which is the neighbourhood of the
//!    aliases and the PATH shim that were removed from this product for good
//!    aliases and the PATH shim that were removed from this product for good
//!    ("it created enormous problems, possibly even bricking machines"). A
//!    config key is a file we can back up, diff and put back.
//! 2. **The upstream comes from the downstream.** The proxy needs the URL the
//!    agent was talking to, which is the very value we are about to overwrite.
//!    So installation READS FIRST, records what it found, and only then writes.
//! 3. **Uninstall restores.** A base URL pointing at a proxy that is gone is
//!    not a degraded agent, it is a broken one — every turn fails. This is the
//!    only rung of the ladder whose installation can break the agent, and the
//!    reason the record below is written before the config is touched.
//!
//! The failure this module is most careful about is not a bad write, it is a
//! SECOND write. Installing twice, naively, reads back the proxy's own address
//! as "what the agent was talking to" and the proxy becomes its own upstream —
//! a loop that answers every request with another request to itself. Every
//! entry point here refuses to capture an upstream that points at us.

use std::path::{Path, PathBuf};
use tokenade_core::detect::Agent;

// Seven subjects. `pub use` hands them back to the `install::` path that the
// rest of the proxy spells.
mod agents_doc;
mod agents_flat;
mod apply;
mod loop_guard;
mod model;
mod shell_profile;
mod state;
use agents_doc::*;
use agents_flat::*;
pub(crate) use apply::*;
pub(crate) use loop_guard::*;
pub(crate) use model::*;
pub(crate) use shell_profile::*;
pub(crate) use state::*;

#[cfg(test)]
#[path = "install_tests.rs"]
mod tests;
