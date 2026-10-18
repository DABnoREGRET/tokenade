//! Pi (earendil-works `@earendil-works/pi-coding-agent`) transcript parser.
//!
//! Sessions are JSONL trees under `~/.pi/agent/sessions/--<cwd-with-/-as->--/
//! <timestamp>_<uuid>.jsonl`. The real cwd is in the header line, so we read it
//! from there rather than decoding the directory name:
//!   `{"type":"session","version":3,"id":"…","timestamp":"ISO","cwd":"/path"}`
//! Conversation lines wrap an `AgentMessage`:
//!   `{"type":"message","timestamp":"ISO","message":{"role":"assistant",
//!     "model":"…","usage":{"input","output","cacheRead","cacheWrite",
//!     "totalTokens"}, …}}`
//!   `{"type":"message", …, "message":{"role":"toolResult","toolName":"…",
//!     "content":[{"type":"text","text":"…"}], …}}`
//!
//! Mapping to [`ProjectUsage`]: Pi `cacheWrite` is Claude's
//! `cache_creation_tokens`. Per-toolResult token counts aren't stored, so
//! `tool_result_tokens` is estimated from the result text. Timestamps are ISO
//! strings → `since` is a lexicographic compare (monotonic).

use crate::agent_transcripts::{Agent, ModelUsageByDay, ProjectUsage, TranscriptParser};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub struct Parser;

/// A pi session's API calls and compactions, from its JSONL (`ctx.sessionManager
/// .getSessionFile()`), with the model it ran last. Each assistant reply with
/// a usage is one call; the context it sent is `input + cacheRead +
/// cacheWrite` (a failed call reports zero and is skipped). A `compaction`
/// entry is a compaction. `None` when unreadable or without a call.
pub fn session_timeline_from_file(
    path: &Path,
) -> Option<(
    crate::claudecode_transcript::SessionTimeline,
    Option<String>,
)> {
    if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
        return None;
    }
    let f = fs::File::open(path).ok()?;
    let mut tl = crate::claudecode_transcript::SessionTimeline::default();
    let mut model = None;
    for line in BufReader::new(f).lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let Some(ts) = v["timestamp"]
            .as_str()
            .and_then(crate::claudecode_transcript::iso8601_to_epoch_ms)
            .map(|ms| ms / 1000)
        else {
            continue;
        };
        match v["type"].as_str() {
            Some("compaction") => tl
                .compactions
                .push((ts, v["tokensBefore"].as_u64().unwrap_or(0))),
            Some("message") if v["message"]["role"] == "assistant" => {
                let u = &v["message"]["usage"];
                let ctx = ["input", "cacheRead", "cacheWrite"]
                    .iter()
                    .map(|k| u[k].as_u64().unwrap_or(0))
                    .sum::<u64>();
                if ctx > 0 {
                    tl.push_call(ts, ctx, u["cacheWrite"].as_u64().unwrap_or(0));
                    model = v["message"]["model"].as_str().map(str::to_string).or(model);
                }
            }
            _ => {}
        }
    }
    (!tl.calls.is_empty()).then_some((tl, model))
}

fn sessions_root(home: &Path) -> PathBuf {
    home.join(".pi").join("agent").join("sessions")
}

/// Concatenate the text of an `AgentMessage.content` array (ignoring images).
fn content_text(content: &serde_json::Value) -> String {
    let Some(arr) = content.as_array() else {
        return content.as_str().unwrap_or("").to_string();
    };
    let mut s = String::new();
    for b in arr {
        if b.get("type").and_then(|t| t.as_str()) == Some("text") {
            if let Some(t) = b.get("text").and_then(|t| t.as_str()) {
                s.push_str(t);
            }
        }
    }
    s
}

fn for_each_session_file(root: &Path, mut f: impl FnMut(&Path)) {
    let Ok(dirs) = fs::read_dir(root) else { return };
    for d in dirs.flatten() {
        let p = d.path();
        if !p.is_dir() {
            continue;
        }
        if let Ok(files) = fs::read_dir(&p) {
            for file in files.flatten() {
                let fp = file.path();
                if fp.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                    f(&fp);
                }
            }
        }
    }
}

