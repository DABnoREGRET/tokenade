//! Kilo Code (`@kilocode/cli`) transcript parser.
//!
//! Kilo Code is a fork of SST/anomalyco OpenCode and reuses its on-disk
//! storage schema VERBATIM — only the data root differs:
//! `~/.local/share/kilo/storage/{message,part,session}/…`. Assistant message
//! JSON carries the same `tokens{input,output,reasoning,cache{read,write}}`
//! block, `modelID`/`providerID`, and `path.cwd`/`path.root` that the OpenCode
//! parser already understands (validated against real local sessions:
//! `storage/message/<sid>/msg_*.json` with e.g.
//! `"tokens":{"total":131198,"input":744,"output":319,"reasoning":34,
//! "cache":{"read":130135,"write":0}}`). So this parser is a thin wrapper that
//! points the shared OpenCode core ([`crate::agent_opencode::aggregate_in_root`]
//! / [`crate::agent_opencode::model_usage_in_root`]) at Kilo's root.
//!
//! Kilo's SQLite store is `kilo.db` (same schema); older installs are JSON-era
//! and the shared core falls back to the JSON message tree.

use crate::agent_transcripts::{Agent, ModelUsageByDay, ProjectUsage, TranscriptParser};
use std::path::{Path, PathBuf};

pub struct Parser;

impl TranscriptParser for Parser {
    fn agent(&self) -> Agent {
        Agent::Kilo
    }

    fn aggregate_since(&self, home_dir: &Path, since_iso: Option<&str>) -> Vec<ProjectUsage> {
        crate::agent_opencode::aggregate_in_root(&data_root(home_dir), since_iso)
    }

    fn model_usage_by_day(&self, home_dir: &Path, since_iso: Option<&str>) -> ModelUsageByDay {
        crate::agent_opencode::model_usage_in_root(&data_root(home_dir), since_iso)
    }

    fn session_models(
        &self,
        home_dir: &Path,
        since_iso: Option<&str>,
    ) -> Vec<crate::agent_transcripts::SessionModel> {
        crate::agent_opencode::session_models_in_root(&data_root(home_dir), since_iso)
    }
}

/// `~/.local/share/kilo` — Kilo mirrors OpenCode's fixed data-dir convention
/// (same `os.UserConfigDir`-independent path on every OS).
/// Kilo's call timeline: the opencode reader at Kilo's root.
pub fn session_timeline(
    home_dir: &Path,
    session_id: &str,
) -> Option<(
    crate::claudecode_transcript::SessionTimeline,
    Option<String>,
)> {
    crate::agent_opencode::session_timeline_in_root(&data_root(home_dir), session_id)
}

fn data_root(home_dir: &Path) -> PathBuf {
    home_dir.join(".local").join("share").join("kilo")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp_home(label: &str) -> PathBuf {
        let p =
            std::env::temp_dir().join(format!("tokenade-kilo-{}-{}", label, std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    /// Real Kilo assistant-message shape (copied from a live
    /// `~/.local/share/kilo/storage/message/<sid>/msg_*.json`).
    fn write_msg(home: &Path, sid: &str, mid: &str, body: &str) {
        let dir = data_root(home).join("storage").join("message").join(sid);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{mid}.json")), body).unwrap();
    }

    #[test]
    fn kilo_reuses_opencode_schema_for_tokens_and_model() {
        let home = tmp_home("schema");
        // Verbatim shape from a real session (2026-02 Kilo install).
        write_msg(
            &home,
            "ses_A",
            "msg_1",
            r#"{
            "id":"msg_1","sessionID":"ses_A","role":"assistant",
            "time":{"created":1771631183369,"completed":1771631195524},
            "modelID":"minimax/minimax-m2.5:free","providerID":"kilo",
            "path":{"cwd":"/home/u/proj","root":"/home/u/proj"},
            "cost":0.0045,
            "tokens":{"total":131198,"input":744,"output":319,"reasoning":34,
                      "cache":{"read":130135,"write":0}},
            "finish":"tool-calls"
        }"#,
        );

        let rows = Parser.aggregate_since(&home, None);
        assert_eq!(rows.len(), 1, "one cwd bucket");
        let r = &rows[0];
        assert_eq!(r.cwd, "/home/u/proj");
        assert_eq!(r.input_tokens, 744);
        // output convention = output + reasoning (shared opencode mapping).
        assert_eq!(r.output_tokens, 319 + 34);
        assert_eq!(r.cache_read_tokens, 130135);
        assert_eq!(r.cache_creation_tokens, 0);

        // Per-day model panel sees the Kilo model on the message's UTC day.
        let m = Parser.model_usage_by_day(&home, None);
        assert!(
            m.values()
                .any(|day| day.contains_key("minimax/minimax-m2.5:free")),
            "model surfaced in model_usage_by_day: {m:?}"
        );
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn kilo_missing_root_is_empty() {
        let home = tmp_home("empty");
        assert!(Parser.aggregate_since(&home, None).is_empty());
        assert!(Parser.model_usage_by_day(&home, None).is_empty());
        let _ = fs::remove_dir_all(&home);
    }

    /// Real-data validation against the developer's actual
    /// `~/.local/share/kilo` install. Ignored by default (machine-specific);
    /// run with `cargo test -p tokenade-core agent_kilo::tests::real_kilo -- --ignored --nocapture`.
    #[test]
    #[ignore = "machine-specific: the developer's real `kilo` install"]
    fn real_kilo_smoke() {
        let home = PathBuf::from(std::env::var("HOME").expect("HOME"));
        let rows = Parser.aggregate_since(&home, None);
        eprintln!("== kilo aggregate: {} cwd bucket(s) ==", rows.len());
        let mut tin = 0u64;
        let mut tout = 0u64;
        for r in &rows {
            tin += r.input_tokens;
            tout += r.output_tokens;
            eprintln!(
                "  cwd={} sessions={} in={} out={} cache_read={}",
                r.cwd, r.sessions, r.input_tokens, r.output_tokens, r.cache_read_tokens
            );
        }
        let m = Parser.model_usage_by_day(&home, None);
        eprintln!(
            "== kilo model_usage_by_day: {} day(s); totals in={tin} out={tout} ==",
            m.len()
        );
        assert!(!rows.is_empty(), "expected ≥1 kilo session on this machine");
    }
}
