//! **Which agents are there, and which file each one reads.**
//!
//! Detection starts from what EXISTS on the machine: offering rules to an
//! absent agent creates a file nobody will read, in a repo somebody else
//! will read.

use super::*;

/// One FILE to scaffold, and every rule-file name that reaches it.
pub(super) struct RuleTarget {
    /// The resolved path — a real file inside the repository, never a link.
    pub(super) path: PathBuf,
    /// The names that led here, for the report.
    pub(super) names: Vec<PathBuf>,
    /// Whether the block must carry the subagent note. True unless the only
    /// name reaching this file is `CLAUDE.md` (Claude propagates the rules to
    /// its subagents itself, so its own file omits the note).
    pub(super) subagent_note: bool,
}

impl RuleTarget {
    pub(super) fn label(&self, root: &Path) -> String {
        let mut names: Vec<String> = self
            .names
            .iter()
            .map(|n| n.strip_prefix(root).unwrap_or(n).display().to_string())
            .collect();
        names.sort();
        names.join(" = ")
    }
}

/// Collapse the rule-file names onto the files they actually name.
///
/// A user who keeps `CLAUDE.md` and `AGENTS.md` in sync with a symlink has ONE
/// file under two names, and this pass used to walk the names: it read the same
/// inode twice, computed two DIFFERENT blocks for it (`CLAUDE.md` omits the
/// subagent note), and wrote whichever came last. The repo-symlink guard then
/// refused the link half — correctly, that guard exists so a cloned repo cannot
/// point `CLAUDE.md` at a file on your machine — and the refusal was counted as
/// a write failure, which left `.tokenade/scaffold.flag` unstamped and re-ran
/// the entire pass on every single session.
///
/// So: resolve each name. A link that stays INSIDE the repository is the user's
/// own arrangement and resolves to a file we were going to write anyway, so it
/// is followed and written ONCE, under whichever variant every name reaching it
/// agrees on. A link that ESCAPES the repository is returned separately and
/// never written — the security property the guard was added for is unchanged,
/// because it is about where the bytes land, not about which name was typed.
pub(super) fn resolve_rule_targets(
    root: &Path,
    candidates: &[PathBuf],
) -> (Vec<RuleTarget>, Vec<PathBuf>) {
    let root_real = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut out: Vec<RuleTarget> = Vec::new();
    let mut escaped: Vec<PathBuf> = Vec::new();
    for name in candidates {
        // A file that does not exist yet is its own resolution: there is no
        // link to follow, and the write will create it where it was named.
        let resolved = match std::fs::canonicalize(name) {
            Ok(p) => p,
            Err(_) => name.clone(),
        };
        if resolved != *name && !resolved.starts_with(&root_real) {
            if !escaped.contains(name) {
                escaped.push(name.clone());
            }
            continue;
        }
        let note = name.file_name().and_then(|n| n.to_str()) != Some("CLAUDE.md");
        match out.iter_mut().find(|t| t.path == resolved) {
            Some(t) => {
                t.names.push(name.clone());
                // Any non-Claude name reaching this file means the note has to
                // be in it: those agents have no other channel to their
                // subagents, and the note is harmless to Claude.
                t.subagent_note |= note;
            }
            None => out.push(RuleTarget {
                path: resolved,
                names: vec![name.clone()],
                subagent_note: note,
            }),
        }
    }
    (out, escaped)
}

