//! Multi-agent transcript aggregation.
//!
//! Generalises the Claude-Code-specific [`crate::claudecode_transcript`]
//! into a uniform interface across coding agents that write per-session
//! token usage to local files. Each supported agent has its own parser
//! module next to this one (`agent_codex`, `agent_cline`, …); they all
//! implement [`TranscriptParser`] and feed into the dispatch fn
//! [`aggregate_all_agents`] consumed by the dashboard.
//!
//! Adding a new agent: implement [`TranscriptParser`] in a new module,
//! add a unit-struct `pub struct Parser;`, plug it into [`parsers`].
//! The dashboard renderer reads agents back out via [`Agent::label`].
//!
//! Schemas (source-verified 2026-06-10 — details in each parser's
//! module doc):
//!   - **Claude Code**: `~/.claude/projects/<encoded-cwd>/<id>.jsonl`,
//!     `type=assistant` records with `message.usage.input_tokens` etc.
//!   - **Codex**: `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`,
//!     model from `turn_context.payload.model`, `event_msg` records
//!     with `payload.info.total_token_usage` (cumulative → deltas).
//!   - **Cline**: VS Code globalStorage (all products/OSes) +
//!     `~/.cline/data/`, `tasks/<id>/ui_messages.json` is a single
//!     JSON ARRAY of `api_req_started` entries (`tokensIn/tokensOut/
//!     cacheWrites/cacheReads` in stringified `text`); model from
//!     `task_metadata.json → model_usage[]` timeline.
//!   - **Copilot Chat**: VS Code `User/workspaceStorage/<hash>/
//!     chatSessions/*.json` + `globalStorage/emptyWindowChatSessions/`;
//!     `requests[].modelId` + `completionTokens` only — promptTokens is
//!     never persisted (output-only agent here).
//!   - **Gemini CLI** / **Qwen Code**: `~/.gemini|.qwen/tmp/<hash>/
//!     chats/*.jsonl`, per-message `model` + token block; project dir
//!     is a hash → per-model view only, no per-project rows.
//!   - **opencode**: `~/.local/share/opencode` on every OS; SQLite
//!     `opencode.db` (v1.2.0+) with JSON-era fallbacks, priority
//!     cascade to avoid double-counting migrated data. Assistant
//!     messages carry `modelID` + `tokens{}` + `path.cwd` → feeds
//!     both the projects and models views.
//!
//! Graceful degradation: any data an agent does not persist is simply
//! not counted — never guessed. Agents that store transcripts off-disk
//! or in undocumented binary formats (Cursor's own chat, Cody,
//! Windsurf, Zed AI, Continue, Aider, Antigravity) contribute nothing.

use std::path::Path;

pub use crate::claudecode_transcript::{ProjectUsage, ToolBreakdown};

/// `day (YYYY-MM-DD) → model → (input_fresh, output, messages)`.
/// Same convention everywhere: input_fresh excludes cache READS
/// (re-sent history) but includes cache writes — see
/// [`crate::claudecode_transcript::model_usage_by_day`].
pub type ModelUsageByDay =
    std::collections::BTreeMap<String, std::collections::BTreeMap<String, (u64, u64, u64)>>;

/// `day (YYYY-MM-DD) → tool → tool_result tokens`. Same per-tool
/// attribution as [`ProjectUsage::tool_result_tokens_by_tool`], but
/// split by the day the tool output landed so the dashboard's "outside
/// tokenade" panel can honour the period selector instead of only
/// showing an all-time total. tokenade-provided namespaces are kept
/// here exactly as the all-time buckets keep them — the renderer
/// subtracts genuine tokenade activity at display time.
pub type ToolTokensByDay =
    std::collections::BTreeMap<String, std::collections::BTreeMap<String, u64>>;

/// One supported coding agent. The discriminant is stable for the
/// dashboard's display column and for telemetry; new agents append
/// to the end of the enum to keep that stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Agent {
    ClaudeCode,
    Codex,
    Cline,
    Copilot,
    Gemini,
    Qwen,
    Opencode,
    Hermes,
    Pi,
    Kilo,
    Grok,
    Openclaw,
    // Cline-family forks: same on-disk task layout, different extension id.
    // Counted separately on purpose — merging them into Cline would tell the
    // user the wrong tool burned the tokens.
    RooCode,
    IbmBob,
    Goose,
    Crush,
    Zcode,
    Zerostack,
    Mux,
    Kimi,
    KimiCode,
    LingtaiTui,
    OpenDesign,
    Droid,
    Codewhale,
    MistralVibe,
    Forge,
    // No hook of any kind (an IDE, not a CLI), so its own accounting was
    // invisible until `agent_antigravity` read it out of its conversation DBs.
    Antigravity,
}

