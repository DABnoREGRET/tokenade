//! OpenClaw (`openclaw`) transcript parser.
//!
//! Layout (validated against a real session generated locally, OpenClaw
//! 2026.1.30): one JSONL file per session at
//! `~/.openclaw/agents/<agentId>/sessions/<sessionId>.jsonl`. Each line is a
//! `{ "type": …, … }` record:
//!
//!  - `{"type":"session","cwd":"/abs/path","timestamp":"ISO","id":…}` — one
//!    per file; gives the cwd every message in the file is attributed to.
//!  - `{"type":"model_change","provider":"openrouter","modelId":"…"}`.
//!  - `{"type":"message","timestamp":"ISO","message":{…}}` where `message.role`
//!    is one of:
//!     - `assistant`: `{role, content:[{type:"toolCall",name,…}|{type:"text"}],
//!        model, provider, usage:{input,output,cacheRead,cacheWrite,totalTokens},
//!        stopReason}` — full per-turn token billing.
//!     - `toolResult`: `{role:"toolResult", toolCallId, toolName,
//!        content:[{type:"text",text}], details, isError}` — the tool name is
//!        recorded inline (no id→name join needed).
//!     - `user`: `{role, content:[{type:"text"}]}`.
//!
//! Token mapping follows the shared convention: `usage.input` → fresh input,
//! `usage.cacheWrite` → cache_creation, `usage.cacheRead` → cache_read
//! (excluded from totals), `usage.output` → output. Per-model "fresh input" is
//! `input + cacheWrite`. cwd comes from the session record; per-day bucketing
//! and the install-date cutoff use each message record's ISO `timestamp`.

use crate::agent_transcripts::{
    Agent, ModelUsageByDay, ProjectUsage, ToolTokensByDay, TranscriptParser,
};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub struct Parser;

impl TranscriptParser for Parser {
    fn agent(&self) -> Agent {
        Agent::Openclaw
    }

    fn aggregate_since(&self, home_dir: &Path, since_iso: Option<&str>) -> Vec<ProjectUsage> {
        let mut by_cwd: HashMap<String, ProjectUsage> = HashMap::new();
        for file in session_files(home_dir) {
            let Some(cwd) = session_cwd(&file) else {
                continue;
            };
            let mut touched = false;
            for_each_message(&file, since_iso, |rec, msg| {
                let e = by_cwd.entry(cwd.clone()).or_insert_with(|| ProjectUsage {
                    cwd: cwd.clone(),
                    ..Default::default()
                });
                match msg["role"].as_str() {
                    Some("assistant") => {
                        let u = &msg["usage"];
                        e.input_tokens = e
                            .input_tokens
                            .saturating_add(u["input"].as_u64().unwrap_or(0));
                        e.output_tokens = e
                            .output_tokens
                            .saturating_add(u["output"].as_u64().unwrap_or(0));
                        e.cache_read_tokens = e
                            .cache_read_tokens
                            .saturating_add(u["cacheRead"].as_u64().unwrap_or(0));
                        e.cache_creation_tokens = e
                            .cache_creation_tokens
                            .saturating_add(u["cacheWrite"].as_u64().unwrap_or(0));
                        e.assistant_messages = e.assistant_messages.saturating_add(1);
                        if let Some(blocks) = msg["content"].as_array() {
                            for b in blocks {
                                if b["type"].as_str() == Some("toolCall") {
                                    if let Some(name) = b["name"].as_str() {
                                        e.tools.bump(name);
                                    }
                                }
                            }
                        }
                        touched = true;
                    }
                    Some("toolResult") => {
                        let toks = tool_result_tokens(msg);
                        if toks > 0 {
                            let tool = msg["toolName"].as_str().unwrap_or("unknown").to_string();
                            *e.tool_result_tokens_by_tool.entry(tool).or_insert(0) += toks;
                            e.tool_result_tokens = e.tool_result_tokens.saturating_add(toks);
                            touched = true;
                        }
                    }
                    _ => {}
                }
                if let Some(ts) = rec["timestamp"].as_str() {
                    if ts > e.last_ts.as_str() {
                        e.last_ts = ts.to_string();
                    }
                }
            });
            if touched {
                if let Some(e) = by_cwd.get_mut(&cwd) {
                    e.sessions = e.sessions.saturating_add(1);
                }
            }
        }
        let mut out: Vec<ProjectUsage> = by_cwd.into_values().collect();
        out.sort_by(|a, b| b.last_ts.cmp(&a.last_ts));
        out
    }

    fn model_usage_by_day(&self, home_dir: &Path, since_iso: Option<&str>) -> ModelUsageByDay {
        let mut out = ModelUsageByDay::default();
        for file in session_files(home_dir) {
            for_each_message(&file, since_iso, |rec, msg| {
                if msg["role"].as_str() != Some("assistant") {
                    return;
                }
                let Some(model) = msg["model"].as_str() else {
                    return;
                };
                let Some(ts) = rec["timestamp"].as_str() else {
                    return;
                };
                if !ts.is_char_boundary(10) {
                    return;
                }
                let u = &msg["usage"];
                let fresh =
                    u["input"].as_u64().unwrap_or(0) + u["cacheWrite"].as_u64().unwrap_or(0);
                let outp = u["output"].as_u64().unwrap_or(0);
                let day = out.entry(ts[..10].to_string()).or_default();
                let e = day.entry(model.to_string()).or_insert((0, 0, 0));
                e.0 += fresh;
                e.1 += outp;
                e.2 += 1;
            });
        }
        out
    }

