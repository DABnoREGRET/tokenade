//! CLAUDE.md cascade auditor.
//!
//! Claude Code (and Cursor, and most agent CLIs) load a *cascade* of
//! always-resident instruction files into the system-prompt prefix on
//! every session: the global `~/.claude/CLAUDE.md`, plus a `CLAUDE.md`
//! (and `.claude/CLAUDE.md`) in every ancestor directory from the repo
//! root down to the working directory. That prefix is paid on every
//! cache *write* and re-sent on every cache *miss*, so bloat and — the
//! usual culprit — the *same* boilerplate copy-pasted into several files
//! of the cascade quietly inflates the resident context.
//!
//! This is the deterministic, no-LLM counterpart to the audit that
//! prompt-driven tools (e.g. claude-context-optimizer) perform: given
//! the cascade's file contents it reports per-file token cost, the total
//! always-resident size, and — the actionable part — the lines that are
//! duplicated *across* files, with the wasted-token total you'd recover
//! by hoisting each duplicate into a single file.
//!
//! Note this attacks the *cacheable prefix*, so the realized saving on a
//! warm session is smaller than the raw number (a cache hit bills the
//! prefix at a fraction of list price) — but the duplication is pure
//! waste on every cache write and every miss, and trimming it never
//! hurts. It is complementary to tokenade's output-side compaction,
//! which targets the *uncacheable* fresh tool output each turn.

mod cascade;
mod dedup;
mod render;
pub use cascade::*;
pub use dedup::*;
pub use render::*;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::tokenizer;

/// Lines shorter than this (after normalization) are ignored for
/// duplicate detection — structural noise (`---`, ` ``` `, short headings)
/// is not worth flagging and would dominate the report.
const MIN_DUP_CHARS: usize = 24;

/// Cosine similarity threshold above which two differently-phrased lines
/// are treated as the same instruction for the semantic dedup pass.
const SEMANTIC_SIM_THRESHOLD: f32 = 0.85;

/// Per-file accounting for one member of the cascade.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileAudit {
    pub path: String,
    pub bytes: usize,
    pub lines: usize,
    pub tokens: usize,
}

/// A normalized line that appears in two or more distinct files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateLine {
    /// The normalized text (trimmed, internal whitespace collapsed).
    pub text: String,
    /// Estimated tokens for a single occurrence.
    pub tokens: usize,
    /// Distinct files this line appears in (load order, deduped).
    pub files: Vec<String>,
}

impl DuplicateLine {
    /// Tokens wasted by the redundant copies: every occurrence past the
    /// first is pure duplication.
    pub fn wasted_tokens(&self) -> usize {
        self.tokens * self.files.len().saturating_sub(1)
    }
}

/// Full result of auditing a cascade.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CascadeAudit {
    pub files: Vec<FileAudit>,
    /// Sum of every file's token estimate — the always-resident prefix size.
    pub total_tokens: usize,
    /// Cross-file duplicated lines, sorted by wasted tokens descending.
    pub duplicates: Vec<DuplicateLine>,
    /// Total recoverable tokens if every duplicate were hoisted to one file.
    pub duplicated_tokens: usize,
}

/// Normalize a line for duplicate comparison: trim ends and collapse
/// every run of internal whitespace to a single space. Returns `None`
/// for lines that are not worth comparing (blank, too short, or pure
/// markdown structure).
fn normalize(line: &str) -> Option<String> {
    let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() < MIN_DUP_CHARS {
        return None;
    }
    // Pure horizontal rules / fences carry no information worth deduping.
    let trimmed = collapsed.trim_matches(|c| c == '-' || c == '=' || c == '`' || c == '*');
    if trimmed.is_empty() {
        return None;
    }
    Some(collapsed)
}

/// Audit a cascade given each member's `(path, content)`. Pure: does no
/// I/O, so it is fully unit-testable. `files` should be in load order.
pub fn audit(files: &[(String, String)]) -> CascadeAudit {
    let mut file_audits = Vec::with_capacity(files.len());
    let mut total_tokens = 0usize;

    // normalized line -> files (in first-seen order, deduped per file)
    let mut occurrences: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for (path, content) in files {
        let tokens = tokenizer::estimate(content);
        total_tokens += tokens;
        file_audits.push(FileAudit {
            path: path.clone(),
            bytes: content.len(),
            lines: content.lines().count(),
            tokens,
        });

        // Record which normalized lines this file contributes (once each).
        let mut seen_in_this_file: BTreeMap<String, ()> = BTreeMap::new();
        for line in content.lines() {
            if let Some(norm) = normalize(line) {
                if seen_in_this_file.insert(norm.clone(), ()).is_none() {
                    occurrences.entry(norm).or_default().push(path.clone());
                }
            }
        }
    }

    let mut duplicates: Vec<DuplicateLine> = occurrences
        .into_iter()
        .filter(|(_, files)| files.len() >= 2)
        .map(|(text, files)| {
            let tokens = tokenizer::estimate(&text);
            DuplicateLine {
                text,
                tokens,
                files,
            }
        })
        .collect();

    // Most wasteful first; tie-break on text for deterministic output.
    duplicates.sort_by(|a, b| {
        b.wasted_tokens()
            .cmp(&a.wasted_tokens())
            .then_with(|| a.text.cmp(&b.text))
    });

    let duplicated_tokens = duplicates.iter().map(DuplicateLine::wasted_tokens).sum();

    CascadeAudit {
        files: file_audits,
        total_tokens,
        duplicates,
        duplicated_tokens,
    }
}

#[cfg(test)]
include!("claude_md_audit_tests.rs");
