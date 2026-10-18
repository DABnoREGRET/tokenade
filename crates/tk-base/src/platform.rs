//! Cross-platform helpers: home directory, cache directory, shell command.
//!
//! Centralised so a Windows port (or a sandboxed environment that hides
//! `$HOME`) only needs to be fixed in one place.

use std::path::PathBuf;

// Seven subjects. The file only keeps the `mod`s and the test mounting.
mod exe;
mod files;
mod home;
mod housekeeping;
mod isolation;
mod paths;
mod time;
pub use exe::*;
pub use files::*;
pub use home::*;
pub use housekeeping::*;
pub use isolation::*;
pub use paths::*;
pub use time::*;

#[cfg(test)]
include!("platform_tests.rs");