impl Agent {
    /// Short label shown in the dashboard column.
    /// The two names an agent answers to: the dashboard label, and the stable id
    /// written into `gain.jsonl`.
    ///
    /// ONE table on purpose. These were two parallel 28-arm matches, which is
    /// exactly how a newly-supported agent ends up with a label and no id (or
    /// the reverse) — the compiler checks that every variant appears, not that
    /// two separate lists stayed in step.
    fn names(&self) -> (&'static str, &'static str) {
        match self {
            Agent::ClaudeCode => ("Claude", "claude-code"),
            Agent::Codex => ("Codex", "codex"),
            Agent::Cline => ("Cline", "cline"),
            Agent::Copilot => ("Copilot", "copilot"),
            Agent::Gemini => ("Gemini", "gemini-cli"),
            Agent::Qwen => ("Qwen", "qwen-code"),
            Agent::Opencode => ("opencode", "opencode"),
            Agent::Hermes => ("Hermes", "hermes"),
            Agent::Pi => ("Pi", "pi"),
            Agent::Kilo => ("Kilo", "kilo-code"),
            Agent::Grok => ("Grok", "grok"),
            Agent::Openclaw => ("OpenClaw", "openclaw"),
            Agent::RooCode => ("Roo Code", "roo-code"),
            Agent::IbmBob => ("IBM Bob", "ibm-bob"),
            Agent::Goose => ("Goose", "goose"),
            Agent::Crush => ("Crush", "crush"),
            Agent::Zcode => ("zcode", "zcode"),
            Agent::Zerostack => ("Zerostack", "zerostack"),
            Agent::Mux => ("Mux", "mux"),
            Agent::Kimi => ("Kimi", "kimi"),
            Agent::KimiCode => ("Kimi Code", "kimicode"),
            Agent::LingtaiTui => ("LingTai", "lingtai-tui"),
            Agent::OpenDesign => ("Open Design", "open-design"),
            Agent::Droid => ("Droid", "droid"),
            Agent::Codewhale => ("CodeWhale", "codewhale"),
            Agent::MistralVibe => ("Mistral Vibe", "mistral-vibe"),
            Agent::Forge => ("Forge", "forge"),
            Agent::Antigravity => ("Antigravity", "antigravity"),
        }
    }

    /// Short label shown in the dashboard column.
    pub fn label(&self) -> &'static str {
        self.names().0
    }

    /// Stable machine-readable id used in `gain.jsonl` records.
    pub fn id(&self) -> &'static str {
        self.names().1
    }
}

/// All plausible VS Code-family `User` data directories under `home`,
/// across OSes and product variants. VS Code extensions (Cline) and
/// VS Code core chat storage (Copilot) both live under one of these:
///   - Linux:   `~/.config/<Product>/User`
///   - macOS:   `~/Library/Application Support/<Product>/User`
///   - Windows: `%APPDATA%\<Product>\User` (= `~/AppData/Roaming/...`)
///
/// Products: stock VS Code, Insiders, VSCodium, Cursor. We probe all
/// of them unconditionally — `read_dir` on a missing path is a cheap
/// no-op, and probing beats per-OS `cfg` branching for users who sync
/// dotfiles across machines.
pub fn vscode_user_dirs(home_dir: &Path) -> Vec<std::path::PathBuf> {
    vscode_user_dirs_for(home_dir, &["Code", "Code - Insiders", "VSCodium", "Cursor"])
}

/// Same layout, arbitrary product names. Some agents ship their own VS Code
/// fork and store under ITS product dir rather than under `Code` — IBM Bob
/// uses `IBM Bob` / `Bob-IDE`, so scanning only the standard four would find
/// nothing and report the agent as unused.
pub fn vscode_user_dirs_for(home_dir: &Path, products: &[&str]) -> Vec<std::path::PathBuf> {
    let mut out = Vec::with_capacity(products.len() * 3);
    for product in products.iter().copied() {
        out.push(home_dir.join(".config").join(product).join("User"));
        out.push(
            home_dir
                .join("Library")
                .join("Application Support")
                .join(product)
                .join("User"),
        );
        out.push(
            home_dir
                .join("AppData")
                .join("Roaming")
                .join(product)
                .join("User"),
        );
    }
    out
}

/// One agent's transcript parser. Implementations are stateless
/// unit-structs (e.g. `agent_codex::Parser`); they walk the file
/// tree on each call and return per-cwd aggregates.
pub trait TranscriptParser: Send + Sync {
    fn agent(&self) -> Agent;

    /// Same shape as [`crate::claudecode_transcript::aggregate_since`].
    /// Pass `None` to count everything; pass an ISO-8601 string
    /// (`YYYY-MM-DDTHH:MM:SSZ`) to filter to messages at or after
    /// the cutoff. The comparison is lexicographic — ISO-8601 is
    /// monotonic in lex order so this is equivalent to chronological.
    ///
    /// The `sessions` field in each returned [`ProjectUsage`] uses a
    /// per-agent counting convention; see the field's doc comment for
    /// the exact rule each implementation applies.
    fn aggregate_since(&self, home_dir: &Path, since_iso: Option<&str>) -> Vec<ProjectUsage>;

    /// Per-day, per-model token usage for the dashboard's "Models"
    /// panel. Default impl returns an empty map: an agent whose
    /// transcript format doesn't record the model name simply
    /// contributes nothing (we never guess a model).
    fn model_usage_by_day(&self, _home_dir: &Path, _since_iso: Option<&str>) -> ModelUsageByDay {
        ModelUsageByDay::default()
    }

    /// Per-day, per-tool tool_result tokens for the dashboard's "outside
    /// tokenade" panel. Default impl returns an empty map: an agent that
    /// can't attribute tool output per day contributes nothing under a
    /// windowed view (same honest degradation as [`Self::model_usage_by_day`]).
    fn tool_tokens_by_day(&self, _home_dir: &Path, _since_iso: Option<&str>) -> ToolTokensByDay {
        ToolTokensByDay::default()
    }

