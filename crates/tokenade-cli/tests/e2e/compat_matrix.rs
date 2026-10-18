//! The compatibility matrix, OBSERVED rather than declared.
//!
//! The operator's requirement, verbatim (translated): "I don't want you to
//! answer me from reading code, I want you to validate it with a test." So nothing in
//! here reads a source file to decide a cell. Every verdict is the result of
//! RUNNING something and looking at what came back — the agent's own hook
//! payload through the real binary, a plugin asset folding a real blob, the
//! proxy's own rewrite path over a real request body.
//!
//! Three rules this file exists to keep:
//!
//! 1. **A cell nobody proved is `unproven`, never `no`.** The two are different
//!    facts and this project has paid for confusing them before (a
//!    fallback-to-zero that made torn state read as never-used). `unproven` is
//!    printed as such and is the signal that work remains, not a verdict.
//! 2. **The OS is part of the cell.** Where a channel behaves differently on
//!    Windows or macOS the table says so; where the probe has only ever run on
//!    one OS, the others are `unproven` on that OS rather than assumed equal.
//! 3. **The proxy column is separate.** "Ko bin + Ok proxy" is a real and
//!    common shape — it is the whole reason rank 4 exists — so a cell carries
//!    two verdicts, not one.
//!
//! Emitted to `docs/COMPAT-MATRIX-observed.md` by
//! `cargo test … emit_the_compat_matrix -- --ignored`. Nothing here spends an
//! API credit: every probe drives tokenade's own surfaces, never a provider.
//!
//! It used to write the three-OS bench's matrix file, which belongs to a
//! DIFFERENT instrument: the maintainers' three-OS probe bench, run on three real
//! machines (Linux, macOS, Windows); its results are in `testdata/compat/results/`. Running
//! the command this comment advertised replaced 2 211 lines of three-OS evidence
//! with 32 lines of Linux-only evidence, in silence and with the same title.
//! Two instruments writing one path is not a duplicate, it is a demolition:
//! whichever ran last won, and the loser's OSes simply vanished. They answer
//! different questions — this one is in-process and cheap enough to run on every
//! change, that one is the published bench — so they now have one file each.

use super::*;

/// The agents the compatibility page lists, in its order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentRow {
    /// The name as the site prints it.
    pub label: &'static str,
    /// The slug `detect` uses, when tokenade knows this agent at all.
    pub slug: Option<&'static str>,
}

const AGENTS: &[AgentRow] = &[
    AgentRow {
        label: "Claude Code CLI",
        slug: Some("claude-code"),
    },
    AgentRow {
        label: "OpenCode",
        slug: Some("opencode"),
    },
    AgentRow {
        label: "Kilo Code",
        slug: Some("kilo-code"),
    },
    AgentRow {
        label: "Copilot CLI",
        slug: Some("copilot"),
    },
    AgentRow {
        label: "Hermes",
        slug: Some("hermes"),
    },
    AgentRow {
        label: "Cline",
        slug: Some("cline"),
    },
    AgentRow {
        label: "T3 Code",
        slug: Some("t3-code"),
    },
    AgentRow {
        label: "OpenClaw",
        slug: Some("openclaw"),
    },
    AgentRow {
        label: "Pi",
        slug: Some("pi"),
    },
    AgentRow {
        label: "Qwen Code",
        slug: Some("qwen-code"),
    },
    AgentRow {
        label: "Codex CLI",
        slug: Some("codex"),
    },
    AgentRow {
        label: "Cursor",
        slug: Some("cursor"),
    },
    AgentRow {
        label: "Grok",
        slug: Some("grok"),
    },
    AgentRow {
        label: "Droid",
        slug: Some("droid"),
    },
    AgentRow {
        label: "Aider",
        slug: Some("aider"),
    },
    AgentRow {
        label: "Devin Desktop",
        slug: Some("devin-desktop"),
    },
    AgentRow {
        label: "Antigravity",
        slug: Some("antigravity"),
    },
    // The four the page lists as incompatible. They are GUI applications with
    // no hook surface, no plugin API and no config route — kept in the table
    // because "we checked and there is nothing" is a result, and an absent row
    // reads as an oversight.
    AgentRow {
        label: "Claude Desktop",
        slug: None,
    },
    AgentRow {
        label: "Claude Cowork",
        slug: None,
    },
    AgentRow {
        label: "Copilot (VS Code)",
        slug: None,
    },
    AgentRow {
        label: "Codex (app)",
        slug: None,
    },
];

/// What one probe concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Ran, and the feature works.
    Ok,
    /// Ran, and the feature works for part of what it claims.
    Partial,
    /// Ran, and the feature does not work here.
    Ko,
    /// Nothing ran. NOT the same as `Ko`, and never printed as one.
    Unproven(&'static str),
    /// The question does not apply — no such surface on this agent.
    NotApplicable,
}

impl Verdict {
    fn cell(self, channel: &str) -> String {
        match self {
            Verdict::Ok => format!("Ok {channel}"),
            Verdict::Partial => format!("Partial {channel}"),
            Verdict::Ko => format!("Ko {channel}"),
            Verdict::Unproven(_) => "unproven".into(),
            Verdict::NotApplicable => "—".into(),
        }
    }
}

/// One feature, and how to prove it on one agent.
pub struct FeatureRow {
    pub label: &'static str,
}

