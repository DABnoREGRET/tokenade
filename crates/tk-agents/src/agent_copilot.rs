//! GitHub Copilot Chat (VS Code) transcript parser.
//!
//! Chat sessions are persisted by VS Code itself (not the extension's
//! own globalStorage), in two places under each product's `User` dir
//! (Code / Code - Insiders / VSCodium / Cursor — see
//! [`crate::agent_transcripts::vscode_user_dirs`]):
//!
//! ```text
//! User/workspaceStorage/<md5-of-workspace>/chatSessions/<uuid>.json
//! User/globalStorage/emptyWindowChatSessions/<uuid>.json
//! ```
//!
//! The workspace folder is NOT in the session file — it comes from the
//! sibling `workspaceStorage/<hash>/workspace.json`, whose `folder`
//! field is a `file://` URI. Empty-window sessions have no workspace
//! at all and therefore contribute to the per-model view only.
//!
//! Session schema (`ISerializableChatData` v3, verified against
//! microsoft/vscode src/vs/workbench/contrib/chat 2026-06-10):
//!
//! ```text
//! {"version":3,"sessionId":"…","creationDate":<epoch-ms>,
//!  "requests":[
//!    {"timestamp":<epoch-ms>,
//!     "modelId":"gpt-4.1",              // optional
//!     "completionTokens":678,            // optional
//!     "message":{"text":"…"},
//!     "response":[{"kind":"toolInvocationSerialized","toolId":"copilot_readFile",…},…]}]}
//! ```
//!
//! Hard limits of the source data:
//!   - **promptTokens is never persisted** → input is always 0 here;
//!     only completion (output) tokens are counted for Copilot.
//!   - No cache metrics of any kind.
//!   - VS Code ≥ 1.109 additionally writes a `.jsonl` append-log of
//!     state mutations next to the `.json` snapshot; replaying it is
//!     out of scope, so the freshest few turns of a still-open window
//!     may lag until VS Code flushes the snapshot.
//!
//! Timestamps are epoch-ms numbers; ISO strings are accepted as a
//! fallback for older formats.

use crate::agent_transcripts::{
    vscode_user_dirs, Agent, ModelUsageByDay, ProjectUsage, ToolBreakdown, TranscriptParser,
};
use crate::platform::epoch_to_iso8601_z;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub struct Parser;

impl TranscriptParser for Parser {
    fn agent(&self) -> Agent {
        Agent::Copilot
    }

    fn aggregate_since(&self, home_dir: &Path, since_iso: Option<&str>) -> Vec<ProjectUsage> {
        let mut by_cwd: HashMap<String, ProjectUsage> = HashMap::new();
        for user in vscode_user_dirs(home_dir) {
            let Ok(workspaces) = fs::read_dir(user.join("workspaceStorage")) else {
                continue;
            };
            for ws in workspaces.flatten() {
                // cwd comes from the sibling workspace.json; without it the
                // session can't be attributed to a project (it still counts
                // in the per-model view via model_usage_by_day).
                let Some(cwd) = workspace_cwd(&ws.path()) else {
                    continue;
                };
                for p in session_files(&ws.path().join("chatSessions")) {
                    scan_session(&p, &cwd, &mut by_cwd, since_iso);
                }
            }
        }
        let mut out: Vec<ProjectUsage> = by_cwd.into_values().collect();
        out.sort_by(|a, b| b.last_ts.cmp(&a.last_ts));
        out
    }

    fn model_usage_by_day(&self, home_dir: &Path, since_iso: Option<&str>) -> ModelUsageByDay {
        let mut out = ModelUsageByDay::default();
        for user in vscode_user_dirs(home_dir) {
            // Workspace-bound sessions…
            if let Ok(workspaces) = fs::read_dir(user.join("workspaceStorage")) {
                for ws in workspaces.flatten() {
                    for p in session_files(&ws.path().join("chatSessions")) {
                        scan_session_models(&p, &mut out, since_iso);
                    }
                }
            }
            // …and empty-window sessions (no workspace → models only).
            for p in session_files(&user.join("globalStorage").join("emptyWindowChatSessions")) {
                scan_session_models(&p, &mut out, since_iso);
            }
        }
        out
    }