    /// Per-day total cache-WRITE (`cache_creation`) input tokens for this agent.
    /// Default empty: only Anthropic-style agents report cache writes. The
    /// dashboard subtracts this from total consumption so the bill-share isn't
    /// dominated by context re-caching tokenade can't touch.
    fn cache_creation_by_day(
        &self,
        _home_dir: &Path,
        _since_iso: Option<&str>,
    ) -> std::collections::BTreeMap<String, u64> {
        std::collections::BTreeMap::new()
    }

    /// One entry per session this agent recorded: its id and the model it
    /// ran, plus the [first, last] message timestamps (epoch millis). This
    /// is what lets a savings event be priced at the EXACT model that ran
    /// it — matched by its `sess` id, or, when it has none, by which
    /// session's time span contains its timestamp — instead of a
    /// day-dominant guess. Default empty: an agent whose transcripts don't
    /// record the model contributes nothing (the caller falls back to the
    /// day-dominant model, then the default rate). A session that ran more
    /// than one model emits one entry per model (each with that model's own
    /// time span), so a mid-session switch still attributes correctly.
    fn session_models(&self, _home_dir: &Path, _since_iso: Option<&str>) -> Vec<SessionModel> {
        Vec::new()
    }

    /// Everything the dashboard reads out of this agent's transcripts, from
    /// ONE walk of them.
    ///
    /// The payload used to call four separate methods plus a fifth lifetime
    /// pass for the output-lean estimate, and every one of them re-walked the
    /// same corpus. MEASURED 2026-09-04 on this machine's 1.3 GB of Claude
    /// Code transcripts: 384 + 174 + 171 + 231 + 223 ms, for five readings of
    /// bytes that had already been decoded into
    /// [`crate::transcript_cache`]'s rows.
    ///
    /// The default implementation below IS those five calls, so a parser that
    /// does not override this keeps working unchanged. A parser whose corpus
    /// is large enough to matter overrides it with a single fused pass —
    /// [`crate::claudecode_transcript`] does.
    fn digest(&self, home_dir: &Path, since_iso: Option<&str>) -> AgentDigest {
        AgentDigest {
            projects: self.aggregate_since(home_dir, since_iso),
            models_by_day: self.model_usage_by_day(home_dir, since_iso),
            tools_by_day: self.tool_tokens_by_day(home_dir, since_iso),
            cache_creation_by_day: self.cache_creation_by_day(home_dir, since_iso),
            lifetime_models_by_day: self.model_usage_by_day(home_dir, None),
            session_models: self.session_models(home_dir, None),
        }
    }
}

/// One agent's whole contribution to the dashboard, from one walk of its
/// transcripts. See [`TranscriptParser::digest`].
#[derive(Default)]
pub struct AgentDigest {
    pub projects: Vec<ProjectUsage>,
    /// Windowed by `since_iso`, like [`TranscriptParser::model_usage_by_day`].
    pub models_by_day: ModelUsageByDay,
    pub tools_by_day: ToolTokensByDay,
    pub cache_creation_by_day: std::collections::BTreeMap<String, u64>,
    /// NOT windowed. Two lifetime figures are built from it — the output-lean
    /// estimate ([`estimated_output_lean_by_agent`]) and the day-dominant
    /// model table inside [`ModelAttributor`] — and both were separate full
    /// passes before this field existed.
    pub lifetime_models_by_day: ModelUsageByDay,
    /// NOT windowed, for the same reason: [`ModelAttributor::build`] asks for
    /// every session ever recorded.
    pub session_models: Vec<SessionModel>,
}