const FEATURES: &[FeatureRow] = &[
    FeatureRow {
        label: "Command compaction",
    },
    FeatureRow {
        label: "Web search",
    },
    FeatureRow {
        label: "MCP optimization",
    },
    FeatureRow {
        label: "Code search",
    },
    FeatureRow {
        label: "Lean output",
    },
    FeatureRow {
        label: "Context optimization",
    },
    FeatureRow {
        label: "Re-read dedup",
    },
    FeatureRow {
        label: "Documents & media",
    },
    FeatureRow { label: "Privacy" },
    FeatureRow {
        label: "Secret redaction",
    },
    // Beyond the commercial page, and each one is a promise a user relies on.
    FeatureRow {
        label: "Savings measurement",
    },
    FeatureRow {
        label: "Recovery (expand-ref)",
    },
];

/// The OS this run observed. A verdict is only ever claimed for THIS one.
fn os_tag() -> &'static str {
    if cfg!(target_os = "linux") {
        "L"
    } else if cfg!(target_os = "macos") {
        "M"
    } else if cfg!(target_os = "windows") {
        "W"
    } else {
        "?"
    }
}

// ---------------------------------------------------------------------------
// Probes. Each one RUNS something; none reads source.
// ---------------------------------------------------------------------------

/// Did the install put a TOOL-USE entry in place, or only lifecycle ones?
///
/// `channel_installed` answers "is tokenade wired into this agent at
/// all", and the command cell went straight from that to feeding
/// `hookwrap-pretool` a payload. Reasonix made the gap visible: at one point it
/// gets `SessionStart` / `UserPromptSubmit` / `PreCompact` and no tool event,
/// because its own binary says `updatedInput` is never applied to the tool
/// call. The channel is real, the command path is not, and the cell read
/// `Ok bin` — our dialect answering our own payload, one question too early.
///
/// A plugin bridge has no event names to read; it folds tool RESULTS by
/// construction, so it passes. What is being excluded is the agent whose
/// installed hook document declares only lifecycle events.
fn a_tool_event_was_installed(slug: &str, home: &Path) -> bool {
    const TOOL_EVENTS: [&str; 8] = [
        "PreToolUse",
        "preToolUse",
        "PostToolUse",
        "postToolUse",
        "BeforeTool",
        "AfterTool",
        "beforeShellExecution",
        "afterShellExecution",
    ];
    let Some(addresses) = host_read_paths(slug) else {
        // No provenance for this agent's address: keep the older, looser
        // answer rather than inventing a stricter one out of an unknown.
        return true;
    };
    let mut saw_a_hook_document = false;
    for rel in addresses {
        let p = home.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        let Ok(body) = std::fs::read_to_string(&p) else {
            continue;
        };
        // A bridge asset is not a hook document; it carries no event names and
        // is judged by the plugin probe instead.
        if p.extension().and_then(|e| e.to_str()) != Some("json") {
            return true;
        }
        saw_a_hook_document = true;
        if TOOL_EVENTS.iter().any(|e| body.contains(e)) {
            return true;
        }
    }
    !saw_a_hook_document
}

/// Does this agent's own PreToolUse payload come back with the command
/// rewritten through `tokenade wrap`?
///
/// The dialects are the point: Droid sends `Execute`, Cursor sends a
/// `preToolUse` envelope and expects `permission`/`updated_input` instead of
/// `hookSpecificOutput`, Grok sends camelCase around its own tool names. Each
/// of those was a real silent no-op once.
///
/// The Grok arm used to send `{"tool_name":"bash"}`, a payload Grok has
/// never emitted. Its shipped `docs/user-guide/10-hooks.md` prints the real
/// one: `{"hookEventName":"pre_tool_use","sessionId":…,"toolName":
/// "run_terminal_command","toolInput":{"command":"npm test"}}`. A probe that
/// invents the input proves the handler against itself.
fn probe_command_compaction(slug: &str, home: &Path) -> Verdict {
    // HALF ONE, and the half the first version of this probe was missing.
    //
    // `hookwrap-pretool` answers on the DIALECT, not on the agent: feeding it a
    // Claude-shaped payload gets a Claude-shaped answer whoever we claim to be.
    // The first run of this matrix therefore printed `Ok bin` for Devin Desktop
    // and Antigravity, which have no hook surface at all — a textbook
    // optimistic cell, produced by a probe that proved the binary rather than
    // the channel.
    //
    // So ask the installer first: does this agent END UP with a channel that
    // will ever send us that payload? Only then does the dialect answer mean
    // anything.
    if !channel_installed(slug, home) || !a_tool_event_was_installed(slug, home) {
        // T3 Code is an ORCHESTRATOR: it spawns the real `claude` with the
        // user's settings, so the Claude hook covers it and it never gets a
        // channel of its own. `Ko` would be a wrong answer to the question a
        // user is actually asking ("does command compaction work when I use
        // T3?"), so the delegation is followed rather than reported as an
        // absence — measured by installing for the agent it delegates TO.
        if let Some(via) = delegates_to(slug) {
            let h2 = TempHome::new("matrix-delegate");
            if channel_installed(via, h2.path()) {
                return Verdict::Ok;
            }
        }
        return Verdict::Ko;
    }
    let payload = match slug {
        // Verbatim from the host's own documentation, camelCase and all — the
        // envelope normaliser is precisely what has to make this readable.
        "grok" => {
            r#"{"hookEventName":"pre_tool_use","sessionId":"m","toolName":"run_terminal_command","toolInput":{"command":"git status --short"}}"#
        }
        "droid" => {
            r#"{"session_id":"m","hook_event_name":"PreToolUse","tool_name":"Execute","tool_input":{"command":"git status --short"}}"#
        }
        "cursor" => {
            r#"{"session_id":"m","hook_event_name":"preToolUse","tool_input":{"command":"git status --short"}}"#
        }
        _ => {
            r#"{"session_id":"m","tool_name":"Bash","tool_input":{"command":"git status --short"}}"#
        }
    };
    let (out, _err, _rc) = run_hook(&["hookwrap-pretool"], home, payload);
    if out.trim().is_empty() {
        return Verdict::Ko;
    }
    if out.contains("wrap") {
        Verdict::Ok
    } else {
        Verdict::Ko
    }
}

