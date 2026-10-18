//! **What a PROJECT consumed**, across all transcripts.
//!
//! Grouping is done on the `cwd` field every line carries, not
//! on the file name: one project can have a hundred sessions, and it is the
//! sentence "Claude consumed N tokens in project X, of which tokenade
//! intercepted none" that justifies this module.
//!
//! `ToolBreakdown` adds up (`AddAssign`) rather than being recomputed:
//! two sessions of the same project sum without re-reading anything.

use super::*;

/// Per-project token usage extracted from Claude Code session
/// transcripts.
#[derive(Debug, Clone, Default)]
pub struct ProjectUsage {
    /// Absolute path the session was opened in — e.g.
    /// `/home/me/projects/foo`. Taken from each
    /// record's `cwd` field, not decoded from the directory name.
    pub cwd: String,
    /// Number of distinct sessions for this project. Counting convention
    /// varies by agent — each parser documents its own rule:
    ///   - **Claude Code**: incremented once per `SessionStart` event in a
    ///     JSONL transcript file.
    ///   - **Codex**: one per JSONL session file that contains usage events.
    ///   - **Cline**: one per task directory under `tasks/<task-id>/`.
    ///   - **Copilot**: one per JSON session file that contributed at least
    ///     one request to this cwd (a single file can span multiple projects).
    pub sessions: u32,
    pub assistant_messages: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    /// Sum of tokenizer-estimated tokens across every
    /// `tool_result` block this session emitted (Bash/Read/Edit/MCP/
    /// etc., bucketed by cwd). This is the apples-to-apples
    /// denominator for "what fraction of NEW tool output did tokenade
    /// touch?" — comparable to gain.jsonl's `saved` aggregate. The
    /// `total_tokens()` figure (input + output + cache_creation)
    /// covers ALL fresh content added across the session —
    /// `cache_creation` accumulates as new tool outputs land,
    /// `output` grows with every Claude response, `input` carries
    /// fresh user messages — and is much larger than per-call
    /// savings even though re-sent history (cache_read) is
    /// explicitly excluded. A later fix corrected the earlier framing that
    /// blamed re-sent history for the gap.
    pub tool_result_tokens: u64,
    /// Per-tool breakdown of `tool_result_tokens`. Keys are
    /// tool names (`Bash`, `Read`, `Edit`, `TaskCreate`,
    /// `mcp__tokenade__exec_compact`, …). Built by tracking the
    /// `tool_use_id ↔ tool_use.name` mapping within each session
    /// file: when a `tool_result` block is seen, its
    /// `tool_use_id` is looked up to find the originating tool's
    /// name, and the content tokens accumulate to that bucket.
    /// Aggregated per-cwd across all session files.
    /// Powers the "AGENT TOKENS (outside of tokenade) - Functions"
    /// dashboard section, which shows where tool-result tokens
    /// are flowing per tool — the breakdown of the per-project
    /// bypass number above.
    pub tool_result_tokens_by_tool: std::collections::HashMap<String, u64>,
    /// ISO-8601 timestamp of the most recent assistant message seen
    /// across all sessions for this `cwd`. Empty when no message had
    /// one. Sortable lexicographically.
    pub last_ts: String,
    /// Tool-call breakdown — counts the `tool_use` blocks emitted by
    /// the model. Used by the dashboard to flag "Claude reached for
    /// Read 80× here but only called tokenade 2× — should be using
    /// skeleton / symbol_find instead".
    pub tools: ToolBreakdown,
}

/// Bucketed counts of `tool_use` blocks for one project. Buckets are
/// hand-picked around the question "did the model bypass tokenade
/// when it could've avoided it?" — so we lump every MCP tokenade
/// variant into one counter, group file-read tools together, etc.
#[derive(Debug, Clone, Default)]
pub struct ToolBreakdown {
    /// Calls to any `mcp__tokenade__*` (or namespaced
    /// `mcp__plugin_tokenade_tokenade__*`) tool — the model IS using
    /// us. Higher is better.
    pub tokenade: u64,
    /// `Read`. Often substitutable by `skeleton` / `symbol_find`
    /// for large files.
    pub read: u64,
    /// `Grep`. Often substitutable by `semantic_search` / `symbol_find`.
    pub grep: u64,
    /// `Glob`. Often substitutable by `structure_map`.
    pub glob: u64,
    /// `Bash`. Should route through `exec_compact` (especially git /
    /// cargo / kubectl / terraform) to compress noisy output.
    pub bash: u64,
    /// `WebFetch` + `WebSearch`. Substitutable by `web_html_to_markdown`
    /// / `web_snapshot_compact` for HTML pages.
    pub web: u64,
    /// `Edit` + `Write` + `MultiEdit` + `NotebookEdit`. No tokenade
    /// alternative — these are mutations, not reductions. Tracked
    /// only so the user sees the total tool-call volume.
    pub edit: u64,
    /// Everything else — non-tokenade MCP tools, Agent calls, etc.
    pub other: u64,
}

