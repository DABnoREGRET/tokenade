//! Agents that keep their token accounting in a SQLite table.
//!
//! Four of them (Goose, Crush, zcode, Zerostack) landed together because the
//! work is the same shape: find one database, run one query, map columns onto
//! [`ProjectUsage`]. Sharing the walk here beats four near-identical modules.
//!
//! Schemas source-verified 2026-07-29 against codeburn's provider parsers.
//!
//! What is deliberately NOT here: Cursor, Cursor Agent, Kiro, Warp and
//! Quickdesk's DB path. Those agents write no token counts at all, and the
//! reference implementation reconstructs them by dividing character counts by
//! four. Our ledger separates measurement from estimate on purpose
//! (`gain.jsonl` vs `gain_estimated.jsonl`), and `coverage_notes()` exists to
//! tell a user why an agent shows nothing. Feeding text-length guesses into
//! the measured totals would trade that honesty for a bigger number.

use crate::agent_transcripts::{Agent, TranscriptParser};
use crate::claudecode_transcript::ProjectUsage;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Milliseconds since the epoch for `2001-09-09T01:46:40Z`. Any timestamp
/// below this, read as milliseconds, is really seconds — the discriminator
/// several of these schemas need because their columns disagree with their own
/// documentation (Crush's comments say ms, its INSERTs write seconds).
const MS_FLOOR: i64 = 1_000_000_000_000;

fn epoch_to_iso(raw: i64) -> String {
    let secs = if raw.abs() >= MS_FLOOR {
        raw / 1000
    } else {
        raw
    };
    crate::platform::epoch_to_iso8601_z(secs.max(0) as u64)
}

/// `SELECT`-and-map, with every failure mode collapsing to "no rows". A
/// missing database is the normal case (the user does not run this agent), a
/// locked or schema-drifted one must not take the dashboard down with it.
pub(crate) fn query_db<T>(
    path: &Path,
    sql: &str,
    mut row: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
) -> Vec<T> {
    if !path.exists() {
        return Vec::new();
    }
    // Read-only, and never create: opening a missing db read-write would
    // materialise an empty file in the user's data dir for an agent they do
    // not use.
    let Ok(conn) = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return Vec::new();
    };
    let Ok(mut stmt) = conn.prepare(sql) else {
        return Vec::new();
    };
    let Ok(mapped) = stmt.query_map([], |r| row(r)) else {
        return Vec::new();
    };
    mapped.flatten().collect()
}

/// Fold one session's totals into the per-cwd aggregate.
#[allow(clippy::too_many_arguments)]
pub(crate) fn add(
    by_cwd: &mut HashMap<String, ProjectUsage>,
    cwd: String,
    input: u64,
    output: u64,
    cache_creation: u64,
    cache_read: u64,
    messages: u64,
    ts_iso: String,
) {
    let e = by_cwd.entry(cwd.clone()).or_insert_with(|| ProjectUsage {
        cwd,
        ..Default::default()
    });
    e.sessions += 1;
    e.assistant_messages += messages;
    e.input_tokens += input;
    e.output_tokens += output;
    e.cache_creation_tokens += cache_creation;
    e.cache_read_tokens += cache_read;
    if ts_iso > e.last_ts {
        e.last_ts = ts_iso;
    }
}

pub(crate) fn finish(by_cwd: HashMap<String, ProjectUsage>) -> Vec<ProjectUsage> {
    let mut out: Vec<ProjectUsage> = by_cwd.into_values().collect();
    out.sort_by(|a, b| b.last_ts.cmp(&a.last_ts));
    out
}

pub(crate) fn after_cutoff(ts_iso: &str, since_iso: Option<&str>) -> bool {
    match since_iso {
        Some(cut) => ts_iso >= cut,
        None => true,
    }
}

// ── Goose ────────────────────────────────────────────────────────────
// `sessions` carries running totals per session, not per turn, so one row
// becomes one ProjectUsage contribution. There is no per-message split to
// recover and we do not invent one.

pub struct GooseParser;

