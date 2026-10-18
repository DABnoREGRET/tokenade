//! **The context at fire time: what an avoided turn would have re-read.**
//!
//! A batch of N targets served in ONE command spares the agent N−1 turns. The
//! batch credit (`batching_cache_saved`) only counted the RESULTS a following
//! turn would have re-sent; yet every turn re-reads the whole conversation
//! context at the cache price. Measured 2026-10-05 over 30 days of local
//! transcripts: that term is worth **×125** the results term.
//!
//! The context is read without estimating: Claude Code gives every Bash
//! command `CLAUDE_CODE_SESSION_ID`, and the message that fired the command is
//! ALREADY written in the session transcript, with its `usage` (checked on a
//! real session: last assistant record = the call in progress, 310 078
//! context tokens). That count is in BILLED tokens, not in
//! `tokenizer::estimate` units — so the row is not converted at valuation
//! (see `gain::weigh`).
//!
//! Codex (`CODEX_THREAD_ID`, the id of its rollout file — checked on a real
//! rollout) writes a call's `token_count` AFTER its tool outputs: at fire
//! time, the last one is that of the PREVIOUS call. Its `input_tokens` (cache
//! included, OpenAI convention) is therefore a LOWER bound of the context at
//! fire time — the direction in which a credit may err.
//!
//! Outside Claude Code and Codex, or when the last message is not the tokenade
//! call (a sub-agent whose command is written elsewhere), nothing is read and
//! nothing is credited: a context other than the one at fire time would be a
//! made-up number.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Bytes read from the end of the transcript: the firing message is the
/// last assistant record, a few KB from the end.
const TAIL: u64 = 512 * 1024;

/// The context (billed tokens: input + cache read + cache write) of the
/// assistant message that fired this tokenade command, when it can be read.
pub fn context_at_fire() -> Option<u64> {
    fired().map(|f| f.context)
}

/// The firing call, when it can be read: its context and how many tool calls
/// it made.
pub struct Fired {
    pub context: u64,
    pub tool_calls: usize,
}

/// [`context_at_fire`], plus the number of tool calls the firing message
/// made: a call that ran tokenade ALONGSIDE others was not added by it.
pub fn fired() -> Option<Fired> {
    claude_code_fired().or_else(codex_fired)
}

fn claude_code_fired() -> Option<Fired> {
    let sid = std::env::var("CLAUDE_CODE_SESSION_ID").ok()?;
    if sid.len() < 8 || !sid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return None;
    }
    let home = tk_base::platform::home_dir()?;
    let projects = tk_base::platform::claude_config_dir_for(&home).join("projects");
    let file = transcript_of(&projects, &sid)?;
    firing_call(&tail(&file)?)
}

fn codex_fired() -> Option<Fired> {
    let id = std::env::var("CODEX_THREAD_ID").ok()?;
    if id.len() < 8 || !id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return None;
    }
    let root = match std::env::var_os("CODEX_HOME") {
        Some(h) => PathBuf::from(h),
        None => tk_base::platform::home_dir()?.join(".codex"),
    };
    codex_firing_call(&tail(&rollout_of(&root.join("sessions"), &id)?)?)
}

/// `sessions/YYYY/MM/DD/rollout-<timestamp>-<id>.jsonl`, most recent days
/// first: a session in progress is almost always there.
fn rollout_of(sessions: &Path, id: &str) -> Option<PathBuf> {
    let suffix = format!("-{id}.jsonl");
    let desc = |p: &Path| {
        let mut v: Vec<PathBuf> = std::fs::read_dir(p)
            .into_iter()
            .flatten()
            .flatten()
            .map(|d| d.path())
            .collect();
        v.sort_unstable_by(|a, b| b.cmp(a));
        v
    };
    desc(sessions)
        .iter()
        .flat_map(|y| desc(y))
        .flat_map(|m| desc(&m))
        .flat_map(|d| desc(&d))
        .find(|f| {
            f.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(&suffix))
        })
}

/// The previous call's context (last `token_count`) and the tool calls the
/// firing response made (those written after it), provided one runs tokenade.
pub(crate) fn codex_firing_call(tail: &str) -> Option<Fired> {
    let (mut tool_calls, mut fires_tokenade) = (0usize, false);
    for line in tail.lines().rev() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let p = &v["payload"];
        match p["type"].as_str() {
            Some("token_count") => {
                let context = p["info"]["last_token_usage"]["input_tokens"]
                    .as_u64()
                    .unwrap_or(0);
                return (fires_tokenade && context > 0).then_some(Fired {
                    context,
                    tool_calls,
                });
            }
            Some("function_call" | "custom_tool_call") => {
                tool_calls += 1;
                let args = p["arguments"]
                    .as_str()
                    .or_else(|| p["input"].as_str())
                    .unwrap_or("");
                fires_tokenade |= args.contains("tokenade");
            }
            // A compaction since: that count measured a context now gone.
            Some("context_compacted") => return None,
            _ if v["type"] == "compacted" => return None,
            _ => {}
        }
    }
    None
}

fn transcript_of(projects: &Path, sid: &str) -> Option<PathBuf> {
    std::fs::read_dir(projects)
        .ok()?
        .flatten()
        .map(|d| d.path().join(format!("{sid}.jsonl")))
        .find(|p| p.is_file())
}

fn tail(file: &Path) -> Option<String> {
    let mut f = std::fs::File::open(file).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(TAIL))).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// The last assistant record's context, provided that record is the call
/// running tokenade.
#[cfg(test)]
pub(crate) fn firing_context(tail: &str) -> Option<u64> {
    firing_call(tail).map(|f| f.context)
}

