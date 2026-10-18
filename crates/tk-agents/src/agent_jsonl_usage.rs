//! Agents that write their token accounting to JSONL event logs or to a JSON
//! session file.
//!
//! Eight of them share this module for the same reason the SQLite four share
//! theirs: the work is walk-a-tree, read-a-record, map-onto-[`ProjectUsage`].
//! What differs per agent is three things — where the tree is, which record
//! carries usage, and what the numbers mean — so each section states those
//! three and reuses everything else.
//!
//! Schemas source-verified 2026-07-29 against codeburn's provider parsers.
//!
//! The recurring trap here is **what a field already includes**. Several of
//! these agents report an input figure with cache folded in and an output
//! figure with reasoning folded in. Our [`ProjectUsage`] keeps fresh input and
//! cache apart, so a straight copy double-counts re-sent history and inflates
//! every total downstream. Each site below says which convention it found.

mod event_logs;
mod sessions;
pub use event_logs::*;
pub use sessions::*;

// One declarer for both usage readers: the SQLite one carried
// the same two helpers.
use crate::agent_sqlite_usage::{after_cutoff, finish};
use crate::agent_transcripts::{Agent, ModelUsageByDay, TranscriptParser};
use crate::claudecode_transcript::ProjectUsage;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ── shared plumbing ──────────────────────────────────────────────────

fn read_jsonl(path: &Path) -> Vec<Value> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect()
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// A count that may arrive as a number or as a numeric string — several of
/// these logs are written by TypeScript and stringify large integers.
fn num(v: Option<&Value>) -> u64 {
    match v {
        Some(Value::Number(n)) => n.as_u64().unwrap_or(0),
        Some(Value::String(s)) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

/// First present key among `keys`, as a count. Agents in this file rename the
/// same quantity across versions (`input_other` / `input_tokens`,
/// `cache_read_input_tokens` / `cached_input_tokens`), and the reference
/// implementation tries them in order rather than pinning one.
fn num_any(obj: &Value, keys: &[&str]) -> u64 {
    for k in keys {
        if let Some(v) = obj.get(*k) {
            if !v.is_null() {
                return num(Some(v));
            }
        }
    }
    0
}

fn str_at<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut cur = v;
    for k in path {
        cur = cur.get(*k)?;
    }
    cur.as_str()
}

/// Epoch (seconds or milliseconds) or an ISO string, to ISO-8601 Z.
fn any_ts_to_iso(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => {
            let raw = n.as_i64().unwrap_or(0).max(0) as u64;
            let secs = if raw >= 1_000_000_000_000 {
                raw / 1000
            } else {
                raw
            };
            crate::platform::epoch_to_iso8601_z(secs)
        }
        _ => String::new(),
    }
}

#[derive(Default, Clone, Copy)]
pub(crate) struct Counts {
    input: u64,
    output: u64,
    cache_creation: u64,
    cache_read: u64,
    messages: u64,
}

impl Counts {
    fn is_empty(&self) -> bool {
        self.input == 0 && self.output == 0 && self.cache_creation == 0 && self.cache_read == 0
    }
}

fn add(by_cwd: &mut HashMap<String, ProjectUsage>, cwd: String, c: Counts, ts_iso: &str) {
    let e = by_cwd.entry(cwd.clone()).or_insert_with(|| ProjectUsage {
        cwd,
        ..Default::default()
    });
    e.assistant_messages += c.messages;
    e.input_tokens += c.input;
    e.output_tokens += c.output;
    e.cache_creation_tokens += c.cache_creation;
    e.cache_read_tokens += c.cache_read;
    if ts_iso > e.last_ts.as_str() {
        e.last_ts = ts_iso.to_string();
    }
}

fn bump_session(by_cwd: &mut HashMap<String, ProjectUsage>, cwd: &str) {
    if let Some(e) = by_cwd.get_mut(cwd) {
        e.sessions += 1;
    }
}

fn model_day_add(out: &mut ModelUsageByDay, iso: &str, model: &str, c: Counts) {
    if model.is_empty() || !iso.is_char_boundary(10) {
        return;
    }
    let e = out
        .entry(iso[..10].to_string())
        .or_default()
        .entry(model.to_string())
        .or_insert((0, 0, 0));
    // Repo-wide convention: fresh input counts cache WRITES, never cache reads.
    e.0 += c.input + c.cache_creation;
    e.1 += c.output;
    e.2 += c.messages;
}