fn scan_session(path: &Path, by_cwd: &mut HashMap<String, ProjectUsage>, since: Option<&str>) {
    let Ok(file) = fs::File::open(path) else {
        return;
    };
    let mut cwd: Option<String> = None;
    let mut counted = false;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("session") => {
                cwd = v.get("cwd").and_then(|c| c.as_str()).map(str::to_string);
            }
            Some("message") => {
                let Some(cwd_s) = cwd.clone().filter(|c| !c.is_empty()) else {
                    continue;
                };
                let ts = v.get("timestamp").and_then(|t| t.as_str()).unwrap_or("");
                if let Some(cut) = since {
                    if ts < cut {
                        continue;
                    }
                }
                let msg = &v["message"];
                let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
                let e = by_cwd.entry(cwd_s.clone()).or_default();
                e.cwd = cwd_s;
                if !counted {
                    e.sessions += 1;
                    counted = true;
                }
                if ts > e.last_ts.as_str() {
                    e.last_ts = ts.to_string();
                }
                match role {
                    "assistant" => {
                        e.assistant_messages += 1;
                        let u = &msg["usage"];
                        let g = |k: &str| u.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
                        e.input_tokens += g("input");
                        e.output_tokens += g("output");
                        e.cache_read_tokens += g("cacheRead");
                        e.cache_creation_tokens += g("cacheWrite");
                    }
                    "toolResult" => {
                        let tool = msg
                            .get("toolName")
                            .and_then(|t| t.as_str())
                            .unwrap_or("?")
                            .to_string();
                        let toks =
                            crate::tokenizer::estimate(&content_text(&msg["content"])) as u64;
                        e.tool_result_tokens += toks;
                        *e.tool_result_tokens_by_tool.entry(tool).or_insert(0) += toks;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

impl TranscriptParser for Parser {
    fn agent(&self) -> Agent {
        Agent::Pi
    }

    fn aggregate_since(&self, home_dir: &Path, since_iso: Option<&str>) -> Vec<ProjectUsage> {
        let mut by_cwd: HashMap<String, ProjectUsage> = HashMap::new();
        for_each_session_file(&sessions_root(home_dir), |p| {
            scan_session(p, &mut by_cwd, since_iso);
        });
        let mut out: Vec<ProjectUsage> = by_cwd.into_values().collect();
        out.sort_by(|a, b| b.last_ts.cmp(&a.last_ts));
        out
    }

    fn model_usage_by_day(&self, home_dir: &Path, since_iso: Option<&str>) -> ModelUsageByDay {
        let mut out = ModelUsageByDay::default();
        for_each_session_file(&sessions_root(home_dir), |path| {
            let Ok(file) = fs::File::open(path) else {
                return;
            };
            for line in BufReader::new(file).lines().map_while(Result::ok) {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
                    continue;
                };
                if v.get("type").and_then(|t| t.as_str()) != Some("message") {
                    continue;
                }
                let ts = v.get("timestamp").and_then(|t| t.as_str()).unwrap_or("");
                if let Some(cut) = since_iso {
                    if ts < cut {
                        continue;
                    }
                }
                let msg = &v["message"];
                if msg.get("role").and_then(|r| r.as_str()) != Some("assistant") {
                    continue;
                }
                let Some(model) = msg
                    .get("model")
                    .and_then(|m| m.as_str())
                    .filter(|m| !m.is_empty())
                else {
                    continue;
                };
                let day = ts.get(..10).unwrap_or("").to_string();
                let u = &msg["usage"];
                let g = |k: &str| u.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
                let e = out
                    .entry(day)
                    .or_default()
                    .entry(model.to_string())
                    .or_insert((0, 0, 0));
                e.0 += g("input");
                e.1 += g("output");
                e.2 += 1;
            }
        });
        out
    }

    fn session_models(
        &self,
        home_dir: &Path,
        since_iso: Option<&str>,
    ) -> Vec<crate::agent_transcripts::SessionModel> {
        // Same record walk as model_usage_by_day (assistant messages with a
        // non-empty `model`, cutoff on each record's ISO `timestamp`) but
        // accumulating the min/max timestamp span in epoch millis. The session
        // id is the transcript file stem — Pi's `<timestamp>_<uuid>.jsonl`,
        // the unit `aggregate_since` counts as one session.
        let mut acc: std::collections::BTreeMap<(String, String), (u64, u64)> = Default::default();
        for_each_session_file(&sessions_root(home_dir), |path| {
            let Some(sess) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string)
            else {
                return;
            };
            let Ok(file) = fs::File::open(path) else {
                return;
            };
            for line in BufReader::new(file).lines().map_while(Result::ok) {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
                    continue;
                };
                if v.get("type").and_then(|t| t.as_str()) != Some("message") {
                    continue;
                }
                let ts = v.get("timestamp").and_then(|t| t.as_str()).unwrap_or("");
                if let Some(cut) = since_iso {
                    if ts < cut {
                        continue;
                    }
                }
                let msg = &v["message"];
                if msg.get("role").and_then(|r| r.as_str()) != Some("assistant") {
                    continue;
                }
                let Some(model) = msg
                    .get("model")
                    .and_then(|m| m.as_str())
                    .filter(|m| !m.is_empty())
                else {
                    continue;
                };
                let Some(ms) = crate::claudecode_transcript::iso8601_to_epoch_ms(ts) else {
                    continue;
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
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(dir: &Path) {
        // sessions/--home-proj--/0001_uuid.jsonl
        let sdir = dir.join(".pi/agent/sessions/--home-proj--");
        fs::create_dir_all(&sdir).unwrap();
        let lines = [
            r#"{"type":"session","version":3,"id":"u","timestamp":"2026-06-01T00:00:00.000Z","cwd":"/home/proj"}"#,
            r#"{"type":"message","id":"a1","parentId":null,"timestamp":"2026-06-01T00:00:01.000Z","message":{"role":"user","content":"hi"}}"#,
            r#"{"type":"message","id":"a2","parentId":"a1","timestamp":"2026-06-01T00:00:02.000Z","message":{"role":"assistant","content":[{"type":"text","text":"ok"}],"model":"claude-x","usage":{"input":100,"output":40,"cacheRead":900,"cacheWrite":80,"totalTokens":1120}}}"#,
            r#"{"type":"message","id":"a3","parentId":"a2","timestamp":"2026-06-01T00:00:03.000Z","message":{"role":"toolResult","toolName":"read_file","content":[{"type":"text","text":"some file body here"}],"isError":false}}"#,
            r#"{"type":"message","id":"a4","parentId":"a3","timestamp":"2025-01-01T00:00:00.000Z","message":{"role":"assistant","content":[{"type":"text","text":"old"}],"model":"old-model","usage":{"input":5000,"output":5000,"cacheRead":0,"cacheWrite":0,"totalTokens":10000}}}"#,
        ];
        fs::write(sdir.join("0001_uuid.jsonl"), lines.join("\n")).unwrap();
    }

    #[test]
    fn aggregates_from_header_cwd_and_usage() {
        let dir = std::env::temp_dir().join(format!("tk-pi-parse-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        seed(&dir);

        let usage = Parser.aggregate_since(&dir, None);
        assert_eq!(usage.len(), 1);
        let p = &usage[0];
        assert_eq!(p.cwd, "/home/proj");
        assert_eq!(p.sessions, 1);
        assert_eq!(p.assistant_messages, 2); // a2 + a4
        assert_eq!(p.input_tokens, 5100); // 100 + 5000
        assert_eq!(p.cache_read_tokens, 900);
        assert_eq!(p.cache_creation_tokens, 80); // cacheWrite mapped
        assert!(p.tool_result_tokens > 0, "read_file result estimated");
        assert!(p.tool_result_tokens_by_tool.contains_key("read_file"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cutoff_filters_and_models() {
        let dir = std::env::temp_dir().join(format!("tk-pi-parse2-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        seed(&dir);

        let usage = Parser.aggregate_since(&dir, Some("2026-01-01T00:00:00Z"));
        let p = &usage[0];
        assert_eq!(p.input_tokens, 100, "2025 assistant msg filtered out");
        assert_eq!(p.assistant_messages, 1);

        let m = Parser.model_usage_by_day(&dir, Some("2026-01-01T00:00:00Z"));
        assert!(m["2026-06-01"].contains_key("claude-x"));
        assert!(!m.values().any(|d| d.contains_key("old-model")));
        let _ = fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod timeline_tests {
    use super::*;

    /// Lines as pi writes them: a failed call (zero usage) is not a call, a
    /// `compaction` entry is a compaction.
    #[test]
    fn a_session_file_gives_its_calls_and_compactions() {
        let dir = std::env::temp_dir().join(format!("tk-pi-tl-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("s.jsonl");
        let msg = |ts: &str, input: u64, read: u64| {
            format!(
                r#"{{"type":"message","timestamp":"{ts}","message":{{"role":"assistant","model":"claude-opus-5","usage":{{"input":{input},"output":3,"cacheRead":{read},"cacheWrite":5}}}}}}"#
            )
        };
        let lines = [
            r#"{"type":"session","version":3,"id":"abc","timestamp":"2026-09-28T11:53:53.292Z"}"#.to_string(),
            r#"{"type":"message","timestamp":"2026-09-28T11:53:54.000Z","message":{"role":"user","content":[]}}"#.to_string(),
            msg("2026-09-28T11:53:55.000Z", 0, 0).replace(r#""cacheWrite":5"#, r#""cacheWrite":0"#),
            msg("2026-09-28T11:53:56.000Z", 100, 0),
            r#"{"type":"compaction","timestamp":"2026-09-28T11:54:00.000Z","summary":"s","firstKeptEntryId":"x","tokensBefore":900}"#.to_string(),
            msg("2026-09-28T11:54:01.000Z", 20, 30),
        ];
        fs::write(&p, lines.join("\n") + "\n").unwrap();
        let (tl, model) = session_timeline_from_file(&p).unwrap();
        let t = 1_790_596_436u64; // 2026-09-28T11:53:56Z
        assert_eq!(tl.calls, vec![(t, 105), (t + 5, 55)]);
        assert_eq!(tl.compactions, vec![(t + 4, 900)]);
        assert_eq!(model.as_deref(), Some("claude-opus-5"));
        assert!(session_timeline_from_file(&dir.join("s.json")).is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
