//! **The FOLD layer: the gate, the budget, elision, the capability tables.**
//!
//! Second crate of the split, after `tk-base`. These 26 modules are the ones
//! that decide WHAT WE DELIVER: the budget (`fold_gate`, `coefficients`),
//! the cut (`elision`, `truncate_middle`, `line_gutter`), repetition
//! (`template_fold`, `phrase_fold`, `dedup_ref`, `superseded`), shape
//! (`textshape`, `prose`, `table_fold`, `archetypes`) and what the agent
//! actually takes in (`agent_caps`, `caps`, `adaptive`).
//!
//! **No edge leaves this layer.** That is the condition for the
//! compiler to accept the boundary, and it was paid for: the fold gate,
//! `disclose`, held five of them — it moved up into the formats, of which it
//! is a module (it NAMES what it stores). See the
//! `layers_only_point_downwards` guard.
//!
//! The `tk-base` modules are re-exported here under their current names:
//! the 26 files spell `crate::platform`, `crate::tokenizer`,
//! `crate::regexes::lazy_regex!` — and keep doing so, without a single
//! import line changing. A `pub use` costs nothing at run time.
//!
//! External dependencies: `serde_json`, `regex`, `rusqlite` and `image`. No
//! `tree-sitter`, no `scraper`: touching a fold no longer relinks sixteen
//! grammars.

pub use tk_base::{
    ansi, argv, cache, cache_align, cachekey, crash_guard, debug_log, edit_distance, env_compat,
    gitignore, line_diff, lru, paths, platform, process_util, profile, progress, prompt, redact,
    regexes, runtime, session, signing, source_guard, sqlite_tuning, sqlite_vec_util, srcsweep,
    stores, testtmp, text_distance, text_encoding, tokenizer,
};

mod savings;
pub use savings::Savings;

pub mod adaptive;
pub mod agent_caps;
pub mod archetypes;
pub mod caps;
pub mod coefficients;
pub mod compress;
pub mod dedup_ref;
pub mod elision;
pub mod fold_count;
pub mod fold_gate;
pub mod fold_marker;
pub mod fold_store;
pub mod glob_filter;
pub mod image_downscale;
pub mod line_gutter;
pub mod phrase_fold;
pub mod prose;
pub mod read_mode;
pub mod read_view;
pub mod richness;
pub mod store_scrub;
pub mod superseded;
pub mod table_fold;
pub mod template_fold;
pub mod textshape;
pub mod turn_summary;