fn goose_db(home: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(root) = crate::platform::ambient_override(home, "GOOSE_PATH_ROOT") {
        out.push(root.join("data/sessions/sessions.db"));
    }
    let xdg = crate::platform::xdg_data_root(home);
    out.push(xdg.join("goose/sessions/sessions.db"));
    out.push(home.join("AppData/Roaming/Block/goose/sessions/sessions.db"));
    out
}

impl TranscriptParser for GooseParser {
    fn agent(&self) -> Agent {
        Agent::Goose
    }

    fn aggregate_since(&self, home_dir: &Path, since_iso: Option<&str>) -> Vec<ProjectUsage> {
        let mut by_cwd = HashMap::new();
        for db in goose_db(home_dir) {
            for (dir, input, output, ts) in query_db(
                &db,
                "SELECT COALESCE(working_dir,''), COALESCE(accumulated_input_tokens,0), \
                 COALESCE(accumulated_output_tokens,0), \
                 COALESCE(updated_at, created_at, '') FROM sessions",
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?.max(0) as u64,
                        r.get::<_, i64>(2)?.max(0) as u64,
                        r.get::<_, String>(3)?,
                    ))
                },
            ) {
                if input == 0 && output == 0 {
                    continue;
                }
                if !after_cutoff(&ts, since_iso) {
                    continue;
                }
                add(&mut by_cwd, dir, input, output, 0, 0, 1, ts);
            }
        }
        finish(by_cwd)
    }
}

// ── Crush ────────────────────────────────────────────────────────────
// Projects are registered centrally; each entry names its own data dir, and
// the database lives inside the project itself.

pub struct CrushParser;

fn crush_projects(home: &Path) -> Vec<(String, PathBuf)> {
    let registry = crate::platform::ambient_override(home, "CRUSH_GLOBAL_DATA")
        .map(|d| d.join("projects.json"))
        .unwrap_or_else(|| crate::platform::xdg_data_root(home).join("crush/projects.json"));
    let Ok(text) = std::fs::read_to_string(&registry) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let entries: Vec<&serde_json::Value> = match &v {
        serde_json::Value::Array(a) => a.iter().collect(),
        serde_json::Value::Object(o) => o.values().collect(),
        _ => return Vec::new(),
    };
    entries
        .into_iter()
        .filter_map(|e| {
            let path = e.get("path")?.as_str()?.to_string();
            let data_dir = e
                .get("data_dir")
                .and_then(|d| d.as_str())
                .filter(|d| !d.is_empty())
                .unwrap_or(".crush");
            Some((
                path.clone(),
                PathBuf::from(&path).join(data_dir).join("crush.db"),
            ))
        })
        .collect()
}

impl TranscriptParser for CrushParser {
    fn agent(&self) -> Agent {
        Agent::Crush
    }

    fn aggregate_since(&self, home_dir: &Path, since_iso: Option<&str>) -> Vec<ProjectUsage> {
        let mut by_cwd = HashMap::new();
        for (project, db) in crush_projects(home_dir) {
            // `parent_session_id IS NULL` keeps sub-sessions from being counted
            // twice: their tokens are already inside the parent's totals.
            for (input, output, msgs, ts) in query_db(
                &db,
                "SELECT COALESCE(prompt_tokens,0), COALESCE(completion_tokens,0), \
                 COALESCE(message_count,0), COALESCE(updated_at, created_at, 0) \
                 FROM sessions WHERE parent_session_id IS NULL",
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?.max(0) as u64,
                        r.get::<_, i64>(1)?.max(0) as u64,
                        r.get::<_, i64>(2)?.max(0) as u64,
                        r.get::<_, i64>(3)?,
                    ))
                },
            ) {
                if input == 0 && output == 0 {
                    continue;
                }
                let iso = epoch_to_iso(ts);
                if !after_cutoff(&iso, since_iso) {
                    continue;
                }
                add(&mut by_cwd, project.clone(), input, output, 0, 0, msgs, iso);
            }
        }
        finish(by_cwd)
    }
}

// ── zcode ────────────────────────────────────────────────────────────
// The best data of the four: one row per model request, with cache columns.

pub struct ZcodeParser;

fn zcode_db(home: &Path) -> PathBuf {
    home.join(".zcode/cli/db/db.sqlite")
}

