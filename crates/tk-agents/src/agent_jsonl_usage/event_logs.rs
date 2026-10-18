//! **The agents that write an event STREAM at a known path.**
//!
//! `chat.jsonl`, `wire.jsonl`, `registry.jsonl`, `events.jsonl`: a named
//! file, appended line by line. We know where to look, and the only question is
//! the shape of the line.
//!
//! Nothing is shared between them beyond that. A common counter would assume
//! that "input tokens" means the same thing everywhere, and that is precisely
//! what is false — caching, reasoning and sub-agents are
//! counted differently by each.

use super::*;

// ── Mux ──────────────────────────────────────────────────────────────
// Tree:  ~/.mux/sessions/<workspaceId>/chat.jsonl (+ subagent-transcripts/*)
// Usage: message.metadata.usage, AI-SDK shape.
// Trap:  inputTokens INCLUDES both cache buckets. Copying it straight would
//        bill the re-sent history twice.

pub struct MuxParser;

pub(crate) fn mux_root(home: &Path) -> PathBuf {
    env_path("CODEBURN_MUX_DIR")
        .or_else(|| env_path("MUX_ROOT"))
        .unwrap_or_else(|| home.join(".mux"))
}

pub(crate) fn mux_chat_files(home: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for ws in sorted_dir(&mux_root(home).join("sessions")) {
        let Some(id) = ws.file_name().and_then(|s| s.to_str()).map(str::to_string) else {
            continue;
        };
        let chat = ws.join("chat.jsonl");
        if chat.is_file() {
            out.push((id.clone(), chat));
        }
        for sub in sorted_dir(&ws.join("subagent-transcripts")) {
            let c = sub.join("chat.jsonl");
            if c.is_file() {
                out.push((id.clone(), c));
            }
        }
    }
    out
}

/// `(counts, iso, model)` for one AI-SDK message, or `None` when it carries no
/// usage.
pub(crate) fn mux_message(msg: &Value) -> Option<(Counts, String, String)> {
    let meta = msg.get("metadata")?;
    let usage = meta.get("usage")?;
    let cache_read = num_any(usage, &["cachedInputTokens"]);
    let cache_creation = num(meta
        .get("providerMetadata")
        .and_then(|p| p.get("anthropic"))
        .and_then(|a| a.get("cacheCreationInputTokens")));
    let raw_input = num_any(usage, &["inputTokens"]);
    let c = Counts {
        // Documented as inclusive of both cache buckets — decompose.
        input: raw_input.saturating_sub(cache_read.saturating_add(cache_creation)),
        // outputTokens is inclusive of reasoningTokens, and reasoning IS
        // billed output, so it stays whole.
        output: num_any(usage, &["outputTokens"]),
        cache_creation,
        cache_read,
        messages: 1,
    };
    if c.is_empty() {
        return None;
    }
    let iso = any_ts_to_iso(meta.get("timestamp").or_else(|| msg.get("createdAt")));
    let model = meta
        .get("model")
        .and_then(|m| m.as_str())
        .map(|m| m.split_once(':').map(|(_, r)| r).unwrap_or(m).to_string())
        .unwrap_or_default();
    Some((c, iso, model))
}

impl TranscriptParser for MuxParser {
    fn agent(&self) -> Agent {
        Agent::Mux
    }

    fn aggregate_since(&self, home_dir: &Path, since_iso: Option<&str>) -> Vec<ProjectUsage> {
        let mut by_cwd = HashMap::new();
        for (ws, chat) in mux_chat_files(home_dir) {
            let mut any = false;
            for msg in read_jsonl(&chat) {
                let Some((c, iso, _)) = mux_message(&msg) else {
                    continue;
                };
                if !after_cutoff(&iso, since_iso) {
                    continue;
                }
                add(&mut by_cwd, ws.clone(), c, &iso);
                any = true;
            }
            if any {
                bump_session(&mut by_cwd, &ws);
            }
        }
        finish(by_cwd)
    }

    fn model_usage_by_day(&self, home_dir: &Path, since_iso: Option<&str>) -> ModelUsageByDay {
        let mut out = ModelUsageByDay::default();
        for (_, chat) in mux_chat_files(home_dir) {
            for msg in read_jsonl(&chat) {
                if let Some((c, iso, model)) = mux_message(&msg) {
                    if after_cutoff(&iso, since_iso) {
                        model_day_add(&mut out, &iso, &model, c);
                    }
                }
            }
        }
        out
    }
}

