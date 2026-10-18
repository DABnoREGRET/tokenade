//! xAI Grok Build CLI transcript parser.
//!
//! Layout (validated against a real `~/.grok` install, 2026-05):
//! ```text
//! ~/.grok/sessions/<url-encoded-cwd>/
//!   prompt_history.jsonl
//!   <session-uuid>/
//!     chat_history.jsonl   ← messages (NO per-message ts, NO token usage)
//!     events.jsonl         ← lifecycle events WITH ISO `ts`
//!     summary.json         ← { info:{id,cwd}, created_at, updated_at,
//!                              current_model_id, num_chat_messages }
//!     signals.json         ← contextTokensUsed / contextWindowTokens (gauge)
//!     subagents/<uuid>/…   ← nested, same shape
//! ```
//!
//! `chat_history.jsonl` records (validated):
//!  - `{"type":"assistant","content":…,"model_id":…,"raw_output":[
//!       {"type":"function_call","name":"read_file","call_id":"call-…"}, …]}`
//!  - `{"type":"tool_result","tool_call_id":"call-…","content":"<output>"}`
//!  - `user` / `system`.
//!
//! What Grok records vs. what we can attribute:
//!  - **Tool-output volume per tool** — YES. We join `function_call.call_id`
//!    → `tool_result.tool_call_id` and estimate the result body with the
//!    tokenizer (same approach every parser uses for tool_result tokens).
//!    This is the bypass-panel signal (`tool_result_tokens_by_tool`).
//!  - **Tool-call breakdown** — YES (one bump per `function_call.name`).
//!  - **Model / cwd / timestamps** — YES (`summary.json`).
//!  - **Per-call input/output/cache billing** — NO. Grok writes only a
//!    context-window gauge (`signals.json`) and subagent token totals, never
//!    a per-turn usage block. So `input/output/cache` stay 0 and Grok is
//!    absent from the per-model panel — see `coverage_notes`. We do NOT
//!    fabricate a split from the gauge.
//!
//! Timestamps: `chat_history.jsonl` has no per-record ts, so per-day bucketing
//! uses the session's `summary.json` `updated_at` day. Sessions are typically
//! same-day; this is the honest granularity Grok's on-disk format allows.

use crate::agent_transcripts::{Agent, ProjectUsage, ToolTokensByDay, TranscriptParser};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub struct Parser;

impl TranscriptParser for Parser {
    fn agent(&self) -> Agent {
        Agent::Grok
    }

    fn aggregate_since(&self, home_dir: &Path, since_iso: Option<&str>) -> Vec<ProjectUsage> {
        let mut by_cwd: HashMap<String, ProjectUsage> = HashMap::new();
        for s in sessions(home_dir) {
            if let (Some(cutoff), Some(ts)) = (since_iso, s.updated_at.as_deref()) {
                if ts < cutoff {
                    continue;
                }
            }
            let parsed = parse_chat(&s.chat_path);
            if parsed.is_empty() {
                continue;
            }
            let e = by_cwd.entry(s.cwd.clone()).or_insert_with(|| ProjectUsage {
                cwd: s.cwd.clone(),
                ..Default::default()
            });
            e.sessions = e.sessions.saturating_add(1);
            e.assistant_messages = e.assistant_messages.saturating_add(parsed.assistant_msgs);
            e.tool_result_tokens = e
                .tool_result_tokens
                .saturating_add(parsed.total_tool_tokens);
            for (tool, n) in parsed.tool_tokens {
                *e.tool_result_tokens_by_tool.entry(tool).or_insert(0) += n;
            }
            for tool in parsed.tool_calls {
                e.tools.bump(&tool);
            }
            if let Some(ts) = s.updated_at.as_deref() {
                if ts > e.last_ts.as_str() {
                    e.last_ts = ts.to_string();
                }
            }
        }
        let mut out: Vec<ProjectUsage> = by_cwd.into_values().collect();
        out.sort_by(|a, b| b.last_ts.cmp(&a.last_ts));
        out
    }

    fn tool_tokens_by_day(&self, home_dir: &Path, since_iso: Option<&str>) -> ToolTokensByDay {
        let mut out = ToolTokensByDay::default();
        for s in sessions(home_dir) {
            let Some(ts) = s.updated_at.as_deref() else {
                continue;
            };
            if let Some(cutoff) = since_iso {
                if ts < cutoff {
                    continue;
                }
            }
            if !ts.is_char_boundary(10) {
                continue;
            }
            let day = ts[..10].to_string();
            let parsed = parse_chat(&s.chat_path);
            if parsed.tool_tokens.is_empty() {
                continue;
            }
            let day_entry = out.entry(day).or_default();
            for (tool, n) in parsed.tool_tokens {
                *day_entry.entry(tool).or_insert(0) += n;
            }
        }
        out
    }