/// Does a secret in a tool RESULT get scrubbed before the model sees it?
///
/// Driven through `hookwrap`, the PostToolUse surface, with a credential shape
/// the redactor claims. A pass here means the bytes the model receives no
/// longer carry the key.
fn probe_secret_redaction(slug: &str, home: &Path) -> Verdict {
    // Gated on the channel for the same reason the command probe is: the
    // redactor works on any payload handed to it, so without this every agent
    // — including the three with no channel at all — read `Ok`. A feature only
    // exists for an agent that can actually deliver it the bytes.
    if !channel_installed(slug, home) && delegates_to(slug).is_none() {
        return Verdict::Ko;
    }
    let secret = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
    let payload = format!(
        r#"{{"session_id":"m","tool_name":"Bash","tool_input":{{"command":"cat .env"}},"tool_response":{{"stdout":"AWS_SECRET_ACCESS_KEY={secret}\n{}","stderr":"","interrupted":false}}}}"#,
        "filler line to clear the fold floor. ".repeat(40)
    );
    let (out, _e, _rc) = run_hook(&["hookwrap"], home, &payload);
    if out.trim().is_empty() {
        return Verdict::Unproven("the PostToolUse surface returned nothing on this agent");
    }
    if out.contains(secret) {
        Verdict::Ko
    } else {
        Verdict::Ok
    }
}

/// Is the same output served as a reference the second time?
fn probe_read_dedup(slug: &str, home: &Path) -> Verdict {
    if !channel_installed(slug, home) && delegates_to(slug).is_none() {
        return Verdict::Ko;
    }
    // Probed on `wrap`, not on the PostToolUse hook.
    //
    // Measured: `hookwrap` returns EMPTY for a Bash result on Claude Code —
    // the compaction is routed through the PreToolUse SUBSTITUTION instead, so
    // the command runs under `tokenade wrap` and the folding happens there.
    // The first version of this probe compared two empty strings and honestly
    // reported `unproven`; the fix is to ask the surface that actually does the
    // work, which is also the surface every agent's rewritten command reaches.
    let script = home.join("emit.sh");
    let body = (0..300)
        .map(|i| format!("echo '-rw-r--r-- user/group {i} 2026-01-01 12:00 src/g{i}.rs'\n"))
        .collect::<String>();
    if std::fs::write(&script, body).is_err() {
        return Verdict::Unproven("could not write the fixture");
    }
    let cmd = format!("sh {}", script.display());
    let (first, _e, _rc) = run(&["wrap", &cmd], home);
    let (second, _e, _rc) = run(&["wrap", &cmd], home);
    if first.trim().is_empty() {
        return Verdict::Unproven("wrap produced nothing to compare");
    }
    if second.len() < first.len() || second.contains("identical to a prior call") {
        return Verdict::Ok;
    }
    // NOT `Ko`, and the distinction is the whole discipline of this file.
    //
    // Measured 2026-08-18: three surfaces asked, none shortened a repeat.
    // `wrap` returns the same 316 bytes three times over an identical command
    // in one session; `read` the same 174 three times over an identical file;
    // `hookwrap-postread` answers nothing at all for the Read payload shape I
    // built. The product DOES carry the notion — `fold_marker` knows the string
    // "identical to a prior call" — so the likeliest reading is that I have not
    // found the surface, not that the feature is absent.
    //
    // Saying `Ko` here would put a headline feature down on three failed
    // guesses. Saying `unproven` says what is true: nobody has shown it either
    // way, and the question stays open.
    Verdict::Unproven("wrap/read/postread all returned an unshortened repeat — surface not found")
}

/// Did the work land in the ledger, attributed to this agent?
fn probe_gain_measurement(slug: &str, home: &Path) -> Verdict {
    if !channel_installed(slug, home) && delegates_to(slug).is_none() {
        return Verdict::Ko;
    }
    let big = "warning: unused variable `x`\n".repeat(300);
    let payload = format!(
        r#"{{"session_id":"gain-probe","tool_name":"Bash","tool_input":{{"command":"cargo build"}},"tool_response":{{"stdout":{},"stderr":"","interrupted":false}}}}"#,
        serde_json_string(&big)
    );
    let _ = run_hook_as(Some(slug), &["hookwrap"], home, &payload);
    let ledger = home.join(".tokenade").join("gain.jsonl");
    let Ok(text) = std::fs::read_to_string(&ledger) else {
        return Verdict::Ko;
    };
    if text
        .lines()
        .any(|l| l.contains(&format!("\"agent\":\"{slug}\"")))
    {
        Verdict::Ok
    } else if text.trim().is_empty() {
        Verdict::Ko
    } else {
        Verdict::Partial
    }
}