/// Rows as stored: `(input_tokens, output, cache_creation, cache_read,
/// completed_at_ms, directory, model_id)`.
type ZcodeRow = (u64, u64, u64, u64, i64, String, String);

fn zcode_rows(home: &Path) -> Vec<ZcodeRow> {
    query_db(
        &zcode_db(home),
        "SELECT COALESCE(m.input_tokens,0), COALESCE(m.output_tokens,0) + COALESCE(m.reasoning_tokens,0), \
         COALESCE(m.cache_creation_input_tokens,0), COALESCE(m.cache_read_input_tokens,0), \
         COALESCE(m.completed_at, m.started_at, 0), COALESCE(s.directory,''), COALESCE(m.model_id,'') \
         FROM model_usage m LEFT JOIN session s ON s.id = m.session_id",
        |r| {
            Ok((
                r.get::<_, i64>(0)?.max(0) as u64,
                r.get::<_, i64>(1)?.max(0) as u64,
                r.get::<_, i64>(2)?.max(0) as u64,
                r.get::<_, i64>(3)?.max(0) as u64,
                r.get::<_, i64>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
            ))
        },
    )
}

/// zcode reports OpenAI-style: `input_tokens` is the whole prompt, cache
/// included. Every other agent here reports fresh input separately, so
/// subtract or the cached history gets billed twice in the totals.
fn zcode_fresh_input(input: u64, cache_creation: u64, cache_read: u64) -> u64 {
    input.saturating_sub(cache_creation.saturating_add(cache_read))
}

impl TranscriptParser for ZcodeParser {
    fn agent(&self) -> Agent {
        Agent::Zcode
    }

    fn aggregate_since(&self, home_dir: &Path, since_iso: Option<&str>) -> Vec<ProjectUsage> {
        let mut by_cwd = HashMap::new();
        for (input, output, cc, cr, ts, dir, _model) in zcode_rows(home_dir) {
            let iso = epoch_to_iso(ts);
            if !after_cutoff(&iso, since_iso) {
                continue;
            }
            add(
                &mut by_cwd,
                dir,
                zcode_fresh_input(input, cc, cr),
                output,
                cc,
                cr,
                1,
                iso,
            );
        }
        finish(by_cwd)
    }

    fn model_usage_by_day(
        &self,
        home_dir: &Path,
        since_iso: Option<&str>,
    ) -> crate::agent_transcripts::ModelUsageByDay {
        let mut out = crate::agent_transcripts::ModelUsageByDay::default();
        for (input, output, cc, cr, ts, _dir, model) in zcode_rows(home_dir) {
            if model.is_empty() {
                continue;
            }
            let iso = epoch_to_iso(ts);
            if !after_cutoff(&iso, since_iso) {
                continue;
            }
            let day = iso.get(..10).unwrap_or("").to_string();
            if day.is_empty() {
                continue;
            }
            let e = out
                .entry(day)
                .or_default()
                .entry(model)
                .or_insert((0, 0, 0));
            // Same convention as every other parser: fresh input counts cache
            // WRITES but not cache reads (re-sent history).
            e.0 += zcode_fresh_input(input, cc, cr) + cc;
            e.1 += output;
            e.2 += 1;
        }
        out
    }
}

// ── Forge ────────────────────────────────────────────────────────────
// SQLite, but the usage is inside a JSON blob column rather than in columns,
// and each figure is wrapped in an `actual` object (the schema also carries
// estimates, which we ignore — only the measured value counts here).

pub struct ForgeParser;

fn forge_db(home: &Path) -> PathBuf {
    home.join(".forge/.forge.db")
}

/// `YYYY-MM-DD HH:MM:SS[.fff]` → ISO-8601 Z. SQLite's own datetime spelling,
/// one space where ISO wants a `T`.
fn sqlite_datetime_to_iso(s: &str) -> String {
    let trimmed = s.split('.').next().unwrap_or(s).trim();
    if !trimmed.is_char_boundary(10)
        || !trimmed.is_char_boundary(11)
        || !trimmed.is_char_boundary(19)
    {
        return String::new();
    }
    format!("{}T{}Z", &trimmed[..10], &trimmed[11..19])
}

fn forge_actual(usage: &serde_json::Value, key: &str) -> u64 {
    usage
        .get(key)
        .and_then(|v| v.get("actual").or(Some(v)))
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
}

