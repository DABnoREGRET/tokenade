//! Claude Code transcript aggregation.
//!
//! Reads token usage from `~/.claude/projects/<encoded-cwd>/<session>.jsonl`
//! to give the dashboard visibility into Claude API usage that **bypassed
//! Tokenade entirely** — sessions where neither the MCP server nor the
//! hook was in the loop, so `gain.jsonl` has no record of them.
//!
//! Each session file contains one JSON object per line; only records with
//! `type == "assistant"` carry a `message.usage` block with
//! `input_tokens`, `output_tokens`, `cache_creation_input_tokens`,
//! `cache_read_input_tokens`. We group by the `cwd` field present on
//! every record (constant per session in practice) so the dashboard can
//! say "Claude consumed N tokens in project X, none of which Tokenade
//! intercepted".
//!
//! Pure aggregation — no I/O outside reading the JSONL files, no network,
//! no panics on malformed lines (they're skipped).

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// Unit-struct `TranscriptParser` impl used by the multi-agent
/// dispatch in [`crate::agent_transcripts`]. The real logic still
/// lives in the free functions below — this is a thin adapter so
/// adding new agents (Codex, Cline, …) doesn't force us to refactor
/// the existing API.
pub struct Parser;

impl crate::agent_transcripts::TranscriptParser for Parser {
    fn agent(&self) -> crate::agent_transcripts::Agent {
        crate::agent_transcripts::Agent::ClaudeCode
    }

    fn aggregate_since(&self, home_dir: &Path, since_iso: Option<&str>) -> Vec<ProjectUsage> {
        aggregate_since(home_dir, since_iso)
    }

    fn tool_tokens_by_day(
        &self,
        home_dir: &Path,
        since_iso: Option<&str>,
    ) -> crate::agent_transcripts::ToolTokensByDay {
        tool_tokens_by_day(home_dir, since_iso)
    }

    fn cache_creation_by_day(
        &self,
        home_dir: &Path,
        since_iso: Option<&str>,
    ) -> std::collections::BTreeMap<String, u64> {
        cache_creation_by_day(home_dir, since_iso)
    }

    fn model_usage_by_day(
        &self,
        home_dir: &Path,
        since_iso: Option<&str>,
    ) -> crate::agent_transcripts::ModelUsageByDay {
        model_usage_by_day(home_dir, since_iso)
    }

    fn session_models(
        &self,
        home_dir: &Path,
        since_iso: Option<&str>,
    ) -> Vec<crate::agent_transcripts::SessionModel> {
        session_models(home_dir, since_iso)
    }

    fn digest(
        &self,
        home_dir: &Path,
        since_iso: Option<&str>,
    ) -> crate::agent_transcripts::AgentDigest {
        digest(home_dir, since_iso)
    }
}