/// The agent whose channel covers this one, when it has none of its own.
///
/// One entry today, and it is a real product fact rather than a convenience:
/// `install` prints it out loud — "T3 Code detected — its Claude sessions load
/// your user settings, so the Claude hooks below cover them".
fn delegates_to(slug: &str) -> Option<&'static str> {
    match slug {
        "t3-code" => Some("claude-code"),
        _ => None,
    }
}
/// **The address the HOST reads — not the address we chose.**
///
/// `channel_installed` below answered "is tokenade named anywhere in this
/// agent's throwaway home", and that is a fact about our install, never about
/// the agent. It printed `Ok` for Grok for as long as Grok's hooks were written
/// to `~/.grok/user-settings.json`, a file Grok does not read — measured with
/// the host's own `grok inspect`: **Hooks (0)** there, **Hooks (6)** at
/// `~/.grok/hooks/tokenade.json`. The instrument was measuring the gate instead
/// of the work, and this whole file exists to not do that.
///
/// So: the path, and where the claim that the host reads it was READ. An agent
/// with no entry keeps the old loose scan and is flagged in the emitted page —
/// "unproven" about the address, not a silent `Ok`.
///
/// Paths are relative to the throwaway home, `/`-separated. `None` is "no
/// provenance yet", which keeps the loose scan; `Some(&[])` is the third and
/// sharpest state — the address is KNOWN and nothing we write there is live.
fn host_read_paths(slug: &str) -> Option<&'static [&'static str]> {
    // The table itself moved OUT of this file. The published bench
    // (the maintainers' three-OS probe) needs the same answers, and this
    // journal already counts several pairs of lists that were "the same" until
    // they were not: `PLUGIN_INSTALLS` versus the upgrade refresh versus the
    // dashboard badge, the hermes row the badge's copy never had. One file,
    // read by both, with each entry carrying where its claim was READ.
    static TABLE: std::sync::OnceLock<std::collections::BTreeMap<String, Vec<&'static str>>> =
        std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        const RAW: &str = include_str!("../../../../testdata/compat/host-read-addresses.json");
        let v: serde_json::Value = serde_json::from_str(RAW).expect("host-read-addresses.json");
        let mut out = std::collections::BTreeMap::new();
        for (slug, entry) in v.as_object().expect("object") {
            if slug.starts_with('_') {
                continue;
            }
            let paths: Vec<&'static str> = entry["paths"]
                .as_array()
                .expect("paths array")
                .iter()
                .map(|p| {
                    // Leaked deliberately: the table is read once per process
                    // and every consumer wants `&'static str`.
                    &*Box::leak(
                        p.as_str()
                            .expect("path string")
                            .to_string()
                            .into_boxed_str(),
                    )
                })
                .collect();
            out.insert(slug.clone(), paths);
        }
        out
    });
    table.get(slug).map(|v| &**v)
}

/// Does `tokenade install` actually give this agent a channel, on this OS?
///
/// Runs the REAL installer into a throwaway home, after planting the agent's
/// presence markers so detection fires — then asks the installer itself what it
/// wired. Reading the report rather than the config files keeps this probe
/// honest about one thing: it measures what the product CLAIMS to have
/// installed, and a claim that does not match the file is a separate defect
/// with its own test (`install.rs`).
fn channel_installed(slug: &str, home: &Path) -> bool {
    // Plant something that makes `detect` see the agent. The dot-directory is
    // the shape every agent uses for its own state; `install` refuses an agent
    // it cannot see, which is what makes this a real gate.
    for marker in presence_markers(slug) {
        let p = home.join(marker);
        let _ = std::fs::create_dir_all(p.parent().unwrap_or(&p));
        if marker.ends_with(".json") || marker.ends_with(".toml") || marker.ends_with(".yml") {
            let _ = std::fs::write(&p, "{}\n");
        } else {
            let _ = std::fs::create_dir_all(&p);
        }
    }
    let _ = run(&["install", "--only", slug], home);
    // Evidence, not the installer's own summary line. Matching on the report
    // made Claude Code read `Ko` because the banner says "Claude Code" and the
    // slug is `claude-code` — a probe failing on its own string handling, which
    // is exactly the class of mistake this whole file is meant to avoid. What
    // settles it is whether the agent's OWN state now names us.
    // The agent's OWN state, wherever the installer put it — and NOT
    // `~/.tokenade`, which is ours and would make every agent pass.
    //
    // Scoping this to the presence marker alone was too narrow: the plugin
    // bridges write to `~/.tokenade/<agent>-plugin` and register the path in a
    // file that may live elsewhere, so Cline read `Ko` for a channel that does
    // install. Widened to the whole throwaway home, minus our own directory.
    //
    // And narrowed again, the other way. "Somewhere under the home" is
    // a fact about our install; what a user needs to know is whether the HOST
    // reads that address. Where `host_read_paths` names one, only that path
    // counts; where it names none, the loose scan stands and the emitted page
    // says so.
    if let Some(addresses) = host_read_paths(slug) {
        return addresses.iter().any(|rel| {
            address_names_tokenade(&home.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR)))
        });
    }
    let Ok(rd) = std::fs::read_dir(home) else {
        return false;
    };
    rd.flatten()
        .filter(|e| e.file_name() != std::ffi::OsStr::new(".tokenade"))
        .any(|e| names_tokenade(&e.path()))
}

