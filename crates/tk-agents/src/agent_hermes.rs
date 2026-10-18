//! Hermes (NousResearch hermes-agent) transcript parser.
//!
//! Unlike every other agent, Hermes persists its session history to a
//! **SQLite** database, not JSONL — `~/.hermes/state.db` (or
//! `$HERMES_HOME/state.db`; `%LOCALAPPDATA%\hermes\state.db` on Windows).
//! Schema (verbatim from `hermes_state.py`, 2026-06):
//!   - `sessions(id TEXT PK, model, started_at REAL, cwd, input_tokens,
//!     output_tokens, cache_read_tokens, cache_write_tokens, archived, …)`
//!     — token columns are CUMULATIVE per-session totals (read directly;
//!     SUM across sessions for a project), `started_at` is epoch SECONDS
//!     as a REAL.
//!   - `messages(session_id→sessions.id, role, tool_name, token_count,
//!     timestamp REAL, …)` — per-message granularity for assistant-turn
//!     counts and per-tool result tokens.
//!
//! Mapping to [`ProjectUsage`]: Hermes `cache_write_tokens` is Claude's
//! `cache_creation_tokens`. Sessions with NULL/empty `cwd` are skipped for
//! per-project rows (they still feed the per-model view), matching the
//! other parsers' "no workspace → models only" rule.

use crate::agent_transcripts::{Agent, ModelUsageByDay, ProjectUsage, TranscriptParser};
use crate::platform::epoch_to_iso8601_z;
use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct Parser;

/// Resolve Hermes' state.db. `$HERMES_HOME` wins (Hermes honours it, incl.
/// the `active_profile` redirect); else the platform default home.
fn state_db_path(home_dir: &Path) -> PathBuf {
    if let Some(h) = std::env::var_os("HERMES_HOME") {
        return PathBuf::from(h).join("state.db");
    }
    if cfg!(windows) {
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir.join("AppData").join("Local"));
        return base.join("hermes").join("state.db");
    }
    home_dir.join(".hermes").join("state.db")
}

/// Open the db read-only. Returns None if absent/unreadable (graceful — the
/// agent simply contributes nothing). Read-only + a short busy timeout so a
/// live Hermes writing in WAL mode doesn't make us fail or block.
fn open_ro(path: &Path) -> Option<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .ok()?;
    let _ = conn.busy_timeout(std::time::Duration::from_millis(200));
    Some(conn)
}

/// epoch seconds (REAL) → ISO-8601 Z, the form the rest of the dashboard
/// sorts/filters on. Mirrors the copilot parser's epoch→ISO comparison so a
/// `since_iso` cutoff is a plain lexicographic compare (ISO is monotonic).
fn ts_iso(epoch_secs: f64) -> String {
    epoch_to_iso8601_z(if epoch_secs > 0.0 {
        epoch_secs as u64
    } else {
        0
    })
}

fn after_cutoff(iso: &str, since_iso: Option<&str>) -> bool {
    match since_iso {
        Some(cut) => iso.as_bytes() >= cut.as_bytes(),
        None => true,
    }
}

impl TranscriptParser for Parser {
    fn agent(&self) -> Agent {
        Agent::Hermes
    }

