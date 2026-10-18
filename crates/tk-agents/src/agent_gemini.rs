//! Gemini CLI (Google) transcript parser.
//!
//! Gemini CLI records chats to
//! `~/.gemini/tmp/<project_hash>/chats/session-<timestamp>-<id>.jsonl`
//! (current versions; pre-2026 wrote whole-session `.json` files —
//! we accept both). Same `~`-relative layout on every OS; the
//! `GEMINI_CLI_HOME` override is intentionally ignored here since we
//! scan from the user's home like every other parser.
//!
//! Each assistant message carries (per
//! packages/core/src/services/chatRecordingService.ts, verified
//! 2026-06-10):
//!
//! ```text
//! {"id":"…","type":"gemini","timestamp":"<ISO>","content":"…",
//!  "model":"gemini-2.5-pro",
//!  "tokens":{"input":N,"output":N,"cached":N,"thoughts":N,"tool":N,"total":N}}
//! ```
//!
//! `cached` is a subset of `input` (maps Gemini's
//! `cachedContentTokenCount` ⊂ `promptTokenCount`), so fresh input =
//! `input - cached` — the shared cache-reads-excluded convention.
//! `thoughts` is reasoning output and folds into output.
//!
//! The project directory name is a HASH of the project path, not the
//! path itself, and messages carry no cwd — so per-project aggregation
//! (`aggregate_since`) is impossible without guessing and returns
//! empty. Per-model aggregation needs no cwd and is fully supported.

use crate::agent_transcripts::{Agent, ModelUsageByDay, ProjectUsage, TranscriptParser};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;

pub struct Parser;

impl TranscriptParser for Parser {
    fn agent(&self) -> Agent {
        Agent::Gemini
    }

    fn aggregate_since(&self, _home_dir: &Path, _since_iso: Option<&str>) -> Vec<ProjectUsage> {
        // No cwd in Gemini CLI transcripts (only a project-path hash)
        // — per-project rows would be guesses, so we contribute none.
        Vec::new()
    }

    fn model_usage_by_day(&self, home_dir: &Path, since_iso: Option<&str>) -> ModelUsageByDay {
        let mut out = ModelUsageByDay::default();
        scan_chats_root(
            &home_dir.join(".gemini").join("tmp"),
            &mut out,
            since_iso,
            &extract_gemini_message,
        );
        out
    }

    fn session_models(
        &self,
        home_dir: &Path,
        since_iso: Option<&str>,
    ) -> Vec<crate::agent_transcripts::SessionModel> {
        session_models(home_dir, since_iso)
    }
}

/// One [`SessionModel`](crate::agent_transcripts::SessionModel) per
/// (chat-file, model) pair. The chat file stem is the Gemini session id.
pub fn session_models(
    home_dir: &Path,
    since_iso: Option<&str>,
) -> Vec<crate::agent_transcripts::SessionModel> {
    session_models_from_chats_root(
        &home_dir.join(".gemini").join("tmp"),
        since_iso,
        &extract_gemini_message,
    )
}

/// Per-message extractor: maps a raw record to
/// `(model, fresh_input, output)`, or None to skip it.
pub(crate) type ExtractFn = dyn Fn(&serde_json::Value) -> Option<(String, u64, u64)>;

fn extract_gemini_message(m: &serde_json::Value) -> Option<(String, u64, u64)> {
    let model = m["model"].as_str()?;
    if model.is_empty() {
        return None;
    }
    let t = &m["tokens"];
    let input = t["input"].as_u64().unwrap_or(0);
    let cached = t["cached"].as_u64().unwrap_or(0);
    let fresh = input.saturating_sub(cached);
    let output = t["output"]
        .as_u64()
        .unwrap_or(0)
        .saturating_add(t["thoughts"].as_u64().unwrap_or(0));
    Some((model.to_string(), fresh, output))
}