/// Return absolute paths to agent-specific rule files we should
/// scaffold based on what's installed / present on this machine.
///
/// Detection signals (cheap fs probes — no network, no shelling):
///   - **Cursor**: `~/.cursor/` OR `<root>/.cursor/` present
///   - **Cline**: VS Code extension's globalStorage dir present
///   - **GitHub Copilot**: `~/.config/github-copilot/` OR
///     `<root>/.github/copilot-instructions.md` already
///
/// `CLAUDE.md` and `AGENTS.md` are written unconditionally (see
/// [`PRIMARY_AGENT_FILES`]) since they're the cross-agent standards.
pub fn detect_installed_agents(root: &Path) -> Vec<PathBuf> {
    let home = std::env::var("HOME").map(PathBuf::from).ok();
    let mut targets = Vec::new();

    let exists_in_home =
        |sub: &str| -> bool { home.as_ref().is_some_and(|h| h.join(sub).exists()) };

    // **Cursor does NOT read `.cursor/rules/*.md`.** Its glob, read in its own
    // package (`cursor-agent` 2026.08.11, `index.js`), is
    // `.cursor/rules/**/*.mdc` — the `.mdc` extension, and only that. The file
    // we had always written there was never read by anyone: one more file in
    // the user's repo, for nothing. The same package declares the list it
    // really reads: `.cursorrules`, `AGENTS.md`, `CLAUDE.md`, `CLAUDE.local.md`
    // — and `AGENTS.md` is written unconditionally (`PRIMARY_AGENT_FILES`), so
    // Cursor is already served.
    // `.cursorrules` stays: it is the historical address, and a Cursor older
    // than `AGENTS.md` support only has that one.
    if exists_in_home(".cursor") || root.join(".cursor").exists() {
        targets.push(root.join(".cursorrules"));
    }
    if exists_in_home(".config/Code/User/globalStorage/saoudrizwan.claude-dev")
        || exists_in_home(".cline")
        || root.join(".clinerules").exists()
        || root.join(".cline").exists()
    {
        targets.push(root.join(".clinerules"));
    }
    // Roo Code (VSCode ext rooveterinaryinc.roo-cline): no hook system, so
    // the rules scaffold is the strongest available integration — same
    // model-cooperative steering rtk uses for its prompt-level agents.
    // Modern Roo reads `.roo/rules/*.md`; `.roorules` is the legacy single
    // file still honored. Loudest token-complaint cohort after Codex in the
    // 2026-07 GitHub issues mining (roo#11459 begs for a CLI proxy).
    if exists_in_home(".config/Code/User/globalStorage/rooveterinaryinc.roo-cline")
        || root.join(".roo").exists()
    {
        targets.push(root.join(".roo").join("rules").join("tokenade.md"));
    }
    // Kilo Code — Roo fork, same rules-dir convention under `.kilocode/`.
    if exists_in_home(".config/Code/User/globalStorage/kilocode.kilo-code")
        || root.join(".kilocode").exists()
    {
        targets.push(root.join(".kilocode").join("rules").join("tokenade.md"));
    }
    if exists_in_home(".config/github-copilot")
        || root
            .join(".github")
            .join("copilot-instructions.md")
            .exists()
    {
        targets.push(root.join(".github").join("copilot-instructions.md"));
    }
    // Qwen Code reads a PROJECT context file — `QWEN.md`, walked up from the
    // working directory (`~/.qwen/QWEN.md` + `<dir>/QWEN.md`, the cascade
    // `claude_md_audit::rule_files_for` models). Same conditional shape as
    // the branches above — only when the agent's home dir proves it is
    // installed. (`GEMINI.md`, `REASONIX.md`: retired agents, which the
    // housekeeping cleans up — `leftover_project_files`. `.windsurfrules` is
    // not a target: Devin Desktop, which reads it, has a global file — the
    // lesson lives there, and `AGENTS.md`, which it also reads, is written in
    // every case.)
    if exists_in_home(".qwen") || root.join(".qwen").exists() {
        targets.push(root.join("QWEN.md"));
    }
    // Same read/write asymmetry as the Gemini family, found by comparing this list against every
    // cascade `claude_md_audit::rule_files_for` models. Grok is a rank-1 HOOK
    // agent and Hermes a plugin agent: both got the compaction channel and no
    // guidance, because the file each one reads was audited and never
    // written.
    if exists_in_home(".grok") || root.join(".grok").exists() {
        targets.push(root.join(".grokrules"));
    }
    // Hermes: first match wins. `.hermes.md` is the file it really loads
    // (parents up to the git root). `.hermes_rules.md` is in no list of its
    // shipped skill.
    if exists_in_home(".hermes") || root.join(".hermes").exists() {
        targets.push(root.join(".hermes.md"));
    }
    targets
}