    fn session_models(
        &self,
        home_dir: &Path,
        since_iso: Option<&str>,
    ) -> Vec<crate::agent_transcripts::SessionModel> {
        // Same file discovery and record selection as model_usage_by_day
        // (assistant messages with a `model`, cutoff on each record's ISO ts)
        // but accumulating the min/max timestamp span in epoch millis. The
        // session id is the transcript file stem — OpenClaw's `<sessionId>.jsonl`.
        let mut acc: std::collections::BTreeMap<(String, String), (u64, u64)> = Default::default();
        for file in session_files(home_dir) {
            let Some(sess) = file
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            for_each_message(&file, since_iso, |rec, msg| {
                if msg["role"].as_str() != Some("assistant") {
                    return;
                }
                let Some(model) = msg["model"].as_str() else {
                    return;
                };
                let Some(ts) = rec["timestamp"].as_str() else {
                    return;
                };
                if !ts.is_char_boundary(10) {
                    return;
                }
                let Some(ms) = crate::claudecode_transcript::iso8601_to_epoch_ms(ts) else {
                    return;
                };
                let e = acc
                    .entry((sess.clone(), model.to_string()))
                    .or_insert((ms, ms));
                if ms < e.0 {
                    e.0 = ms;
                }
                if ms > e.1 {
                    e.1 = ms;
                }
            });
        }
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

    fn tool_tokens_by_day(&self, home_dir: &Path, since_iso: Option<&str>) -> ToolTokensByDay {
        let mut out = ToolTokensByDay::default();
        for file in session_files(home_dir) {
            for_each_message(&file, since_iso, |rec, msg| {
                if msg["role"].as_str() != Some("toolResult") {
                    return;
                }
                let Some(ts) = rec["timestamp"].as_str() else {
                    return;
                };
                if !ts.is_char_boundary(10) {
                    return;
                }
                let toks = tool_result_tokens(msg);
                if toks == 0 {
                    return;
                }
                let tool = msg["toolName"].as_str().unwrap_or("unknown").to_string();
                *out.entry(ts[..10].to_string())
                    .or_default()
                    .entry(tool)
                    .or_insert(0) += toks;
            });
        }
        out
    }
}

/// Sum the tokenizer estimate of every text block in a `toolResult` message.
fn tool_result_tokens(msg: &serde_json::Value) -> u64 {
    msg["content"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|b| b["text"].as_str())
                .map(|s| crate::tokenizer::estimate(s) as u64)
                .sum()
        })
        .unwrap_or(0)
}

/// Every `~/.openclaw/agents/<id>/sessions/*.jsonl` transcript file.
fn session_files(home_dir: &Path) -> Vec<PathBuf> {
    let agents = home_dir.join(".openclaw").join("agents");
    let mut out = Vec::new();
    let Ok(agent_dirs) = fs::read_dir(&agents) else {
        return out;
    };
    for a in agent_dirs.flatten() {
        let sdir = a.path().join("sessions");
        let Ok(files) = fs::read_dir(&sdir) else {
            continue;
        };
        for f in files.flatten() {
            let p = f.path();
            if p.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                out.push(p);
            }
        }
    }
    out
}

/// The `cwd` from the file's `session` record (first one wins).
fn session_cwd(file: &Path) -> Option<String> {
    let f = fs::File::open(file).ok()?;
    for line in BufReader::new(f).lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if v["type"].as_str() == Some("session") {
            if let Some(cwd) = v["cwd"].as_str() {
                return Some(cwd.to_string());
            }
        }
    }
    None
}

