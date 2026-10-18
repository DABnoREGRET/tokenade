//! **Who is calling us, right now?**
//!
//! Three paths, from most to least reliable: a variable the host sets, the
//! parent process name, presence on disk. The ORDER is the subject —
//! the last one guesses, and letting it guess what could have been read attributes
//! the usage to the wrong agent.

use super::*;

/// Detect the currently-running agent from environment variables.
pub fn from_env() -> Agent {
    let has = |k: &str| std::env::var(k).is_ok();
    // Explicit override — highest precedence. A wrapper that KNOWS which agent
    // it serves (e.g. the Hermes plugin, which runs in Hermes' own process but
    // may inherit a parent agent's env vars) sets `TOKENADE_AGENT=<slug>` so
    // savings attribute correctly regardless of ambient env. Unknown/empty
    // values fall through to sniffing.
    if let Ok(slug) = std::env::var("TOKENADE_AGENT") {
        if let Some(a) = Agent::from_slug(slug.trim()) {
            return a;
        }
    }
    if has("CLAUDECODE") || has("CLAUDE_CODE_ENTRYPOINT") || has("ANTHROPIC_CLAUDE_CODE") {
        Agent::ClaudeCode
    } else if has("CURSOR_TRACE_ID")
        || has("CURSOR_SESSION_ID")
        || std::env::var("TERM_PROGRAM").as_deref() == Ok("cursor")
    {
        Agent::Cursor
    // `CODEX_SANDBOX`/`OPENAI_CODEX` only appear inside Codex's sandbox; a
    // run with `--dangerously-bypass-approvals-and-sandbox` (and, as of Codex
    // 0.144, ordinary runs too) exposes `CODEX_CI` / `CODEX_THREAD_ID` instead.
    // Without these, Codex-driven compaction lands as `agent=unknown` in the
    // gain ledger (verified 2026-07: a live `codex exec` shell env carried
    // CODEX_CI + CODEX_THREAD_ID but neither of the old markers). Match any.
    } else if has("CODEX_SANDBOX")
        || has("OPENAI_CODEX")
        || has("CODEX_CI")
        || has("CODEX_THREAD_ID")
    {
        Agent::Codex
    } else if has("FACTORY_PROJECT_DIR") || has("FACTORY_SESSION_ID") || has("DROID_SESSION") {
        // Factory Droid CLI — FACTORY_PROJECT_DIR is documented (hooks
        // guide) as set for hook/tool subprocesses. Without this arm every
        // Droid session logged agent:"unknown" (wrong caps + attribution).
        Agent::Droid
    } else if has("COPILOT_AGENT") || has("GITHUB_COPILOT_TOKEN") {
        Agent::Copilot
    } else if has("KILO_CODE") || has("KILOCODE_SESSION") {
        Agent::KiloCode
    // Devin Desktop (a Windsurf fork) sets `WINDSURF_CASCADE_TERMINAL_ID`
    // in the env of the terminals its agent opens (`shellLaunchConfig.env`,
    // read in the installed bundle); `WINDSURF_SESSION` was Windsurf's
    // signal, kept for the builds that still set it.
    } else if has("WINDSURF_CASCADE_TERMINAL_ID") || has("WINDSURF_SESSION") {
        Agent::DevinDesktop
    } else if has("OPENCODE_SESSION") || has("OPENCODE_HOME") {
        Agent::OpenCode
    } else if has("HERMES_SESSION") {
        Agent::Hermes
    } else if has("CLINE_SESSION") || has("CLAUDE_DEV_SESSION") {
        Agent::Cline
    } else if has("AIDER_SESSION") || has("AIDER_MODEL") || has("AIDER_API_KEY") {
        Agent::Aider
    } else if has("OPENCLAW_SESSION") || has("OPENCLAW_HOME") {
        Agent::OpenClaw
    // Qwen Code (gemini-cli fork) rebrands the identification var to
    // QWEN_CODE.
    } else if has("QWEN_CODE") || has("QWEN_CODE_SESSION") {
        Agent::Qwen
    // Grok CLI injects GROK_SESSION_ID and GROK_WORKSPACE_ROOT into hook
    // environments; GROK_AGENT is set on the Grok process itself.
    } else if has("GROK_SESSION_ID") || has("GROK_WORKSPACE_ROOT") || has("GROK_AGENT") {
        Agent::Grok
    // Odysseus AI injects ODYSSEUS_DATA_DIR into every subprocess it spawns.
    // ODYSSEUS_ADMIN_PASSWORD is present only when the user is running the
    // Odysseus server process itself (not a client session), so it is a
    // secondary signal used only as a fallback.
    } else if has("ODYSSEUS_DATA_DIR") || has("ODYSSEUS_SESSION_ID") {
        Agent::Odysseus
    } else {
        // Last resort: walk the PARENT PROCESS chain. 44% of prod events
        // arrived agent="unknown" — mostly the `wrap` channel, whose child
        // processes don't always inherit the agent's marker env vars
        // (opencode/pi plugins, ssh-spawned shells). The process tree still
        // knows who we are.
        from_parent_processes()
    }
}