    // model_usage_by_day intentionally left as the default (empty): Grok records
    // no per-call input/output token billing, so it contributes nothing to the
    // per-model panel rather than a misleading all-zero row.
}

/// One Grok session's locating info.
struct Session {
    cwd: String,
    updated_at: Option<String>,
    chat_path: PathBuf,
}

/// Enumerate every `chat_history.jsonl` under `~/.grok/sessions` (main +
/// nested subagents), pairing each with its sibling `summary.json` for cwd +
/// `updated_at`. Falls back to the URL-decoded session-group dir name for cwd.
fn sessions(home_dir: &Path) -> Vec<Session> {
    let root = home_dir.join(".grok").join("sessions");
    let mut out = Vec::new();
    let Ok(groups) = fs::read_dir(&root) else {
        return out;
    };
    for group in groups.flatten() {
        let gpath = group.path();
        if !gpath.is_dir() {
            continue;
        }
        let group_cwd = crate::text_encoding::percent_decode(&group.file_name().to_string_lossy());
        collect_chats(&gpath, &group_cwd, &mut out);
    }
    out
}

/// Recursively find `chat_history.jsonl` files under `dir`, reading the sibling
/// `summary.json` for the authoritative cwd + `updated_at`.
fn collect_chats(dir: &Path, fallback_cwd: &str, out: &mut Vec<Session>) {
    let chat = dir.join("chat_history.jsonl");
    if chat.is_file() {
        let (cwd, updated_at) = read_summary(&dir.join("summary.json"))
            .unwrap_or_else(|| (fallback_cwd.to_string(), None));
        out.push(Session {
            cwd,
            updated_at,
            chat_path: chat,
        });
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_chats(&p, fallback_cwd, out);
        }
    }
}

/// `(cwd, updated_at)` from a session `summary.json`.
fn read_summary(path: &Path) -> Option<(String, Option<String>)> {
    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
    let cwd = v["info"]["cwd"].as_str()?.to_string();
    let updated = v["updated_at"]
        .as_str()
        .or_else(|| v["last_active_at"].as_str())
        .or_else(|| v["created_at"].as_str())
        .map(|s| s.to_string());
    Some((cwd, updated))
}

#[derive(Default)]
struct ParsedChat {
    tool_tokens: HashMap<String, u64>,
    total_tool_tokens: u64,
    tool_calls: Vec<String>,
    assistant_msgs: u64,
}
impl ParsedChat {
    fn is_empty(&self) -> bool {
        self.assistant_msgs == 0 && self.tool_tokens.is_empty()
    }
}

/// Single streaming pass over a `chat_history.jsonl`: assistant records seed a
/// `call_id → tool_name` map from their `raw_output[].function_call` entries
/// (which always precede the matching `tool_result`); each `tool_result`
/// charges its tokenizer-estimated body to that tool.
fn parse_chat(path: &Path) -> ParsedChat {
    let mut out = ParsedChat::default();
    let Ok(file) = fs::File::open(path) else {
        return out;
    };
    let mut id_to_name: HashMap<String, String> = HashMap::new();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        match v["type"].as_str() {
            Some("assistant") => {
                out.assistant_msgs += 1;
                if let Some(raw) = v["raw_output"].as_array() {
                    for entry in raw {
                        if entry["type"].as_str() == Some("function_call") {
                            if let Some(name) = entry["name"].as_str() {
                                out.tool_calls.push(name.to_string());
                                if let Some(id) = entry["call_id"].as_str() {
                                    id_to_name.insert(id.to_string(), name.to_string());
                                }
                            }
                        }
                    }
                }
            }
            Some("tool_result") => {
                let body = v["content"].as_str().unwrap_or("");
                if body.is_empty() {
                    continue;
                }
                let toks = crate::tokenizer::estimate(body) as u64;
                let tool = v["tool_call_id"]
                    .as_str()
                    .and_then(|id| id_to_name.get(id).cloned())
                    .unwrap_or_else(|| "unknown".to_string());
                *out.tool_tokens.entry(tool).or_insert(0) += toks;
                out.total_tool_tokens = out.total_tool_tokens.saturating_add(toks);
            }
            _ => {}
        }
    }
    out
}

