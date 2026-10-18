//! opencode (anomalyco/opencode, ex sst/opencode) transcript parser.
//!
//! Data root is `~/.local/share/opencode` on EVERY OS — opencode uses
//! `xdg-basedir` with no platform branching, so even Windows gets
//! `%USERPROFILE%\.local\share\opencode` (source-verified against
//! packages/core/src/global.ts, 2026-06-10). The `XDG_DATA_HOME` /
//! `OPENCODE_DB` overrides are intentionally ignored, like every other
//! parser's env overrides.
//!
//! Three storage generations, scanned in priority order (NEVER more
//! than one — the v1.2.0 migration keeps the old files around, so
//! reading both would double-count):
//!
//! 1. **SQLite** `<root>/opencode.db` (v1.2.0+, 2026-02-14; `kilo.db` for Kilo). Tables:
//!    `session(id, directory, …)` and `message(id, session_id,
//!    time_created, data)` where `data` is the assistant-message JSON
//!    below; tool calls live in `part(message_id, session_id,
//!    time_created, data)`.
//! 2. **Era B JSON** (late v0.x → v1.1.x):
//!    `<root>/storage/message/<sessionID>/<messageID>.json`.
//! 3. **Era A JSON** (early v0.x):
//!    `<root>/project/<slug>/storage/session/message/<sessionID>/<messageID>.json`.
//!
//! Assistant message JSON (`SessionV1.Assistant`, packages/core/src/v1/
//! session.ts):
//!
//! ```text
//! {"role":"assistant","modelID":"claude-opus-4","providerID":"anthropic",
//!  "time":{"created":<epoch-ms>,"completed":<epoch-ms>},
//!  "path":{"cwd":"/home/u/proj","root":"/home/u/proj"},
//!  "tokens":{"input":N,"output":N,"reasoning":N,
//!            "cache":{"read":N,"write":N}}}
//! ```
//!
//! Token mapping follows the shared convention: `tokens.input` is
//! fresh input, `cache.write` → cache_creation, `cache.read` →
//! cache_read (excluded from totals), `output + reasoning` → output.
//! The per-model "fresh input" is `input + cache.write`.
//!
//! cwd attribution: SQLite uses `session.directory`; JSON eras use the
//! message's own `path.cwd` (fallback `path.root`). Tool-call counting
//! is SQLite-only — JSON-era part files aren't worth a second tree
//! walk for v0.x installs — and defensive: parts that don't look like
//! `{"type":"tool","tool":"bash"}` contribute nothing.

use crate::agent_transcripts::{
    Agent, ModelUsageByDay, ProjectUsage, ToolBreakdown, TranscriptParser,
};
use crate::platform::epoch_to_iso8601_z;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

pub struct Parser;

impl TranscriptParser for Parser {
    fn agent(&self) -> Agent {
        Agent::Opencode
    }

    fn aggregate_since(&self, home_dir: &Path, since_iso: Option<&str>) -> Vec<ProjectUsage> {
        aggregate_in_root(&data_root(home_dir), since_iso)
    }

    fn model_usage_by_day(&self, home_dir: &Path, since_iso: Option<&str>) -> ModelUsageByDay {
        model_usage_in_root(&data_root(home_dir), since_iso)
    }

    fn session_models(
        &self,
        home_dir: &Path,
        since_iso: Option<&str>,
    ) -> Vec<crate::agent_transcripts::SessionModel> {
        session_models_in_root(&data_root(home_dir), since_iso)
    }
}

/// Aggregate usage from an opencode-schema data root. Exposed so forks that
/// reuse the SAME storage layout but a different root (Kilo Code →
/// `~/.local/share/kilo`) can delegate here instead of duplicating the parse.
pub(crate) fn aggregate_in_root(root: &Path, since_iso: Option<&str>) -> Vec<ProjectUsage> {
    let mut by_cwd: HashMap<String, ProjectUsage> = HashMap::new();
    let mut sessions: HashSet<(String, String)> = HashSet::new();
    if let Some(conn) = open_store(root) {
        sqlite_aggregate(&conn, &mut by_cwd, &mut sessions, since_iso);
    } else {
        for_each_json_message(root, &mut |sid, v| {
            let Some(cwd) = json_cwd(v) else { return };
            if record_usage(v, &cwd, &mut by_cwd, since_iso) {
                sessions.insert((cwd, sid.to_string()));
            }
        });
    }
    for (cwd, _) in sessions {
        if let Some(e) = by_cwd.get_mut(&cwd) {
            e.sessions = e.sessions.saturating_add(1);
        }
    }
    let mut out: Vec<ProjectUsage> = by_cwd.into_values().collect();
    out.sort_by(|a, b| b.last_ts.cmp(&a.last_ts));
    out
}

