//! **The AGENTS layer: who is installed, what it said, what it spent.**
//!
//! Fourth crate of the split. These 29 modules are the ones that know the
//! AGENTS — one module per recognised harness (`agent_codex`, `agent_gemini`,
//! `agent_cline`, `agent_kilo`, `agent_opencode`, `agent_pi`… fourteen in
//! all), the reading of their transcripts (`claudecode_transcript`,
//! `agent_transcripts`, `transcript_cache`, `usage_dedup`), the detection of what
//! is set up on the machine (`detect`, `competitors`), and what is counted
//! per agent (`agent_jsonl_usage`, `agent_sqlite_usage`, `cache_frame`,
//! `cache_thrash`, `mcp_stats`).
//!
//! **It asks NOTHING of the formats.** Measured before the cut: these
//! 29 modules name seven modules outside themselves — `paths`, `platform`,
//! `process_util`, `text_encoding`, `testtmp`, `tokenizer` and `coefficients` —
//! and not a single format module. A transcript reader reads JSONL and
//! SQLite, not markdown.
//!
//! External dependencies: `serde`, `serde_json`, `rusqlite`.
//!
//! **No edge leaves this layer**, tests included — the closure was
//! computed on both halves of the files, because the layer guard only
//! reads the production half and the compiler does not make that
//! distinction.

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

pub mod agent_antigravity;
pub mod agent_cline;
pub mod agent_codex;
pub mod agent_copilot;
pub mod agent_gemini;
pub mod agent_grok;
pub mod agent_hermes;
pub mod agent_jsonl_usage;
pub mod agent_kilo;
pub mod agent_openclaw;
pub mod agent_opencode;
pub mod agent_pi;
pub mod agent_qwen;
pub mod agent_sqlite_usage;
pub mod agent_transcripts;
pub mod cache_frame;
pub mod cache_thrash;
pub mod claudecode_transcript;
pub mod competitors;
pub mod detect;
pub mod mcp_stats;
pub mod mcp_wrap;
pub mod nudge;
pub mod pack;
pub mod review_context;
pub mod style;
pub mod transcript_cache;
pub mod usage_dedup;
pub mod webdash;