/// Every agent's digest, merged — the ONE transcript walk a dashboard render
/// needs. Parsers run in parallel; only Claude Code's takes the shared
/// row-cache lock, so the others genuinely overlap.
pub struct MergedDigest {
    pub projects: Vec<(Agent, ProjectUsage)>,
    pub models_by_day: ModelUsageByDay,
    pub tools_by_day: ToolTokensByDay,
    pub cache_creation_by_day: std::collections::BTreeMap<String, u64>,
    pub lifetime_models_by_day: ModelUsageByDay,
    pub session_models: Vec<SessionModel>,
    /// `(agent label, estimated saved)` — the output-lean estimate, EXCLUDING
    /// Claude Code (credited live by its own hook; counting it here would
    /// double-count). Same rule as [`estimated_output_lean_by_agent`].
    pub output_lean_by_agent: Vec<(&'static str, u64)>,
}

fn merge_models_into(out: &mut ModelUsageByDay, src: ModelUsageByDay) {
    for (day, models) in src {
        let day_entry = out.entry(day).or_default();
        for (model, (i, o, n)) in models {
            let e = day_entry.entry(model).or_insert((0, 0, 0));
            e.0 = e.0.saturating_add(i);
            e.1 = e.1.saturating_add(o);
            e.2 = e.2.saturating_add(n);
        }
    }
}

pub fn digest_all_agents(home_dir: &Path, since_iso: Option<&str>) -> MergedDigest {
    use rayon::prelude::*;
    let ratio = crate::coefficients::output_lean_ratio();
    let per_agent: Vec<(Agent, AgentDigest)> = parsers()
        .par_iter()
        .map(|p| (p.agent(), p.digest(home_dir, since_iso)))
        .collect();

    let mut out = MergedDigest {
        projects: Vec::new(),
        models_by_day: ModelUsageByDay::default(),
        tools_by_day: ToolTokensByDay::default(),
        cache_creation_by_day: Default::default(),
        lifetime_models_by_day: ModelUsageByDay::default(),
        session_models: Vec::new(),
        output_lean_by_agent: Vec::new(),
    };
    for (agent, d) in per_agent {
        for u in d.projects {
            out.projects.push((agent, u));
        }
        merge_models_into(&mut out.models_by_day, d.models_by_day);
        for (day, tools) in d.tools_by_day {
            let e = out.tools_by_day.entry(day).or_default();
            for (tool, n) in tools {
                let t = e.entry(tool).or_insert(0);
                *t = t.saturating_add(n);
            }
        }
        for (day, n) in d.cache_creation_by_day {
            let e = out.cache_creation_by_day.entry(day).or_insert(0);
            *e = e.saturating_add(n);
        }
        if agent != Agent::ClaudeCode {
            let total_output: u64 = d
                .lifetime_models_by_day
                .values()
                .flat_map(|by_model| by_model.values())
                .map(|&(_, output, _)| output)
                .sum();
            let saved = (total_output as f64 * ratio).round() as u64;
            if saved > 0 {
                out.output_lean_by_agent.push((agent.label(), saved));
            }
        }
        merge_models_into(&mut out.lifetime_models_by_day, d.lifetime_models_by_day);
        out.session_models.extend(d.session_models);
    }
    out.projects.sort_by(|a, b| b.1.last_ts.cmp(&a.1.last_ts));
    out
}

/// The model a session ran and the window it ran in — see
/// [`TranscriptParser::session_models`].
#[derive(Debug, Clone)]
pub struct SessionModel {
    /// Session id AS THE AGENT NAMES IT. Matching against a gain row's
    /// `sess` is done dash-insensitively + lowercased (Claude session
    /// UUIDs carry dashes; the hook stores them stripped).
    pub sess: String,
    pub model: String,
    /// Epoch millis of the first / last dated message for this (session,
    /// model). `end_ms` is inclusive-ish; callers treat [start, end] as the
    /// span that owns a timestamp.
    pub start_ms: u64,
    pub end_ms: u64,
}

/// Normalize a session id for cross-source matching: drop dashes, lowercase.
pub fn norm_sess(s: &str) -> String {
    s.chars()
        .filter(|c| *c != '-')
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Every agent's [`SessionModel`] entries, merged. The reporter and the
/// dashboards attribute savings through this.
pub fn session_models_all_agents(home_dir: &Path, since_iso: Option<&str>) -> Vec<SessionModel> {
    let mut out = Vec::new();
    for p in parsers() {
        out.extend(p.session_models(home_dir, since_iso));
    }
    out
}

/// Maps a savings event to the EXACT model that ran it. ONE algorithm,
/// shared by the client dashboards/statusline valuation and the reporter's
/// upload stamping, so a token is priced identically on the client and the
/// server. Attribution order (first hit wins):
///   1. the event's own `sess` id → that session's model (the model whose
///      span owns the event ts if the session ran several);
///   2. no sess → whichever session's time span contains the event ts;
///   3. the day-dominant model for the event's day;
///   4. the overall-dominant model across all transcripts;
///   5. `None` → priced at the default rate (client and server agree on the
///      same env defaults).
pub struct ModelAttributor {
    by_sess: std::collections::HashMap<String, Vec<(String, u64, u64)>>,
    intervals: Vec<(u64, u64, String)>,
    day_dominant: std::collections::HashMap<String, String>,
    overall: Option<String>,
}

impl ModelAttributor {
    /// Build from already-gathered parts. Kept separate from [`Self::build`]
    /// so the attribution logic is unit-testable without a filesystem.
    pub(crate) fn from_parts(
        sessions: Vec<SessionModel>,
        day_dominant: std::collections::HashMap<String, String>,
        overall: Option<String>,
    ) -> Self {
        let mut by_sess: std::collections::HashMap<String, Vec<(String, u64, u64)>> =
            Default::default();
        let mut intervals: Vec<(u64, u64, String)> = Vec::new();
        for sm in &sessions {
            by_sess.entry(norm_sess(&sm.sess)).or_default().push((
                sm.model.clone(),
                sm.start_ms,
                sm.end_ms,
            ));
            intervals.push((sm.start_ms, sm.end_ms, sm.model.clone()));
        }
        intervals.sort_by_key(|(s, _, _)| *s);
        Self {
            by_sess,
            intervals,
            day_dominant,
            overall,
        }
    }

    pub fn build(home_dir: &Path) -> Self {
        let sms = session_models_all_agents(home_dir, None);
        let by_day = model_usage_all_agents(home_dir, None);
        Self::from_sessions_and_usage(sms, &by_day)
    }

    /// Same construction from parts a caller ALREADY has. The dashboard walks
    /// every transcript once ([`digest_all_agents`]) and both of these fall
    /// out of that walk; [`Self::build`] would walk them a second and third
    /// time (measured 461 ms on this machine).
    pub fn from_sessions_and_usage(sms: Vec<SessionModel>, by_day: &ModelUsageByDay) -> Self {
        let mut day_dominant = std::collections::HashMap::new();
        let mut overall_vol: std::collections::HashMap<String, u64> = Default::default();
        for (day, models) in by_day {
            let mut best: Option<(String, u64)> = None;
            for (m, (i, o, _)) in models {
                let vol = i.saturating_add(*o);
                *overall_vol.entry(m.clone()).or_default() += vol;
                if best.as_ref().map(|(_, v)| vol > *v).unwrap_or(true) {
                    best = Some((m.clone(), vol));
                }
            }
            if let Some((m, _)) = best {
                day_dominant.insert(day.clone(), m);
            }
        }
        let overall = dominant_key(overall_vol.into_iter());
        Self::from_parts(sms, day_dominant, overall)
    }

    /// `ts_ms` is epoch millis. Returns the attributed model id, or `None`
    /// when nothing matches (caller prices at the default rate).
    pub fn model_for(&self, sess: Option<&str>, ts_ms: u64) -> Option<String> {
        if let Some(s) = sess {
            if let Some(cands) = self.by_sess.get(&norm_sess(s)) {
                // Span that contains ts wins (tightest first); else the model
                // whose span is nearest to ts (a session that switched models
                // mid-way still attributes each event to the right side).
                if let Some((m, _, _)) = cands
                    .iter()
                    .filter(|(_, a, b)| ts_ms >= *a && ts_ms <= *b)
                    .min_by_key(|(_, a, b)| b.saturating_sub(*a))
                {
                    return Some(m.clone());
                }
                if let Some((m, _, _)) = cands.iter().min_by_key(|(_, a, b)| {
                    if ts_ms == 0 {
                        0
                    } else {
                        (ts_ms as i64 - *a as i64)
                            .abs()
                            .min((ts_ms as i64 - *b as i64).abs())
                    }
                }) {
                    return Some(m.clone());
                }
            }
        }
        if ts_ms > 0 {
            // Among all sessions whose span contains ts, prefer the TIGHTEST
            // (smallest span) — with concurrent sessions (two terminals, or a
            // long session that brackets a short one), the narrowest match is
            // the most specific owner of that instant.
            if let Some((_, _, m)) = self
                .intervals
                .iter()
                .filter(|(a, b, _)| ts_ms >= *a && ts_ms <= *b)
                .min_by_key(|(a, b, _)| b.saturating_sub(*a))
            {
                return Some(m.clone());
            }
            let days = (ts_ms / 1000 / 86_400) as i64;
            let (y, mo, d) = crate::platform::civil_from_days(days);
            let day = format!("{y:04}-{mo:02}-{d:02}");
            if let Some(m) = self.day_dominant.get(&day) {
                return Some(m.clone());
            }
        }
        self.overall.clone()
    }
}

#[cfg(test)]
mod attributor_tests {
    use super::*;
    use std::collections::HashMap;

    fn sm(sess: &str, model: &str, a: u64, b: u64) -> SessionModel {
        SessionModel {
            sess: sess.into(),
            model: model.into(),
            start_ms: a,
            end_ms: b,
        }
    }

    #[test]
    fn norm_sess_strips_dashes_and_lowercases() {
        assert_eq!(norm_sess("AB-cd-12"), "abcd12");
        assert_eq!(norm_sess("b233aaf9-5ef2-46bb"), "b233aaf95ef246bb");
    }

    #[test]
    fn sess_id_match_wins_dash_and_case_insensitive() {
        let a = ModelAttributor::from_parts(
            vec![sm("ABC-123", "claude-opus-4-8", 100, 200)],
            HashMap::new(),
            None,
        );
        assert_eq!(
            a.model_for(Some("abc123"), 150).as_deref(),
            Some("claude-opus-4-8")
        );
        // ts outside the span still resolves via the session's nearest model
        assert_eq!(
            a.model_for(Some("ABC-123"), 999).as_deref(),
            Some("claude-opus-4-8")
        );
    }

    #[test]
    fn ts_interval_tightest_span_wins() {
        let a = ModelAttributor::from_parts(
            vec![
                sm("long", "claude-opus-4-8", 0, 1000),
                sm("short", "claude-sonnet-4-6", 400, 500),
            ],
            HashMap::new(),
            None,
        );
        assert_eq!(a.model_for(None, 450).as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(a.model_for(None, 100).as_deref(), Some("claude-opus-4-8"));
    }

    #[test]
    fn day_dominant_then_overall_fallback() {
        let mut dd = HashMap::new();
        dd.insert("2026-07-04".to_string(), "claude-haiku-4-5".to_string());
        let a = ModelAttributor::from_parts(vec![], dd, Some("claude-opus-4-8".into()));
        let on_day =
            crate::claudecode_transcript::iso8601_to_epoch_ms("2026-07-04T12:00:00Z").unwrap();
        assert_eq!(
            a.model_for(None, on_day).as_deref(),
            Some("claude-haiku-4-5")
        );
        let other_day =
            crate::claudecode_transcript::iso8601_to_epoch_ms("2020-01-01T00:00:00Z").unwrap();
        assert_eq!(
            a.model_for(None, other_day).as_deref(),
            Some("claude-opus-4-8")
        );
    }

    #[test]
    fn empty_attributor_returns_none() {
        let a = ModelAttributor::from_parts(vec![], HashMap::new(), None);
        assert_eq!(a.model_for(None, 123), None);
        assert_eq!(a.model_for(Some("x"), 0), None);
    }
}

/// All currently-registered parsers. Order is the priority shown in
/// the dashboard — Claude first since it's the reference impl with
/// the richest schema.
pub fn parsers() -> &'static [&'static dyn TranscriptParser] {
    &[
        &crate::claudecode_transcript::Parser as &'static dyn TranscriptParser,
        &crate::agent_codex::Parser as &'static dyn TranscriptParser,
        &crate::agent_cline::Parser as &'static dyn TranscriptParser,
        &crate::agent_copilot::Parser as &'static dyn TranscriptParser,
        &crate::agent_gemini::Parser as &'static dyn TranscriptParser,
        &crate::agent_qwen::Parser as &'static dyn TranscriptParser,
        &crate::agent_opencode::Parser as &'static dyn TranscriptParser,
        &crate::agent_hermes::Parser as &'static dyn TranscriptParser,
        &crate::agent_pi::Parser as &'static dyn TranscriptParser,
        &crate::agent_kilo::Parser as &'static dyn TranscriptParser,
        &crate::agent_grok::Parser as &'static dyn TranscriptParser,
        &crate::agent_openclaw::Parser as &'static dyn TranscriptParser,
        &crate::agent_cline::RooCodeParser as &'static dyn TranscriptParser,
        &crate::agent_cline::IbmBobParser as &'static dyn TranscriptParser,
        &crate::agent_sqlite_usage::GooseParser as &'static dyn TranscriptParser,
        &crate::agent_sqlite_usage::CrushParser as &'static dyn TranscriptParser,
        &crate::agent_sqlite_usage::ZcodeParser as &'static dyn TranscriptParser,
        &crate::agent_sqlite_usage::ZerostackParser as &'static dyn TranscriptParser,
        &crate::agent_sqlite_usage::ForgeParser as &'static dyn TranscriptParser,
        &crate::agent_antigravity::AntigravityParser as &'static dyn TranscriptParser,
        &crate::agent_jsonl_usage::MuxParser as &'static dyn TranscriptParser,
        &crate::agent_jsonl_usage::KimiParser as &'static dyn TranscriptParser,
        &crate::agent_jsonl_usage::KimiCodeParser as &'static dyn TranscriptParser,
        &crate::agent_jsonl_usage::LingtaiParser as &'static dyn TranscriptParser,
        &crate::agent_jsonl_usage::OpenDesignParser as &'static dyn TranscriptParser,
        &crate::agent_jsonl_usage::DroidParser as &'static dyn TranscriptParser,
        &crate::agent_jsonl_usage::CodewhaleParser as &'static dyn TranscriptParser,
        &crate::agent_jsonl_usage::MistralVibeParser as &'static dyn TranscriptParser,
    ]
}

/// `(agents, why_en, why_fr)` triples explaining agents whose token
/// usage is partial or unmeasurable. Both dashboards (web + terminal)
/// surface these verbatim so users read an agent's absence as "doesn't
/// log it" rather than "zero usage". Keep each note one short sentence
/// — the terminal renders them as single dim lines. Both languages
/// live here (not in the renderers) so the web payload can embed them
/// and switch client-side without regenerating the page.
pub fn coverage_notes() -> &'static [(&'static str, &'static str, &'static str)] {
    &[
        (
            "Copilot Chat",
            "input tokens are never written to disk, so input shows 0 — only output is counted",
            "les tokens d'entrée ne sont jamais écrits sur disque, l'entrée affiche donc 0 — seule la sortie est comptée",
        ),
        (
            "CodeWhale",
            "reports one aggregate token counter with no input/output split — the whole figure is counted as input",
            "ne publie qu'un compteur agrégé sans séparation entrée/sortie — le total entier est compté en entrée",
        ),
        (
            "Goose, Crush, Zerostack, Droid, Mistral Vibe",
            "totals are recorded per session, not per request — daily and per-model views are coarser than for other agents",
            "les totaux sont enregistrés par session, pas par requête — les vues par jour et par modèle sont moins fines que pour les autres agents",
        ),
        (
            "Kimi, Kimi Code, LingTai, Open Design, Mux",
            "logs record the session or run but not the project folder — grouped under that id instead of a path",
            "les journaux enregistrent la session ou le run mais pas le dossier projet — regroupés sous cet identifiant plutôt qu'un chemin",
        ),
        (
            "Cursor, Cursor Agent, Kiro, Warp",
            "write no token counts at all; tokenade reports nothing rather than estimating them from text length",
            "n'écrivent aucun compteur de tokens ; tokenade n'affiche rien plutôt que de les estimer d'après la longueur du texte",
        ),
        (
            "Gemini CLI, Qwen Code",
            "transcripts record the model but not the project folder — counted under Models, absent from per-project rows",
            "les transcripts enregistrent le modèle mais pas le dossier projet — comptés dans Modèles, absents des lignes par projet",
        ),
        (
            "Grok Build CLI",
            "records tool-output volume + tool/model breakdown but no per-call input/output token billing — counted under tool output, absent from the per-model panel",
            "enregistre le volume de sortie d'outils + le détail outils/modèle mais pas la facturation input/output par appel — compté dans la sortie d'outils, absent du panneau par modèle",
        ),
        (
            "Antigravity",
            "conversations record no project folder, and only the calls its generation metadata names can be attributed to a model — the rest count in the totals only",
            "les conversations n'enregistrent aucun dossier projet, et seuls les appels nommés par ses métadonnées de génération sont attribuables à un modèle — les autres ne comptent que dans les totaux",
        ),
        (
            "Cursor chat, Cody, Windsurf, Zed AI, Continue, Aider",
            "no readable local usage logs — token usage can't be measured",
            "aucun journal local exploitable — la consommation de tokens ne peut pas être mesurée",
        ),
    ]
}