/// Per-day, per-model usage from an opencode-schema data root. Shared with
/// forks (see [`aggregate_in_root`]).
pub(crate) fn model_usage_in_root(root: &Path, since_iso: Option<&str>) -> ModelUsageByDay {
    let mut out = ModelUsageByDay::default();
    if let Some(conn) = open_store(root) {
        let Ok(mut stmt) = conn.prepare("SELECT data FROM message") else {
            return out;
        };
        let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0)) else {
            return out;
        };
        for data in rows.flatten() {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) {
                record_model(&v, &mut out, since_iso);
            }
        }
    } else {
        for_each_json_message(root, &mut |_sid, v| record_model(v, &mut out, since_iso));
    }
    out
}

/// Session `session_id`'s API calls and compactions, from an opencode-schema
/// data root, with the model it ran last: what makes its re-reads countable
/// call by call, as on Claude Code and Codex. Every API call ends in a
/// `step-finish` part carrying its tokens — the context it sent is `input +
/// cache.read + cache.write`; a `compaction` part marks a compaction. SQLite
/// storage only (v1.2.0+); `None` without it, or without a call.
pub fn session_timeline_in_root(
    root: &Path,
    session_id: &str,
) -> Option<(
    crate::claudecode_transcript::SessionTimeline,
    Option<String>,
)> {
    let conn = open_store(root)?;
    let mut tl = crate::claudecode_transcript::SessionTimeline::default();
    let mut stmt = conn
        .prepare("SELECT time_created, data FROM part WHERE session_id = ?1 ORDER BY time_created")
        .ok()?;
    let rows = stmt
        .query_map([session_id], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })
        .ok()?;
    for (ms, data) in rows.flatten() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) else {
            continue;
        };
        let ts = (ms.max(0) as u64) / 1000;
        match v["type"].as_str() {
            Some("step-finish") => {
                let t = &v["tokens"];
                let ctx = t["input"].as_u64().unwrap_or(0)
                    + t["cache"]["read"].as_u64().unwrap_or(0)
                    + t["cache"]["write"].as_u64().unwrap_or(0);
                if ctx > 0 {
                    tl.push_call(ts, ctx, t["cache"]["write"].as_u64().unwrap_or(0));
                }
            }
            Some("compaction") => tl.compactions.push((ts, 0)),
            _ => {}
        }
    }
    if tl.calls.is_empty() {
        return None;
    }
    let model = conn
        .query_row(
            "SELECT json_extract(data, '$.modelID') FROM message WHERE session_id = ?1 AND json_extract(data, '$.role') = 'assistant' ORDER BY time_created DESC LIMIT 1",
            [session_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten();
    Some((tl, model))
}

/// [`session_timeline_in_root`] at opencode's own root.
pub fn session_timeline(
    home_dir: &Path,
    session_id: &str,
) -> Option<(
    crate::claudecode_transcript::SessionTimeline,
    Option<String>,
)> {
    session_timeline_in_root(&data_root(home_dir), session_id)
}

/// `~/.local/share/opencode` — identical on Linux/macOS/Windows.
fn data_root(home_dir: &Path) -> PathBuf {
    home_dir.join(".local").join("share").join("opencode")
}

/// The store of an opencode-schema root: `opencode.db`, or `kilo.db` — Kilo
/// renamed it. MEASURED 2026-10-15 on a real Kilo 7.2.1 install: `kilo.db`
/// held 6 717 messages up to 2026-09-28, while the JSON tree the reader fell
/// back to stopped on 2026-02-23 (2 592 messages) — every Kilo call since its
/// migration was missing from usage.
fn open_store(root: &Path) -> Option<rusqlite::Connection> {
    ["opencode.db", "kilo.db"]
        .iter()
        .find_map(|f| open_db(&root.join(f)))
}

fn open_db(path: &Path) -> Option<rusqlite::Connection> {
    if !path.is_file() {
        return None;
    }
    rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()
}

/// Assistant usage out of a message JSON:
/// `(iso_ts, fresh_input, output_incl_reasoning, cache_write, cache_read)`.
fn assistant_usage(v: &serde_json::Value) -> Option<(String, u64, u64, u64, u64)> {
    if v["role"].as_str() != Some("assistant") {
        return None;
    }
    let t = &v["tokens"];
    if t.is_null() {
        return None;
    }
    let ms = v["time"]["created"].as_u64()?;
    let iso = epoch_to_iso8601_z(ms / 1000);
    let input = t["input"].as_u64().unwrap_or(0);
    let output = t["output"]
        .as_u64()
        .unwrap_or(0)
        .saturating_add(t["reasoning"].as_u64().unwrap_or(0));
    let write = t["cache"]["write"].as_u64().unwrap_or(0);
    let read = t["cache"]["read"].as_u64().unwrap_or(0);
    Some((iso, input, output, write, read))
}

/// Returns true when the message contributed (for the session counter).
fn record_usage(
    v: &serde_json::Value,
    cwd: &str,
    by_cwd: &mut HashMap<String, ProjectUsage>,
    since_iso: Option<&str>,
) -> bool {
    let Some((iso, input, output, write, read)) = assistant_usage(v) else {
        return false;
    };
    if since_iso.is_some_and(|c| iso.as_str() < c) {
        return false;
    }
    let entry = by_cwd
        .entry(cwd.to_string())
        .or_insert_with(|| ProjectUsage {
            cwd: cwd.to_string(),
            ..Default::default()
        });
    entry.input_tokens = entry.input_tokens.saturating_add(input);
    entry.output_tokens = entry.output_tokens.saturating_add(output);
    entry.cache_creation_tokens = entry.cache_creation_tokens.saturating_add(write);
    entry.cache_read_tokens = entry.cache_read_tokens.saturating_add(read);
    entry.assistant_messages = entry.assistant_messages.saturating_add(1);
    if iso.as_str() > entry.last_ts.as_str() {
        entry.last_ts = iso;
    }
    true
}

/// Per-(session, model) timestamp span from an opencode-schema data root.
/// Shared with forks (Kilo). Same record selection as [`record_model`] (same
/// `assistant_usage`, same non-empty-model + non-zero-usage filter, same cutoff)
/// — only the aggregation differs. The session id is opencode's own session id
/// (`session.id` in SQLite; the session dir name in the JSON eras), matching how
/// [`aggregate_in_root`] counts a session.
pub(crate) fn session_models_in_root(
    root: &Path,
    since_iso: Option<&str>,
) -> Vec<crate::agent_transcripts::SessionModel> {
    let mut acc: std::collections::BTreeMap<(String, String), (u64, u64)> = Default::default();
    if let Some(conn) = open_store(root) {
        if let Ok(mut stmt) =
            conn.prepare("SELECT s.id, m.data FROM message m JOIN session s ON m.session_id = s.id")
        {
            if let Ok(rows) =
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            {
                for (sid, data) in rows.flatten() {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) {
                        record_session_model(&sid, &v, &mut acc, since_iso);
                    }
                }
            }
        }
    } else {
        for_each_json_message(root, &mut |sid, v| {
            record_session_model(sid, v, &mut acc, since_iso)
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

fn record_session_model(
    sid: &str,
    v: &serde_json::Value,
    acc: &mut std::collections::BTreeMap<(String, String), (u64, u64)>,
    since_iso: Option<&str>,
) {
    let Some(model) = v["modelID"].as_str().filter(|m| !m.is_empty()) else {
        return;
    };
    let Some((iso, input, output, write, _read)) = assistant_usage(v) else {
        return;
    };
    if !iso.is_char_boundary(10) || since_iso.is_some_and(|c| iso.as_str() < c) {
        return;
    }
    let fresh = input.saturating_add(write);
    if fresh == 0 && output == 0 {
        return;
    }
    let Some(ms) = v["time"]["created"].as_u64() else {
        return;
    };
    let e = acc
        .entry((sid.to_string(), model.to_string()))
        .or_insert((ms, ms));
    if ms < e.0 {
        e.0 = ms;
    }
    if ms > e.1 {
        e.1 = ms;
    }
}

fn record_model(v: &serde_json::Value, out: &mut ModelUsageByDay, since_iso: Option<&str>) {
    let Some(model) = v["modelID"].as_str().filter(|m| !m.is_empty()) else {
        return;
    };
    let Some((iso, input, output, write, _read)) = assistant_usage(v) else {
        return;
    };
    if !iso.is_char_boundary(10) || since_iso.is_some_and(|c| iso.as_str() < c) {
        return;
    }
    let fresh = input.saturating_add(write);
    if fresh == 0 && output == 0 {
        return;
    }
    let e = out
        .entry(iso[..10].to_string())
        .or_default()
        .entry(model.to_string())
        .or_insert((0, 0, 0));
    e.0 = e.0.saturating_add(fresh);
    e.1 = e.1.saturating_add(output);
    e.2 = e.2.saturating_add(1);
}

fn sqlite_aggregate(
    conn: &rusqlite::Connection,
    by_cwd: &mut HashMap<String, ProjectUsage>,
    sessions: &mut HashSet<(String, String)>,
    since_iso: Option<&str>,
) {
    let Ok(mut stmt) = conn.prepare(
        "SELECT s.directory, s.id, m.data FROM message m JOIN session s ON m.session_id = s.id",
    ) else {
        return;
    };
    let Ok(rows) = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    }) else {
        return;
    };
    for (dir, sid, data) in rows.flatten() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) else {
            continue;
        };
        if record_usage(&v, &dir, by_cwd, since_iso) {
            sessions.insert((dir, sid));
        }
    }
    // Tool calls — defensive: only parts shaped {"type":"tool","tool":"…"}.
    let Ok(mut stmt) = conn.prepare(
        "SELECT s.directory, p.time_created, p.data FROM part p JOIN session s ON p.session_id = s.id",
    ) else {
        return;
    };
    let Ok(rows) = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1).unwrap_or(0),
            r.get::<_, String>(2)?,
        ))
    }) else {
        return;
    };
    for (dir, ts_ms, data) in rows.flatten() {
        if ts_ms > 0 {
            let iso = epoch_to_iso8601_z(ts_ms as u64 / 1000);
            if since_iso.is_some_and(|c| iso.as_str() < c) {
                continue;
            }
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) else {
            continue;
        };
        if v["type"].as_str() != Some("tool") {
            continue;
        }
        let Some(tool) = v["tool"].as_str() else {
            continue;
        };
        // Tool parts only enrich projects that already have messages —
        // a part without any assistant usage shouldn't create a row.
        if let Some(entry) = by_cwd.get_mut(&dir) {
            bump_opencode(&mut entry.tools, tool);
        }
    }
}

