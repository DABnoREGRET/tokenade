//! **The agents that store their accounting per SESSION.**
//!
//! No named file: one directory per session, sometimes nested, sometimes
//! with its sub-agents keeping their own totals.
//!
//! The difference matters for a practical reason: a stream is read going forward,
//! a directory is WALKED — and a walk that finds nothing is green. The
//! floors are therefore in the tests, not in the code.

use super::*;

// ── Droid (Factory) ──────────────────────────────────────────────────
// Totals live in a companion settings file, per session, not per turn.

pub struct DroidParser;

pub(crate) fn droid_sessions(home: &Path) -> Vec<PathBuf> {
    let root = env_path("FACTORY_DIR").unwrap_or_else(|| home.join(".factory"));
    let mut out = Vec::new();
    for sub in sorted_dir(&root.join("sessions")) {
        if sub.is_dir() {
            out.extend(
                sorted_dir(&sub)
                    .into_iter()
                    .filter(|p| p.extension().map(|e| e == "jsonl").unwrap_or(false)),
            );
        } else if sub.extension().map(|e| e == "jsonl").unwrap_or(false) {
            out.push(sub);
        }
    }
    out
}

impl TranscriptParser for DroidParser {
    fn agent(&self) -> Agent {
        Agent::Droid
    }

    fn aggregate_since(&self, home_dir: &Path, since_iso: Option<&str>) -> Vec<ProjectUsage> {
        let mut by_cwd = HashMap::new();
        for jsonl in droid_sessions(home_dir) {
            let settings = jsonl.with_extension("settings.json");
            let Some(sv) = read_json(&settings) else {
                continue;
            };
            let Some(u) = sv.get("tokenUsage") else {
                continue;
            };
            let c = Counts {
                input: num_any(u, &["inputTokens"]),
                output: num_any(u, &["outputTokens"]) + num_any(u, &["thinkingTokens"]),
                cache_creation: num_any(u, &["cacheCreationTokens"]),
                cache_read: num_any(u, &["cacheReadTokens"]),
                messages: 1,
            };
            if c.is_empty() {
                continue;
            }
            // The totals are session-scoped, so the session's own last event
            // timestamp dates them. Reference splits them evenly per call; we
            // aggregate per project, so no split is needed — and none is
            // invented.
            let records = read_jsonl(&jsonl);
            let iso = records
                .iter()
                .filter_map(|r| {
                    let t = any_ts_to_iso(r.get("timestamp"));
                    (!t.is_empty()).then_some(t)
                })
                .max()
                .unwrap_or_default();
            if !after_cutoff(&iso, since_iso) {
                continue;
            }
            let cwd = records
                .iter()
                .find_map(|r| str_at(r, &["cwd"]).map(str::to_string))
                .unwrap_or_default();
            add(&mut by_cwd, cwd.clone(), c, &iso);
            bump_session(&mut by_cwd, &cwd);
        }
        finish(by_cwd)
    }
}

// ── CodeWhale ────────────────────────────────────────────────────────
// One JSON file per session, and a SINGLE aggregate counter.

pub struct CodewhaleParser;

pub(crate) fn codewhale_dirs(home: &Path) -> Vec<PathBuf> {
    if let Some(p) = env_path("CODEWHALE_HOME") {
        return vec![p.join("sessions")];
    }
    vec![
        home.join(".codewhale/sessions"),
        home.join(".deepseek/sessions"),
    ]
}

impl TranscriptParser for CodewhaleParser {
    fn agent(&self) -> Agent {
        Agent::Codewhale
    }