/// Aggregate across every registered agent. Returns
/// `(agent, project_usage)` pairs; the same `cwd` can appear under
/// multiple agents when a user works on the same project from
/// different IDEs. The caller decides whether to render rows
/// separately or fold by cwd.
pub fn aggregate_all_agents(
    home_dir: &Path,
    since_iso: Option<&str>,
) -> Vec<(Agent, ProjectUsage)> {
    parsers()
        .iter()
        .flat_map(|p| {
            let agent = p.agent();
            p.aggregate_since(home_dir, since_iso)
                .into_iter()
                .map(move |u| (agent, u))
        })
        .collect()
}

/// Estimated output-lean SAVED per agent, EXCLUDING Claude Code (which
/// is credited live via its UserPromptSubmit hook — counting it here too would
/// double-count). For every other agent we sum the output tokens it produced
/// (`model_usage_by_day`, the one source every parser implements) and apply
/// the calibrated lean ratio (`coefficients::output_lean_ratio`). Agents with
/// no readable token logs sum to 0 and are dropped — we don't estimate what we
/// can't measure. Returns `(agent_label, saved_tokens)` for saved > 0.
pub fn estimated_output_lean_by_agent(home_dir: &Path) -> Vec<(&'static str, u64)> {
    let ratio = crate::coefficients::output_lean_ratio();
    let mut out = Vec::new();
    for p in parsers() {
        if p.agent() == Agent::ClaudeCode {
            continue;
        }
        let total_output: u64 = p
            .model_usage_by_day(home_dir, None)
            .values()
            .flat_map(|by_model| by_model.values())
            .map(|&(_, output, _)| output)
            .sum();
        let saved = (total_output as f64 * ratio).round() as u64;
        if saved > 0 {
            out.push((p.agent().label(), saved));
        }
    }
    out
}

