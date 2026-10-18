#![allow(unused_imports)]
//! Mechanical module split of install.rs (no behaviour change). Wired
//! back into the `install` namespace by mod.rs via `pub(crate) use <name>::*;`.
use super::*;
use crate::hooks::*;
use crate::wrap::*;
use crate::*;
use std::fs;
use std::path::{Path, PathBuf};
mod register;
mod serve;
pub(crate) use register::*;
pub(crate) use serve::*;

#[cfg(test)]
include!("gemini_hooks_tests.rs");
