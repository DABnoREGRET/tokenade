//! Cline (VS Code extension) transcript parser.
//!
//! Cline stores per-task data in
//! `~/.config/Code/User/globalStorage/saoudrizwan.claude-dev/tasks/<task-id>/`.
//! Each task directory contains:
//!
//!   - `task_metadata.json` — cwd, plus the `model_usage` timeline:
//!     `[{"ts":<ms>,"model_id":"…","model_provider_id":"…","mode":"act"}]`
//!     (an entry is appended whenever model/provider/mode changes)
//!   - `ui_messages.json` — a single JSON ARRAY of UI events (written
//!     via `JSON.stringify` in core/storage/disk.ts — NOT JSONL)
//!   - `api_conversation_history.json` — full message log
//!
//! Token usage lives in `ui_messages.json` records of the form:
//!
//! ```text
//! {"type":"say","say":"api_req_started","ts":…,
//!  "text":"{\"request\":…,\"tokensIn\":N,\"tokensOut\":N,\"cacheReads\":N,\"cacheWrites\":N,\"cost\":…}"}
//! ```
//!
//! Note `text` is a JSON-encoded string (stringified); it carries NO
//! model field — the model comes from the `task_metadata.json`
//! timeline above. Usage is per-request (one `api_req_started` per
//! API call, updated in place when the stream ends); sessions sum
//! across these events. Tool usage shows up in `say:"tool"` records
//! or `ask:"tool"`.
//!
//! Schema verified against cline/cline @ de987a5 (2026-06-09):
//! shared/ExtensionMessage.ts (`ClineApiReqInfo`), core/storage/disk.ts,
//! context-tracking/ContextTrackerTypes.ts. Tests use synthetic
//! fixtures matching that shape.

use crate::agent_transcripts::{Agent, ProjectUsage, ToolBreakdown, TranscriptParser};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

pub struct Parser;

/// Roo Code — a Cline fork under its own extension id, in the stock VS Code
/// product dirs.
pub struct RooCodeParser;

/// IBM Bob — a Cline fork shipped as its own VS Code application, so the
/// task tree lives under `IBM Bob` / `Bob-IDE` rather than `Code`.
pub struct IbmBobParser;

impl TranscriptParser for RooCodeParser {
    fn agent(&self) -> Agent {
        Agent::RooCode
    }

    fn aggregate_since(&self, home_dir: &Path, since_iso: Option<&str>) -> Vec<ProjectUsage> {
        aggregate_family(home_dir, &ROO_CODE, since_iso)
    }

    fn model_usage_by_day(
        &self,
        home_dir: &Path,
        since_iso: Option<&str>,
    ) -> crate::agent_transcripts::ModelUsageByDay {
        models_family(home_dir, &ROO_CODE, since_iso)
    }

    fn session_models(
        &self,
        home_dir: &Path,
        since_iso: Option<&str>,
    ) -> Vec<crate::agent_transcripts::SessionModel> {
        session_models_family(home_dir, &ROO_CODE, since_iso)
    }
}

impl TranscriptParser for IbmBobParser {
    fn agent(&self) -> Agent {
        Agent::IbmBob
    }

    fn aggregate_since(&self, home_dir: &Path, since_iso: Option<&str>) -> Vec<ProjectUsage> {
        aggregate_family(home_dir, &IBM_BOB, since_iso)
    }

    fn model_usage_by_day(
        &self,
        home_dir: &Path,
        since_iso: Option<&str>,
    ) -> crate::agent_transcripts::ModelUsageByDay {
        models_family(home_dir, &IBM_BOB, since_iso)
    }

    fn session_models(
        &self,
        home_dir: &Path,
        since_iso: Option<&str>,
    ) -> Vec<crate::agent_transcripts::SessionModel> {
        session_models_family(home_dir, &IBM_BOB, since_iso)
    }
}

