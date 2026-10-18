//! Codex CLI (OpenAI) transcript parser.
//!
//! Codex writes per-session JSONL to
//! `~/.codex/sessions/YYYY/MM/DD/<rollout-id>.jsonl`. Schema (observed
//! on cli_version 0.130.0-alpha.5 in May 2026):
//!
//! ```text
//! {"timestamp":"2026-05-14T13:47:02.988Z","type":"session_meta",
//!  "payload":{"id":"…","cwd":"/path","model_provider":"openai",…}}
//! {"timestamp":"…","type":"turn_context","payload":{…}}
//! {"timestamp":"…","type":"response_item",
//!  "payload":{"type":"function_call","name":"shell","arguments":"…"}}
//! {"timestamp":"…","type":"event_msg",
//!  "payload":{"info":{"total_token_usage":{
//!    "input_tokens":14419,"cached_input_tokens":12672,
//!    "output_tokens":118,"reasoning_output_tokens":0}}}}
//! ```
//!
//! Key differences vs Claude Code:
//!   - `cwd` is nested under `payload`, not top-level
//!   - `total_token_usage` is CUMULATIVE per session — we take the
//!     final value rather than summing per-message deltas
//!   - Tool calls live in `response_item` records with
//!     `payload.type == "function_call"` and `payload.name` = tool
//!   - Reasoning tokens are a separate bucket (folded into output)

use crate::agent_transcripts::{Agent, ProjectUsage, ToolBreakdown, TranscriptParser};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;

pub struct Parser;

impl TranscriptParser for Parser {
    fn agent(&self) -> Agent {
        Agent::Codex
    }

    fn aggregate_since(&self, home_dir: &Path, since_iso: Option<&str>) -> Vec<ProjectUsage> {
        let root = home_dir.join(".codex").join("sessions");
        let mut by_cwd: HashMap<String, ProjectUsage> = HashMap::new();
        // Walk YYYY/MM/DD/*.jsonl. We're depth-3 deep; rather than
        // pulling walkdir we hand-roll it (cheap, no extra dep).
        walk_jsonl(&root, 3, &mut |path| {
            scan_session(path, &mut by_cwd, since_iso);
        });
        let mut out: Vec<ProjectUsage> = by_cwd.into_values().collect();
        out.sort_by(|a, b| b.last_ts.cmp(&a.last_ts));
        out
    }

    fn model_usage_by_day(
        &self,
        home_dir: &Path,
        since_iso: Option<&str>,
    ) -> crate::agent_transcripts::ModelUsageByDay {
        let root = home_dir.join(".codex").join("sessions");
        let mut out = crate::agent_transcripts::ModelUsageByDay::default();
        walk_jsonl(&root, 3, &mut |path| {
            scan_session_models(path, &mut out, since_iso);
        });
        out
    }

    fn session_models(
        &self,
        home_dir: &Path,
        since_iso: Option<&str>,
    ) -> Vec<crate::agent_transcripts::SessionModel> {
        let root = home_dir.join(".codex").join("sessions");
        let mut acc: std::collections::BTreeMap<(String, String), (u64, u64)> = Default::default();
        walk_jsonl(&root, 3, &mut |path| {
            scan_session_model_spans(path, &mut acc, since_iso);
        });
        acc.into_iter()
            .map(
                |((sess, model), (start_ms, end_ms))| crate::agent_transcripts::SessionModel {
                    sess,
                    model,
                    start_ms,
                    end_ms,
                },
            )
            .collect()
    }
}