    fn session_models(
        &self,
        home_dir: &Path,
        since_iso: Option<&str>,
    ) -> Vec<crate::agent_transcripts::SessionModel> {
        let mut acc: std::collections::BTreeMap<(String, String), (u64, u64)> = Default::default();
        for user in vscode_user_dirs(home_dir) {
            if let Ok(workspaces) = fs::read_dir(user.join("workspaceStorage")) {
                for ws in workspaces.flatten() {
                    for p in session_files(&ws.path().join("chatSessions")) {
                        scan_session_model_spans(&p, &mut acc, since_iso);
                    }
                }
            }
            for p in session_files(&user.join("globalStorage").join("emptyWindowChatSessions")) {
                scan_session_model_spans(&p, &mut acc, since_iso);
            }
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
}

/// Walk one Copilot session's requests and hand every counted one to
/// `on_request`, with its model and ISO timestamp.
///
/// The two scanners carried this filter chain twice. What must not drift is
/// the set of records that COUNT: a request with no `modelId` contributes
/// nothing (never guess a model), one with zero completion tokens is not a
/// request, and one before the cutoff is out of window. Two copies disagreeing
/// on any of the three would report two different totals for the same session.
///
/// The raw record rides along because the span scanner still needs its
/// `timestamp` field, which the per-day scanner has no use for — resolving it
/// here would make the per-day pass drop records it currently counts.
fn for_each_counted_request(
    path: &Path,
    since_iso: Option<&str>,
    mut on_request: impl FnMut(&serde_json::Value, &str, &str, u64),
) {
    let Some(requests) = read_requests(path) else {
        return;
    };
    for req in &requests {
        // Never guess a model: requests without modelId contribute nothing.
        let Some(model) = req["modelId"].as_str().filter(|m| !m.is_empty()) else {
            continue;
        };
        let completion = req["completionTokens"].as_u64().unwrap_or(0);
        if completion == 0 {
            continue;
        }
        let Some(ts) = request_iso(req) else {
            continue;
        };
        if !ts.is_char_boundary(10) || since_iso.is_some_and(|c| ts.as_str() < c) {
            continue;
        }
        on_request(req, model, &ts, completion);
    }
}

/// Per-(session-file, model) timestamp span. Mirrors [`scan_session_models`]
/// exactly (same modelId + non-zero-completion selection, same cutoff) but
/// records the min/max request timestamp in epoch millis. The session id is the
/// snapshot file stem — Copilot's `<uuid>.json`, one session per file.
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
    for_each_counted_request(path, since_iso, |req, model, ts, _completion| {
        // epoch-ms number normally; ISO-string fallback for older formats.
        let ms = match req["timestamp"].as_u64() {
            Some(msnum) => msnum,
            None => match crate::claudecode_transcript::iso8601_to_epoch_ms(ts) {
                Some(m) => m,
                None => return,
            },
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

/// All `*.json` session snapshots in a directory (ignores the `.jsonl`
/// append-logs — see module doc).
fn session_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
        .collect()
}

/// Resolve `workspaceStorage/<hash>/workspace.json` → decoded folder path.
fn workspace_cwd(ws_dir: &Path) -> Option<String> {
    let text = fs::read_to_string(ws_dir.join("workspace.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    file_uri_to_path(v["folder"].as_str()?)
}

/// `file:///home/u/My%20Proj` → `/home/u/My Proj`;
/// `file:///C:/dev/x` → `C:/dev/x` (Windows drive form).
fn file_uri_to_path(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    let decoded = crate::text_encoding::percent_decode(rest);
    let b = decoded.as_bytes();
    if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
        return Some(decoded[1..].to_string());
    }
    Some(decoded)
}

/// Request timestamp as ISO-8601: epoch-ms number normally, ISO string
/// as fallback.
fn request_iso(req: &serde_json::Value) -> Option<String> {
    if let Some(ms) = req["timestamp"].as_u64() {
        return Some(epoch_to_iso8601_z(ms / 1000));
    }
    req["timestamp"].as_str().map(str::to_string)
}

fn scan_session(
    path: &Path,
    cwd: &str,
    by_cwd: &mut HashMap<String, ProjectUsage>,
    since_iso: Option<&str>,
) {
    let Some(requests) = read_requests(path) else {
        return;
    };
    let mut contributed = false;
    for req in &requests {
        let Some(ts) = request_iso(req) else {
            continue;
        };
        if since_iso.is_some_and(|c| ts.as_str() < c) {
            continue;
        }
        // promptTokens is never persisted (module doc) → input stays 0.
        let completion = req["completionTokens"].as_u64().unwrap_or(0);
        let mut tools = ToolBreakdown::default();
        if let Some(parts) = req["response"].as_array() {
            for part in parts {
                if part["kind"].as_str() == Some("toolInvocationSerialized") {
                    if let Some(id) = part["toolId"].as_str() {
                        bump_copilot(&mut tools, id);
                    }
                }
            }
        }
        if completion == 0 && tools.total() == 0 {
            continue;
        }
        let entry = by_cwd
            .entry(cwd.to_string())
            .or_insert_with(|| ProjectUsage {
                cwd: cwd.to_string(),
                ..Default::default()
            });
        entry.output_tokens = entry.output_tokens.saturating_add(completion);
        entry.assistant_messages = entry.assistant_messages.saturating_add(1);
        if ts.as_str() > entry.last_ts.as_str() {
            entry.last_ts = ts;
        }
        entry.tools += tools;
        contributed = true;
    }
    // One session per snapshot file, like Codex (per JSONL) and Cline
    // (per task dir).
    if contributed {
        if let Some(entry) = by_cwd.get_mut(cwd) {
            entry.sessions = entry.sessions.saturating_add(1);
        }
    }
}

fn scan_session_models(path: &Path, out: &mut ModelUsageByDay, since_iso: Option<&str>) {
    for_each_counted_request(path, since_iso, |_req, model, ts, completion| {
        let e = out
            .entry(ts[..10].to_string())
            .or_default()
            .entry(model.to_string())
            .or_insert((0, 0, 0));
        // e.0 (fresh input) stays 0 — promptTokens is never persisted.
        e.1 = e.1.saturating_add(completion);
        e.2 = e.2.saturating_add(1);
    });
}

fn read_requests(path: &Path) -> Option<Vec<serde_json::Value>> {
    let text = fs::read_to_string(path).ok()?;
    let session: serde_json::Value = serde_json::from_str(&text).ok()?;
    session["requests"].as_array().cloned()
}

/// Copilot Chat tool ids look like `copilot_readFile`,
/// `copilot_runInTerminal`, `copilot_replaceString`… Bucket by keyword
/// so vocabulary drift between VS Code versions degrades to `other`
/// instead of breaking.
fn bump_copilot(b: &mut ToolBreakdown, id: &str) {
    let n = id.to_ascii_lowercase();
    if n.contains("read") || n.contains("listdir") || n.contains("list_dir") {
        b.read = b.read.saturating_add(1);
    } else if n.contains("terminal") || n.contains("command") {
        b.bash = b.bash.saturating_add(1);
    } else if n.contains("edit")
        || n.contains("create")
        || n.contains("replace")
        || n.contains("write")
    {
        b.edit = b.edit.saturating_add(1);
    } else if n.contains("search") || n.contains("grep") || n.contains("find") {
        b.grep = b.grep.saturating_add(1);
    } else if n.contains("fetch") || n.contains("web") {
        b.web = b.web.saturating_add(1);
    } else {
        b.other = b.other.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_home(label: &str) -> std::path::PathBuf {
        let d =
            std::env::temp_dir().join(format!("tokenade-copilot-{}-{}", label, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// Create `workspaceStorage/<hash>/` with a workspace.json pointing at
    /// `folder_uri`, return its chatSessions dir.
    fn workspace(home: &Path, hash: &str, folder_uri: &str) -> std::path::PathBuf {
        let ws = home.join(".config/Code/User/workspaceStorage").join(hash);
        let sessions = ws.join("chatSessions");
        fs::create_dir_all(&sessions).unwrap();
        fs::write(
            ws.join("workspace.json"),
            format!(r#"{{"folder":"{folder_uri}"}}"#),
        )
        .unwrap();
        sessions
    }

    // 1780308000000 ms = 2026-06-01T10:00:00Z
    const T1: u64 = 1780308000000;

    #[test]
    fn aggregates_requests_with_cwd_from_workspace_json() {
        let home = tmp_home("aggregate");
        let dir = workspace(&home, "aaa111", "file:///projects/My%20Proj");
        fs::write(
            dir.join("s1.json"),
            format!(
                r#"{{"version":3,"sessionId":"s1","creationDate":{T1},"requests":[
                    {{"timestamp":{T1},"modelId":"gpt-4.1","completionTokens":120,
                      "response":[{{"kind":"toolInvocationSerialized","toolId":"copilot_readFile"}},
                                  {{"kind":"toolInvocationSerialized","toolId":"copilot_findTextInFiles"}}]}},
                    {{"timestamp":{},"modelId":"gpt-4.1","completionTokens":200,
                      "response":[{{"kind":"toolInvocationSerialized","toolId":"copilot_replaceString"}}]}}
                ]}}"#,
                T1 + 60_000
            ),
        )
        .unwrap();
        let stats = Parser.aggregate_since(&home, None);
        assert_eq!(stats.len(), 1);
        let p = &stats[0];
        assert_eq!(p.cwd, "/projects/My Proj", "file:// URI decoded incl. %20");
        assert_eq!(p.input_tokens, 0, "promptTokens never persisted by VS Code");
        assert_eq!(p.output_tokens, 320);
        assert_eq!(p.assistant_messages, 2);
        assert_eq!(p.sessions, 1);
        assert_eq!(p.tools.read, 1);
        assert_eq!(p.tools.grep, 1);
        assert_eq!(p.tools.edit, 1);
        assert!(p.last_ts.starts_with("2026-06-01T10:01"));
    }

    #[test]
    fn model_usage_counts_completion_only_and_includes_empty_window() {
        let home = tmp_home("models");
        let dir = workspace(&home, "bbb222", "file:///p");
        fs::write(
            dir.join("s1.json"),
            format!(
                r#"{{"version":3,"requests":[
                    {{"timestamp":{T1},"modelId":"gpt-4.1","completionTokens":50}},
                    {{"timestamp":{T1},"completionTokens":99}},
                    {{"timestamp":{T1},"modelId":"claude-sonnet-4.5"}}
                ]}}"#
            ),
        )
        .unwrap();
        let empty = home.join(".config/Code/User/globalStorage/emptyWindowChatSessions");
        fs::create_dir_all(&empty).unwrap();
        fs::write(
            empty.join("e1.json"),
            format!(
                r#"{{"version":3,"requests":[
                    {{"timestamp":{T1},"modelId":"gpt-4.1","completionTokens":7}}
                ]}}"#
            ),
        )
        .unwrap();
        let m = Parser.model_usage_by_day(&home, None);
        assert_eq!(
            m["2026-06-01"]["gpt-4.1"],
            (0, 57, 2),
            "input always 0; empty-window session merged in"
        );
        assert!(
            !m["2026-06-01"].contains_key("claude-sonnet-4.5"),
            "request without completionTokens contributes nothing"
        );

        // Empty-window sessions have no cwd → no project rows.
        let stats = Parser.aggregate_since(&home, None);
        assert_eq!(stats.len(), 1);
        assert_eq!(
            stats[0].output_tokens,
            50 + 99,
            "request without modelId still counts for the project"
        );
    }

    #[test]
    fn cutoff_filters_epoch_ms_and_accepts_iso_fallback() {
        let home = tmp_home("cutoff");
        let dir = workspace(&home, "ccc333", "file:///p");
        fs::write(
            dir.join("s1.json"),
            format!(
                r#"{{"version":3,"requests":[
                    {{"timestamp":1735689600000,"modelId":"gpt-4.1","completionTokens":999}},
                    {{"timestamp":{T1},"modelId":"gpt-4.1","completionTokens":10}},
                    {{"timestamp":"2026-06-02T08:00:00.000Z","modelId":"gpt-4.1","completionTokens":5}}
                ]}}"#
            ),
        )
        .unwrap();
        let stats = Parser.aggregate_since(&home, Some("2026-01-01T00:00:00Z"));
        assert_eq!(
            stats[0].output_tokens, 15,
            "2025 request filtered; ISO-string fallback kept"
        );
        let m = Parser.model_usage_by_day(&home, Some("2026-01-01T00:00:00Z"));
        assert_eq!(m["2026-06-01"]["gpt-4.1"], (0, 10, 1));
        assert_eq!(m["2026-06-02"]["gpt-4.1"], (0, 5, 1));
    }

    #[test]
    fn workspace_without_workspace_json_is_skipped_for_projects() {
        let home = tmp_home("nows");
        let sessions = home.join(".config/Code/User/workspaceStorage/ddd444/chatSessions");
        fs::create_dir_all(&sessions).unwrap();
        fs::write(
            sessions.join("s1.json"),
            format!(
                r#"{{"version":3,"requests":[{{"timestamp":{T1},"modelId":"gpt-4.1","completionTokens":50}}]}}"#
            ),
        )
        .unwrap();
        assert!(Parser.aggregate_since(&home, None).is_empty());
        // …but it still feeds the per-model view.
        let m = Parser.model_usage_by_day(&home, None);
        assert_eq!(m["2026-06-01"]["gpt-4.1"], (0, 50, 1));
    }

    #[test]
    fn no_storage_dir_returns_empty() {
        let home = tmp_home("nodir");
        assert!(Parser.aggregate_since(&home, None).is_empty());
        assert!(Parser.model_usage_by_day(&home, None).is_empty());
    }

    #[test]
    fn file_uri_decoding() {
        assert_eq!(
            file_uri_to_path("file:///home/u/proj").as_deref(),
            Some("/home/u/proj")
        );
        assert_eq!(
            file_uri_to_path("file:///c%3A/dev/x").as_deref(),
            Some("c:/dev/x"),
            "Windows drive URIs lose the leading slash"
        );
        assert_eq!(file_uri_to_path("vscode-remote://wsl/x"), None);
    }
}