/// Per-day, per-model usage merged across every registered agent.
/// Two agents reporting the same model string on the same day (e.g.
/// the same Anthropic model used from Claude Code and Cline) sum into
/// one cell — the panel is about model consumption, not agent
/// attribution.
pub fn model_usage_all_agents(home_dir: &Path, since_iso: Option<&str>) -> ModelUsageByDay {
    let mut out = ModelUsageByDay::default();
    for p in parsers() {
        for (day, models) in p.model_usage_by_day(home_dir, since_iso) {
            let day_entry = out.entry(day).or_default();
            for (model, (i, o, n)) in models {
                let e = day_entry.entry(model).or_insert((0, 0, 0));
                e.0 = e.0.saturating_add(i);
                e.1 = e.1.saturating_add(o);
                e.2 = e.2.saturating_add(n);
            }
        }
    }
    out
}

/// Fold the result of [`aggregate_all_agents`] into a single row per `cwd`
/// by summing every agent's contribution. Useful for dashboard "totals"
/// columns that want one row per project regardless of which IDEs were used.
///
/// Token fields, session counts, message counts, and tool breakdowns are all
/// summed across agents. `last_ts` keeps the maximum (most-recent) value.
pub fn merge_by_cwd(pairs: Vec<(Agent, ProjectUsage)>) -> Vec<ProjectUsage> {
    use std::collections::HashMap;
    let mut by_cwd: HashMap<String, ProjectUsage> = HashMap::new();
    for (_, u) in pairs {
        let e = by_cwd.entry(u.cwd.clone()).or_insert_with(|| ProjectUsage {
            cwd: u.cwd.clone(),
            ..Default::default()
        });
        e.sessions = e.sessions.saturating_add(u.sessions);
        e.assistant_messages = e.assistant_messages.saturating_add(u.assistant_messages);
        e.input_tokens = e.input_tokens.saturating_add(u.input_tokens);
        e.output_tokens = e.output_tokens.saturating_add(u.output_tokens);
        e.cache_creation_tokens = e
            .cache_creation_tokens
            .saturating_add(u.cache_creation_tokens);
        e.cache_read_tokens = e.cache_read_tokens.saturating_add(u.cache_read_tokens);
        e.tool_result_tokens = e.tool_result_tokens.saturating_add(u.tool_result_tokens);
        for (k, v) in u.tool_result_tokens_by_tool {
            let slot = e.tool_result_tokens_by_tool.entry(k).or_insert(0);
            *slot = slot.saturating_add(v);
        }
        if u.last_ts > e.last_ts {
            e.last_ts = u.last_ts;
        }
        e.tools += u.tools;
    }
    let mut out: Vec<ProjectUsage> = by_cwd.into_values().collect();
    out.sort_by(|a, b| b.last_ts.cmp(&a.last_ts));
    out
}