/// The five readers above, fused into ONE pass over each session's rows.
///
/// This is [`crate::agent_transcripts::TranscriptParser::digest`] for the
/// corpus that actually costs something. It is not a new algorithm: each block
/// below is the body of the reader it replaces, in that reader's own order,
/// with that reader's own `UsageDedup` scope kept separate — the dedup state
/// is what makes the numbers, so a shared one would silently change them.
/// What is saved is the walking: five traversals of the same rows become one.
pub fn digest(home_dir: &Path, since_iso: Option<&str>) -> crate::agent_transcripts::AgentDigest {
    use crate::agent_transcripts::{AgentDigest, SessionModel};
    let projects_root = crate::platform::claude_config_dir_for(home_dir).join("projects");

    let mut by_cwd: HashMap<String, ProjectUsage> = HashMap::new();
    let mut models: crate::agent_transcripts::ModelUsageByDay = Default::default();
    let mut models_life: crate::agent_transcripts::ModelUsageByDay = Default::default();
    let mut tools = crate::agent_transcripts::ToolTokensByDay::default();
    let mut cache_creation: std::collections::BTreeMap<String, u64> = Default::default();
    let mut sessions: std::collections::BTreeMap<(String, String), (u64, u64)> = Default::default();

    // One dedup scope per reader, exactly as each reader had its own.
    let mut dedup_models = crate::usage_dedup::UsageDedup::new();
    let mut dedup_models_life = crate::usage_dedup::UsageDedup::new();
    let mut dedup_cache = crate::usage_dedup::UsageDedup::new();

    crate::transcript_cache::with_shared(home_dir, |cache| {
        for path in jsonl_transcripts(&projects_root) {
            let rows = cache.rows_for(&path);
            let sess = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string);
            // `tool_tokens_by_day` scopes its id→name map to the session file.
            let mut tool_id_to_name: HashMap<String, String> = HashMap::new();

            for r in rows {
                let dated = !r.ts.is_empty() && r.ts.is_char_boundary(10);
                let in_window = match since_iso {
                    Some(cutoff) => !r.ts.is_empty() && r.ts.as_str() >= cutoff,
                    None => true,
                };

                if r.assistant && dated && !r.model.is_empty() && r.model != "<synthetic>" {
                    if in_window && dedup_models.admit_key(r.dedup_key) {
                        let day = models.entry(r.ts[..10].to_string()).or_default();
                        let e = day.entry(r.model.clone()).or_insert((0, 0, 0));
                        e.0 += r.input + r.cache_creation;
                        e.1 += r.output;
                        e.2 += 1;
                    }
                    if dedup_models_life.admit_key(r.dedup_key) {
                        let day = models_life.entry(r.ts[..10].to_string()).or_default();
                        let e = day.entry(r.model.clone()).or_insert((0, 0, 0));
                        e.0 += r.input + r.cache_creation;
                        e.1 += r.output;
                        e.2 += 1;
                    }
                }

                if r.assistant
                    && dated
                    && in_window
                    && dedup_cache.admit_key(r.dedup_key)
                    && r.cache_creation > 0
                {
                    *cache_creation.entry(r.ts[..10].to_string()).or_insert(0) += r.cache_creation;
                }

                if dated && in_window {
                    for u in &r.tool_uses {
                        if !u.id.is_empty() {
                            tool_id_to_name.insert(u.id.clone(), u.name.clone());
                        }
                    }
                    for res in &r.tool_results {
                        if res.tokens == 0 {
                            continue;
                        }
                        let tool = if res.use_id.is_empty() {
                            "unknown".to_string()
                        } else {
                            tool_id_to_name
                                .get(&res.use_id)
                                .cloned()
                                .unwrap_or_else(|| "unknown".to_string())
                        };
                        *tools
                            .entry(r.ts[..10].to_string())
                            .or_default()
                            .entry(tool)
                            .or_insert(0) += res.tokens;
                    }
                }

                // `session_models` is lifetime here (the attributor asks for
                // every session), and keys on the file stem.
                if let Some(sess) = sess.as_deref() {
                    if r.assistant
                        && !r.ts.is_empty()
                        && !r.model.is_empty()
                        && r.model != "<synthetic>"
                    {
                        if let Some(ms) = iso8601_to_epoch_ms(&r.ts) {
                            let e = sessions
                                .entry((sess.to_string(), r.model.clone()))
                                .or_insert((ms, ms));
                            if ms < e.0 {
                                e.0 = ms;
                            }
                            if ms > e.1 {
                                e.1 = ms;
                            }
                        }
                    }
                }
            }

            scan_session_rows(rows, &mut by_cwd, since_iso);
        }
    });

    let mut projects: Vec<ProjectUsage> = by_cwd.into_values().collect();
    projects.sort_by(|a, b| b.last_ts.cmp(&a.last_ts));

    AgentDigest {
        projects,
        models_by_day: models,
        tools_by_day: tools,
        cache_creation_by_day: cache_creation,
        lifetime_models_by_day: models_life,
        session_models: sessions
            .into_iter()
            .map(|((sess, model), (start_ms, end_ms))| SessionModel {
                sess,
                model,
                start_ms,
                end_ms,
            })
            .collect(),
    }
}

mod by_day;
mod clock;
mod estimate;
mod usage;
pub use by_day::*;
pub use clock::*;
pub use estimate::*;
pub use usage::*;

#[cfg(test)]
include!("claudecode_transcript_tests.rs");