    fn aggregate_since(&self, home_dir: &Path, since_iso: Option<&str>) -> Vec<ProjectUsage> {
        let Some(conn) = open_ro(&state_db_path(home_dir)) else {
            return Vec::new();
        };
        let mut by_cwd: HashMap<String, ProjectUsage> = HashMap::new();

        // Pass 1 — per-session cumulative token totals, grouped by cwd.
        if let Ok(mut stmt) = conn.prepare(
            "SELECT cwd, started_at, input_tokens, output_tokens, \
             cache_read_tokens, cache_write_tokens \
             FROM sessions WHERE archived = 0",
        ) {
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, Option<f64>>(1)?.unwrap_or(0.0),
                    r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                    r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                    r.get::<_, Option<i64>>(4)?.unwrap_or(0),
                    r.get::<_, Option<i64>>(5)?.unwrap_or(0),
                ))
            });
            if let Ok(rows) = rows {
                for row in rows.flatten() {
                    let (cwd, started, inp, out, cr, cw) = row;
                    let cwd = match cwd {
                        Some(c) if !c.trim().is_empty() => c,
                        _ => continue, // no project → models-only, skip per-project
                    };
                    let iso = ts_iso(started);
                    if !after_cutoff(&iso, since_iso) {
                        continue;
                    }
                    let e = by_cwd.entry(cwd.clone()).or_default();
                    e.cwd = cwd;
                    e.sessions += 1;
                    e.input_tokens += inp.max(0) as u64;
                    e.output_tokens += out.max(0) as u64;
                    e.cache_read_tokens += cr.max(0) as u64;
                    e.cache_creation_tokens += cw.max(0) as u64; // Hermes cache_write
                    if iso > e.last_ts {
                        e.last_ts = iso;
                    }
                }
            }
        }

        // Pass 2 — message-level: assistant-turn counts + per-tool result
        // tokens (Hermes records token_count per message, with tool_name on
        // tool-result rows). Only attaches to projects pass 1 already found.
        if let Ok(mut stmt) = conn.prepare(
            "SELECT s.cwd, m.role, m.tool_name, m.token_count, m.timestamp \
             FROM messages m JOIN sessions s ON m.session_id = s.id \
             WHERE s.archived = 0",
        ) {
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                    r.get::<_, Option<f64>>(4)?.unwrap_or(0.0),
                ))
            });
            if let Ok(rows) = rows {
                for row in rows.flatten() {
                    let (cwd, role, tool_name, tok, ts) = row;
                    let Some(cwd) = cwd.filter(|c| !c.trim().is_empty()) else {
                        continue;
                    };
                    if !after_cutoff(&ts_iso(ts), since_iso) {
                        continue;
                    }
                    let Some(e) = by_cwd.get_mut(&cwd) else {
                        continue;
                    };
                    if role.as_deref() == Some("assistant") {
                        e.assistant_messages += 1;
                    }
                    if let Some(name) = tool_name.filter(|n| !n.trim().is_empty()) {
                        let t = tok.max(0) as u64;
                        e.tool_result_tokens += t;
                        *e.tool_result_tokens_by_tool.entry(name).or_insert(0) += t;
                    }
                }
            }
        }

        let mut out: Vec<ProjectUsage> = by_cwd.into_values().collect();
        out.sort_by(|a, b| b.last_ts.cmp(&a.last_ts));
        out
    }

    fn model_usage_by_day(&self, home_dir: &Path, since_iso: Option<&str>) -> ModelUsageByDay {
        let mut out = ModelUsageByDay::default();
        let Some(conn) = open_ro(&state_db_path(home_dir)) else {
            return out;
        };
        let Ok(mut stmt) = conn.prepare(
            "SELECT model, started_at, input_tokens, output_tokens \
             FROM sessions WHERE archived = 0",
        ) else {
            return out;
        };
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, Option<f64>>(1)?.unwrap_or(0.0),
                r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                r.get::<_, Option<i64>>(3)?.unwrap_or(0),
            ))
        });
        if let Ok(rows) = rows {
            for row in rows.flatten() {
                let (model, started, inp, out_tok) = row;
                let Some(model) = model.filter(|m| !m.trim().is_empty()) else {
                    continue; // never guess a model
                };
                let iso = ts_iso(started);
                if !after_cutoff(&iso, since_iso) {
                    continue;
                }
                let day = iso.get(..10).unwrap_or("").to_string();
                let e = out
                    .entry(day)
                    .or_default()
                    .entry(model)
                    .or_insert((0, 0, 0));
                e.0 += inp.max(0) as u64;
                e.1 += out_tok.max(0) as u64;
                e.2 += 1;
            }
        }
        out
    }

    fn session_models(
        &self,
        home_dir: &Path,
        since_iso: Option<&str>,
    ) -> Vec<crate::agent_transcripts::SessionModel> {
        // Same source rows as model_usage_by_day (the `sessions` table, non-empty
        // model, `started_at` cutoff). Hermes stores one `started_at` per session
        // (token totals are cumulative — there is no per-message model timeline),
        // so the span is that single point: start_ms == end_ms. The session id is
        // `sessions.id`.
        let mut out = Vec::new();
        let Some(conn) = open_ro(&state_db_path(home_dir)) else {
            return out;
        };
        let Ok(mut stmt) =
            conn.prepare("SELECT id, model, started_at FROM sessions WHERE archived = 0")
        else {
            return out;
        };
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<f64>>(2)?.unwrap_or(0.0),
            ))
        });
        if let Ok(rows) = rows {
            for row in rows.flatten() {
                let (id, model, started) = row;
                let Some(id) = id.filter(|s| !s.trim().is_empty()) else {
                    continue;
                };
                let Some(model) = model.filter(|m| !m.trim().is_empty()) else {
                    continue; // never guess a model
                };
                if !after_cutoff(&ts_iso(started), since_iso) {
                    continue;
                }
                let ms = if started > 0.0 {
                    (started * 1000.0) as u64
                } else {
                    0
                };
                out.push(crate::agent_transcripts::SessionModel {
                    sess: id,
                    model,
                    start_ms: ms,
                    end_ms: ms,
                });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes the tests that pin `HERMES_HOME`. `state_db_path` resolves
    /// `$HERMES_HOME/state.db` first on EVERY OS — without pinning it, the
    /// Windows branch reads `%LOCALAPPDATA%\hermes` instead of the seeded
    /// tempdir and the parser finds nothing (a former Windows-only panic).
    static HERMES_HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// RAII guard: pin `HERMES_HOME` to `dir/.hermes` so the seeded db is found
    /// on all OSes; restores the previous value (and releases the lock) on drop.
    struct HermesHomeGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        saved: Option<std::ffi::OsString>,
    }
    impl HermesHomeGuard {
        fn pin(hermes_home: &Path) -> Self {
            let lock = HERMES_HOME_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            let saved = std::env::var_os("HERMES_HOME");
            std::env::set_var("HERMES_HOME", hermes_home);
            Self { _lock: lock, saved }
        }
    }
    impl Drop for HermesHomeGuard {
        fn drop(&mut self) {
            match self.saved.take() {
                Some(v) => std::env::set_var("HERMES_HOME", v),
                None => std::env::remove_var("HERMES_HOME"),
            }
        }
    }

    fn seed_db(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions(id TEXT PRIMARY KEY, model TEXT, started_at REAL NOT NULL, \
               cwd TEXT, input_tokens INTEGER DEFAULT 0, output_tokens INTEGER DEFAULT 0, \
               cache_read_tokens INTEGER DEFAULT 0, cache_write_tokens INTEGER DEFAULT 0, \
               archived INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE messages(id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL, \
               role TEXT, tool_name TEXT, token_count INTEGER, timestamp REAL NOT NULL);
             -- 2026-06-01T00:00:00Z = 1780272000 ; project /proj/a
             INSERT INTO sessions VALUES('s1','claude-x',1780272000,'/proj/a',100,40,900,80,0);
             INSERT INTO sessions VALUES('s2','claude-x',1780272100,'/proj/a',10,5,0,0,0);
             -- archived row must be ignored
             INSERT INTO sessions VALUES('s3','claude-x',1780272200,'/proj/a',999,999,999,999,1);
             -- NULL cwd: models-only, skipped from per-project rows
             INSERT INTO sessions VALUES('s4','claude-x',1780272300,NULL,7,3,0,0,0);
             -- old session, filtered by a 2026 cutoff (2025-01-01 = 1735689600)
             INSERT INTO sessions VALUES('s5','old-model',1735689600,'/proj/a',5000,5000,0,0,0);
             INSERT INTO messages(session_id,role,tool_name,token_count,timestamp) \
               VALUES('s1','assistant',NULL,12,1780272000);
             INSERT INTO messages(session_id,role,tool_name,token_count,timestamp) \
               VALUES('s1','tool','read_file',300,1780272001);
             INSERT INTO messages(session_id,role,tool_name,token_count,timestamp) \
               VALUES('s2','assistant',NULL,4,1780272100);",
        )
        .unwrap();
    }

    #[test]
    fn aggregates_sessions_by_cwd_with_cumulative_tokens() {
        let dir = std::env::temp_dir().join(format!("tk-hermes-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let _hh = HermesHomeGuard::pin(&dir.join(".hermes"));
        let db = dir.join(".hermes").join("state.db");
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        seed_db(&db);

        // No cutoff: /proj/a = s1 + s2 + s5 (s3 archived and s4's NULL cwd
        // are both excluded from per-project rows).
        let usage = Parser.aggregate_since(&dir, None);
        assert_eq!(usage.len(), 1, "one project (/proj/a)");
        let p = &usage[0];
        assert_eq!(p.cwd, "/proj/a");
        assert_eq!(p.sessions, 3);
        assert_eq!(p.input_tokens, 5110); // 100 + 10 + 5000
        assert_eq!(p.cache_creation_tokens, 80); // cache_write mapped, summed
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cutoff_filters_old_sessions_and_models_skip_null_cwd() {
        let dir = std::env::temp_dir().join(format!("tk-hermes-test2-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let _hh = HermesHomeGuard::pin(&dir.join(".hermes"));
        let db = dir.join(".hermes").join("state.db");
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        seed_db(&db);

        let usage = Parser.aggregate_since(&dir, Some("2026-01-01T00:00:00Z"));
        let p = usage.iter().find(|p| p.cwd == "/proj/a").unwrap();
        // s5 (2025) filtered out; s1+s2 remain.
        assert_eq!(p.sessions, 2);
        assert_eq!(p.input_tokens, 110); // 100 + 10
        assert_eq!(p.output_tokens, 45); // 40 + 5
        assert_eq!(p.cache_read_tokens, 900);
        assert_eq!(p.cache_creation_tokens, 80); // cache_write mapped
        assert_eq!(p.assistant_messages, 2); // s1 + s2 assistant rows
        assert_eq!(p.tool_result_tokens, 300);
        assert_eq!(p.tool_result_tokens_by_tool.get("read_file"), Some(&300));

        // models include the NULL-cwd session (s4) but exclude old (s5).
        let m = Parser.model_usage_by_day(&dir, Some("2026-01-01T00:00:00Z"));
        let day = &m["2026-06-01"];
        assert!(day.contains_key("claude-x"));
        assert!(!m.values().any(|d| d.contains_key("old-model")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Real-data validation against an actual `~/.hermes/state.db`. Ignored by
    /// default (machine-specific; needs a real Hermes install + session). Run
    /// with `cargo test -p tokenade-core agent_hermes::tests::real_hermes -- --ignored --nocapture`.
    #[test]
    #[ignore = "machine-specific: the developer's real `hermes` install"]
    fn real_hermes_smoke() {
        let home = std::path::PathBuf::from(std::env::var("HOME").expect("HOME"));
        let _hh = HermesHomeGuard::pin(&home.join(".hermes"));
        let rows = Parser.aggregate_since(&home, None);
        eprintln!("== hermes aggregate: {} cwd bucket(s) ==", rows.len());
        for r in &rows {
            eprintln!(
                "  cwd={} sessions={} in={} out={} cache_read={} cache_create={} tools={} by_tool={:?}",
                r.cwd,
                r.sessions,
                r.input_tokens,
                r.output_tokens,
                r.cache_read_tokens,
                r.cache_creation_tokens,
                r.tools.total(),
                r.tool_result_tokens_by_tool
            );
        }
        let m = Parser.model_usage_by_day(&home, None);
        eprintln!("== hermes model_usage_by_day: {} day(s) ==", m.len());
        assert!(
            !rows.is_empty(),
            "expected ≥1 hermes session on this machine"
        );
    }
}