/// Walk one Codex session file and hand every non-zero token delta to
/// `on_delta`, with the model in force and the record's ISO timestamp.
///
/// Two scanners carried this loop verbatim, and one of them said so in its own
/// doc comment ("mirrors `scan_session_models`'s cumulative-delta walk
/// exactly"). The part that must not drift is the arithmetic: Codex reports
/// `total_token_usage` CUMULATIVELY, so each record contributes its difference
/// against the previous one. A second copy computing that difference slightly
/// differently would over- or under-count tokens with nothing to show for it.
fn for_each_usage_delta(
    path: &Path,
    since_iso: Option<&str>,
    mut on_delta: impl FnMut(&str, &str, u64, u64),
) {
    let Ok(file) = fs::File::open(path) else {
        return;
    };
    let mut model: Option<String> = None;
    let mut prev = TokenSums::default();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        match v["type"].as_str().unwrap_or("") {
            "session_meta" | "turn_context" => {
                if let Some(m) = v["payload"]["model"].as_str() {
                    if !m.is_empty() {
                        model = Some(m.to_string());
                    }
                }
            }
            "event_msg" => {
                let Some(usage) = v["payload"]["info"]["total_token_usage"].as_object() else {
                    continue;
                };
                let get = |k: &str| usage.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
                let cur = TokenSums {
                    input: get("input_tokens"),
                    cached: get("cached_input_tokens"),
                    output: get("output_tokens").saturating_add(get("reasoning_output_tokens")),
                };
                let d_fresh = cur
                    .input
                    .saturating_sub(cur.cached)
                    .saturating_sub(prev.input.saturating_sub(prev.cached));
                let d_out = cur.output.saturating_sub(prev.output);
                prev = cur;

                let ts = v["timestamp"].as_str().unwrap_or("");
                if !ts.is_char_boundary(10) || since_iso.is_some_and(|c| ts < c) {
                    continue;
                }
                let Some(m) = model.as_deref() else {
                    continue;
                };
                if d_fresh == 0 && d_out == 0 {
                    continue;
                }
                on_delta(m, ts, d_fresh, d_out);
            }
            _ => {}
        }
    }
}

/// Per-(session-file, model) timestamp span. Mirrors [`scan_session_models`]'s
/// cumulative-delta walk exactly (same model tracking, same delta filtering,
/// same cutoff) but records the min/max `event_msg` timestamp in epoch millis
/// instead of day-bucketed tokens. The session id is the rollout file stem —
/// Codex's "one session per JSONL file" identity.
fn scan_session_model_spans(
    path: &Path,
    acc: &mut std::collections::BTreeMap<(String, String), (u64, u64)>,
    since_iso: Option<&str>,
) {
    let Some(sess) = path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(str::to_string)
    else {
        return;
    };
    for_each_usage_delta(path, since_iso, |m, ts, _d_fresh, _d_out| {
        let Some(ms) = crate::claudecode_transcript::iso8601_to_epoch_ms(ts) else {
            return;
        };
        let e = acc.entry((sess.clone(), m.to_string())).or_insert((ms, ms));
        if ms < e.0 {
            e.0 = ms;
        }
        if ms > e.1 {
            e.1 = ms;
        }
    });
}

/// Per-model pass over one Codex session. The model name comes from
/// `turn_context.payload.model` (or `session_meta.payload.model` when
/// present); `total_token_usage` is CUMULATIVE, so each `event_msg`
/// contributes its delta vs the previous one, attributed to the model
/// active at that point. Fresh input excludes `cached_input_tokens`
/// (the cache-read equivalent), matching the Claude Code convention.
fn scan_session_models(
    path: &Path,
    out: &mut crate::agent_transcripts::ModelUsageByDay,
    since_iso: Option<&str>,
) {
    for_each_usage_delta(path, since_iso, |m, ts, d_fresh, d_out| {
        let e = out
            .entry(ts[..10].to_string())
            .or_default()
            .entry(m.to_string())
            .or_insert((0, 0, 0));
        e.0 = e.0.saturating_add(d_fresh);
        e.1 = e.1.saturating_add(d_out);
        e.2 = e.2.saturating_add(1);
    });
}

/// DFS walk that calls `visit` on every `*.jsonl` file up to
/// `max_depth` directories below `root`. Silent on permission errors.
fn walk_jsonl(root: &Path, max_depth: usize, visit: &mut dyn FnMut(&Path)) {
    fn recurse(dir: &Path, depth_left: usize, visit: &mut dyn FnMut(&Path)) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                if depth_left > 0 {
                    recurse(&p, depth_left - 1, visit);
                }
            } else if p.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                visit(&p);
            }
        }
    }
    recurse(root, max_depth, visit);
}