fn session_models_family(
    home_dir: &Path,
    fam: &Family,
    since_iso: Option<&str>,
) -> Vec<crate::agent_transcripts::SessionModel> {
    let mut acc: std::collections::BTreeMap<(String, String), (u64, u64)> = Default::default();
    for_each_task_dir_of(home_dir, fam, &mut |task_dir| {
        scan_task_model_spans(task_dir, &mut acc, since_iso);
    });
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

fn aggregate_family(home_dir: &Path, fam: &Family, since_iso: Option<&str>) -> Vec<ProjectUsage> {
    let mut by_cwd: HashMap<String, ProjectUsage> = HashMap::new();
    for_each_task_dir_of(home_dir, fam, &mut |task_dir| {
        scan_task(task_dir, &mut by_cwd, since_iso);
    });
    let mut out: Vec<ProjectUsage> = by_cwd.into_values().collect();
    out.sort_by(|a, b| b.last_ts.cmp(&a.last_ts));
    out
}

fn models_family(
    home_dir: &Path,
    fam: &Family,
    since_iso: Option<&str>,
) -> crate::agent_transcripts::ModelUsageByDay {
    let mut out = crate::agent_transcripts::ModelUsageByDay::default();
    for_each_task_dir_of(home_dir, fam, &mut |task_dir| {
        scan_task_models(task_dir, &mut out, since_iso);
    });
    out
}

impl TranscriptParser for Parser {
    fn agent(&self) -> Agent {
        Agent::Cline
    }

    fn aggregate_since(&self, home_dir: &Path, since_iso: Option<&str>) -> Vec<ProjectUsage> {
        let mut by_cwd: HashMap<String, ProjectUsage> = HashMap::new();
        for_each_task_dir(home_dir, &mut |task_dir| {
            scan_task(task_dir, &mut by_cwd, since_iso);
        });
        let mut out: Vec<ProjectUsage> = by_cwd.into_values().collect();
        out.sort_by(|a, b| b.last_ts.cmp(&a.last_ts));
        out
    }

    fn model_usage_by_day(
        &self,
        home_dir: &Path,
        since_iso: Option<&str>,
    ) -> crate::agent_transcripts::ModelUsageByDay {
        let mut out = crate::agent_transcripts::ModelUsageByDay::default();
        for_each_task_dir(home_dir, &mut |task_dir| {
            scan_task_models(task_dir, &mut out, since_iso);
        });
        out
    }

    fn session_models(
        &self,
        home_dir: &Path,
        since_iso: Option<&str>,
    ) -> Vec<crate::agent_transcripts::SessionModel> {
        let mut acc: std::collections::BTreeMap<(String, String), (u64, u64)> = Default::default();
        for_each_task_dir(home_dir, &mut |task_dir| {
            scan_task_model_spans(task_dir, &mut acc, since_iso);
        });
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

/// Walk one Cline task's `api_req_started` records and hand every non-zero
/// token pair to `on_usage`, with the model in force at that request.
///
/// `scan_task_model_spans` and `scan_task_models` carried this walk twice. The
/// part that must not drift is the model lookup: Cline records the model on a
/// SEPARATE timeline, so "which model ran this request" is a decision (latest
/// entry at or before the timestamp, falling back to the first), not a field.
/// Two copies deciding it differently would attribute the same tokens to two
/// different models, and nothing in the output would say so.
fn for_each_task_usage(
    task_dir: &Path,
    since_iso: Option<&str>,
    mut on_usage: impl FnMut(&str, u64, &str, u64, u64),
) {
    let timeline = model_timeline(task_dir);
    if timeline.is_empty() {
        return;
    }
    for v in read_ui_messages(task_dir) {
        if v["say"].as_str() != Some("api_req_started") {
            continue;
        }
        let Some(inner) = v["text"].as_str() else {
            continue;
        };
        let Ok(usage) = serde_json::from_str::<serde_json::Value>(inner) else {
            continue;
        };
        let ts_ms = v["ts"].as_u64().unwrap_or(0);
        let iso = ts_ms_to_iso(ts_ms);
        if !iso.is_char_boundary(10) || since_iso.is_some_and(|c| iso.as_str() < c) {
            continue;
        }
        let fresh =
            usage["tokensIn"].as_u64().unwrap_or(0) + usage["cacheWrites"].as_u64().unwrap_or(0);
        let outp = usage["tokensOut"].as_u64().unwrap_or(0);
        if fresh == 0 && outp == 0 {
            continue;
        }
        // Latest timeline entry at or before this request; a request
        // earlier than every entry falls back to the first one.
        let model = timeline
            .iter()
            .rev()
            .find(|(t, _)| *t <= ts_ms)
            .or_else(|| timeline.first())
            .map(|(_, m)| m.as_str())
            .unwrap_or_default();
        on_usage(model, ts_ms, &iso, fresh, outp);
    }
}

/// Per-(task, model) timestamp span. Mirrors [`scan_task_models`] exactly (same
/// timeline lookup, same per-request `api_req_started` selection, same fresh/out
/// filtering, same cutoff) but records the min/max request timestamp in epoch
/// millis. The session id is the task directory name — Cline's
/// `tasks/<task-id>/` identity.
fn scan_task_model_spans(
    task_dir: &Path,
    acc: &mut std::collections::BTreeMap<(String, String), (u64, u64)>,
    since_iso: Option<&str>,
) {
    let Some(task_id) = task_dir
        .file_name()
        .and_then(|s| s.to_str())
        .map(str::to_string)
    else {
        return;
    };
    for_each_task_usage(task_dir, since_iso, |model, ts_ms, _iso, _fresh, _outp| {
        let e = acc
            .entry((task_id.clone(), model.to_string()))
            .or_insert((ts_ms, ts_ms));
        if ts_ms < e.0 {
            e.0 = ts_ms;
        }
        if ts_ms > e.1 {
            e.1 = ts_ms;
        }
    });
}

/// Visit every Cline task directory under every plausible install
/// location: VS Code-family globalStorage (all OSes/products) plus the
/// standalone Cline CLI data dir (`~/.cline/data`).
fn for_each_task_dir(home_dir: &Path, visit: &mut dyn FnMut(&Path)) {
    for_each_task_dir_of(home_dir, &CLINE, visit)
}

/// One member of the Cline family: same `tasks/<id>/ui_messages.json` layout,
/// different VS Code extension id (and, for a vendor fork, different product
/// directories). Everything below this struct — the scanning, the usage
/// extraction, the model regex — is shared verbatim.
struct Family {
    extension_id: &'static str,
    /// `None` = the stock VS Code product set. `Some(&[…])` for a fork that
    /// ships its own application directory.
    products: Option<&'static [&'static str]>,
    /// Non-VS-Code roots, already ending in the tasks directory.
    extra_task_roots: &'static [&'static str],
}

const CLINE: Family = Family {
    extension_id: "saoudrizwan.claude-dev",
    products: None,
    extra_task_roots: &[".cline/data/tasks"],
};

const ROO_CODE: Family = Family {
    extension_id: "rooveterinaryinc.roo-cline",
    products: None,
    extra_task_roots: &[],
};

const IBM_BOB: Family = Family {
    extension_id: "ibm.bob-code",
    products: Some(&["IBM Bob", "Bob-IDE"]),
    extra_task_roots: &[],
};

fn for_each_task_dir_of(home_dir: &Path, fam: &Family, visit: &mut dyn FnMut(&Path)) {
    let user_dirs = match fam.products {
        Some(products) => crate::agent_transcripts::vscode_user_dirs_for(home_dir, products),
        None => crate::agent_transcripts::vscode_user_dirs(home_dir),
    };
    let mut roots: Vec<std::path::PathBuf> = user_dirs
        .into_iter()
        .map(|u| u.join("globalStorage").join(fam.extension_id).join("tasks"))
        .collect();
    for extra in fam.extra_task_roots {
        roots.push(home_dir.join(extra));
    }
    for tasks_dir in roots {
        let Ok(entries) = fs::read_dir(&tasks_dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                visit(&p);
            }
        }
    }
}