// ── Kimi / Kimi Code ─────────────────────────────────────────────────
// Both write `wire.jsonl` event logs; the event name and the key spellings
// differ, so one walker serves both with a small descriptor.

pub struct KimiParser;
pub struct KimiCodeParser;

pub(crate) fn kimi_roots(home: &Path) -> Vec<PathBuf> {
    vec![env_path("KIMI_SHARE_DIR").unwrap_or_else(|| home.join(".kimi"))]
}

pub(crate) fn kimicode_roots(home: &Path) -> Vec<PathBuf> {
    if let Some(p) = env_path("KIMI_CODE_HOME") {
        return vec![p];
    }
    vec![
        home.join(".kimi-code"),
        home.join(
            "Library/Application Support/kimi-desktop/daimon-share/daimon/runtime/kimi-code/home",
        ),
    ]
}

/// Every `wire.jsonl` under `<root>/sessions`, at any depth. Kimi nests
/// `<hash>/<session>/wire.jsonl`, Kimi Code nests
/// `wd_<hash>/<session>/agents/<agent>/wire.jsonl` — walking beats encoding
/// two shapes that both drift.
pub(crate) fn wire_files(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        if depth > 6 {
            return;
        }
        for p in sorted_dir(dir) {
            if p.is_dir() {
                walk(&p, depth + 1, out);
            } else if p.file_name().and_then(|s| s.to_str()) == Some("wire.jsonl") {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    walk(&root.join("sessions"), 0, &mut out);
    out
}

/// Kimi: usage rides `StatusUpdate` payloads, with the record optionally
/// wrapped in `{message: {...}}`.
pub(crate) fn kimi_record(rec: &Value) -> Option<(Counts, String, String)> {
    let inner = rec.get("message").unwrap_or(rec);
    let payload = inner.get("payload")?;
    let usage = payload
        .get("token_usage")
        .or_else(|| payload.get("usage"))?;
    let cache_read = num_any(
        usage,
        &[
            "input_cache_read",
            "cache_read_input_tokens",
            "cached_input_tokens",
        ],
    );
    let cache_creation = num_any(
        usage,
        &["input_cache_creation", "cache_creation_input_tokens"],
    );
    // `input_other` is already cache-free; a bare `input` is not.
    let input = if usage.get("input_other").is_some() || usage.get("input_tokens").is_some() {
        num_any(usage, &["input_other", "input_tokens"])
    } else {
        num_any(usage, &["input"]).saturating_sub(cache_read.saturating_add(cache_creation))
    };
    let c = Counts {
        input,
        output: num_any(usage, &["output", "output_tokens"]),
        cache_creation,
        cache_read,
        messages: 1,
    };
    if c.is_empty() {
        return None;
    }
    let iso = any_ts_to_iso(rec.get("timestamp").or_else(|| inner.get("timestamp")));
    let model = payload
        .get("model")
        .or_else(|| payload.get("model_name"))
        .and_then(|m| m.as_str())
        .unwrap_or_default()
        .to_string();
    Some((c, iso, model))
}

/// Kimi Code: one `usage.record` event per request, already decomposed.
pub(crate) fn kimicode_record(rec: &Value) -> Option<(Counts, String, String)> {
    if rec.get("type").and_then(|t| t.as_str()) != Some("usage.record") {
        return None;
    }
    let usage = rec.get("usage")?;
    let c = Counts {
        input: num_any(usage, &["inputOther"]),
        output: num_any(usage, &["output"]),
        cache_creation: num_any(usage, &["inputCacheCreation"]),
        cache_read: num_any(usage, &["inputCacheRead"]),
        messages: 1,
    };
    if c.is_empty() {
        return None;
    }
    Some((
        c,
        any_ts_to_iso(rec.get("time")),
        rec.get("model")
            .and_then(|m| m.as_str())
            .unwrap_or_default()
            .to_string(),
    ))
}

pub(crate) fn kimi_family_aggregate(
    roots: Vec<PathBuf>,
    parse: fn(&Value) -> Option<(Counts, String, String)>,
    since_iso: Option<&str>,
) -> Vec<ProjectUsage> {
    let mut by_cwd = HashMap::new();
    for root in roots {
        for wire in wire_files(&root) {
            // Session identity is the directory holding the log; there is no
            // cwd in the wire format, so the session id IS the project label.
            let sess = wire
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let mut any = false;
            for rec in read_jsonl(&wire) {
                let Some((c, iso, _)) = parse(&rec) else {
                    continue;
                };
                if !after_cutoff(&iso, since_iso) {
                    continue;
                }
                add(&mut by_cwd, sess.clone(), c, &iso);
                any = true;
            }
            if any {
                bump_session(&mut by_cwd, &sess);
            }
        }
    }
    finish(by_cwd)
}

pub(crate) fn kimi_family_models(
    roots: Vec<PathBuf>,
    parse: fn(&Value) -> Option<(Counts, String, String)>,
    since_iso: Option<&str>,
) -> ModelUsageByDay {
    let mut out = ModelUsageByDay::default();
    for root in roots {
        for wire in wire_files(&root) {
            for rec in read_jsonl(&wire) {
                if let Some((c, iso, model)) = parse(&rec) {
                    if after_cutoff(&iso, since_iso) {
                        model_day_add(&mut out, &iso, &model, c);
                    }
                }
            }
        }
    }
    out
}

impl TranscriptParser for KimiParser {
    fn agent(&self) -> Agent {
        Agent::Kimi
    }
    fn aggregate_since(&self, home_dir: &Path, since_iso: Option<&str>) -> Vec<ProjectUsage> {
        kimi_family_aggregate(kimi_roots(home_dir), kimi_record, since_iso)
    }
    fn model_usage_by_day(&self, home_dir: &Path, since_iso: Option<&str>) -> ModelUsageByDay {
        kimi_family_models(kimi_roots(home_dir), kimi_record, since_iso)
    }
}

impl TranscriptParser for KimiCodeParser {
    fn agent(&self) -> Agent {
        Agent::KimiCode
    }
    fn aggregate_since(&self, home_dir: &Path, since_iso: Option<&str>) -> Vec<ProjectUsage> {
        kimi_family_aggregate(kimicode_roots(home_dir), kimicode_record, since_iso)
    }
    fn model_usage_by_day(&self, home_dir: &Path, since_iso: Option<&str>) -> ModelUsageByDay {
        kimi_family_models(kimicode_roots(home_dir), kimicode_record, since_iso)
    }
}

// ── LingTai TUI ──────────────────────────────────────────────────────
// A dedicated ledger file, one usage record per line. `input` includes the
// cached portion.

pub struct LingtaiParser;

pub(crate) fn lingtai_homes(home: &Path) -> Vec<PathBuf> {
    let sep = if cfg!(windows) { ';' } else { ':' };
    for key in ["LINGTAI_HOME", "LINGTAI_TUI_HOME"] {
        if let Ok(v) = std::env::var(key) {
            if !v.is_empty() {
                return v
                    .split(sep)
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from)
                    .collect();
            }
        }
    }
    let mut out = vec![home.join(".lingtai")];
    // Registered projects each keep their own `.lingtai` beside the code.
    let global = env_path("LINGTAI_TUI_GLOBAL_DIR").unwrap_or_else(|| home.join(".lingtai-tui"));
    for rec in read_jsonl(&global.join("registry.jsonl")) {
        if let Some(p) = rec.get("path").and_then(|p| p.as_str()) {
            out.push(PathBuf::from(p).join(".lingtai"));
        }
    }
    out
}

pub(crate) fn lingtai_entry(rec: &Value) -> Option<(Counts, String, String)> {
    let cached = num_any(rec, &["cached"]);
    let c = Counts {
        input: num_any(rec, &["input"]).saturating_sub(cached),
        // `thinking` is generated text the user paid for.
        output: num_any(rec, &["output"]) + num_any(rec, &["thinking"]),
        cache_creation: 0,
        cache_read: cached,
        messages: 1,
    };
    if c.is_empty() {
        return None;
    }
    Some((
        c,
        any_ts_to_iso(rec.get("ts")),
        rec.get("model")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string(),
    ))
}

impl TranscriptParser for LingtaiParser {
    fn agent(&self) -> Agent {
        Agent::LingtaiTui
    }

    fn aggregate_since(&self, home_dir: &Path, since_iso: Option<&str>) -> Vec<ProjectUsage> {
        let mut by_cwd = HashMap::new();
        for h in lingtai_homes(home_dir) {
            let label = h
                .parent()
                .and_then(|p| p.to_str())
                .unwrap_or("")
                .to_string();
            let mut any = false;
            for rec in read_jsonl(&h.join("logs/token_ledger.jsonl")) {
                let Some((c, iso, _)) = lingtai_entry(&rec) else {
                    continue;
                };
                if !after_cutoff(&iso, since_iso) {
                    continue;
                }
                add(&mut by_cwd, label.clone(), c, &iso);
                any = true;
            }
            if any {
                bump_session(&mut by_cwd, &label);
            }
        }
        finish(by_cwd)
    }

    fn model_usage_by_day(&self, home_dir: &Path, since_iso: Option<&str>) -> ModelUsageByDay {
        let mut out = ModelUsageByDay::default();
        for h in lingtai_homes(home_dir) {
            let fallback = read_json(&h.join(".agent.json"))
                .and_then(|v| str_at(&v, &["llm", "model"]).map(str::to_string))
                .unwrap_or_default();
            for rec in read_jsonl(&h.join("logs/token_ledger.jsonl")) {
                if let Some((c, iso, model)) = lingtai_entry(&rec) {
                    let model = if model.is_empty() {
                        fallback.clone()
                    } else {
                        model
                    };
                    if after_cutoff(&iso, since_iso) {
                        model_day_add(&mut out, &iso, &model, c);
                    }
                }
            }
        }
        out
    }
}

// ── Open Design ──────────────────────────────────────────────────────
// Run-scoped event logs; usage arrives as `event=agent`, `data.type=usage`.

pub struct OpenDesignParser;

pub(crate) fn open_design_bases(home: &Path) -> Vec<PathBuf> {
    if let Some(p) = env_path("CODEBURN_OPEN_DESIGN_DIR") {
        return vec![p];
    }
    vec![
        home.join("Library/Application Support/Open Design"),
        home.join("AppData/Roaming/Open Design"),
        home.join(".config/Open Design"),
    ]
}

/// `(namespace, events.jsonl)` for every run under a base.
pub(crate) fn open_design_runs(base: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let mut collect = |runs_dir: PathBuf, ns: String| {
        for run in sorted_dir(&runs_dir) {
            let ev = run.join("events.jsonl");
            if ev.is_file() {
                out.push((ns.clone(), ev));
            }
        }
    };
    collect(base.join("data/runs"), "open-design".to_string());
    collect(base.join("runs"), "open-design".to_string());
    for ns_dir in sorted_dir(&base.join("namespaces")) {
        let ns = ns_dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("open-design")
            .to_string();
        collect(ns_dir.join("data/runs"), ns);
    }
    out
}

pub(crate) fn open_design_usage(rec: &Value) -> Option<(Counts, String)> {
    if rec.get("event").and_then(|e| e.as_str()) != Some("agent") {
        return None;
    }
    let data = rec.get("data")?;
    if data.get("type").and_then(|t| t.as_str()) != Some("usage") {
        return None;
    }
    let usage = data.get("usage")?;
    let c = Counts {
        input: num_any(usage, &["input_tokens"]),
        output: num_any(usage, &["output_tokens"]) + num_any(usage, &["thought_tokens"]),
        cache_creation: 0,
        cache_read: num_any(usage, &["cached_read_tokens"]),
        messages: 1,
    };
    if c.is_empty() {
        return None;
    }
    Some((c, any_ts_to_iso(rec.get("timestamp"))))
}

impl TranscriptParser for OpenDesignParser {
    fn agent(&self) -> Agent {
        Agent::OpenDesign
    }

    fn aggregate_since(&self, home_dir: &Path, since_iso: Option<&str>) -> Vec<ProjectUsage> {
        let mut by_cwd = HashMap::new();
        for base in open_design_bases(home_dir) {
            for (ns, events) in open_design_runs(&base) {
                let mut any = false;
                for rec in read_jsonl(&events) {
                    let Some((c, iso)) = open_design_usage(&rec) else {
                        continue;
                    };
                    if !after_cutoff(&iso, since_iso) {
                        continue;
                    }
                    add(&mut by_cwd, ns.clone(), c, &iso);
                    any = true;
                }
                if any {
                    bump_session(&mut by_cwd, &ns);
                }
            }
        }
        finish(by_cwd)
    }

    fn model_usage_by_day(&self, home_dir: &Path, since_iso: Option<&str>) -> ModelUsageByDay {
        let mut out = ModelUsageByDay::default();
        for base in open_design_bases(home_dir) {
            for (_, events) in open_design_runs(&base) {
                // The model is announced once and applies until re-announced —
                // a state machine, not a per-record field.
                let mut current = String::new();
                for rec in read_jsonl(&events) {
                    if let Some(m) = rec
                        .get("data")
                        .and_then(|d| d.get("model"))
                        .and_then(|m| m.as_str())
                    {
                        current = m.to_string();
                    }
                    if let Some((c, iso)) = open_design_usage(&rec) {
                        if after_cutoff(&iso, since_iso) {
                            model_day_add(&mut out, &iso, &current, c);
                        }
                    }
                }
            }
        }
        out
    }
}