impl std::ops::AddAssign for ToolBreakdown {
    fn add_assign(&mut self, rhs: Self) {
        self.tokenade = self.tokenade.saturating_add(rhs.tokenade);
        self.read = self.read.saturating_add(rhs.read);
        self.grep = self.grep.saturating_add(rhs.grep);
        self.glob = self.glob.saturating_add(rhs.glob);
        self.bash = self.bash.saturating_add(rhs.bash);
        self.web = self.web.saturating_add(rhs.web);
        self.edit = self.edit.saturating_add(rhs.edit);
        self.other = self.other.saturating_add(rhs.other);
    }
}

impl ToolBreakdown {
    pub fn total(&self) -> u64 {
        self.tokenade
            + self.read
            + self.grep
            + self.glob
            + self.bash
            + self.web
            + self.edit
            + self.other
    }

    /// Sum of buckets where tokenade has a known alternative.
    /// Higher = bigger missed-opportunity surface.
    pub fn bypass_eligible(&self) -> u64 {
        self.read + self.grep + self.glob + self.bash + self.web
    }

    /// Classify a `tool_use.name` into one of our buckets.
    pub fn bump(&mut self, name: &str) {
        // Tokenade MCP namespace covers both `mcp__tokenade__*` (direct
        // install) and `mcp__plugin_tokenade_tokenade__*` (Claude Code
        // plugin layer). Match on these two explicit prefixes only —
        // `name.contains("tokenade__")` would falsely classify a
        // third-party server like `mcp__rogue__tokenade__shim` as ours.
        //
        // Also count `mcp__supertokensaver__*` and `mcp__sts__*`
        // as tokenade calls. These are legacy install names from the
        // pre-rename project ("super-token-saver" → "tokenade") that
        // many users still have wired into ~/.codex/~/.copilot configs
        // via `tokenade install`'s wrap-mcps step. Calls to a wrapped
        // MCP DO route through tokenade (the wrapper proxies to the
        // original binary), so the dashboard was understating the
        // user's actual tokenade footprint by treating these as
        // `other`. Without this, a user on a long-lived install would
        // see "60M tok consumed but barely any tokenade calls" — yet
        // every single one of those calls did go through the wrapper.
        if name.starts_with("mcp__tokenade__")
            || name.starts_with("mcp__plugin_tokenade_tokenade__")
            || name.starts_with("mcp__supertokensaver__")
            || name.starts_with("mcp__sts__")
        {
            self.tokenade += 1;
            return;
        }
        match name {
            "Read" => self.read += 1,
            "Grep" => self.grep += 1,
            "Glob" => self.glob += 1,
            "Bash" => self.bash += 1,
            "WebFetch" | "WebSearch" => self.web += 1,
            "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => self.edit += 1,
            _ => self.other += 1,
        }
    }
}

impl ProjectUsage {
    /// Tokens that consumed Anthropic prompt-space or output budget on
    /// THIS project: `input + output + cache_creation`. `cache_read` is
    /// excluded because it's a free reuse of an already-paid prompt —
    /// counting it would double-count the prompt that created it.
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_creation_tokens)
    }
}

/// Walk `<home_dir>/.claude/projects/*/*.jsonl` and aggregate token
/// usage per cwd. Returns rows sorted most-recently-active first so
/// the dashboard can `take(N)` for the top of the list.
///
/// `home_dir` is parameterised so callers (and tests) can point at a
/// fixture tree instead of the real `$HOME`.
/// Every `*.jsonl` transcript under `projects_root`, one project directory
/// deep — the layout Claude Code writes.
///
/// Five readers walked this by hand, identically, which is five places to
/// teach when the layout changes. An unreadable root yields nothing instead of
/// returning early: every caller's tail already produces the same empty result
/// from an empty walk.
pub(crate) fn jsonl_transcripts(
    projects_root: &std::path::Path,
) -> impl Iterator<Item = std::path::PathBuf> {
    fs::read_dir(projects_root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .flat_map(|d| fs::read_dir(d).into_iter().flatten().flatten())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("jsonl"))
}

pub fn aggregate(home_dir: &Path) -> Vec<ProjectUsage> {
    aggregate_since(home_dir, None)
}

/// Same as [`aggregate`] but only counts assistant messages whose
/// `timestamp` (ISO-8601, lexicographically sortable) is >= `since_iso`.
/// Pass `None` to count everything. Records before the cutoff are
/// silently skipped — they don't contribute to totals OR to `last_ts`.
///
/// Use [`install_timestamp_iso`] to derive a cutoff matching when
/// Tokenade itself was first installed — i.e. "how much usage have I
/// pushed past Tokenade since I started using it?"
pub fn aggregate_since(home_dir: &Path, since_iso: Option<&str>) -> Vec<ProjectUsage> {
    let projects_root = crate::platform::claude_config_dir_for(home_dir).join("projects");

    let mut by_cwd: HashMap<String, ProjectUsage> = HashMap::new();

    crate::transcript_cache::with_shared(home_dir, |cache| {
        for path in jsonl_transcripts(&projects_root) {
            let rows = cache.rows_for(&path);
            scan_session_rows(rows, &mut by_cwd, since_iso);
        }
    });

    let mut out: Vec<ProjectUsage> = by_cwd.into_values().collect();
    // Most-recent activity first — what the user wants at the top of
    // the dashboard. Lexicographic on ISO-8601 = chronological.
    out.sort_by(|a, b| b.last_ts.cmp(&a.last_ts));
    out
}