/// Read `ui_messages.json`. Real Cline writes a single JSON ARRAY
/// (`JSON.stringify(uiMessages)` in core/storage/disk.ts) — but we
/// also accept one-object-per-line (JSONL) since early fixture-based
/// versions of this parser documented it that way and it costs one
/// fallback branch to stay compatible.
fn read_ui_messages(task_dir: &Path) -> Vec<serde_json::Value> {
    let Ok(text) = fs::read_to_string(task_dir.join("ui_messages.json")) else {
        return Vec::new();
    };
    if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(&text) {
        return arr;
    }
    text.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .collect()
}

/// `task_metadata.json` → `model_usage` entries as `(ts_ms, model_id)`,
/// sorted by timestamp. Cline appends an entry whenever the model,
/// provider, or mode changes, so a request at time T belongs to the
/// last entry with `ts <= T`.
fn model_timeline(task_dir: &Path) -> Vec<(u64, String)> {
    let Some(meta) = fs::read_to_string(task_dir.join("task_metadata.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
    else {
        return Vec::new();
    };
    let Some(entries) = meta["model_usage"].as_array() else {
        return Vec::new();
    };
    let mut out: Vec<(u64, String)> = entries
        .iter()
        .filter_map(|e| {
            let model = e["model_id"].as_str()?;
            if model.is_empty() {
                return None;
            }
            Some((e["ts"].as_u64().unwrap_or(0), model.to_string()))
        })
        .collect();
    out.sort_by_key(|(ts, _)| *ts);
    out
}

/// Per-model pass over one Cline task. The model comes from the
/// `task_metadata.json` model-usage timeline (it is NOT in the
/// `api_req_started` payload); usage is per-request. Tasks without a
/// model timeline contribute nothing — we never guess. Fresh input =
/// `tokensIn` + `cacheWrites`, cache READS excluded (the shared
/// convention).
fn scan_task_models(
    task_dir: &Path,
    out: &mut crate::agent_transcripts::ModelUsageByDay,
    since_iso: Option<&str>,
) {
    for_each_task_usage(task_dir, since_iso, |model, _ts_ms, iso, fresh, outp| {
        let e = out
            .entry(iso[..10].to_string())
            .or_default()
            .entry(model.to_string())
            .or_insert((0, 0, 0));
        e.0 = e.0.saturating_add(fresh);
        e.1 = e.1.saturating_add(outp);
        e.2 = e.2.saturating_add(1);
    });
}

fn scan_task(task_dir: &Path, by_cwd: &mut HashMap<String, ProjectUsage>, since_iso: Option<&str>) {
    // cwd from task_metadata.json. If missing we skip — without a
    // project path we can't honestly attribute the tokens.
    let cwd = match fs::read_to_string(task_dir.join("task_metadata.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v["cwd"].as_str().map(str::to_string))
    {
        Some(c) => c,
        None => return,
    };

    let mut total_in: u64 = 0;
    let mut total_out: u64 = 0;
    let mut total_cache_read: u64 = 0;
    let mut total_cache_write: u64 = 0;
    let mut messages: u64 = 0;
    let mut last_ts_ms: u64 = 0;
    let mut tools = ToolBreakdown::default();

    for v in read_ui_messages(task_dir) {
        let say = v["say"].as_str().unwrap_or("");
        let ts_ms = v["ts"].as_u64().unwrap_or(0);
        if ts_ms > last_ts_ms {
            last_ts_ms = ts_ms;
        }

        match say {
            "api_req_started" => {
                // `text` is a JSON-encoded string holding the per-
                // request usage. Decoding it gives us tokensIn,
                // tokensOut, cacheReads, cacheWrites.
                if let Some(inner) = v["text"].as_str() {
                    if let Ok(usage) = serde_json::from_str::<serde_json::Value>(inner) {
                        let ti = usage["tokensIn"].as_u64().unwrap_or(0);
                        let to = usage["tokensOut"].as_u64().unwrap_or(0);
                        let cr = usage["cacheReads"].as_u64().unwrap_or(0);
                        let cw = usage["cacheWrites"].as_u64().unwrap_or(0);
                        // Cutoff filter applies per-request — but Cline
                        // ts is epoch ms, our cutoff is ISO-8601. We
                        // compare via the converted form.
                        if since_iso.is_some_and(|c| ts_ms_to_iso(ts_ms).as_str() < c) {
                            continue;
                        }
                        total_in = total_in.saturating_add(ti);
                        total_out = total_out.saturating_add(to);
                        total_cache_read = total_cache_read.saturating_add(cr);
                        total_cache_write = total_cache_write.saturating_add(cw);
                        messages = messages.saturating_add(1);
                    }
                }
            }
            "tool" | "tool_use" => {
                if let Some(name) = v["text"].as_str().or_else(|| v["tool"].as_str()) {
                    bump_cline(&mut tools, name);
                }
            }
            _ => {}
        }
    }

    if total_in == 0 && total_out == 0 && tools.total() == 0 {
        return;
    }

    let entry = by_cwd.entry(cwd.clone()).or_insert_with(|| ProjectUsage {
        cwd,
        ..Default::default()
    });
    entry.sessions = entry.sessions.saturating_add(1);
    entry.input_tokens = entry.input_tokens.saturating_add(total_in);
    entry.output_tokens = entry.output_tokens.saturating_add(total_out);
    entry.cache_creation_tokens = entry
        .cache_creation_tokens
        .saturating_add(total_cache_write);
    entry.cache_read_tokens = entry.cache_read_tokens.saturating_add(total_cache_read);
    entry.assistant_messages = entry.assistant_messages.saturating_add(messages);
    let iso = ts_ms_to_iso(last_ts_ms);
    if iso > entry.last_ts {
        entry.last_ts = iso;
    }
    entry.tools += tools;
}

/// Convert Cline's epoch-millisecond timestamp to ISO-8601 so it
/// compares lexicographically against the dashboard cutoff.
fn ts_ms_to_iso(ts_ms: u64) -> String {
    crate::platform::epoch_to_iso8601_z(ts_ms / 1000)
}

/// Cline's tool vocab differs from Claude/Codex. Names observed:
///   - `execute_command` → bash
///   - `read_file` → read
///   - `write_to_file`, `replace_in_file` → edit
///   - `search_files` → grep
///   - `list_files` → glob
///   - `browser_action` → web
fn bump_cline(b: &mut ToolBreakdown, name: &str) {
    // #490: also recognise legacy `supertokensaver` / `sts` namespaces
    // — see the claudecode_transcript bump() comment for rationale.
    if name.starts_with("mcp_tokenade")
        || name.starts_with("mcp_supertokensaver")
        || name.starts_with("mcp_sts")
        || (name.starts_with("use_mcp_tool")
            && (name.contains("tokenade")
                || name.contains("supertokensaver")
                || name.contains("sts")))
    {
        b.tokenade = b.tokenade.saturating_add(1);
        return;
    }
    match name {
        "execute_command" | "command" => b.bash = b.bash.saturating_add(1),
        "read_file" => b.read = b.read.saturating_add(1),
        "write_to_file" | "replace_in_file" | "new_file" => b.edit = b.edit.saturating_add(1),
        "search_files" => b.grep = b.grep.saturating_add(1),
        "list_files" => b.glob = b.glob.saturating_add(1),
        "browser_action" | "web_fetch" => b.web = b.web.saturating_add(1),
        _ => b.other = b.other.saturating_add(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_home(label: &str) -> std::path::PathBuf {
        let d =
            std::env::temp_dir().join(format!("tokenade-cline-{}-{}", label, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_task(home: &Path, task_id: &str, cwd: &str, ui_lines: &[&str]) {
        let dir = home
            .join(".config")
            .join("Code")
            .join("User")
            .join("globalStorage")
            .join("saoudrizwan.claude-dev")
            .join("tasks")
            .join(task_id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("task_metadata.json"),
            format!(r#"{{"task":"x","ts":1717200000000,"cwd":"{cwd}"}}"#),
        )
        .unwrap();
        fs::write(dir.join("ui_messages.json"), ui_lines.join("\n")).unwrap();
    }

    #[test]
    fn aggregates_cline_api_req_started_events() {
        let home = tmp_home("aggregate");
        // `text` is a JSON-encoded string — note the escaped quotes.
        write_task(
            &home,
            "task-a",
            "/p",
            &[
                r#"{"type":"say","say":"api_req_started","ts":1717200000000,"text":"{\"tokensIn\":100,\"tokensOut\":50,\"cacheReads\":200,\"cacheWrites\":80}"}"#,
                r#"{"type":"say","say":"tool","ts":1717200005000,"text":"execute_command"}"#,
                r#"{"type":"say","say":"api_req_started","ts":1717200010000,"text":"{\"tokensIn\":5,\"tokensOut\":3,\"cacheReads\":0,\"cacheWrites\":0}"}"#,
            ],
        );
        let stats = Parser.aggregate_since(&home, None);
        assert_eq!(stats.len(), 1);
        let p = &stats[0];
        assert_eq!(p.cwd, "/p");
        assert_eq!(p.input_tokens, 105);
        assert_eq!(p.output_tokens, 53);
        assert_eq!(p.cache_read_tokens, 200);
        assert_eq!(p.cache_creation_tokens, 80);
        assert_eq!(p.tools.bash, 1);
        assert_eq!(p.sessions, 1);
    }

    /// A task written by a Cline FORK must be found at that fork's own
    /// location and attributed to that fork — never rolled into Cline's row.
    /// The dashboard's whole job is telling the user which tool spent the
    /// tokens; a merged row would answer that question wrongly.
    #[test]
    fn cline_forks_are_found_and_counted_separately() {
        let home = tmp_home("cline-forks");
        let bodies = [
            (
                "roo",
                home.join(".config/Code/User/globalStorage/rooveterinaryinc.roo-cline/tasks/t1"),
                11u64,
            ),
            (
                "bob",
                home.join(
                    "Library/Application Support/IBM Bob/User/globalStorage/ibm.bob-code/tasks/t2",
                ),
                22u64,
            ),
            (
                "cline",
                home.join(".config/Code/User/globalStorage/saoudrizwan.claude-dev/tasks/t3"),
                33u64,
            ),
        ];
        for (tag, dir, tokens) in &bodies {
            fs::create_dir_all(dir).unwrap();
            fs::write(
                dir.join("task_metadata.json"),
                format!(r#"{{"task":"{tag}","ts":1780272000000,"cwd":"/p-{tag}","files_in_context":[],"model_usage":[]}}"#),
            )
            .unwrap();
            fs::write(
                dir.join("ui_messages.json"),
                format!(
                    r#"[{{"type":"say","say":"api_req_started","ts":1780272050000,"text":"{{\"tokensIn\":{tokens},\"tokensOut\":1,\"cacheReads\":0,\"cacheWrites\":0}}"}}]"#
                ),
            )
            .unwrap();
        }

        let roo = RooCodeParser.aggregate_since(&home, None);
        assert_eq!(roo.len(), 1, "Roo Code task not discovered");
        assert_eq!(roo[0].input_tokens, 11);

        let bob = IbmBobParser.aggregate_since(&home, None);
        assert_eq!(
            bob.len(),
            1,
            "IBM Bob task not discovered under its own product dir"
        );
        assert_eq!(bob[0].input_tokens, 22);

        // The point of the split: Cline sees only its own extension id.
        let cline = Parser.aggregate_since(&home, None);
        assert_eq!(cline.len(), 1, "Cline must not absorb the forks' tasks");
        assert_eq!(cline[0].input_tokens, 33);

        assert_eq!(RooCodeParser.agent().id(), "roo-code");
        assert_eq!(IbmBobParser.agent().id(), "ibm-bob");
    }

    #[test]
    fn parses_real_json_array_format_and_model_timeline() {
        // Real Cline writes ui_messages.json as ONE JSON array and the
        // model timeline in task_metadata.json — exercise both, plus
        // model attribution across a mid-task model switch.
        let home = tmp_home("array-models");
        let dir = home.join(".config/Code/User/globalStorage/saoudrizwan.claude-dev/tasks/task-m");
        fs::create_dir_all(&dir).unwrap();
        // 1780272000000 ms = 2026-06-01T00:00:00Z
        fs::write(
            dir.join("task_metadata.json"),
            r#"{"task":"x","ts":1780272000000,"cwd":"/p",
                "files_in_context":[],
                "model_usage":[
                  {"ts":1780272000000,"model_id":"claude-sonnet-4-5","model_provider_id":"anthropic","mode":"act"},
                  {"ts":1780272100000,"model_id":"gpt-5","model_provider_id":"openai","mode":"act"}]}"#,
        )
        .unwrap();
        fs::write(
            dir.join("ui_messages.json"),
            r#"[
              {"type":"say","say":"api_req_started","ts":1780272050000,"text":"{\"tokensIn\":100,\"tokensOut\":10,\"cacheReads\":500,\"cacheWrites\":20}"},
              {"type":"say","say":"api_req_started","ts":1780272200000,"text":"{\"tokensIn\":5,\"tokensOut\":7,\"cacheReads\":900,\"cacheWrites\":0}"}
            ]"#,
        )
        .unwrap();

        // Aggregate must work on the array format too.
        let stats = Parser.aggregate_since(&home, None);
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].input_tokens, 105);

        let m = Parser.model_usage_by_day(&home, None);
        let day = &m["2026-06-01"];
        assert_eq!(
            day["claude-sonnet-4-5"],
            (120, 10, 1),
            "fresh = tokensIn 100 + cacheWrites 20; cacheReads excluded"
        );
        assert_eq!(day["gpt-5"], (5, 7, 1), "request after the switch");
    }

    #[test]
    fn model_usage_without_timeline_contributes_nothing() {
        let home = tmp_home("no-timeline");
        write_task(
            &home,
            "task-a",
            "/p",
            &[
                r#"{"type":"say","say":"api_req_started","ts":1780272050000,"text":"{\"tokensIn\":100,\"tokensOut\":10}"}"#,
            ],
        );
        assert!(
            Parser.model_usage_by_day(&home, None).is_empty(),
            "no model_usage in metadata → contribute nothing, never guess"
        );
    }

    #[test]
    fn cutoff_filter_drops_old_requests() {
        let home = tmp_home("cutoff");
        write_task(
            &home,
            "task-x",
            "/p",
            &[
                r#"{"type":"say","say":"api_req_started","ts":1700000000000,"text":"{\"tokensIn\":999,\"tokensOut\":999}"}"#,
                r#"{"type":"say","say":"api_req_started","ts":1780000000000,"text":"{\"tokensIn\":10,\"tokensOut\":5}"}"#,
            ],
        );
        let stats = Parser.aggregate_since(&home, Some("2026-01-01T00:00:00Z"));
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].input_tokens, 10, "old request must be filtered");
        assert_eq!(stats[0].output_tokens, 5);
    }

    #[test]
    fn missing_cline_dir_returns_empty() {
        let home = tmp_home("missing");
        let stats = Parser.aggregate_since(&home, None);
        assert!(stats.is_empty());
    }

    #[test]
    fn task_without_metadata_is_skipped() {
        let home = tmp_home("no-metadata");
        let dir = home.join(".config/Code/User/globalStorage/saoudrizwan.claude-dev/tasks/x");
        fs::create_dir_all(&dir).unwrap();
        // No task_metadata.json — we have no cwd to attribute to.
        fs::write(
            dir.join("ui_messages.json"),
            r#"{"type":"say","say":"api_req_started","ts":1780000000000,"text":"{\"tokensIn\":10,\"tokensOut\":5}"}"#,
        )
        .unwrap();
        let stats = Parser.aggregate_since(&home, None);
        assert!(stats.is_empty(), "no cwd → no row");
    }

    #[test]
    fn cline_tool_buckets_are_correct() {
        let mut tb = ToolBreakdown::default();
        bump_cline(&mut tb, "execute_command");
        bump_cline(&mut tb, "read_file");
        bump_cline(&mut tb, "write_to_file");
        bump_cline(&mut tb, "search_files");
        bump_cline(&mut tb, "list_files");
        bump_cline(&mut tb, "browser_action");
        bump_cline(&mut tb, "mcp_tokenade_exec_compact");
        bump_cline(&mut tb, "something_else");
        assert_eq!(tb.bash, 1);
        assert_eq!(tb.read, 1);
        assert_eq!(tb.edit, 1);
        assert_eq!(tb.grep, 1);
        assert_eq!(tb.glob, 1);
        assert_eq!(tb.web, 1);
        assert_eq!(tb.tokenade, 1);
        assert_eq!(tb.other, 1);
    }
}