fn scan_session(path: &Path, by_cwd: &mut HashMap<String, ProjectUsage>, since_iso: Option<&str>) {
    let Ok(file) = fs::File::open(path) else {
        return;
    };
    let reader = BufReader::new(file);

    let mut cwd: Option<String> = None;
    let mut last_ts = String::new();
    let mut last_total = TokenSums::default();
    let mut tools = ToolBreakdown::default();
    let mut messages: u64 = 0;

    for line in reader.lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let kind = v["type"].as_str().unwrap_or("");
        let ts = v["timestamp"].as_str().unwrap_or("");

        if !ts.is_empty() && ts > last_ts.as_str() {
            last_ts = ts.to_string();
        }

        match kind {
            "session_meta"
                if cwd.is_none() => {
                    if let Some(c) = v["payload"]["cwd"].as_str() {
                        cwd = Some(c.to_string());
                    }
                }
            "response_item"
                // tool_use equivalent — count by `payload.name`.
                // We classify Codex tool names into the shared
                // `ToolBreakdown` buckets so the dashboard's
                // bypass-eligible totals stay comparable across
                // agents.
                if v["payload"]["type"].as_str() == Some("function_call") => {
                    if let Some(name) = v["payload"]["name"].as_str() {
                        bump_codex(&mut tools, name);
                    }
                }
            "event_msg" => {
                // Cumulative token usage — overwrites each time.
                if let Some(usage) = v["payload"]["info"]["total_token_usage"].as_object() {
                    last_total = TokenSums {
                        input: usage
                            .get("input_tokens")
                            .and_then(|x| x.as_u64())
                            .unwrap_or(0),
                        cached: usage
                            .get("cached_input_tokens")
                            .and_then(|x| x.as_u64())
                            .unwrap_or(0),
                        output: usage
                            .get("output_tokens")
                            .and_then(|x| x.as_u64())
                            .unwrap_or(0)
                            .saturating_add(
                                usage
                                    .get("reasoning_output_tokens")
                                    .and_then(|x| x.as_u64())
                                    .unwrap_or(0),
                            ),
                    };
                    messages = messages.saturating_add(1);
                }
            }
            _ => {}
        }
    }

    let Some(cwd) = cwd else {
        return;
    };

    // Apply the cutoff using the LAST timestamp we saw. If the whole
    // session ended before the cutoff, we skip it entirely. We can't
    // do per-message cutoffs here because token totals are cumulative
    // (taking a partial cumulative number would over-count).
    if since_iso.is_some_and(|cutoff| last_ts.as_str() < cutoff) {
        return;
    }

    let entry = by_cwd.entry(cwd.clone()).or_insert_with(|| ProjectUsage {
        cwd,
        ..Default::default()
    });
    entry.sessions = entry.sessions.saturating_add(1);
    // Codex's total_token_usage is cumulative per session, so each
    // session contributes its FINAL total — we just sum across
    // sessions for the project.
    // Codex reports `input_tokens` INCLUSIVE of `cached_input_tokens`, where
    // Anthropic reports the two as disjoint buckets. Measured 2026-08-26 over
    // this machine's seven real Codex sessions: 15 189 653 input of which
    // 14 811 904 cached — **97.5 %** — so storing the raw figure made Codex
    // look 35.8× more input-heavy than it is, on a dashboard that puts it
    // beside Claude Code. `scan_session_models` in this same file has always
    // decomposed (`d_fresh = input - cached`, "matching the Claude Code
    // convention"); the per-project path did not, so the two views of one
    // session disagreed by the whole cached amount.
    entry.input_tokens = entry
        .input_tokens
        .saturating_add(last_total.input.saturating_sub(last_total.cached));
    entry.output_tokens = entry.output_tokens.saturating_add(last_total.output);
    // `cached_input_tokens` is Codex's equivalent of Anthropic's
    // `cache_read_input_tokens` — free reuse of an already-paid
    // prompt. Goes into `cache_read_tokens` so `total_tokens()`
    // (which excludes cache_read) gives the comparable billing
    // figure.
    entry.cache_read_tokens = entry.cache_read_tokens.saturating_add(last_total.cached);
    entry.assistant_messages = entry.assistant_messages.saturating_add(messages);
    if last_ts > entry.last_ts {
        entry.last_ts = last_ts;
    }
    entry.tools += tools;
}