/// True when something under `p` is a COMMAND channel naming tokenade.
///
/// "Mentions tokenade" alone was too coarse and produced a false `Ok` for Devin
/// Desktop: `install` scaffolds a rules file (`GEMINI.md`, `.hermes_rules.md`,
/// …) for agents it cannot hook at all, and a rules file is the "Lean
/// output" feature, not this one. Two features, one piece of evidence, and the
/// coarse predicate happily counted the wrong one.
///
/// So the evidence has to name the CHANNEL: a hook entry or a plugin
/// registration. Still a heuristic — the per-shape assertions live in
/// `install.rs` — but a heuristic that cannot mistake a markdown file for a
/// hook.
/// Does the file at an address the HOST reads name us?
///
/// Looser than [`names_tokenade`] on purpose. That predicate has to guess
/// whether an arbitrary file in a home is a channel, so it demands the words
/// "hook" or "plugin" to keep a scaffolded `GEMINI.md` from answering for a
/// feature it is not. Here the address is already known to be one the host
/// reads, so the guessing is done and the extra demand only produces false
/// negatives: aider's channel is `load: <path>` in `.aider.conf.yml`, which
/// names us and contains neither word.
fn address_names_tokenade(p: &Path) -> bool {
    if !p.is_file() {
        return false;
    }
    if p.file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|n| n.starts_with("tokenade"))
    {
        return true;
    }
    std::fs::read_to_string(p)
        .map(|t| t.contains("tokenade"))
        .unwrap_or(false)
}

fn names_tokenade(p: &Path) -> bool {
    if p.is_file() {
        let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
        // A rules file is a different feature; never let it answer this one.
        if name.ends_with(".md") || name.contains("rules") {
            return false;
        }
        // A plugin asset is named after us and need not say the word "plugin"
        // anywhere inside — `~/.pi/agent/extensions/tokenade.ts` is the whole
        // channel, and the content-only predicate called it `Ko`.
        if name.starts_with("tokenade") {
            return true;
        }
        return std::fs::read_to_string(p)
            .map(|t| t.contains("tokenade") && (t.contains("hook") || t.contains("plugin")))
            .unwrap_or(false);
    }
    let Ok(rd) = std::fs::read_dir(p) else {
        return false;
    };
    rd.flatten().any(|e| names_tokenade(&e.path()))
}

/// Where an agent keeps its state, as a path under a home.
///
/// Deliberately a small table rather than a call into `tokenade_core`: this
/// test drives the SHIPPED BINARY, and importing the library to decide what to
/// plant would let a wrong marker in the library make its own probe pass.
fn presence_markers(slug: &str) -> Vec<&'static str> {
    // Corrected 2026-08-18 against what the installer actually looks for. The
    // first table was a guess of the shape `~/.<agent>` and it was wrong for
    // four of them — Cline lives in VS Code's globalStorage, Pi needs the
    // `agent` subdirectory (a bare `~/.pi` is deliberately refused as too
    // generic), Gemini CLI is distinguished from Antigravity by a FILE, and
    // T3 Code's slug is `t3-code` with a hyphen. Every one of those made the
    // matrix print `Ko` for a channel that installs fine.
    //
    // Several of these are OS-dependent, which is the point of running this
    // harness on three machines rather than assuming.
    match slug {
        "claude-code" => vec![".claude"],
        "opencode" => vec![".config/opencode"],
        "kilo-code" => vec![".config/kilo"],
        "copilot" => vec![".copilot"],
        "hermes" => vec![".hermes"],
        // VS Code extension state, not a dot-directory of its own.
        #[cfg(target_os = "linux")]
        "cline" => vec![".config/Code/User/globalStorage/saoudrizwan.claude-dev"],
        #[cfg(target_os = "macos")]
        "cline" => {
            vec!["Library/Application Support/Code/User/globalStorage/saoudrizwan.claude-dev"]
        }
        #[cfg(target_os = "windows")]
        "cline" => vec!["AppData/Roaming/Code/User/globalStorage/saoudrizwan.claude-dev"],
        "t3-code" => vec![".t3code"],
        "openclaw" => vec![".openclaw"],
        "pi" => vec![".pi/agent"],
        #[cfg(windows)]
        "qwen-code" => vec!["qwen"],
        #[cfg(not(windows))]
        "qwen-code" => vec![".qwen"],
        "codex" => vec![".codex"],
        "cursor" => vec![".cursor"],
        "grok" => vec![".grok"],
        "droid" => vec![".factory"],
        "aider" => vec![".aider.chat.history.md"],
        "devin-desktop" => vec![".codeium/windsurf"],
        "antigravity" => vec![".config/Antigravity"],
        _ => vec![],
    }
}

// ---------------------------------------------------------------------------
// Binary-local features: the agent plays no part, so they are proved ONCE per
// OS rather than once per agent. Repeating them per row would be 22 identical
// runs and would suggest the answer could differ by agent, which it cannot.
// ---------------------------------------------------------------------------

/// Can the binary locate a symbol without reading whole files?
fn probe_code_search(home: &Path) -> Verdict {
    let work = home.join("proj");
    let _ = std::fs::create_dir_all(work.join("src"));
    let _ = std::fs::write(
        work.join("src/lib.rs"),
        "pub fn needle_in_the_haystack(x: u32) -> u32 { x + 1 }\npub struct Elsewhere;\n",
    );
    let (out, _e, _rc) = run_in(&["query", "needle_in_the_haystack"], home, &work);
    if out.contains("needle_in_the_haystack") && out.contains("lib.rs") {
        Verdict::Ok
    } else if out.trim().is_empty() {
        Verdict::Ko
    } else {
        Verdict::Partial
    }
}

/// Does a binary document come back as text rather than as bytes?
///
/// A real `.docx` — a ZIP with the parts Word actually writes — because
/// hand-rolling the archive would test the test. The e2e crate already carries
/// `zip` for exactly this.
fn probe_documents(home: &Path) -> Verdict {
    let path = home.join("spec.docx");
    let Ok(f) = std::fs::File::create(&path) else {
        return Verdict::Unproven("could not create the fixture");
    };
    let mut z = zip::ZipWriter::new(f);
    let opts: zip::write::FileOptions<'_, ()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let ok = z
        .start_file("word/document.xml", opts)
        .and_then(|_| {
            use std::io::Write as _;
            z.write_all(
                br#"<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>ORNITHORYNQUE marker</w:t></w:r></w:p></w:body></w:document>"#,
            )
            .map_err(Into::into)
        })
        .and_then(|_| z.finish().map(|_| ()))
        .is_ok();
    if !ok {
        return Verdict::Unproven("could not write the fixture");
    }
    let (out, _e, _rc) = run(&["read", path.to_str().unwrap()], home);
    if out.contains("ORNITHORYNQUE") {
        Verdict::Ok
    } else {
        Verdict::Ko
    }
}