/// Map a process name (comm / argv[0] basename) to an agent.
///
/// Unix-only: its single caller walks `/proc`. Cross-checking for Windows is
/// what surfaced it as dead there (D5).
#[cfg(unix)]
pub(super) fn agent_from_process_name(name: &str) -> Option<Agent> {
    let n = name.to_ascii_lowercase();
    let n = n.trim_end_matches(".exe");
    // Order matters: more specific names first ("cursor-server" before
    // generic "node"-hosted names, which we deliberately do NOT map).
    if n == "claude" || n.starts_with("claude-") {
        Some(Agent::ClaudeCode)
    } else if n.contains("cursor") {
        Some(Agent::Cursor)
    } else if n == "codex" {
        Some(Agent::Codex)
    } else if n == "copilot" {
        Some(Agent::Copilot)
    } else if n == "opencode" {
        Some(Agent::OpenCode)
    } else if n == "droid" {
        Some(Agent::Droid)
    } else if n == "qwen" {
        Some(Agent::Qwen)
    } else if n == "grok" {
        Some(Agent::Grok)
    } else if n == "openclaw" {
        Some(Agent::OpenClaw)
    } else if n == "hermes-agent" || n == "hermes" {
        Some(Agent::Hermes)
    } else if n == "aider" {
        Some(Agent::Aider)
    } else {
        None
    }
}

/// Linux: walk /proc ppid chain (≤10 hops, ~µs — no subprocess). macOS:
/// `ps -o ppid=,comm=` per hop (a few ms, but this path only runs when
/// every env signal already failed). Windows / failure: Unknown.
pub(super) fn from_parent_processes() -> Agent {
    #[cfg(target_os = "linux")]
    {
        let mut pid = std::process::id();
        for _ in 0..10 {
            let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
                Ok(s) => s,
                Err(_) => break,
            };
            // stat: `pid (comm) state ppid …` — comm may contain spaces.
            let (Some(open), Some(close)) = (stat.find('('), stat.rfind(')')) else {
                break;
            };
            let comm = &stat[open + 1..close];
            if let Some(a) = agent_from_process_name(comm) {
                return a;
            }
            let ppid: u32 = match stat[close + 1..]
                .split_whitespace()
                .nth(1)
                .and_then(|x| x.parse().ok())
            {
                Some(p) if p > 1 => p,
                _ => break,
            };
            pid = ppid;
        }
    }
    #[cfg(target_os = "macos")]
    {
        let mut pid = std::process::id();
        for _ in 0..10 {
            let out = crate::process_util::internal_command("ps")
                .args(["-o", "ppid=,comm=", "-p", &pid.to_string()])
                .output();
            let Ok(out) = out else { break };
            let line = String::from_utf8_lossy(&out.stdout);
            let mut it = line.trim().split_whitespace();
            let Some(ppid) = it.next().and_then(|x| x.parse::<u32>().ok()) else {
                break;
            };
            let comm = it.next_back().unwrap_or("");
            let base = comm.rsplit('/').next().unwrap_or(comm);
            if let Some(a) = agent_from_process_name(base) {
                return a;
            }
            if ppid <= 1 {
                break;
            }
            pid = ppid;
        }
    }
    Agent::Unknown
}

/// Detect from marker files/dirs under `root`. Project-local fallback for
/// when env is ambiguous.
pub fn from_filesystem(root: &Path) -> Agent {
    let exists = |p: &str| root.join(p).exists();
    if exists(".claude") || exists("CLAUDE.md") || exists(".mcp.json") {
        Agent::ClaudeCode
    } else if exists(".cursor") {
        Agent::Cursor
    } else if exists(".codex") {
        Agent::Codex
    } else if exists(".pi") {
        Agent::Pi
    } else if exists("opencode.json") || exists(".opencode") {
        Agent::OpenCode
    } else {
        Agent::Unknown
    }
}

/// Best-effort detection of the agent invoking us right now.
pub fn detect(root: &Path) -> Agent {
    match from_env() {
        Agent::Unknown => from_filesystem(root),
        a => a,
    }
}

/// Scan the user's home directory for every agent that's INSTALLED on
/// this machine, regardless of whether it's the one currently invoking
/// us. The one-command installer uses this to register STS with all of
/// them in a single pass.
pub fn detect_all_present(home: Option<&Path>) -> Vec<Agent> {
    let mut found = Vec::new();
    for a in Agent::all() {
        if a.presence_markers(home).iter().any(|p| p.exists()) {
            found.push(*a);
        }
    }
    found
}

// ---- helpers --------------------------------------------------------------

pub(super) fn home_dir() -> Option<PathBuf> {
    // Delegates to the cross-platform helper so we only fix HOME/
    // USERPROFILE resolution in one place.
    crate::platform::home_dir()
}

pub(super) fn resolve(p: &str, root: &Path, home: Option<&Path>) -> PathBuf {
    if let Some(stripped) = p.strip_prefix("~/") {
        match home {
            Some(h) => h.join(stripped),
            None => home_dir()
                .map(|h| h.join(stripped))
                .unwrap_or_else(|| PathBuf::from(p)),
        }
    } else if root.as_os_str().is_empty() {
        PathBuf::from(p)
    } else {
        root.join(p)
    }
}