impl TranscriptParser for ForgeParser {
    fn agent(&self) -> Agent {
        Agent::Forge
    }

    fn aggregate_since(&self, home_dir: &Path, since_iso: Option<&str>) -> Vec<ProjectUsage> {
        let mut by_cwd = HashMap::new();
        for (title, workspace, context, ts) in query_db(
            &forge_db(home_dir),
            "SELECT COALESCE(title,''), COALESCE(workspace_id,''), COALESCE(context,''), \
             COALESCE(updated_at, created_at, '') FROM conversations",
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            },
        ) {
            let iso = sqlite_datetime_to_iso(&ts);
            if !after_cutoff(&iso, since_iso) {
                continue;
            }
            let Ok(ctx) = serde_json::from_str::<serde_json::Value>(&context) else {
                continue;
            };
            let Some(messages) = ctx.get("messages").and_then(|m| m.as_array()) else {
                continue;
            };
            let (mut input, mut output, mut cached, mut msgs) = (0u64, 0u64, 0u64, 0u64);
            for m in messages {
                let Some(usage) = m.get("usage") else {
                    continue;
                };
                let prompt = forge_actual(usage, "prompt_tokens");
                let cache = forge_actual(usage, "cached_tokens");
                // prompt_tokens is the whole prompt, cache included.
                input += prompt.saturating_sub(cache);
                cached += cache;
                output += forge_actual(usage, "completion_tokens");
                msgs += 1;
            }
            if input == 0 && output == 0 && cached == 0 {
                continue;
            }
            let label = if !workspace.is_empty() {
                workspace
            } else {
                title
            };
            add(&mut by_cwd, label, input, output, 0, cached, msgs, iso);
        }
        finish(by_cwd)
    }
}

// ── Zerostack ────────────────────────────────────────────────────────
// One JSON file per session with cumulative totals. Not SQLite, but the same
// one-row-per-session shape, so it shares this module's plumbing.

pub struct ZerostackParser;

fn zerostack_dirs(home: &Path) -> Vec<PathBuf> {
    if let Some(d) = crate::platform::ambient_override(home, "ZS_DATA_DIR") {
        return vec![d.join("sessions")];
    }
    let xdg = crate::platform::xdg_data_root(home);
    vec![
        home.join("Library/Application Support/zerostack/sessions"),
        xdg.join("zerostack/sessions"),
    ]
}

impl TranscriptParser for ZerostackParser {
    fn agent(&self) -> Agent {
        Agent::Zerostack
    }

