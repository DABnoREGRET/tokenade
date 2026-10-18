//! Qwen Code (Alibaba) transcript parser.
//!
//! Qwen Code is a Gemini CLI fork and keeps the same chat layout:
//! `~/.qwen/tmp/<project_id>/chats/<sessionId>.jsonl` — `~`-relative
//! on every OS. Unlike upstream it persists the raw Gemini-API-style
//! usage block per record (verified against QwenLM/qwen-code session
//! logging, 2026-06-10):
//!
//! ```text
//! {"sessionId":"…","timestamp":"<ISO>","type":"assistant",
//!  "model":"qwen3-coder-plus",
//!  "usageMetadata":{"promptTokenCount":N,"candidatesTokenCount":N,
//!                   "cachedContentTokenCount":N,"thoughtsTokenCount":N,
//!                   "totalTokenCount":N}}
//! ```
//!
//! `cachedContentTokenCount` ⊂ `promptTokenCount`, so fresh input =
//! prompt − cached (shared convention: cache reads excluded);
//! `thoughtsTokenCount` is reasoning output and folds into output.
//!
//! Like Gemini, the project directory is an opaque id and records
//! carry no cwd → `aggregate_since` returns empty by design; only the
//! per-model view is populated.

use crate::agent_gemini::scan_chats_root;
use crate::agent_transcripts::{Agent, ModelUsageByDay, ProjectUsage, TranscriptParser};
use std::path::Path;

pub struct Parser;

impl TranscriptParser for Parser {
    fn agent(&self) -> Agent {
        Agent::Qwen
    }

    fn aggregate_since(&self, _home_dir: &Path, _since_iso: Option<&str>) -> Vec<ProjectUsage> {
        // No cwd in Qwen Code transcripts (opaque project id only).
        Vec::new()
    }

    fn model_usage_by_day(&self, home_dir: &Path, since_iso: Option<&str>) -> ModelUsageByDay {
        let mut out = ModelUsageByDay::default();
        scan_chats_root(
            &home_dir.join(".qwen").join("tmp"),
            &mut out,
            since_iso,
            &extract_qwen_message,
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
/// (chat-file, model) pair. Reuses the shared Gemini-fork chats walk, pointed
/// at Qwen's `~/.qwen/tmp` root with the Qwen usage extractor.
pub fn session_models(
    home_dir: &Path,
    since_iso: Option<&str>,
) -> Vec<crate::agent_transcripts::SessionModel> {
    crate::agent_gemini::session_models_from_chats_root(
        &home_dir.join(".qwen").join("tmp"),
        since_iso,
        &extract_qwen_message,
    )
}

/// Per-record extraction: `(model, fresh_input, output)` or None.
fn extract_qwen_message(m: &serde_json::Value) -> Option<(String, u64, u64)> {
    let model = m["model"].as_str()?;
    if model.is_empty() {
        return None;
    }
    let u = &m["usageMetadata"];
    if u.is_null() {
        return None;
    }
    let prompt = u["promptTokenCount"].as_u64().unwrap_or(0);
    let cached = u["cachedContentTokenCount"].as_u64().unwrap_or(0);
    let fresh = prompt.saturating_sub(cached);
    let output = u["candidatesTokenCount"]
        .as_u64()
        .unwrap_or(0)
        .saturating_add(u["thoughtsTokenCount"].as_u64().unwrap_or(0));
    Some((model.to_string(), fresh, output))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp_home(label: &str) -> std::path::PathBuf {
        let d =
            std::env::temp_dir().join(format!("tokenade-qwen-{}-{}", label, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn model_usage_from_usage_metadata() {
        let home = tmp_home("usage");
        let chats = home.join(".qwen/tmp/proj1/chats");
        fs::create_dir_all(&chats).unwrap();
        fs::write(
            chats.join("abc.jsonl"),
            [
                r#"{"sessionId":"s","timestamp":"2026-06-01T08:00:00.000Z","type":"user","content":"hi"}"#,
                r#"{"sessionId":"s","timestamp":"2026-06-01T08:00:04.000Z","type":"assistant","model":"qwen3-coder-plus","usageMetadata":{"promptTokenCount":200,"candidatesTokenCount":30,"cachedContentTokenCount":150,"thoughtsTokenCount":12,"totalTokenCount":242}}"#,
                r#"{"sessionId":"s","timestamp":"2026-06-03T08:00:00.000Z","type":"assistant","model":"qwen3-coder-flash","usageMetadata":{"promptTokenCount":50,"candidatesTokenCount":8,"totalTokenCount":58}}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        let m = Parser.model_usage_by_day(&home, None);
        assert_eq!(
            m["2026-06-01"]["qwen3-coder-plus"],
            (50, 42, 1),
            "fresh = 200-150 cached; output = 30 candidates + 12 thoughts"
        );
        assert_eq!(m["2026-06-03"]["qwen3-coder-flash"], (50, 8, 1));

        let m2 = Parser.model_usage_by_day(&home, Some("2026-06-02T00:00:00Z"));
        assert!(!m2.contains_key("2026-06-01"), "cutoff filters per record");
        assert!(m2.contains_key("2026-06-03"));
    }

    #[test]
    fn records_without_model_or_usage_are_skipped() {
        let home = tmp_home("skip");
        let chats = home.join(".qwen/tmp/proj1/chats");
        fs::create_dir_all(&chats).unwrap();
        fs::write(
            chats.join("abc.jsonl"),
            [
                r#"{"sessionId":"s","timestamp":"2026-06-01T08:00:00.000Z","type":"assistant","usageMetadata":{"promptTokenCount":9,"candidatesTokenCount":1}}"#,
                r#"{"sessionId":"s","timestamp":"2026-06-01T08:00:01.000Z","type":"assistant","model":"qwen3-coder-plus"}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        assert!(Parser.model_usage_by_day(&home, None).is_empty());
        assert!(Parser.aggregate_since(&home, None).is_empty());
    }
}