/// Directory entries, sorted, so a failure reproduces in the same order.
fn sorted_dir(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    v.sort();
    v
}

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var(key)
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_home(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "tokenade-jsonl-agents-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn write(path: PathBuf, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn every_parser_is_silent_on_an_empty_home() {
        let home = tmp_home("empty");
        assert!(MuxParser.aggregate_since(&home, None).is_empty());
        assert!(KimiParser.aggregate_since(&home, None).is_empty());
        assert!(KimiCodeParser.aggregate_since(&home, None).is_empty());
        assert!(LingtaiParser.aggregate_since(&home, None).is_empty());
        assert!(OpenDesignParser.aggregate_since(&home, None).is_empty());
        assert!(DroidParser.aggregate_since(&home, None).is_empty());
        assert!(CodewhaleParser.aggregate_since(&home, None).is_empty());
        assert!(MistralVibeParser.aggregate_since(&home, None).is_empty());
    }

    #[test]
    fn mux_decomposes_an_inclusive_input_figure() {
        let home = tmp_home("mux");
        write(
            home.join(".mux/sessions/ws1/chat.jsonl"),
            r#"{"role":"assistant","metadata":{"model":"anthropic:claude-sonnet-4-5","timestamp":1783000000000,"usage":{"inputTokens":1000,"outputTokens":80,"cachedInputTokens":600,"reasoningTokens":30},"providerMetadata":{"anthropic":{"cacheCreationInputTokens":300}}}}"#,
        );
        let out = MuxParser.aggregate_since(&home, None);
        assert_eq!(out.len(), 1);
        // 1000 − (600 read + 300 written) = 100 fresh. A straight copy would
        // have billed 1000 and double-counted the history.
        assert_eq!(out[0].input_tokens, 100);
        assert_eq!(out[0].cache_read_tokens, 600);
        assert_eq!(out[0].cache_creation_tokens, 300);
        // reasoning is already inside outputTokens — do not add it again.
        assert_eq!(out[0].output_tokens, 80);
        let m = MuxParser.model_usage_by_day(&home, None);
        // provider prefix stripped; fresh + cache writes.
        assert_eq!(
            m.values().next().unwrap()["claude-sonnet-4-5"],
            (400, 80, 1)
        );
    }

    #[test]
    fn kimi_accepts_both_key_generations_and_string_numbers() {
        let home = tmp_home("kimi");
        write(
            home.join(".kimi/sessions/h1/s1/wire.jsonl"),
            concat!(
                // Old spelling, bare `input` that still contains the cache.
                r#"{"timestamp":1783000000,"message":{"type":"StatusUpdate","payload":{"model":"kimi-k2","usage":{"input":500,"output":40,"input_cache_read":300}}}}"#,
                "\n",
                // New spelling, already decomposed, counts as strings.
                r#"{"timestamp":"2026-07-02T00:00:00Z","payload":{"model":"kimi-k2","token_usage":{"input_other":"70","output":"9","cache_creation_input_tokens":"20"}}}"#,
                "\n"
            ),
        );
        let out = KimiParser.aggregate_since(&home, None);
        assert_eq!(out.len(), 1);
        // (500 − 300) + 70
        assert_eq!(out[0].input_tokens, 270);
        assert_eq!(out[0].output_tokens, 49);
        assert_eq!(out[0].cache_read_tokens, 300);
        assert_eq!(out[0].cache_creation_tokens, 20);
    }

    #[test]
    fn kimicode_walks_its_deeper_agent_nesting() {
        let home = tmp_home("kimicode");
        write(
            home.join(".kimi-code/sessions/wd_abc/session_1/agents/a1/wire.jsonl"),
            concat!(
                r#"{"type":"llm.request","model":"kimi-k2","modelAlias":"fast"}"#,
                "\n",
                r#"{"type":"usage.record","time":"2026-07-03T00:00:00Z","model":"fast","usage":{"inputOther":10,"output":3,"inputCacheRead":90,"inputCacheCreation":5}}"#,
                "\n"
            ),
        );
        let out = KimiCodeParser.aggregate_since(&home, None);
        assert_eq!(out.len(), 1, "four-deep nesting must still be walked");
        assert_eq!(out[0].input_tokens, 10);
        assert_eq!(out[0].cache_read_tokens, 90);
        assert_eq!(out[0].sessions, 1);
    }

    #[test]
    fn lingtai_subtracts_the_cached_share_and_honours_the_cutoff() {
        let home = tmp_home("lingtai");
        write(
            home.join(".lingtai/logs/token_ledger.jsonl"),
            concat!(
                r#"{"ts":1783000000,"model":"m","input":100,"output":10,"cached":40,"thinking":5}"#,
                "\n",
                r#"{"ts":"2026-01-01T00:00:00Z","model":"m","input":9,"output":1}"#,
                "\n"
            ),
        );
        let all = LingtaiParser.aggregate_since(&home, None);
        assert_eq!(all[0].input_tokens, 60 + 9);
        assert_eq!(all[0].output_tokens, 15 + 1);
        assert_eq!(all[0].cache_read_tokens, 40);
        let cut = LingtaiParser.aggregate_since(&home, Some("2026-06-01T00:00:00Z"));
        assert_eq!(
            cut[0].input_tokens, 60,
            "the 2026-01 record must be dropped"
        );
    }

    #[test]
    fn open_design_carries_the_announced_model_forward() {
        let home = tmp_home("opendesign");
        write(
            home.join(".config/Open Design/namespaces/team-a/data/runs/r1/events.jsonl"),
            concat!(
                r#"{"event":"start","timestamp":"2026-07-04T00:00:00Z","data":{"model":"gpt-5"}}"#,
                "\n",
                r#"{"event":"agent","timestamp":"2026-07-04T00:00:01Z","data":{"type":"usage","usage":{"input_tokens":50,"output_tokens":6,"cached_read_tokens":10,"thought_tokens":4}}}"#,
                "\n"
            ),
        );
        let out = OpenDesignParser.aggregate_since(&home, None);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].cwd, "team-a");
        assert_eq!(out[0].output_tokens, 10, "thought tokens are billed output");
        let m = OpenDesignParser.model_usage_by_day(&home, None);
        // The usage record itself names no model; the earlier `start` does.
        assert_eq!(m["2026-07-04"]["gpt-5"], (50, 10, 1));
    }

    #[test]
    fn droid_reads_session_totals_from_the_companion_settings_file() {
        let home = tmp_home("droid");
        write(
            home.join(".factory/sessions/g1/s1.jsonl"),
            concat!(
                r#"{"type":"session_start","id":"s1","timestamp":"2026-07-05T00:00:00Z","cwd":"/w/d"}"#,
                "\n",
                r#"{"type":"message","timestamp":"2026-07-05T00:10:00Z"}"#,
                "\n"
            ),
        );
        write(
            home.join(".factory/sessions/g1/s1.settings.json"),
            r#"{"model":"custom:claude-opus-4-8","tokenUsage":{"inputTokens":200,"outputTokens":20,"cacheCreationTokens":30,"cacheReadTokens":400,"thinkingTokens":7}}"#,
        );
        let out = DroidParser.aggregate_since(&home, None);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].cwd, "/w/d");
        assert_eq!(out[0].input_tokens, 200);
        assert_eq!(out[0].output_tokens, 27);
        assert_eq!(out[0].cache_read_tokens, 400);
        assert_eq!(out[0].last_ts, "2026-07-05T00:10:00Z");
    }

    #[test]
    fn codewhale_keeps_its_unsplit_total_whole() {
        let home = tmp_home("codewhale");
        write(
            home.join(".codewhale/sessions/a.json"),
            r#"{"metadata":{"id":"a","model":"deepseek-v4","workspace":"/w/c","total_tokens":1234,"updated_at":"2026-07-06T00:00:00Z"}}"#,
        );
        let out = CodewhaleParser.aggregate_since(&home, None);
        assert_eq!(out.len(), 1);
        // One number in, one number out — never split by a guessed ratio.
        assert_eq!(out[0].input_tokens, 1234);
        assert_eq!(out[0].output_tokens, 0);
    }

    #[test]
    fn mistral_vibe_counts_sub_agents_as_their_own_sessions() {
        let home = tmp_home("vibe");
        write(
            home.join(".vibe/logs/session/s1/meta.json"),
            r#"{"session_id":"s1","config":{"active_model":"mistral-medium-3.5"},
                "environment":{"working_directory":"/w/v"},
                "stats":{"session_prompt_tokens":300,"session_completion_tokens":40},
                "end_time":"2026-07-07T00:00:00Z"}"#,
        );
        write(
            home.join(".vibe/logs/session/s1/agents/sub/meta.json"),
            r#"{"session_id":"sub","config":{"active_model":"mistral-medium-3.5"},
                "environment":{"working_directory":"/w/v"},
                "stats":{"session_prompt_tokens":10,"session_completion_tokens":2},
                "end_time":"2026-07-07T01:00:00Z"}"#,
        );
        let out = MistralVibeParser.aggregate_since(&home, None);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].input_tokens, 310);
        assert_eq!(out[0].sessions, 2);
        let m = MistralVibeParser.model_usage_by_day(&home, None);
        assert_eq!(m["2026-07-07"]["mistral-medium-3.5"], (310, 42, 2));
    }
}