    fn aggregate_since(&self, home_dir: &Path, since_iso: Option<&str>) -> Vec<ProjectUsage> {
        let mut by_cwd = HashMap::new();
        for dir in zerostack_dirs(home_dir) {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().map(|x| x != "json").unwrap_or(true) {
                    continue;
                }
                let Ok(v) = std::fs::read_to_string(&p)
                    .ok()
                    .map(|t| serde_json::from_str::<serde_json::Value>(&t))
                    .unwrap_or_else(|| serde_json::from_str("null"))
                else {
                    continue;
                };
                let input = v
                    .get("total_input_tokens")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0);
                let output = v
                    .get("total_output_tokens")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0);
                if input == 0 && output == 0 {
                    continue;
                }
                let ts = v
                    .get("updated_at")
                    .or_else(|| v.get("created_at"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                if !after_cutoff(&ts, since_iso) {
                    continue;
                }
                let cwd = v
                    .get("working_dir")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                add(&mut by_cwd, cwd, input, output, 0, 0, 1, ts);
            }
        }
        finish(by_cwd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_home(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "tokenade-sqlite-agents-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn make_db(path: &Path, ddl: &[&str]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let conn = rusqlite::Connection::open(path).unwrap();
        for s in ddl {
            conn.execute_batch(s).unwrap();
        }
    }

    #[test]
    fn a_missing_database_is_silence_not_an_error() {
        let home = tmp_home("absent");
        assert!(GooseParser.aggregate_since(&home, None).is_empty());
        assert!(CrushParser.aggregate_since(&home, None).is_empty());
        assert!(ZcodeParser.aggregate_since(&home, None).is_empty());
        assert!(ZerostackParser.aggregate_since(&home, None).is_empty());
        // And no file was conjured into existence by opening it.
        assert!(!zcode_db(&home).exists());
    }

    #[test]
    fn goose_reads_accumulated_session_totals() {
        let home = tmp_home("goose");
        make_db(
            &home.join(".local/share/goose/sessions/sessions.db"),
            &["CREATE TABLE sessions(id TEXT, working_dir TEXT, created_at TEXT, \
               updated_at TEXT, accumulated_input_tokens INTEGER, accumulated_output_tokens INTEGER);
               INSERT INTO sessions VALUES('s1','/proj/a','2026-07-01T00:00:00Z','2026-07-02T00:00:00Z',900,100);
               INSERT INTO sessions VALUES('s2','/proj/a','2026-07-01T00:00:00Z','2026-07-03T00:00:00Z',10,5);
               INSERT INTO sessions VALUES('s3','/proj/b','2026-07-01T00:00:00Z','2026-07-01T00:00:00Z',0,0);"],
        );
        let out = ZerostackParser.aggregate_since(&home, None);
        assert!(
            out.is_empty(),
            "goose db must not feed the zerostack parser"
        );
        let out = GooseParser.aggregate_since(&home, None);
        // /proj/b contributed nothing but zeros and must not become a row.
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].cwd, "/proj/a");
        assert_eq!(out[0].input_tokens, 910);
        assert_eq!(out[0].output_tokens, 105);
        assert_eq!(out[0].sessions, 2);
        assert_eq!(out[0].last_ts, "2026-07-03T00:00:00Z");
        // Cutoff drops the older session.
        let cut = GooseParser.aggregate_since(&home, Some("2026-07-03T00:00:00Z"));
        assert_eq!(cut[0].input_tokens, 10);
    }

    #[test]
    fn zcode_subtracts_cache_from_an_openai_style_prompt_total() {
        let home = tmp_home("zcode");
        make_db(
            &home.join(".zcode/cli/db/db.sqlite"),
            &["CREATE TABLE session(id TEXT, directory TEXT);
               CREATE TABLE model_usage(session_id TEXT, model_id TEXT, input_tokens INTEGER, \
                 output_tokens INTEGER, reasoning_tokens INTEGER, cache_creation_input_tokens INTEGER, \
                 cache_read_input_tokens INTEGER, started_at INTEGER, completed_at INTEGER);
               INSERT INTO session VALUES('s1','/w/x');
               INSERT INTO model_usage VALUES('s1','claude-sonnet-4-5',1000,50,7,200,700,1783000000000,1783000001000);"],
        );
        let out = ZcodeParser.aggregate_since(&home, None);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].cwd, "/w/x");
        // 1000 total prompt − (200 written + 700 read) = 100 genuinely fresh.
        assert_eq!(out[0].input_tokens, 100);
        // reasoning tokens are output the user paid for.
        assert_eq!(out[0].output_tokens, 57);
        assert_eq!(out[0].cache_creation_tokens, 200);
        assert_eq!(out[0].cache_read_tokens, 700);

