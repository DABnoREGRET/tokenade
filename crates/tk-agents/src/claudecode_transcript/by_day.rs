//! **The series** — per day, per model, per tool.
//!
//! What the dashboard plots. A total does not say whether usage is rising; a
//! series does, and it is the only shape that makes a trend readable.
//!
//! The three views share `scan_session_rows`, which reads once and returns the
//! usable rows: three sweeps of the same JSONL for three questions
//! would cost the file three times.

use super::*;

/// Per-model token usage across all Claude Code transcripts, bucketed
/// by UTC day (`YYYY-MM-DD` from the record's ISO timestamp). Value per
/// model = (input_fresh, output, messages), where input_fresh =
/// `input_tokens` + `cache_creation_input_tokens` — cache READS are
/// excluded on purpose: re-sent history would dwarf fresh content and
/// make every model look identical.
///
/// Powers the web dashboard's "Models" panel. Records with model
/// `<synthetic>` (Claude Code's marker for client-generated error
/// messages) carry no real API usage and are skipped.
pub fn model_usage_by_day(
    home_dir: &Path,
    since_iso: Option<&str>,
) -> std::collections::BTreeMap<String, std::collections::BTreeMap<String, (u64, u64, u64)>> {
    let mut out: std::collections::BTreeMap<
        String,
        std::collections::BTreeMap<String, (u64, u64, u64)>,
    > = Default::default();
    // One API response, one count. Claude Code writes a single assistant
    // response as several transcript entries that all repeat the same
    // `message.usage`; summing per entry tripled every figure here
    // (measured +203.7% over 129 real transcripts, 2026-07-31).
    let mut dedup = crate::usage_dedup::UsageDedup::new();
    let projects_root = crate::platform::claude_config_dir_for(home_dir).join("projects");
    crate::transcript_cache::with_shared(home_dir, |cache| {
        for path in jsonl_transcripts(&projects_root) {
            for r in cache.rows_for(&path) {
                if !r.assistant {
                    continue;
                }
                if r.ts.is_empty() {
                    continue;
                }
                if let Some(cutoff) = since_iso {
                    if r.ts.as_str() < cutoff {
                        continue;
                    }
                }
                if r.model.is_empty() || r.model == "<synthetic>" || !r.ts.is_char_boundary(10) {
                    continue;
                }
                if !dedup.admit_key(r.dedup_key) {
                    continue;
                }
                let day = out.entry(r.ts[..10].to_string()).or_default();
                let e = day.entry(r.model.clone()).or_insert((0, 0, 0));
                e.0 += r.input + r.cache_creation;
                e.1 += r.output;
                e.2 += 1;
            }
        }
    });
    out
}

/// Per-day total `cache_creation_input_tokens` (cache WRITES) across all Claude
/// Code transcripts (`YYYY-MM-DD`). Powers the dashboard's choice to EXCLUDE
/// cache-creation from the "total agent consumption" bill-share denominator:
/// cache writes are dominated by re-caching the conversation/system prompt/file
/// context — tokens tokenade structurally cannot compact — so counting them
/// against tokenade's tool-output savings makes the share misleadingly tiny.
/// (cache_read is already excluded everywhere; this isolates the write side.)
pub fn cache_creation_by_day(
    home_dir: &Path,
    since_iso: Option<&str>,
) -> std::collections::BTreeMap<String, u64> {
    let mut out: std::collections::BTreeMap<String, u64> = Default::default();
    // Same one-response-one-count rule as `model_usage_by_day`.
    let mut dedup = crate::usage_dedup::UsageDedup::new();
    let projects_root = crate::platform::claude_config_dir_for(home_dir).join("projects");
    crate::transcript_cache::with_shared(home_dir, |cache| {
        for path in jsonl_transcripts(&projects_root) {
            for r in cache.rows_for(&path) {
                if !r.assistant || r.ts.is_empty() {
                    continue;
                }
                if let Some(cutoff) = since_iso {
                    if r.ts.as_str() < cutoff {
                        continue;
                    }
                }
                if !r.ts.is_char_boundary(10) {
                    continue;
                }
                if !dedup.admit_key(r.dedup_key) {
                    continue;
                }
                if r.cache_creation > 0 {
                    *out.entry(r.ts[..10].to_string()).or_insert(0) += r.cache_creation;
                }
            }
        }
    });
    out
}