/// The key with the largest value, ties broken on the KEY.
///
/// `max_by_key` over a `HashMap` returns whichever maximum the iteration
/// happened to reach last, and that order changes between processes. Every
/// caller here is choosing the DOMINANT MODEL, which selects the price table:
/// two models level on volume — a user on two models, or a fresh install with
/// one session each — made the money figure differ run to run on identical
/// data. Same class as the ranking non-determinism fixed in 1.1.4, in the one
/// place the product cannot afford it.
///
/// One helper, one rule, three call sites: the tie cannot drift apart again.
pub fn dominant_key<K: Ord, V: Ord>(entries: impl Iterator<Item = (K, V)>) -> Option<K> {
    entries
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
        .map(|(k, _)| k)
}

#[cfg(test)]
mod tests {
    /// The money path cannot flip between runs. Two models level on volume is
    /// the ordinary case — a user on two models, a fresh install with one
    /// session each — and whichever wins selects the price table.
    #[test]
    fn a_tie_between_models_resolves_the_same_way_every_time() {
        use std::collections::HashMap;
        let mut first: Option<String> = None;
        for _ in 0..200 {
            let mut counts: HashMap<String, u64> = HashMap::new();
            counts.insert("claude-opus-5".into(), 7);
            counts.insert("claude-sonnet-5".into(), 7);
            counts.insert("claude-haiku-4-5".into(), 7);
            let got = super::dominant_key(counts.into_iter()).unwrap();
            match &first {
                None => first = Some(got),
                Some(f) => assert_eq!(f, &got, "the dominant model changed between runs"),
            }
        }
        // And a clear winner still wins, ties or not.
        let mut counts: HashMap<String, u64> = HashMap::new();
        counts.insert("claude-opus-5".into(), 1);
        counts.insert("claude-sonnet-5".into(), 9);
        assert_eq!(
            super::dominant_key(counts.into_iter()).as_deref(),
            Some("claude-sonnet-5")
        );
        assert_eq!(
            super::dominant_key(HashMap::<String, u64>::new().into_iter()),
            None
        );
    }

