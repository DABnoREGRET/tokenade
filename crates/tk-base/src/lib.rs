//! **The BASE layer of tokenade: what everything else names.**
//!
//! First crate extracted from `tokenade-core`. Its content is not a matter of taste,
//! it is a checkable rule: **none of these modules names anything
//! outside of here**, and it is the repo's `layers_only_point_downwards` test
//! that establishes it, test code included.
//!
//! For that to hold, eight primitives parked with their first requester had
//! to be handed back to this layer: the session key and the state
//! directory (which lived in the LEDGER), ed25519 signature verification
//! (in the updater), CSV and JSON cookie scrubbing (in the readers of
//! those formats), the civil calendar algorithm (written TWICE), the
//! ISO-8601 formatting (in a transcript reader), the "once per period"
//! rate-limit token (in the reporter), the pattern of a
//! hexadecimal literal (in the formats) and the question "would this write
//! land in the user's real store?" (in the
//! ledger).
//!
//! What is NOT here is the point: no tree-sitter and its sixteen grammars,
//! no `scraper`, no document extractors. Touching a primitive no
//! longer relinks half the world.
//!
//! `tokenade-core` re-exports each of these modules under its old path, so
//! that no downstream caller changed an import line.

pub mod ansi;
pub mod argv;
pub mod cache;
pub mod cache_align;
pub mod cachekey;
pub mod crash_guard;
pub mod debug_log;
pub mod delivery;
pub mod edit_distance;
pub mod env_compat;
pub mod gitignore;
pub mod leaders;
pub mod line_diff;
pub mod lru;
pub mod oem;
pub mod paths;
pub mod platform;
pub mod process_util;
pub mod profile;
pub mod progress;
pub mod prompt;
pub mod redact;
pub mod regexes;
pub mod runtime;
pub mod session;
pub mod signing;
pub mod source_guard;
pub mod sqlite_tuning;
pub mod sqlite_vec_util;
pub mod srcsweep;
pub mod stores;
pub mod testtmp;
pub mod text_distance;
pub mod text_encoding;
pub mod tokenizer;
pub mod url;