#[derive(Default)]
struct TokenSums {
    input: u64,
    cached: u64,
    output: u64,
}

/// Map a Codex `function_call.name` into our shared
/// [`ToolBreakdown`] buckets. Codex's vocab is shell-centric:
///   - `shell`, `local_shell`, `exec` → bash
///   - `apply_patch`, `write_file`, `edit_file` → edit
///   - `read_file`, `view_file` → read
///   - `search`, `grep`, `ripgrep` → grep
///   - anything starting with `tokenade_` → tokenade
fn bump_codex(b: &mut ToolBreakdown, name: &str) {
    // Tokenade MCP namespace works whether the agent invokes us via
    // `mcp_tokenade_*` (Codex MCP) or `tokenade_*` (direct tool).
    //
    // #490: also recognise legacy `supertokensaver` / `sts` namespaces
    // — see the claudecode_transcript bump() comment for rationale.
    if name.starts_with("mcp_tokenade_")
        || name.starts_with("tokenade_")
        || name.starts_with("mcp_supertokensaver_")
        || name.starts_with("supertokensaver_")
        || name.starts_with("mcp_sts_")
        || name.starts_with("sts_")
    {
        b.tokenade = b.tokenade.saturating_add(1);
        return;
    }
    match name {
        "shell" | "local_shell" | "exec" | "exec_command" => b.bash = b.bash.saturating_add(1),
        "apply_patch" | "write_file" | "edit_file" | "create_file" => {
            b.edit = b.edit.saturating_add(1)
        }
        "read_file" | "view_file" | "cat" => b.read = b.read.saturating_add(1),
        "search" | "grep" | "ripgrep" | "rg" => b.grep = b.grep.saturating_add(1),
        "glob" | "find" => b.glob = b.glob.saturating_add(1),
        "fetch" | "web_search" | "browse" => b.web = b.web.saturating_add(1),
        _ => b.other = b.other.saturating_add(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_home(label: &str) -> std::path::PathBuf {
        let d =
            std::env::temp_dir().join(format!("tokenade-codex-{}-{}", label, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_session(home: &Path, ymd: (u32, u32, u32), file: &str, lines: &[&str]) {
        let dir = home
            .join(".codex")
            .join("sessions")
            .join(format!("{:04}", ymd.0))
            .join(format!("{:02}", ymd.1))
            .join(format!("{:02}", ymd.2));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(file), lines.join("\n")).unwrap();
    }

    #[test]
    fn aggregates_codex_session_with_cumulative_total() {
        let home = tmp_home("aggregate");
        write_session(
            &home,
            (2026, 5, 14),
            "rollout-a.jsonl",
            &[
                r#"{"timestamp":"2026-05-14T10:00:00Z","type":"session_meta","payload":{"id":"a","cwd":"/p"}}"#,
                r#"{"timestamp":"2026-05-14T10:00:05Z","type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{}"}}"#,
                r#"{"timestamp":"2026-05-14T10:00:06Z","type":"event_msg","payload":{"info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":50,"output_tokens":20,"reasoning_output_tokens":5}}}}"#,
                r#"{"timestamp":"2026-05-14T10:00:10Z","type":"response_item","payload":{"type":"function_call","name":"apply_patch","arguments":"{}"}}"#,
                // Later cumulative — overrides the earlier one.
                r#"{"timestamp":"2026-05-14T10:00:12Z","type":"event_msg","payload":{"info":{"total_token_usage":{"input_tokens":200,"cached_input_tokens":80,"output_tokens":55,"reasoning_output_tokens":10}}}}"#,
            ],
        );
        let stats = Parser.aggregate_since(&home, None);
        assert_eq!(stats.len(), 1);
        let p = &stats[0];
        assert_eq!(p.cwd, "/p");
        assert_eq!(
            p.input_tokens, 120,
            "takes the LAST cumulative input total (200), minus the cached part of \
             it (80) — Codex counts cache reads inside `input_tokens`"
        );
        assert_eq!(
            p.output_tokens, 65,
            "output 55 + reasoning 10 folded together"
        );
        assert_eq!(p.cache_read_tokens, 80, "cached → cache_read bucket");
        assert_eq!(p.tools.bash, 1, "`shell` bucketed as bash");
        assert_eq!(p.tools.edit, 1, "`apply_patch` bucketed as edit");
        assert_eq!(p.sessions, 1);
        assert_eq!(p.last_ts, "2026-05-14T10:00:12Z");
    }

    #[test]
    fn since_cutoff_skips_old_sessions_entirely() {
        let home = tmp_home("cutoff");
        write_session(
            &home,
            (2026, 1, 1),
            "old.jsonl",
            &[
                r#"{"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{"id":"o","cwd":"/p"}}"#,
                r#"{"timestamp":"2026-01-01T00:00:05Z","type":"event_msg","payload":{"info":{"total_token_usage":{"input_tokens":99999,"output_tokens":99999}}}}"#,
            ],
        );
        write_session(
            &home,
            (2026, 5, 14),
            "new.jsonl",
            &[
                r#"{"timestamp":"2026-05-14T00:00:00Z","type":"session_meta","payload":{"id":"n","cwd":"/p"}}"#,
                r#"{"timestamp":"2026-05-14T00:00:05Z","type":"event_msg","payload":{"info":{"total_token_usage":{"input_tokens":10,"output_tokens":5}}}}"#,
            ],
        );
        let stats = Parser.aggregate_since(&home, Some("2026-03-01T00:00:00Z"));
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].input_tokens, 10, "old session must be excluded");
        assert_eq!(stats[0].output_tokens, 5);
    }

    #[test]
    fn model_usage_attributes_cumulative_deltas_to_current_model() {
        let home = tmp_home("models");
        write_session(
            &home,
            (2026, 5, 14),
            "rollout-m.jsonl",
            &[
                r#"{"timestamp":"2026-05-14T10:00:00Z","type":"session_meta","payload":{"id":"a","cwd":"/p"}}"#,
                r#"{"timestamp":"2026-05-14T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5-codex"}}"#,
                // fresh = (100-50) - 0 = 50, out = 25
                r#"{"timestamp":"2026-05-14T10:00:06Z","type":"event_msg","payload":{"info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":50,"output_tokens":20,"reasoning_output_tokens":5}}}}"#,
                // model switch mid-session
                r#"{"timestamp":"2026-05-14T10:00:08Z","type":"turn_context","payload":{"model":"gpt-5"}}"#,
                // fresh = (200-80) - (100-50) = 70, out = 65-25 = 40
                r#"{"timestamp":"2026-05-14T10:00:12Z","type":"event_msg","payload":{"info":{"total_token_usage":{"input_tokens":200,"cached_input_tokens":80,"output_tokens":55,"reasoning_output_tokens":10}}}}"#,
            ],
        );
        let m = Parser.model_usage_by_day(&home, None);
        let day = &m["2026-05-14"];
        assert_eq!(day["gpt-5-codex"], (50, 25, 1));
        assert_eq!(day["gpt-5"], (70, 40, 1));

        // Cutoff after the first event keeps only the second delta.
        let m2 = Parser.model_usage_by_day(&home, Some("2026-05-14T10:00:10Z"));
        assert!(!m2["2026-05-14"].contains_key("gpt-5-codex"));
        assert_eq!(m2["2026-05-14"]["gpt-5"], (70, 40, 1));
    }

    #[test]
    fn model_usage_skips_sessions_without_model_record() {
        let home = tmp_home("models-nomodel");
        write_session(
            &home,
            (2026, 5, 14),
            "rollout-n.jsonl",
            &[
                r#"{"timestamp":"2026-05-14T10:00:00Z","type":"session_meta","payload":{"id":"a","cwd":"/p"}}"#,
                r#"{"timestamp":"2026-05-14T10:00:06Z","type":"event_msg","payload":{"info":{"total_token_usage":{"input_tokens":100,"output_tokens":20}}}}"#,
            ],
        );
        assert!(
            Parser.model_usage_by_day(&home, None).is_empty(),
            "no model recorded → contribute nothing, never guess"
        );
    }

    #[test]
    fn malformed_or_missing_files_dont_panic() {
        let home = tmp_home("malformed");
        let stats = Parser.aggregate_since(&home, None);
        assert!(stats.is_empty(), "missing ~/.codex returns empty");
    }

    #[test]
    fn tokenade_namespace_classified_correctly() {
        let mut tb = ToolBreakdown::default();
        bump_codex(&mut tb, "mcp_tokenade_exec_compact");
        bump_codex(&mut tb, "tokenade_symbol_find");
        bump_codex(&mut tb, "shell");
        bump_codex(&mut tb, "random_other");
        assert_eq!(tb.tokenade, 2);
        assert_eq!(tb.bash, 1);
        assert_eq!(tb.other, 1);
    }
}

#[cfg(test)]
mod codex_input_is_reported_cache_free {
    //! Codex's `input_tokens` INCLUDES `cached_input_tokens`; Anthropic's
    //! excludes its `cache_read_input_tokens`. Storing the raw Codex figure
    //! next to a Claude Code one compares two different quantities.

    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn tmp(label: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "tokenade-codexcache-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn session(home: &Path, lines: &[&str]) {
        let dir = home.join(".codex/sessions/2026/05/14");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("rollout-x.jsonl"), lines.join("\n")).unwrap();
    }

    const META: &str = r#"{"timestamp":"2026-05-14T10:00:00Z","type":"session_meta","payload":{"id":"a","cwd":"/w","model":"gpt-5"}}"#;

    #[test]
    fn the_cached_part_of_input_does_not_land_in_the_input_bucket() {
        let home = tmp("split");
        session(
            &home,
            &[
                META,
                r#"{"timestamp":"2026-05-14T10:00:06Z","type":"event_msg","payload":{"info":{"total_token_usage":{"input_tokens":1000,"cached_input_tokens":975,"output_tokens":40}}}}"#,
            ],
        );
        let rows = Parser.aggregate_since(&home, None);
        let p = rows.iter().find(|r| r.cwd == "/w").expect("row");
        assert_eq!(p.input_tokens, 25, "1000 reported, 975 of it cached");
        assert_eq!(
            p.cache_read_tokens, 975,
            "the cached part keeps its own bucket"
        );
        assert_eq!(
            p.input_tokens + p.cache_read_tokens,
            1000,
            "the two buckets must still add back to what Codex reported"
        );
    }

    #[test]
    fn a_session_with_no_cache_is_unchanged() {
        let home = tmp("nocache");
        session(
            &home,
            &[
                META,
                r#"{"timestamp":"2026-05-14T10:00:06Z","type":"event_msg","payload":{"info":{"total_token_usage":{"input_tokens":300,"output_tokens":40}}}}"#,
            ],
        );
        let rows = Parser.aggregate_since(&home, None);
        let p = rows.iter().find(|r| r.cwd == "/w").expect("row");
        assert_eq!(p.input_tokens, 300);
        assert_eq!(p.cache_read_tokens, 0);
    }

    #[test]
    fn the_project_view_now_agrees_with_the_per_model_view() {
        // `scan_session_models` has always subtracted the cached part. The
        // two views of one session disagreed by exactly that amount.
        let home = tmp("agree");
        session(
            &home,
            &[
                META,
                r#"{"timestamp":"2026-05-14T10:00:06Z","type":"event_msg","payload":{"info":{"total_token_usage":{"input_tokens":1000,"cached_input_tokens":975,"output_tokens":40}}}}"#,
            ],
        );
        let by_project = Parser
            .aggregate_since(&home, None)
            .iter()
            .find(|r| r.cwd == "/w")
            .unwrap()
            .input_tokens;
        let by_model: u64 = Parser
            .model_usage_by_day(&home, None)
            .values()
            .flat_map(|m| m.values())
            .map(|c| c.0)
            .sum();
        assert_eq!(by_project, by_model, "one session, one input figure");
    }
}