    use super::*;

    fn make_usage(
        cwd: &str,
        sessions: u32,
        input: u64,
        output: u64,
        last_ts: &str,
    ) -> ProjectUsage {
        ProjectUsage {
            cwd: cwd.to_string(),
            sessions,
            input_tokens: input,
            output_tokens: output,
            last_ts: last_ts.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn merge_by_cwd_sums_across_agents() {
        let pairs = vec![
            (
                Agent::ClaudeCode,
                make_usage("/proj/foo", 2, 1000, 200, "2026-06-07T10:00:00Z"),
            ),
            (
                Agent::Codex,
                make_usage("/proj/foo", 1, 500, 100, "2026-06-07T11:00:00Z"),
            ),
            (
                Agent::Cline,
                make_usage("/proj/bar", 3, 300, 50, "2026-06-06T09:00:00Z"),
            ),
        ];
        let mut merged = merge_by_cwd(pairs);
        // sorted by last_ts descending → foo (11:00) before bar (09:00)
        assert_eq!(merged.len(), 2);
        merged.sort_by(|a, b| a.cwd.cmp(&b.cwd)); // deterministic for assertions
        let bar = merged.iter().find(|p| p.cwd == "/proj/bar").unwrap();
        let foo = merged.iter().find(|p| p.cwd == "/proj/foo").unwrap();
        assert_eq!(foo.sessions, 3, "sessions summed across agents");
        assert_eq!(foo.input_tokens, 1500, "input_tokens summed");
        assert_eq!(foo.output_tokens, 300, "output_tokens summed");
        assert_eq!(foo.last_ts, "2026-06-07T11:00:00Z", "last_ts keeps max");
        assert_eq!(bar.sessions, 3);
        assert_eq!(bar.input_tokens, 300);
    }

    #[test]
    fn merge_by_cwd_empty_input_returns_empty() {
        assert!(merge_by_cwd(vec![]).is_empty());
    }

    #[test]
    fn merge_by_cwd_sums_tool_result_tokens_by_tool() {
        let mut u1 = make_usage("/proj/foo", 1, 100, 10, "2026-06-07T10:00:00Z");
        u1.tool_result_tokens_by_tool.insert("Read".into(), 40);
        u1.tool_result_tokens_by_tool.insert("Bash".into(), 20);

        let mut u2 = make_usage("/proj/foo", 1, 200, 20, "2026-06-07T11:00:00Z");
        u2.tool_result_tokens_by_tool.insert("Read".into(), 60);
        u2.tool_result_tokens_by_tool.insert("WebFetch".into(), 30);

        let merged = merge_by_cwd(vec![(Agent::ClaudeCode, u1), (Agent::Codex, u2)]);
        assert_eq!(merged.len(), 1);
        let row = &merged[0];
        assert_eq!(
            row.tool_result_tokens_by_tool
                .get("Read")
                .copied()
                .unwrap_or(0),
            100,
            "Read tokens summed across agents"
        );
        assert_eq!(
            row.tool_result_tokens_by_tool
                .get("Bash")
                .copied()
                .unwrap_or(0),
            20,
            "Bash only in agent 1 — must survive merge"
        );
        assert_eq!(
            row.tool_result_tokens_by_tool
                .get("WebFetch")
                .copied()
                .unwrap_or(0),
            30,
            "WebFetch only in agent 2 — must survive merge"
        );
    }

    #[test]
    fn agent_labels_are_distinct() {
        let labels = [
            Agent::ClaudeCode.label(),
            Agent::Codex.label(),
            Agent::Cline.label(),
            Agent::Copilot.label(),
        ];
        let set: std::collections::HashSet<_> = labels.iter().collect();
        assert_eq!(set.len(), labels.len(), "labels must be unique");
    }

    #[test]
    fn parsers_registry_non_empty() {
        // Smoke test: the dispatch table contains every variant of
        // `Agent`. If someone adds a new agent without registering it
        // here, this should fail.
        let registered: std::collections::HashSet<_> =
            parsers().iter().map(|p| p.agent()).collect();
        assert!(registered.contains(&Agent::ClaudeCode));
        assert!(registered.contains(&Agent::Codex));
        assert!(registered.contains(&Agent::Cline));
        assert!(registered.contains(&Agent::Copilot));
    }
}