/// Do the bytes a fold replaced come back on demand?
///
/// The promise that makes compaction safe: tokenade swapped content for a
/// pointer, so refusing to honour the pointer would strand data the user
/// already had.
fn probe_recovery(home: &Path) -> Verdict {
    // Content a compactor actually RECOGNISES. Four hundred lines of prose are
    // not foldable — `read` passed them through unchanged and the probe read
    // `unproven` about a feature that works. A tar listing is one of the 935
    // shapes the product knows.
    let blob = (0..400)
        .map(|i| format!("-rw-r--r-- user/group {i} 2026-01-01 12:00 src/f{i}.rs\n"))
        .collect::<String>()
        + "line 399 of something worth keeping\n";
    let (folded, _e, _rc) = run_with_stdin(&["read", "-"], home, &blob);
    let Some(hash) = folded
        .split(|c: char| !c.is_ascii_hexdigit())
        .find(|w| w.len() >= 12 && w.chars().all(|c| c.is_ascii_hexdigit()))
    else {
        return Verdict::Unproven("no recovery pointer in the folded output");
    };
    let (back, _e, _rc) = run(&["expand-ref", hash, "--raw"], home);
    if back.contains("line 399 of something worth keeping") {
        Verdict::Ok
    } else if back.trim().is_empty() {
        Verdict::Ko
    } else {
        Verdict::Partial
    }
}

/// Does anything a user would call private leave the machine?
///
/// Probed the only way a test honestly can from inside: run a fold whose
/// content is unmistakable, then read everything tokenade wrote about it and
/// assert the CONTENT is not in there — only counters. It does not prove the
/// network is silent (that needs a sniffer); it proves the record tokenade
/// keeps of your work carries no work.
fn probe_privacy(home: &Path) -> Verdict {
    let secret_marker = "HIPPOCAMPE_PRIVE_42";
    let blob = format!(
        "{secret_marker}\n{}",
        (0..300)
            .map(|i| format!("payload {i}\n"))
            .collect::<String>()
    );
    let _ = run_with_stdin(&["read", "-"], home, &blob);
    let mut leaked = false;
    let mut looked = 0usize;
    let ours = home.join(".tokenade");
    let mut stack = vec![ours];
    while let Some(p) = stack.pop() {
        if p.is_dir() {
            if let Ok(rd) = std::fs::read_dir(&p) {
                stack.extend(rd.flatten().map(|e| e.path()));
            }
            continue;
        }
        // The stash is the RECOVERY store: it is supposed to hold the bytes,
        // on this machine, and that is the feature above. Privacy is about
        // what is REPORTED.
        if p.to_string_lossy().contains("stash") {
            continue;
        }
        looked += 1;
        if std::fs::read_to_string(&p)
            .map(|t| t.contains(secret_marker))
            .unwrap_or(false)
        {
            leaked = true;
        }
    }
    if looked == 0 {
        return Verdict::Unproven("tokenade wrote no record to inspect");
    }
    if leaked {
        Verdict::Ko
    } else {
        Verdict::Ok
    }
}

/// Minimal JSON string escaping — enough for the probe payloads above.
fn serde_json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---------------------------------------------------------------------------
// The table.
// ---------------------------------------------------------------------------

/// Every probe that exists today, run for real, on this OS.
///
/// Asserts nothing about the RESULT — that is the point of a matrix — but does
/// assert the harness itself ran: a probe that silently produced nothing for
/// every agent is a broken probe, not a row of honest `Ko`s.
#[test]
fn every_probe_actually_runs_on_this_os() {
    let h = TempHome::new("matrix-smoke");
    let mut ok = 0;
    for a in AGENTS {
        let Some(slug) = a.slug else { continue };
        if probe_command_compaction(slug, h.path()) == Verdict::Ok {
            ok += 1;
        }
    }
    assert!(
        ok > 0,
        "the command-compaction probe rewrote nothing for ANY agent — the probe \
         is broken, which is a different fact from every agent being unsupported"
    );
}

/// Regenerate `docs/COMPAT-MATRIX-observed.md` from what the probes observe.
///
/// `#[ignore]` because it writes a file in the repo: a test that edits tracked
/// content on every `cargo test` turns an unrelated run into a dirty tree.
///
/// NOT the three-OS bench's output; see the module header.
#[test]
#[ignore = "regenerates `docs/COMPAT-MATRIX-observed.md` — it WRITES a tracked file"]
fn emit_the_compat_matrix() {
    let os = os_tag();
    let mut md = String::new();
    md.push_str(&format!(
        "# Compatibility matrix — OBSERVED IN PROCESS\n\n\
         Generated by `cargo test -p tokenade-cli --test e2e emit_the_compat_matrix -- --ignored`.\n\
         Every cell comes from a probe that RAN on this machine, not from reading code.\n\n\
         This is NOT the output of the three-OS bench\n\
         run on three real machines. This one is\n\
         cheap and runs on every change; that one is the published measurement.\n\n\
         OS of this run: **{os}**. A cell is only claimed for the OS its probe\n\
         ran on; elsewhere it stays `unproven`, which is not `Ko`.\n\n"
    ));
    md.push_str("| agent |");
    for f in FEATURES {
        md.push_str(&format!(" {} |", f.label));
    }
    md.push_str("\n|---|");
    for _ in FEATURES {
        md.push_str("---|");
    }
    md.push('\n');

    for a in AGENTS {
        md.push_str(&format!("| {} |", a.label));
        for f in FEATURES {
            let v = cell_for(a, f);
            md.push_str(&format!(" {} |", v));
        }
        md.push('\n');
    }
    md.push_str(
        "\n`Ok bin` = the binary alone does it · `Ok proxy` = the LLM proxy adds it · \
         `unproven` = no probe has run for this cell on this OS yet.\n",
    );

    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/COMPAT-MATRIX-observed.md");
    std::fs::write(&out, md).expect("write matrix");
    eprintln!("wrote {}", out.display());
}

