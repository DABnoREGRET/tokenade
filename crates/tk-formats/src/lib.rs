//! **The FORMATS layer: recognise a format and make it shorter.**
//!
//! Third crate of the split, after `tk-base` and `tk-fold`. These 41 modules
//! are the ones that know what a text IS — JSON, YAML, a `Cargo.lock`,
//! a log, a JUnit report, a PowerShell table — and how to make it
//! shorter without lying about it.
//!
//! **No heavy dependency, and that is the boundary criterion.** The formats
//! layer has seven modules that carry one: `docs` and `pdf_layout`
//! (`lopdf`), `treesitter` and `dom_dedup` (`tree-sitter` and its sixteen
//! grammars), `web_tree` and `tree_dedup` (`scraper`, `reqwest`), `cmd`
//! (`zip`). They stay in `tokenade-core` until `tk-web` and
//! `tk-code` exist: bringing them in here would give a change to `json.rs` the
//! price of sixteen grammars, which is what this split exists to remove.
//!
//! External dependencies: `serde`, `serde_json`, `regex`. Nothing else.
//!
//! **No edge leaves this layer** — the compiler would not accept it
//! otherwise. The modules of `tk-base` and `tk-fold` are re-exported
//! here under their current names: files spell `crate::archetypes`,
//! `crate::tokenizer`, `crate::redact`, and keep doing so without a single
//! import line changing. A `pub use` costs nothing at run time.
//!
//! The five modules that stay outside and that one would expect here are outside for
//! a written reason: `auto` is the ROUTER (it names `cmd` and `filter`),
//! `disclose` is the fold gate (it calls back into `gain` and `rag`), `xml` reads
//! Atom feeds through `site_readers`, `diff` needs `skeleton`, and `archive`
//! goes through `filelist`.

pub use tk_base::{
    ansi, argv, cache, cache_align, cachekey, crash_guard, debug_log, edit_distance, env_compat,
    gitignore, line_diff, lru, paths, platform, process_util, profile, progress, prompt, redact,
    regexes, runtime, session, signing, source_guard, sqlite_tuning, sqlite_vec_util, srcsweep,
    stores, testtmp, text_distance, text_encoding, tokenizer,
};

pub use tk_fold::{
    adaptive, agent_caps, archetypes, caps, coefficients, compress, dedup_ref, elision, fold_count,
    fold_gate, fold_marker, fold_store, glob_filter, image_downscale, line_gutter, phrase_fold,
    prose, read_mode, read_view, richness, store_scrub, superseded, table_fold, template_fold,
    textshape, turn_summary,
};

pub use tk_fold::Savings;

pub mod ansible;
pub mod base64_blob;
pub mod bibtex;
pub mod cargobench;
pub mod containerlogs;
pub mod csv;
pub mod edge_error;
pub mod email;
pub mod env;
pub mod fences;
pub mod ghactions;
pub mod graphql;
pub mod har;
pub mod head_tail;
pub mod html_nesting;
pub mod icalendar;
pub mod json;
pub mod junit;
pub mod jvmbuild;
pub mod k8smanifest;
pub mod kubectl;
pub mod latex;
pub mod lockfile;
pub mod logs;
pub mod makefile;
pub mod markdown;
pub mod metrics;
pub mod minified;
pub mod ndjson;
pub mod notebook;
pub mod openapi;
pub mod pdf_text;
pub mod pem;
pub mod post_passes;
pub mod source_signals;
pub mod sourcemap;
pub mod speaks_for;
pub mod sql;
pub mod ssh_keys;
pub mod systemd;
pub mod table_profile;
pub mod tabular;
pub mod terraform;
pub mod transcript_view;
pub mod value_shapes;
pub mod vcard;
pub mod yaml;