/// Walk `<root>/<project_hash>/chats/*` and feed every session file's
/// messages through `extract`. Shared by the Gemini and Qwen parsers
/// (Qwen Code is a Gemini CLI fork with the same directory shape).
pub(crate) fn scan_chats_root(
    root: &Path,
    out: &mut ModelUsageByDay,
    since_iso: Option<&str>,
    extract: &ExtractFn,
) {
    let Ok(projects) = fs::read_dir(root) else {
        return;
    };
    for proj in projects.flatten() {
        let chats = proj.path().join("chats");
        let Ok(files) = fs::read_dir(&chats) else {
            continue;
        };
        for f in files.flatten() {
            let path = f.path();
            match path.extension().and_then(|e| e.to_str()) {
                Some("jsonl") => {
                    let Ok(file) = fs::File::open(&path) else {
                        continue;
                    };
                    for line in BufReader::new(file).lines().map_while(Result::ok) {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                            record_message(&v, out, since_iso, extract);
                        }
                    }
                }
                // Legacy whole-session JSON: {"messages":[…]} or a bare array.
                Some("json") => {
                    let Some(v) = fs::read_to_string(&path)
                        .ok()
                        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                    else {
                        continue;
                    };
                    let msgs = v["messages"]
                        .as_array()
                        .or_else(|| v.as_array())
                        .cloned()
                        .unwrap_or_default();
                    for m in &msgs {
                        record_message(m, out, since_iso, extract);
                    }
                }
                _ => {}
            }
        }
    }
}

fn record_message(
    m: &serde_json::Value,
    out: &mut ModelUsageByDay,
    since_iso: Option<&str>,
    extract: &ExtractFn,
) {
    let Some((model, fresh, output)) = extract(m) else {
        return;
    };
    if fresh == 0 && output == 0 {
        return;
    }
    // Timestamp: ISO string normally; tolerate epoch ms numbers.
    let iso = match m["timestamp"].as_str() {
        Some(s) => s.to_string(),
        None => match m["timestamp"].as_u64() {
            Some(ms) => crate::platform::epoch_to_iso8601_z(ms / 1000),
            None => return,
        },
    };
    if !iso.is_char_boundary(10) || since_iso.is_some_and(|c| iso.as_str() < c) {
        return;
    }
    let e = out
        .entry(iso[..10].to_string())
        .or_default()
        .entry(model)
        .or_insert((0, 0, 0));
    e.0 = e.0.saturating_add(fresh);
    e.1 = e.1.saturating_add(output);
    e.2 = e.2.saturating_add(1);
}

/// Walk the same `<root>/<project_hash>/chats/*` tree as [`scan_chats_root`]
/// and, for every (chat file, model), record the min/max message-timestamp
/// span in epoch millis. Shared by the Gemini and Qwen parsers. Same file
/// discovery, record extraction, and `since_iso` cutoff as `model_usage_by_day`
/// — only the aggregation differs (timestamp span vs day-bucketed tokens).
pub(crate) fn session_models_from_chats_root(
    root: &Path,
    since_iso: Option<&str>,
    extract: &ExtractFn,
) -> Vec<crate::agent_transcripts::SessionModel> {
    use crate::agent_transcripts::SessionModel;
    let mut acc: std::collections::BTreeMap<(String, String), (u64, u64)> = Default::default();
    let Ok(projects) = fs::read_dir(root) else {
        return Vec::new();
    };
    for proj in projects.flatten() {
        let chats = proj.path().join("chats");
        let Ok(files) = fs::read_dir(&chats) else {
            continue;
        };
        for f in files.flatten() {
            let path = f.path();
            let Some(sess) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            match path.extension().and_then(|e| e.to_str()) {
                Some("jsonl") => {
                    let Ok(file) = fs::File::open(&path) else {
                        continue;
                    };
                    for line in BufReader::new(file).lines().map_while(Result::ok) {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                            record_session_model(&sess, &v, &mut acc, since_iso, extract);
                        }
                    }
                }
                Some("json") => {
                    let Some(v) = fs::read_to_string(&path)
                        .ok()
                        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                    else {
                        continue;
                    };
                    let msgs = v["messages"]
                        .as_array()
                        .or_else(|| v.as_array())
                        .cloned()
                        .unwrap_or_default();
                    for m in &msgs {
                        record_session_model(&sess, m, &mut acc, since_iso, extract);
                    }
                }
                _ => {}
            }
        }
    }
    acc.into_iter()
        .map(|((sess, model), (start_ms, end_ms))| SessionModel {
            sess,
            model,
            start_ms,
            end_ms,
        })
        .collect()
}