/// Claude Code writes one record per content block of a message, all under
/// the same `message.id`: the firing message is the last assistant record
/// and every record before it with its id.
pub(crate) fn firing_call(tail: &str) -> Option<Fired> {
    let mut id: Option<String> = None;
    let mut fires_tokenade = false;
    let mut tool_calls = 0usize;
    let mut context = 0u64;
    for line in tail.lines().rev() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v["type"] != "assistant" {
            if id.is_some() && v["type"] == "user" {
                break;
            }
            continue;
        }
        let mid = v["message"]["id"].as_str().unwrap_or("").to_string();
        match &id {
            None => {
                let u = &v["message"]["usage"];
                context = [
                    "input_tokens",
                    "cache_read_input_tokens",
                    "cache_creation_input_tokens",
                ]
                .iter()
                .map(|k| u[k].as_u64().unwrap_or(0))
                .sum::<u64>();
                id = Some(mid);
            }
            Some(first) if *first != mid || mid.is_empty() => break,
            Some(_) => {}
        }
        for b in v["message"]["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|b| b["type"] == "tool_use")
        {
            tool_calls += 1;
            fires_tokenade |= b["input"]["command"]
                .as_str()
                .is_some_and(|c| c.contains("tokenade"));
        }
    }
    (fires_tokenade && context > 0).then_some(Fired {
        context,
        tool_calls,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codex(t: &str, extra: &str) -> String {
        serde_json::json!({"type": "event_msg", "payload": serde_json::from_str::<serde_json::Value>(&format!("{{\"type\":\"{t}\"{extra}}}")).unwrap()}).to_string()
    }

    /// Codex writes a call's `token_count` after its tool outputs: at fire,
    /// the last one is the previous call's — a lower bound of the context.
    #[test]
    fn a_codex_call_reads_the_previous_call_s_context() {
        let usage =
            r#","info":{"last_token_usage":{"input_tokens":13008,"cached_input_tokens":10112}}"#;
        let call = |cmd: &str| {
            serde_json::json!({"type": "response_item", "payload": {"type": "function_call", "arguments": serde_json::json!({"cmd": cmd}).to_string()}}).to_string()
        };
        let t = [codex("token_count", usage), call("tokenade expand-ref abc")].join("\n");
        let f = codex_firing_call(&t).expect("fired");
        assert_eq!((f.context, f.tool_calls), (13_008, 1));
        let two = [
            codex("token_count", usage),
            call("ls"),
            call("tokenade expand-ref abc"),
        ]
        .join("\n");
        assert_eq!(codex_firing_call(&two).map(|f| f.tool_calls), Some(2));
        let other = [codex("token_count", usage), call("ls")].join("\n");
        assert!(codex_firing_call(&other).is_none(), "not fired by tokenade");
        let next_turn = [
            codex("token_count", usage),
            codex("user_message", ""),
            call("tokenade web a b"),
        ]
        .join("\n");
        assert_eq!(
            codex_firing_call(&next_turn).map(|f| f.context),
            Some(13_008),
            "still a lower bound"
        );
        let compacted = [
            codex("token_count", usage),
            codex("context_compacted", ""),
            call("tokenade web a b"),
        ]
        .join("\n");
        assert!(
            codex_firing_call(&compacted).is_none(),
            "measured before a compaction"
        );
    }

    fn rec(cmd: &str, ctx: u64) -> String {
        serde_json::json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "input": {"command": cmd}}],
            "usage": {"input_tokens": 3, "cache_read_input_tokens": ctx - 3, "cache_creation_input_tokens": 0}}})
        .to_string()
    }

    #[test]
    fn the_context_is_read_off_the_message_that_fired_tokenade() {
        let t = format!(
            "{}\n{{\"type\":\"user\"}}\n{}\n",
            rec("ls", 1_000),
            rec("tokenade web a b", 250_000)
        );
        assert_eq!(firing_context(&t), Some(250_000));
    }

    /// A subagent's command is not in the parent's transcript: the last
    /// message there is the parent's own call, whose context is not the one
    /// a saved turn would have re-read.
    /// One message, several records (one per block): its tool calls are
    /// counted across them, not past the previous message.
    #[test]
    fn the_firing_message_counts_its_tool_calls_across_its_records() {
        let rec = |mid: &str, cmd: &str| {
            serde_json::json!({"type": "assistant", "message": {"id": mid, "content": [{"type": "tool_use", "input": {"command": cmd}}],
                "usage": {"input_tokens": 3, "cache_read_input_tokens": 997}}})
            .to_string()
        };
        let t = [
            rec("m1", "ls"),
            r#"{"type":"user"}"#.to_string(),
            rec("m2", "git status"),
            rec("m2", "tokenade expand-ref abc"),
        ]
        .join("\n");
        let f = firing_call(&t).unwrap();
        assert_eq!((f.context, f.tool_calls), (1_000, 2));
        let t = [
            rec("m1", "ls"),
            r#"{"type":"user"}"#.to_string(),
            rec("m2", "tokenade expand-ref abc"),
        ]
        .join("\n");
        assert_eq!(firing_call(&t).unwrap().tool_calls, 1);
    }

    #[test]
    fn no_context_when_the_last_call_is_not_tokenade() {
        let t = format!(
            "{}\n{}\n",
            rec("tokenade web a b", 9_000),
            rec("Task", 250_000)
        );
        assert_eq!(firing_context(&t), None);
        assert_eq!(firing_context("not json\n"), None);
    }
}