/// Per-day, per-tool tool_result token totals across all Claude Code
/// transcripts (`YYYY-MM-DD` from each tool_result record's ISO
/// timestamp). Mirrors the per-tool attribution in [`scan_session`]:
/// `tool_use` blocks (assistant records) seed an id→name map scoped to
/// the session file, and each `tool_result` block (user record) charges
/// its body token estimate to that tool on the result's day.
///
/// tokenade-provided namespaces (`mcp__tokenade__*`, …) are kept in the
/// buckets exactly as [`aggregate_since`] keeps them — the dashboard
/// subtracts genuine tokenade activity at display time. Powers the
/// windowed "outside tokenade" panel.
pub fn tool_tokens_by_day(
    home_dir: &Path,
    since_iso: Option<&str>,
) -> crate::agent_transcripts::ToolTokensByDay {
    let mut out = crate::agent_transcripts::ToolTokensByDay::default();
    let projects_root = crate::platform::claude_config_dir_for(home_dir).join("projects");
    crate::transcript_cache::with_shared(home_dir, |cache| {
        for path in jsonl_transcripts(&projects_root) {
            // tool_use_id → name, scoped per session file (ids aren't unique
            // across files). A tool_use always precedes its tool_result.
            let mut tool_id_to_name: HashMap<String, String> = HashMap::new();
            for r in cache.rows_for(&path) {
                if r.ts.is_empty() {
                    continue;
                }
                if let Some(cutoff) = since_iso {
                    if r.ts.as_str() < cutoff {
                        continue;
                    }
                }
                if !r.ts.is_char_boundary(10) {
                    continue;
                }
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
                    *out.entry(r.ts[..10].to_string())
                        .or_default()
                        .entry(tool)
                        .or_insert(0) += res.tokens;
                }
            }
        }
    });
    out
}