fn record_session_model(
    sess: &str,
    m: &serde_json::Value,
    acc: &mut std::collections::BTreeMap<(String, String), (u64, u64)>,
    since_iso: Option<&str>,
    extract: &ExtractFn,
) {
    let Some((model, fresh, output)) = extract(m) else {
        return;
    };
    if fresh == 0 && output == 0 {
        return;
    }
    // Same timestamp derivation as record_message: ISO string normally,
    // epoch-ms number tolerated.
    let (iso, ms) = match m["timestamp"].as_str() {
        Some(s) => (
            s.to_string(),
            crate::claudecode_transcript::iso8601_to_epoch_ms(s),
        ),
        None => match m["timestamp"].as_u64() {
            Some(msnum) => (
                crate::platform::epoch_to_iso8601_z(msnum / 1000),
                Some(msnum),
            ),
            None => return,
        },
    };
    if !iso.is_char_boundary(10) || since_iso.is_some_and(|c| iso.as_str() < c) {
        return;
    }
    let Some(ms) = ms else {
        return;
    };
    let e = acc.entry((sess.to_string(), model)).or_insert((ms, ms));
    if ms < e.0 {
        e.0 = ms;
    }
    if ms > e.1 {
        e.1 = ms;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_home(label: &str) -> std::path::PathBuf {
        let d =
            std::env::temp_dir().join(format!("tokenade-gemini-{}-{}", label, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn model_usage_from_jsonl_excludes_cached_and_folds_thoughts() {
        let home = tmp_home("jsonl");
        let chats = home.join(".gemini/tmp/abc123/chats");
        fs::create_dir_all(&chats).unwrap();
        fs::write(
            chats.join("session-2026-06-01-x.jsonl"),
            [
                r#"{"id":"1","type":"user","timestamp":"2026-06-01T10:00:00.000Z","content":"hi"}"#,
                r#"{"id":"2","type":"gemini","timestamp":"2026-06-01T10:00:05.000Z","model":"gemini-2.5-pro","tokens":{"input":100,"output":20,"cached":60,"thoughts":5,"tool":0,"total":125}}"#,
                r#"{"id":"3","type":"gemini","timestamp":"2026-06-02T09:00:00.000Z","model":"gemini-2.5-flash","tokens":{"input":10,"output":3,"cached":0,"thoughts":0,"tool":0,"total":13}}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        let m = Parser.model_usage_by_day(&home, None);
        assert_eq!(
            m["2026-06-01"]["gemini-2.5-pro"],
            (40, 25, 1),
            "fresh = 100-60 cached; output folds thoughts"
        );
        assert_eq!(m["2026-06-02"]["gemini-2.5-flash"], (10, 3, 1));

        let m2 = Parser.model_usage_by_day(&home, Some("2026-06-02T00:00:00Z"));
        assert!(!m2.contains_key("2026-06-01"), "cutoff filters per message");
    }

    #[test]
    fn legacy_json_session_file_is_parsed() {
        let home = tmp_home("legacy");
        let chats = home.join(".gemini/tmp/abc123/chats");
        fs::create_dir_all(&chats).unwrap();
        fs::write(
            chats.join("session-old.json"),
            r#"{"sessionId":"s","messages":[
              {"id":"1","type":"gemini","timestamp":"2026-05-01T10:00:00.000Z","model":"gemini-2.5-pro","tokens":{"input":7,"output":2,"cached":0,"thoughts":0,"tool":0,"total":9}}
            ]}"#,
        )
        .unwrap();
        let m = Parser.model_usage_by_day(&home, None);
        assert_eq!(m["2026-05-01"]["gemini-2.5-pro"], (7, 2, 1));
    }

    #[test]
    fn missing_dir_and_aggregate_are_empty() {
        let home = tmp_home("missing");
        assert!(Parser.model_usage_by_day(&home, None).is_empty());
        assert!(
            Parser.aggregate_since(&home, None).is_empty(),
            "no cwd in Gemini transcripts → no per-project rows, by design"
        );
    }
}