// Minimal percent-decoder for the URL-encoded cwd that names a session group
// dir (`%2Fhome%2Fu%2Fproj` → `/home/u/proj`). Leaves malformed escapes as-is.

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_home(label: &str) -> PathBuf {
        let p =
            std::env::temp_dir().join(format!("tokenade-grok-{}-{}", label, std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn write_session(home: &Path, enc_cwd: &str, uuid: &str, chat: &[&str], summary: &str) {
        let dir = home.join(".grok").join("sessions").join(enc_cwd).join(uuid);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("chat_history.jsonl"), chat.join("\n")).unwrap();
        fs::write(dir.join("summary.json"), summary).unwrap();
    }

    // Records below are trimmed copies of REAL Grok chat_history lines.
    const ASSISTANT: &str = r#"{"type":"assistant","content":"ok","model_id":"grok-build","raw_output":[{"type":"reasoning","id":"rs_1"},{"type":"function_call","name":"read_file","call_id":"call-1","id":"fc_1"},{"type":"function_call","name":"run_terminal_command","call_id":"call-2","id":"fc_2"}]}"#;
    const TR1: &str = r#"{"type":"tool_result","tool_call_id":"call-1","content":"line one\nline two\nline three of file body"}"#;
    const TR2: &str =
        r#"{"type":"tool_result","tool_call_id":"call-2","content":"command stdout output here"}"#;

    fn summary(cwd: &str, updated: &str) -> String {
        format!(
            r#"{{"info":{{"id":"u1","cwd":"{cwd}"}},"updated_at":"{updated}","current_model_id":"grok-build"}}"#
        )
    }

    #[test]
    fn grok_attributes_tool_output_per_tool() {
        let home = tmp_home("attr");
        write_session(
            &home,
            "%2Fhome%2Fu%2Fproj",
            "019e64e6",
            &[ASSISTANT, TR1, TR2],
            &summary("/home/u/proj", "2026-05-26T21:05:48.935Z"),
        );
        let rows = Parser.aggregate_since(&home, None);
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r.cwd, "/home/u/proj");
        assert!(r.tool_result_tokens_by_tool.contains_key("read_file"));
        assert!(r
            .tool_result_tokens_by_tool
            .contains_key("run_terminal_command"));
        assert!(r.tool_result_tokens > 0);
        // No fabricated billing.
        assert_eq!(r.input_tokens, 0);
        assert_eq!(r.output_tokens, 0);
        // Tool-call breakdown counted both function_calls.
        assert_eq!(r.tools.total(), 2);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn grok_tool_tokens_by_day_buckets_on_summary_day_with_cutoff() {
        let home = tmp_home("day");
        write_session(
            &home,
            "%2Fp",
            "s1",
            &[ASSISTANT, TR1, TR2],
            &summary("/p", "2026-05-26T10:00:00Z"),
        );
        write_session(
            &home,
            "%2Fp",
            "s2",
            &[ASSISTANT, TR1],
            &summary("/p", "2026-06-10T10:00:00Z"),
        );
        let all = Parser.tool_tokens_by_day(&home, None);
        assert!(all.contains_key("2026-05-26"));
        assert!(all.contains_key("2026-06-10"));
        // Cutoff drops the earlier session.
        let win = Parser.tool_tokens_by_day(&home, Some("2026-06-01T00:00:00Z"));
        assert!(!win.contains_key("2026-05-26"));
        assert!(win.contains_key("2026-06-10"));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn url_decode_handles_encoded_path() {
        assert_eq!(
            crate::text_encoding::percent_decode("%2Fhome%2Fu%2Fp"),
            "/home/u/p"
        );
        assert_eq!(crate::text_encoding::percent_decode("plain"), "plain");
    }

    /// Real-data validation against the developer's actual `~/.grok` install.
    /// Ignored by default (machine-specific); run with
    /// `cargo test -p tokenade-core agent_grok::tests::real_grok -- --ignored --nocapture`.
    #[test]
    #[ignore = "machine-specific: the developer's real `grok` install"]
    fn real_grok_smoke() {
        let home = PathBuf::from(std::env::var("HOME").expect("HOME"));
        let rows = Parser.aggregate_since(&home, None);
        eprintln!("== grok aggregate: {} cwd bucket(s) ==", rows.len());
        for r in &rows {
            let mut top: Vec<_> = r.tool_result_tokens_by_tool.iter().collect();
            top.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
            eprintln!(
                "  cwd={} sessions={} tool_calls={} tool_tokens={} top={:?}",
                r.cwd,
                r.sessions,
                r.tools.total(),
                r.tool_result_tokens,
                top.into_iter().take(5).collect::<Vec<_>>()
            );
        }
        let byday = Parser.tool_tokens_by_day(&home, None);
        eprintln!("== grok tool_tokens_by_day: {} day(s) ==", byday.len());
        for (d, tools) in byday.iter().take(8) {
            eprintln!("  {d}: {} tokens", tools.values().sum::<u64>());
        }
        assert!(!rows.is_empty(), "expected ≥1 grok session on this machine");
    }
}