/// Fold one session's already-extracted rows into `by_cwd`.
///
/// This used to open the file and materialise every line as a
/// `serde_json::Value`; its own comment had measured that as 24 of the
/// dashboard's 25 seconds. The extraction now happens once per file in
/// [`crate::transcript_cache`] and is reused by every reader in this module
/// and across runs, so the accounting below is all that is left — and the
/// order of operations is preserved exactly, because it is what the numbers
/// are made of.
pub(crate) fn scan_session_rows(
    rows: &[crate::transcript_cache::Row],
    by_cwd: &mut HashMap<String, ProjectUsage>,
    since_iso: Option<&str>,
) {
    // Scoped to this file: the same `message.id` in another session is another
    // response, and must not be masked by this one.
    let mut dedup = crate::usage_dedup::UsageDedup::new();
    // Track which cwds this session file has touched, so the `sessions`
    // counter increments at-most-once per (cwd, file) pair.
    let mut session_cwds: std::collections::HashSet<String> = std::collections::HashSet::new();
    // tool_use_id → tool_name, scoped per session file (ids aren't unique
    // across files). A tool_use always precedes its tool_result.
    let mut tool_id_to_name: HashMap<String, String> = HashMap::new();

    for r in rows {
        // Cutoff filter: byte-wise compare, which on ISO-8601's fixed layout
        // is chronological. An empty ts means the record carried none, and the
        // live reader only applied the cutoff when both were present.
        if let Some(cutoff) = since_iso {
            if !r.ts.is_empty() && r.ts.as_str() < cutoff {
                continue;
            }
        }
        // Records without a cwd have no honest bucket to attribute them to.
        if r.cwd.is_empty() {
            continue;
        }
        let is_new_session_for_cwd = session_cwds.insert(r.cwd.clone());
        let entry = by_cwd.entry(r.cwd.clone()).or_insert_with(|| ProjectUsage {
            cwd: r.cwd.clone(),
            ..Default::default()
        });
        if is_new_session_for_cwd {
            entry.sessions = entry.sessions.saturating_add(1);
        }

        for u in &r.tool_uses {
            entry.tools.bump(&u.name);
            if !u.id.is_empty() {
                tool_id_to_name.insert(u.id.clone(), u.name.clone());
            }
        }
        for res in &r.tool_results {
            let tool = if res.use_id.is_empty() {
                "unknown".to_string()
            } else {
                tool_id_to_name
                    .get(&res.use_id)
                    .cloned()
                    .unwrap_or_else(|| "unknown".to_string())
            };
            // tokenade's own MCP responses ARE tokenade output, not compaction
            // opportunities, so they stay out of the comparable denominator —
            // while remaining in the per-tool bucket for display.
            let is_tokenade_provided = tool.starts_with("mcp__tokenade__")
                || tool.starts_with("mcp__plugin_tokenade_tokenade__")
                || tool.starts_with("mcp__supertokensaver__")
                || tool.starts_with("mcp__sts__");
            if !is_tokenade_provided {
                entry.tool_result_tokens = entry.tool_result_tokens.saturating_add(res.tokens);
            }
            *entry.tool_result_tokens_by_tool.entry(tool).or_insert(0) += res.tokens;
        }

        // `user` records contribute tool_result accounting only.
        if !r.assistant {
            continue;
        }
        if !r.usage_is_object {
            continue;
        }
        if !dedup.admit_key(r.dedup_key) {
            continue;
        }

        entry.input_tokens = entry.input_tokens.saturating_add(r.input);
        entry.output_tokens = entry.output_tokens.saturating_add(r.output);
        entry.cache_creation_tokens = entry.cache_creation_tokens.saturating_add(r.cache_creation);
        entry.cache_read_tokens = entry.cache_read_tokens.saturating_add(r.cache_read);
        entry.assistant_messages = entry.assistant_messages.saturating_add(1);

        if !r.ts.is_empty() && r.ts.as_str() > entry.last_ts.as_str() {
            entry.last_ts = r.ts.clone();
        }
    }
}

/// One [`SessionModel`](crate::agent_transcripts::SessionModel) per
/// (session-file, model) pair, with the message-timestamp span for each.
/// The file stem is the Claude session id (dash-carrying UUID; matched
/// against a gain row's stripped `sess` via `norm_sess`).
pub fn session_models(
    home_dir: &Path,
    since_iso: Option<&str>,
) -> Vec<crate::agent_transcripts::SessionModel> {
    use crate::agent_transcripts::SessionModel;
    let mut acc: std::collections::BTreeMap<(String, String), (u64, u64)> = Default::default();
    let projects_root = crate::platform::claude_config_dir_for(home_dir).join("projects");
    crate::transcript_cache::with_shared(home_dir, |cache| {
        for path in jsonl_transcripts(&projects_root) {
            let Some(sess) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            for r in cache.rows_for(&path) {
                if !r.assistant || r.ts.is_empty() {
                    continue;
                }
                if let Some(cutoff) = since_iso {
                    if r.ts.as_str() < cutoff {
                        continue;
                    }
                }
                if r.model.is_empty() || r.model == "<synthetic>" {
                    continue;
                }
                let Some(ms) = iso8601_to_epoch_ms(&r.ts) else {
                    continue;
                };
                let e = acc
                    .entry((sess.clone(), r.model.clone()))
                    .or_insert((ms, ms));
                if ms < e.0 {
                    e.0 = ms;
                }
                if ms > e.1 {
                    e.1 = ms;
                }
            }
        }
    });
    acc.into_iter()
        .map(|((sess, model), (start_ms, end_ms))| SessionModel {
            sess,
            model,
            start_ms,
            end_ms,
        })
        .collect()
}