/// Stream `message` records from one transcript, applying the install-date
/// cutoff on the record's ISO `timestamp`. `visit(record, message)` gets both
/// the outer record (for `timestamp`) and the inner `message` object.
fn for_each_message(
    file: &Path,
    since_iso: Option<&str>,
    mut visit: impl FnMut(&serde_json::Value, &serde_json::Value),
) {
    let Ok(f) = fs::File::open(file) else {
        return;
    };
    for line in BufReader::new(f).lines().map_while(Result::ok) {
        let Ok(rec) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if rec["type"].as_str() != Some("message") {
            continue;
        }
        if let (Some(cutoff), Some(ts)) = (since_iso, rec["timestamp"].as_str()) {
            if ts < cutoff {
                continue;
            }
        }
        let msg = &rec["message"];
        if msg.is_object() {
            visit(&rec, msg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_home(label: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "tokenade-openclaw-{}-{}",
            label,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn write_session(home: &Path, agent: &str, sid: &str, lines: &[&str]) {
        let dir = home
            .join(".openclaw")
            .join("agents")
            .join(agent)
            .join("sessions");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{sid}.jsonl")), lines.join("\n")).unwrap();
    }

    // Records below are trimmed copies of a REAL OpenClaw 2026.1.30 session.
    const SESSION: &str = r#"{"type":"session","version":3,"id":"s1","timestamp":"2026-06-29T17:21:09.965Z","cwd":"/work/proj"}"#;
    const MODEL: &str = r#"{"type":"model_change","provider":"openrouter","modelId":"openai/gpt-4o-mini","timestamp":"2026-06-29T17:21:09.970Z"}"#;
    const ASSISTANT_TOOLCALL: &str = r#"{"type":"message","timestamp":"2026-06-29T17:21:09.977Z","message":{"role":"assistant","content":[{"type":"toolCall","id":"call_1","name":"exec","arguments":{"command":"cat README.txt"}}],"model":"openai/gpt-4o-mini","provider":"openrouter","usage":{"input":9564,"output":19,"cacheRead":0,"cacheWrite":0,"totalTokens":9583},"stopReason":"toolUse"}}"#;
    const TOOLRESULT: &str = r#"{"type":"message","timestamp":"2026-06-29T17:21:11.670Z","message":{"role":"toolResult","toolCallId":"call_1","toolName":"exec","content":[{"type":"text","text":"tokenade-marker-line-1\nsecret-needle-42\ntokenade-marker-line-3"}],"isError":false}}"#;
    const ASSISTANT_TEXT: &str = r#"{"type":"message","timestamp":"2026-06-29T17:21:11.671Z","message":{"role":"assistant","content":[{"type":"text","text":"The needle is 42."}],"model":"openai/gpt-4o-mini","provider":"openrouter","usage":{"input":137,"output":15,"cacheRead":9472,"cacheWrite":0,"totalTokens":9624},"stopReason":"stop"}}"#;

    #[test]
    fn openclaw_full_token_and_tool_attribution() {
        let home = tmp_home("full");
        write_session(
            &home,
            "tkval",
            "s1",
            &[
                SESSION,
                MODEL,
                ASSISTANT_TOOLCALL,
                TOOLRESULT,
                ASSISTANT_TEXT,
            ],
        );
        let rows = Parser.aggregate_since(&home, None);
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r.cwd, "/work/proj");
        // input = 9564 + 137 ; output = 19 + 15 ; cache_read = 0 + 9472.
        assert_eq!(r.input_tokens, 9564 + 137);
        assert_eq!(r.output_tokens, 19 + 15);
        assert_eq!(r.cache_read_tokens, 9472);
        assert_eq!(r.assistant_messages, 2);
        // exec tool output attributed by toolName.
        assert!(r.tool_result_tokens_by_tool.contains_key("exec"));
        assert!(r.tool_result_tokens > 0);
        assert_eq!(r.tools.total(), 1, "one toolCall (exec)");

        // model panel: fresh input = input + cacheWrite, on the message day.
        let m = Parser.model_usage_by_day(&home, None);
        let day = &m["2026-06-29"]["openai/gpt-4o-mini"];
        assert_eq!(day.0, 9564 + 137);
        assert_eq!(day.1, 19 + 15);
        assert_eq!(day.2, 2);

        // bypass-panel feed.
        let td = Parser.tool_tokens_by_day(&home, None);
        assert!(td["2026-06-29"].contains_key("exec"));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn openclaw_cutoff_filters_old_messages() {
        let home = tmp_home("cutoff");
        write_session(&home, "a", "s1", &[SESSION, ASSISTANT_TEXT]);
        // cutoff after the message → dropped.
        let rows = Parser.aggregate_since(&home, Some("2026-07-01T00:00:00Z"));
        assert!(rows.is_empty());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn openclaw_missing_root_is_empty() {
        let home = tmp_home("empty");
        assert!(Parser.aggregate_since(&home, None).is_empty());
        assert!(Parser.model_usage_by_day(&home, None).is_empty());
        let _ = fs::remove_dir_all(&home);
    }

    /// Real-data validation against the session this parser's author generated
    /// locally. Ignored by default (machine-specific); run with
    /// `cargo test -p tokenade-core agent_openclaw::tests::real_openclaw -- --ignored --nocapture`.
    #[test]
    #[ignore = "machine-specific: the developer's real `openclaw` install"]
    fn real_openclaw_smoke() {
        let home = PathBuf::from(std::env::var("HOME").expect("HOME"));
        let rows = Parser.aggregate_since(&home, None);
        eprintln!("== openclaw aggregate: {} cwd bucket(s) ==", rows.len());
        for r in &rows {
            eprintln!(
                "  cwd={} sessions={} in={} out={} cache_read={} tools={} tool_tokens={} by_tool={:?}",
                r.cwd,
                r.sessions,
                r.input_tokens,
                r.output_tokens,
                r.cache_read_tokens,
                r.tools.total(),
                r.tool_result_tokens,
                r.tool_result_tokens_by_tool
            );
        }
        assert!(
            !rows.is_empty(),
            "expected ≥1 openclaw session on this machine"
        );
    }
}