        let m = ZcodeParser.model_usage_by_day(&home, None);
        let day = m.values().next().expect("one day");
        // fresh + cache WRITES, never cache reads.
        assert_eq!(day["claude-sonnet-4-5"], (300, 57, 1));
    }

    #[test]
    fn zcode_never_underflows_when_cache_exceeds_the_prompt_total() {
        // Defensive: if a future zcode build reports cache separately rather
        // than inside input_tokens, the subtraction must clamp, not wrap.
        assert_eq!(zcode_fresh_input(10, 50, 50), 0);
    }

    #[test]
    fn crush_reads_the_project_registry_and_skips_child_sessions() {
        let home = tmp_home("crush");
        let proj = home.join("code/myproj");
        make_db(
            &proj.join(".crush/crush.db"),
            &["CREATE TABLE sessions(id TEXT, parent_session_id TEXT, prompt_tokens INTEGER, \
                 completion_tokens INTEGER, cost REAL, message_count INTEGER, created_at INTEGER, updated_at INTEGER);
               CREATE TABLE messages(session_id TEXT, model TEXT);
               INSERT INTO sessions VALUES('p1',NULL,400,60,0.1,8,1783000000,1783000900);
               INSERT INTO sessions VALUES('c1','p1',999,999,0.9,3,1783000000,1783000900);"],
        );
        std::fs::create_dir_all(home.join(".local/share/crush")).unwrap();
        // Serialised, not hand-formatted: on Windows the path is `C:\…`, and
        // splicing it raw into a JSON string literal yields invalid escapes
        // (`\U`), so the registry parsed to nothing and this test failed there
        // only. A real client writes escaped backslashes, which is what this
        // now produces.
        let registry = serde_json::json!([
            { "path": proj.to_string_lossy(), "data_dir": ".crush" }
        ]);
        std::fs::write(
            home.join(".local/share/crush/projects.json"),
            serde_json::to_string(&registry).unwrap(),
        )
        .unwrap();
        let out = CrushParser.aggregate_since(&home, None);
        assert_eq!(out.len(), 1);
        // The child session's 999s are already inside the parent's totals.
        assert_eq!(out[0].input_tokens, 400);
        assert_eq!(out[0].output_tokens, 60);
        // updated_at is SECONDS here despite schema comments claiming ms.
        assert!(
            out[0].last_ts.starts_with("2026-"),
            "second/millisecond detection failed: {}",
            out[0].last_ts
        );
    }

    #[test]
    fn forge_unwraps_actual_and_subtracts_the_cached_prompt_share() {
        let home = tmp_home("forge");
        let ctx = r#"{"messages":[
            {"text":{"model":"forge-1"},"usage":{"prompt_tokens":{"actual":500,"estimated":9},
             "completion_tokens":{"actual":40},"cached_tokens":{"actual":200}}},
            {"text":{"model":"forge-1"},"usage":{"prompt_tokens":{"actual":10},
             "completion_tokens":{"actual":2},"cached_tokens":{"actual":0}}}]}"#
            .replace('\n', "")
            .replace("            ", "");
        make_db(
            &home.join(".forge/.forge.db"),
            &[
                "CREATE TABLE conversations(conversation_id TEXT, title TEXT, workspace_id TEXT, \
               context TEXT, created_at TEXT, updated_at TEXT);",
            ],
        );
        let conn = rusqlite::Connection::open(home.join(".forge/.forge.db")).unwrap();
        conn.execute(
            "INSERT INTO conversations VALUES('c1','My chat','/w/f',?1,'2026-07-08 09:00:00','2026-07-08 10:30:15.221')",
            [&ctx],
        )
        .unwrap();
        drop(conn);
        let out = ForgeParser.aggregate_since(&home, None);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].cwd, "/w/f");
        // (500 − 200) + 10 fresh; the estimated figure beside `actual` is ignored.
        assert_eq!(out[0].input_tokens, 310);
        assert_eq!(out[0].output_tokens, 42);
        assert_eq!(out[0].cache_read_tokens, 200);
        // SQLite spells its datetimes with a space and optional millis.
        assert_eq!(out[0].last_ts, "2026-07-08T10:30:15Z");
    }

    #[test]
    fn sqlite_datetime_conversion_rejects_a_truncated_value() {
        assert_eq!(
            sqlite_datetime_to_iso("2026-07-08 10:30:15"),
            "2026-07-08T10:30:15Z"
        );
        assert_eq!(sqlite_datetime_to_iso("2026-07-08"), "");
    }

    #[test]
    fn zerostack_reads_cumulative_totals_per_session_file() {
        let home = tmp_home("zerostack");
        let dir = home.join(".local/share/zerostack/sessions");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("a.json"),
            r#"{"id":"a","working_dir":"/w/z","model":"openrouter/x","total_input_tokens":70,
                "total_output_tokens":30,"updated_at":"2026-07-10T00:00:00Z"}"#,
        )
        .unwrap();
        std::fs::write(dir.join("notes.txt"), "ignored").unwrap();
        let out = ZerostackParser.aggregate_since(&home, None);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].input_tokens, 70);
        assert_eq!(out[0].output_tokens, 30);
        assert_eq!(out[0].cwd, "/w/z");
    }
}