/// Visit every message JSON under the era-B layout, falling back to
/// era A when `<root>/storage/` doesn't exist. Callback gets
/// `(sessionID, message_json)`.
fn for_each_json_message(root: &Path, visit: &mut dyn FnMut(&str, &serde_json::Value)) {
    let era_b = root.join("storage").join("message");
    let roots: Vec<PathBuf> = if era_b.is_dir() {
        vec![era_b]
    } else {
        let Ok(slugs) = fs::read_dir(root.join("project")) else {
            return;
        };
        slugs
            .flatten()
            .map(|s| s.path().join("storage").join("session").join("message"))
            .filter(|p| p.is_dir())
            .collect()
    };
    for msg_root in roots {
        let Ok(session_dirs) = fs::read_dir(&msg_root) else {
            continue;
        };
        for sdir in session_dirs.flatten() {
            let sid = sdir.file_name().to_string_lossy().into_owned();
            let Ok(files) = fs::read_dir(sdir.path()) else {
                continue;
            };
            for f in files.flatten() {
                let p = f.path();
                if p.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                if let Some(v) = fs::read_to_string(&p)
                    .ok()
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                {
                    visit(&sid, &v);
                }
            }
        }
    }
}

fn json_cwd(v: &serde_json::Value) -> Option<String> {
    v["path"]["cwd"]
        .as_str()
        .or_else(|| v["path"]["root"].as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// opencode core tool vocabulary: bash, edit, write, patch, read,
/// grep, glob, list, webfetch, websearch, task, todowrite/todoread.
fn bump_opencode(b: &mut ToolBreakdown, name: &str) {
    match name {
        "read" => b.read = b.read.saturating_add(1),
        "glob" | "list" => b.glob = b.glob.saturating_add(1),
        "bash" => b.bash = b.bash.saturating_add(1),
        "edit" | "write" | "patch" => b.edit = b.edit.saturating_add(1),
        "grep" => b.grep = b.grep.saturating_add(1),
        "webfetch" | "websearch" => b.web = b.web.saturating_add(1),
        _ => b.other = b.other.saturating_add(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_home(label: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "tokenade-opencode-{}-{}",
            label,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    // 1780308000000 ms = 2026-06-01T10:00:00Z
    const T1: u64 = 1_780_308_000_000;

    fn assistant_json(ms: u64, model: &str, cwd: &str) -> String {
        format!(
            r#"{{"role":"assistant","modelID":"{model}","providerID":"anthropic",
                "time":{{"created":{ms},"completed":{}}},
                "path":{{"cwd":"{cwd}","root":"{cwd}"}},
                "tokens":{{"input":100,"output":20,"reasoning":5,"cache":{{"read":400,"write":30}}}}}}"#,
            ms + 4000
        )
    }

    fn make_db(home: &Path) -> rusqlite::Connection {
        let root = data_root(home);
        fs::create_dir_all(&root).unwrap();
        let conn = rusqlite::Connection::open(root.join("opencode.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY, project_id TEXT, directory TEXT NOT NULL);
             CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT);
             CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, time_created INTEGER, data TEXT);",
        )
        .unwrap();
        conn
    }

    /// Each `step-finish` is an API call with the context it sent; a
    /// `compaction` part a compaction; other sessions are not read.
    #[test]
    fn a_session_timeline_is_read_from_its_step_finish_parts() {
        let home = tmp_home("timeline");
        let conn = make_db(&home);
        conn.execute(
            "INSERT INTO message VALUES ('m1','ses_t',?1,?2)",
            rusqlite::params![T1 as i64, assistant_json(T1, "claude-opus-4", "/p")],
        )
        .unwrap();
        let step = |id: &str, sess: &str, ms: u64, input: u64, read: u64| {
            let data = format!(
                r#"{{"type":"step-finish","tokens":{{"input":{input},"output":5,"cache":{{"read":{read},"write":10}}}}}}"#
            );
            conn.execute(
                "INSERT INTO part VALUES (?1,'m1',?2,?3,?4)",
                rusqlite::params![id, sess, ms as i64, data],
            )
            .unwrap();
        };
        step("p1", "ses_t", T1, 100, 0);
        step("p2", "ses_t", T1 + 2_000, 20, 110);
        conn.execute(
            "INSERT INTO part VALUES ('p3','m1','ses_t',?1,'{\"type\":\"compaction\"}')",
            rusqlite::params![(T1 + 3_000) as i64],
        )
        .unwrap();
        step("p4", "ses_other", T1, 999, 0);
        let (tl, model) = session_timeline(&home, "ses_t").unwrap();
        let t = T1 / 1000;
        assert_eq!(tl.calls, vec![(t, 110), (t + 2, 140)]);
        assert_eq!(tl.compactions, vec![(t + 3, 0)]);
        assert_eq!(model.as_deref(), Some("claude-opus-4"));
        assert!(session_timeline(&home, "ses_none").is_none());
        let _ = fs::remove_dir_all(&home);
    }

    /// Kilo's store is `kilo.db`, same schema: read like `opencode.db`.
    #[test]
    fn a_kilo_db_is_the_store_too() {
        let root = tmp_home("kilodb");
        let conn = rusqlite::Connection::open(root.join("kilo.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT);
             CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, time_created INTEGER, data TEXT);",
        )
        .unwrap();
        conn.execute("INSERT INTO part VALUES ('p1','m1','ses_k',?1,'{\"type\":\"step-finish\",\"tokens\":{\"input\":7,\"cache\":{\"read\":3,\"write\":0}}}')", rusqlite::params![T1 as i64]).unwrap();
        let (tl, _) = session_timeline_in_root(&root, "ses_k").unwrap();
        assert_eq!(tl.calls, vec![(T1 / 1000, 10)]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn sqlite_aggregate_models_and_tools() {
        let home = tmp_home("sqlite");
        let conn = make_db(&home);
        conn.execute(
            "INSERT INTO session VALUES ('ses_1','proj','/projects/foo')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message VALUES ('msg_1','ses_1',?1,?2)",
            rusqlite::params![
                T1 as i64,
                assistant_json(T1, "claude-opus-4", "/projects/foo")
            ],
        )
        .unwrap();
        // User message: ignored.
        conn.execute(
            "INSERT INTO message VALUES ('msg_2','ses_1',?1,'{\"role\":\"user\",\"time\":{\"created\":1}}')",
            rusqlite::params![T1 as i64],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part VALUES ('prt_1','msg_1','ses_1',?1,'{\"type\":\"tool\",\"tool\":\"bash\"}')",
            rusqlite::params![T1 as i64],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part VALUES ('prt_2','msg_1','ses_1',?1,'{\"type\":\"text\",\"text\":\"hi\"}')",
            rusqlite::params![T1 as i64],
        )
        .unwrap();
        drop(conn);

        let stats = Parser.aggregate_since(&home, None);
        assert_eq!(stats.len(), 1);
        let p = &stats[0];
        assert_eq!(p.cwd, "/projects/foo");
        assert_eq!(p.input_tokens, 100);
        assert_eq!(p.output_tokens, 25, "output folds reasoning");
        assert_eq!(p.cache_creation_tokens, 30);
        assert_eq!(p.cache_read_tokens, 400);
        assert_eq!(p.assistant_messages, 1);
        assert_eq!(p.sessions, 1);
        assert_eq!(p.tools.bash, 1);
        assert_eq!(p.tools.other, 0, "non-tool parts ignored");
        assert!(p.last_ts.starts_with("2026-06-01T10:00"));

        let m = Parser.model_usage_by_day(&home, None);
        assert_eq!(
            m["2026-06-01"]["claude-opus-4"],
            (130, 25, 1),
            "fresh = input 100 + cache.write 30; cache.read excluded"
        );
    }

    #[test]
    fn sqlite_cutoff_filters_messages() {
        let home = tmp_home("cutoff");
        let conn = make_db(&home);
        conn.execute("INSERT INTO session VALUES ('ses_1','p','/p')", [])
            .unwrap();
        // 1735689600000 ms = 2025-01-01.
        conn.execute(
            "INSERT INTO message VALUES ('msg_old','ses_1',1735689600000,?1)",
            rusqlite::params![assistant_json(1_735_689_600_000, "claude-opus-4", "/p")],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message VALUES ('msg_new','ses_1',?1,?2)",
            rusqlite::params![T1 as i64, assistant_json(T1, "claude-opus-4", "/p")],
        )
        .unwrap();
        drop(conn);
        let stats = Parser.aggregate_since(&home, Some("2026-01-01T00:00:00Z"));
        assert_eq!(stats[0].assistant_messages, 1, "2025 message filtered");
        let m = Parser.model_usage_by_day(&home, Some("2026-01-01T00:00:00Z"));
        assert_eq!(m.len(), 1);
        assert!(m.contains_key("2026-06-01"));
    }

    #[test]
    fn era_b_json_fallback_and_db_priority() {
        let home = tmp_home("erab");
        let msgs = data_root(&home).join("storage/message/ses_9");
        fs::create_dir_all(&msgs).unwrap();
        fs::write(
            msgs.join("msg_1.json"),
            assistant_json(T1, "gpt-5", "/projects/bar"),
        )
        .unwrap();

        let stats = Parser.aggregate_since(&home, None);
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].cwd, "/projects/bar");
        assert_eq!(stats[0].sessions, 1);
        let m = Parser.model_usage_by_day(&home, None);
        assert_eq!(m["2026-06-01"]["gpt-5"], (130, 25, 1));

        // Now drop in a db: the JSON must be IGNORED (migration keeps
        // originals — counting both would double-count).
        let conn = make_db(&home);
        conn.execute("INSERT INTO session VALUES ('ses_1','p','/db-only')", [])
            .unwrap();
        conn.execute(
            "INSERT INTO message VALUES ('msg_1','ses_1',?1,?2)",
            rusqlite::params![T1 as i64, assistant_json(T1, "gpt-5", "/db-only")],
        )
        .unwrap();
        drop(conn);
        let stats = Parser.aggregate_since(&home, None);
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].cwd, "/db-only", "db present → JSON era ignored");
    }

    #[test]
    fn era_a_legacy_layout() {
        let home = tmp_home("eraa");
        let msgs = data_root(&home).join("project/-home-u-proj/storage/session/message/ses_2");
        fs::create_dir_all(&msgs).unwrap();
        fs::write(
            msgs.join("msg_1.json"),
            assistant_json(T1, "claude-sonnet-4", "/home/u/proj"),
        )
        .unwrap();
        let stats = Parser.aggregate_since(&home, None);
        assert_eq!(stats.len(), 1);
        assert_eq!(
            stats[0].cwd, "/home/u/proj",
            "cwd from message path, not slug"
        );
        let m = Parser.model_usage_by_day(&home, None);
        assert_eq!(m["2026-06-01"]["claude-sonnet-4"], (130, 25, 1));
    }

    #[test]
    fn missing_root_returns_empty() {
        let home = tmp_home("missing");
        assert!(Parser.aggregate_since(&home, None).is_empty());
        assert!(Parser.model_usage_by_day(&home, None).is_empty());
    }
}