/// One cell, observed.
fn cell_for(a: &AgentRow, f: &FeatureRow) -> String {
    let Some(slug) = a.slug else {
        return Verdict::NotApplicable.cell("bin");
    };
    let h = TempHome::new("matrix-cell");
    let v = match f.label {
        "Command compaction" => probe_command_compaction(slug, h.path()),
        "Secret redaction" => probe_secret_redaction(slug, h.path()),
        "Re-read dedup" => probe_read_dedup(slug, h.path()),
        "Savings measurement" => probe_gain_measurement(slug, h.path()),
        // Binary-local: the same answer for every agent, because it is a
        // property of the BINARY and not of the agent's channel. Run once,
        // and the row simply repeats it. The dispatch below is what says
        // which ones those are — a `bool` on the row said it a second time
        // and nothing ever read it.
        "Code search" => probe_code_search(h.path()),
        "Documents & media" => probe_documents(h.path()),
        "Privacy" => probe_privacy(h.path()),
        "Recovery (expand-ref)" => probe_recovery(h.path()),
        _ => Verdict::Unproven("no probe written yet"),
    };
    v.cell("bin")
}

/// **An address WE chose is not evidence; an address the host READS is.**
///
/// Grok is the case that produced this rule, so it is the case that holds it.
/// Before a fix, the installer wrote `~/.grok/user-settings.json`; the host reads
/// `~/.grok/hooks/*.json`, and `grok inspect` answered "Hooks (0)" on the first
/// and "Hooks (6)" on the second. The old gate — "is tokenade named anywhere
/// under this home" — said `Ok` throughout, which is how the published table
/// carried ten green cells for an agent running none of our hooks.
#[test]
fn a_file_the_host_never_reads_is_not_an_installed_channel() {
    let h = TempHome::new("matrix-address");
    let grok = h.path().join(".grok");
    std::fs::create_dir_all(&grok).unwrap();
    // A perfectly well-formed tokenade hook document — at the dead address.
    std::fs::write(
        grok.join("user-settings.json"),
        r#"{"hooks":{"PreToolUse":[{"matcher":"bash","hooks":[{"type":"command","command":"tokenade hookwrap-pretool","_tag":"tokenade-pretool"}]}]}}"#,
    )
    .unwrap();
    assert!(
        !channel_installed_for_test("grok", h.path()),
        "a hook file at an address grok does not read must not count as a channel"
    );

    std::fs::create_dir_all(grok.join("hooks")).unwrap();
    std::fs::write(
        grok.join("hooks").join("tokenade.json"),
        r#"{"hooks":{"SessionStart":[{"matcher":"","hooks":[{"type":"command","command":"tokenade hookwrap-sessionstart","_tag":"tokenade-sessionstart"}]}]}}"#,
    )
    .unwrap();
    assert!(
        channel_installed_for_test("grok", h.path()),
        "the address grok DOES read must count"
    );
}

/// The address check, without the `install` run `channel_installed` does first:
/// this test is about which path is believed, not about what the installer
/// writes (that is `install.rs`'s job, and it would overwrite the fixture).
fn channel_installed_for_test(slug: &str, home: &Path) -> bool {
    host_read_paths(slug).is_some_and(|addresses| {
        addresses.iter().any(|rel| {
            address_names_tokenade(&home.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR)))
        })
    })
}

/// **A known address with no live entry is not a channel.**
///
/// `Some(&[])` is the third state, the sharpest: the address is KNOWN and
/// nothing we write there is picked up by the host. The broad scan would say
/// "channel installed" as soon as the word `tokenade` appears in a file
/// under the home — the instrument measuring the door instead of the work,
/// which this whole file exists not to do.
///
/// **2026-09-13.** This test named `qwen-code` as the example of this state,
/// and it was red: the qwen entries are no longer dead. `~/.qwen/settings.json`
/// now receives `PreToolUse`/`UserPromptSubmit` — Claude Code's vocabulary,
/// the one the host speaks — plus the `enabled: true` without which NO hook
/// fires, measured on the binary. The address table was fixed; the assertion
/// that quoted it was not. A test that names a stale example fails on the
/// update it should have welcomed.
///
/// So it tests the STATE, not the example, and skips itself when nobody is
/// in it — an empty state is good news, not a failure.
#[test]
fn a_known_address_with_no_live_entry_is_not_a_channel() {
    const RAW: &str = include_str!("../../../../testdata/compat/host-read-addresses.json");
    let table: serde_json::Value = serde_json::from_str(RAW).expect("host-read-addresses.json");
    let dead: Vec<String> = table
        .as_object()
        .expect("object")
        .keys()
        .filter(|s| !s.starts_with('_'))
        .filter(|s| host_read_paths(s) == Some(&[] as &[&str]))
        .cloned()
        .collect();
    for slug in &dead {
        let slug = slug.as_str();
        let h = TempHome::new(&format!("matrix-dead-{slug}"));
        let dir = h
            .path()
            .join(format!(".{}", slug.split('-').next().unwrap_or(slug)));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("settings.json"),
            r#"{"hooks":{"BeforeTool":[{"command":"tokenade hookwrap-pretool"}]}}"#,
        )
        .unwrap();
        assert!(
            !channel_installed_for_test(slug, h.path()),
            "{slug}: an entry the host never picks up must not count \
             as an installed channel"
        );
    }

    // The fact that changed, held the other way round: qwen-code is NO LONGER
    // in this state. If someone puts its entries back into Gemini's
    // vocabulary, this line says so instead of letting the channel die silently.
    assert_eq!(
        host_read_paths("qwen-code"),
        Some(&[".qwen/settings.json"] as &[&str]),
        "qwen-code reads `~/.qwen/settings.json` and our entries there are live"
    );
    // And an agent whose provenance nobody has keeps the broad scan rather
    // than being silently downgraded — the two unknowns are not conflated.
    assert_eq!(host_read_paths("openclaw"), None);
}