    fn aggregate_since(&self, home_dir: &Path, since_iso: Option<&str>) -> Vec<ProjectUsage> {
        let mut by_cwd = HashMap::new();
        for dir in codewhale_dirs(home_dir) {
            for p in sorted_dir(&dir) {
                if p.extension().map(|e| e != "json").unwrap_or(true) {
                    continue;
                }
                let Some(v) = read_json(&p) else { continue };
                let Some(meta) = v.get("metadata") else {
                    continue;
                };
                let total = num_any(meta, &["total_tokens"]);
                if total == 0 {
                    continue;
                }
                let iso = meta
                    .get("updated_at")
                    .or_else(|| meta.get("created_at"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string();
                if !after_cutoff(&iso, since_iso) {
                    continue;
                }
                let cwd = str_at(&v, &["metadata", "workspace"])
                    .unwrap_or("")
                    .to_string();
                // CodeWhale publishes ONE number with no input/output split.
                // It lands in input because that is where an unsplit total is
                // least wrong: input dominates a coding session by an order of
                // magnitude. Splitting it by a guessed ratio would invent data.
                add(
                    &mut by_cwd,
                    cwd.clone(),
                    Counts {
                        input: total,
                        messages: 1,
                        ..Default::default()
                    },
                    &iso,
                );
                bump_session(&mut by_cwd, &cwd);
            }
        }
        finish(by_cwd)
    }
}

// ── Mistral Vibe ─────────────────────────────────────────────────────
// meta.json holds session totals; messages.jsonl is transcript only.

pub struct MistralVibeParser;

pub(crate) fn vibe_session_dirs(home: &Path) -> Vec<PathBuf> {
    let root = env_path("VIBE_HOME").unwrap_or_else(|| home.join(".vibe"));
    let mut out = Vec::new();
    for d in sorted_dir(&root.join("logs/session")) {
        if !d.is_dir() {
            continue;
        }
        out.push(d.clone());
        // Sub-agents keep their own meta.json with their own totals.
        for a in sorted_dir(&d.join("agents")) {
            if a.is_dir() {
                out.push(a);
            }
        }
    }
    out
}

impl TranscriptParser for MistralVibeParser {
    fn agent(&self) -> Agent {
        Agent::MistralVibe
    }

    fn aggregate_since(&self, home_dir: &Path, since_iso: Option<&str>) -> Vec<ProjectUsage> {
        let mut by_cwd = HashMap::new();
        for d in vibe_session_dirs(home_dir) {
            let Some(meta) = read_json(&d.join("meta.json")) else {
                continue;
            };
            let Some(stats) = meta.get("stats") else {
                continue;
            };
            let c = Counts {
                input: num_any(stats, &["session_prompt_tokens"]),
                output: num_any(stats, &["session_completion_tokens"]),
                messages: 1,
                ..Default::default()
            };
            if c.is_empty() {
                continue;
            }
            let iso = meta
                .get("end_time")
                .or_else(|| meta.get("start_time"))
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();
            if !after_cutoff(&iso, since_iso) {
                continue;
            }
            let cwd = str_at(&meta, &["environment", "working_directory"])
                .unwrap_or("")
                .to_string();
            add(&mut by_cwd, cwd.clone(), c, &iso);
            bump_session(&mut by_cwd, &cwd);
        }
        finish(by_cwd)
    }

    fn model_usage_by_day(&self, home_dir: &Path, since_iso: Option<&str>) -> ModelUsageByDay {
        let mut out = ModelUsageByDay::default();
        for d in vibe_session_dirs(home_dir) {
            let Some(meta) = read_json(&d.join("meta.json")) else {
                continue;
            };
            let Some(stats) = meta.get("stats") else {
                continue;
            };
            let c = Counts {
                input: num_any(stats, &["session_prompt_tokens"]),
                output: num_any(stats, &["session_completion_tokens"]),
                messages: 1,
                ..Default::default()
            };
            let iso = meta
                .get("end_time")
                .or_else(|| meta.get("start_time"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            let model = str_at(&meta, &["config", "active_model"]).unwrap_or("");
            if after_cutoff(iso, since_iso) {
                model_day_add(&mut out, iso, model, c);
            }
        }
        out
    }
}