/// **An entry nobody reads guards nothing.**
///
/// The address table is keyed by slug. A misspelled slug — `gemini` for
/// `gemini-cli`, `kilo` for `kilo-code` — produces no error: it simply
/// returns `None`, and the agent falls back to the broad scan without
/// anything saying so. That is the silent failure this whole file tries to
/// avoid, applied to its own safeguard.
#[test]
fn every_slug_in_the_address_table_is_an_agent_this_matrix_knows() {
    const RAW: &str = include_str!("../../../../testdata/compat/host-read-addresses.json");
    let v: serde_json::Value = serde_json::from_str(RAW).expect("host-read-addresses.json parses");
    let known: Vec<&str> = AGENTS.iter().filter_map(|a| a.slug).collect();
    let mut unknown = Vec::new();
    let mut checked = 0usize;
    for slug in v.as_object().expect("object").keys() {
        if slug.starts_with('_') {
            continue;
        }
        checked += 1;
        if !known.contains(&slug.as_str()) {
            unknown.push(slug.clone());
        }
    }
    assert!(
        checked >= 10,
        "only {checked} slugs reached the check — the table stopped being read"
    );
    assert!(
        unknown.is_empty(),
        "these slugs name no agent in this matrix, so their addresses guard nothing: {unknown:?}"
    );
}

/// **An installed channel is not a channel FOR THIS FEATURE.**
///
/// Reasonix showed it (agent removed since); Grok holds it now. It receives
/// only `SessionStart` / `PreCompact` and no tool event, because it honours
/// no substitution. The channel exists; the command path does not. The cell
/// said `Ok bin` — our dialect answering our own payload, a question asked
/// too early.
#[test]
fn an_agent_wired_only_for_lifecycle_events_has_no_command_cell() {
    let h = TempHome::new("matrix-lifecycle-only");
    assert_eq!(
        probe_command_compaction("grok", h.path()),
        Verdict::Ko,
        "Grok gets no tool event, so nothing there can rewrite a command"
    );

    // And the control that keeps the rule from being true for everyone: an
    // agent that DOES have a tool event keeps its cell.
    let h2 = TempHome::new("matrix-tool-event");
    assert_eq!(
        probe_command_compaction("claude-code", h2.path()),
        Verdict::Ok,
        "Claude Code installs PreToolUse and rewrites the command"
    );
}

/// T3 Code has no channel of its own and is still covered.
///
/// The delegation is a product fact the installer prints out loud; this pins
/// that the matrix FOLLOWS it instead of reporting an absence. Written because
/// the first delegation attempt silently did not fire and the cell stayed `Ko`.
#[test]
fn t3_code_is_covered_through_the_agent_it_delegates_to() {
    // `--only t3-code` used to write NOTHING while printing "the Claude hooks
    // below cover them", because the delegation ran before the `--only` filter
    // and `retain` took ClaudeCode straight back out. Measured 2026-08-20 in a
    // throwaway home: `~/.tokenade` and not one other file. The assertion here
    // was the inverse of today's — it pinned the absence — and it passed for
    // the whole time the promise was empty.
    let own = TempHome::new("t3-own");
    assert!(
        channel_installed("t3-code", own.path()),
        "`install --only t3-code` must leave the channel it announces: its own \
         message says the Claude hooks cover it, so they have to be written"
    );
    let via = TempHome::new("t3-via");
    assert_eq!(delegates_to("t3-code"), Some("claude-code"));
    assert!(
        channel_installed("claude-code", via.path()),
        "installing for claude-code left no hook — the delegation cannot be \
         measured and every T3 cell would read Ko"
    );
    let cell = TempHome::new("t3-cell");
    assert_eq!(
        probe_command_compaction("t3-code", cell.path()),
        Verdict::Ok
    );
    // And the TABLE must say the same thing as the probe. It did not, once:
    // the emitted cell read `Ko` while this very probe returned `Ok`, which
    // means the table and the proof had drifted apart — the one failure mode
    // that would make the whole document worthless.
    let row = AGENTS
        .iter()
        .find(|a| a.slug == Some("t3-code"))
        .expect("T3 Code is in the table");
    let f = FEATURES
        .iter()
        .find(|f| f.label == "Command compaction")
        .unwrap();
    assert_eq!(cell_for(row, f), "Ok bin");
}
