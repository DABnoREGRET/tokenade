#![allow(unused_imports)]
//! Split out of the monolithic `hooks.rs` (mechanical module split, no
//! behaviour change). Wired back into the `hooks` namespace by mod.rs via
//! `pub(crate) use <name>::*;`.
use super::*;
use crate::*;

/// PreToolUse handler for `Task` / `Agent`. Pre-pends a short
/// tokenade preamble to the subagent's `prompt` field so the spawned
/// agent inherits the "use MCP tokenade tools BEFORE grep/find/Read"
/// preference without the parent having to write it manually each
/// time.
///
/// Architecture: we use the SAME `permissionDecision: allow +
/// updatedInput` pattern as the Bash rewrite. Claude Code then
/// invokes the original tool with the modified `prompt` — no
/// "Error:" UI, no double execution, no parent involvement.
///
/// The preamble is intentionally tiny (~80 tokens). The first
/// subagent call that AVOIDS a single full-file Read pays for ~50
/// preambles.
///
/// Idempotency: if the preamble marker is already in the prompt
/// (because the parent wrote it explicitly, or because a previous
/// hook fired — Claude Code in some configs re-runs PreToolUse), we
/// skip the injection. No double-prepend.
pub(crate) fn handle_subagent_pretool(
    parsed: &serde_json::Value,
    started: std::time::Instant,
    tool: &str,
) -> i32 {
    // Sidechain window marker: while subagents run, they SHARE session_id
    // with the main thread but NOT its context — every "already in your
    // context" dedup claim becomes unverifiable (fleet forensics: 4 readers
    // hit false "identical to a prior call" on first-ever reads). Read
    // substitutions check this marker and degrade to additive notes.
    if let Some(sess) = crate::hooks::session_id_of(parsed) {
        let skey: String = sess
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(40)
            .collect();
        let p = super::read_staging::read_pending_dir().join(format!("sidechain-{skey}"));
        tokenade_core::platform::ensure_parent_dir(&p);
        // `count:ts` — the SubagentStop hook decrements; ts refreshes as the
        // crash-safety TTL. Claims resume the moment the LAST subagent stops.
        let (count, _) = super::read_sidechain_state(&p);
        let now = tokenade_core::platform::now_unix_secs();
        let _ = std::fs::write(&p, format!("{}:{now}", count + 1));
    }

    if hook_disabled_by_env() {
        return 0;
    }
    let original_prompt = parsed
        .get("tool_input")
        .and_then(|i| i.get("prompt"))
        .and_then(|p| p.as_str())
        .unwrap_or("");
    if original_prompt.is_empty() {
        return 0;
    }

    let subagent_type = parsed
        .get("tool_input")
        .and_then(|i| i.get("subagent_type"))
        .and_then(|s| s.as_str())
        .unwrap_or("");
    let Some(briefed) = brief_prompt(subagent_type, original_prompt) else {
        return 0;
    };
    let preamble_len = briefed.len() - original_prompt.len();
    let skips_claude_md = !loads_rules_file(subagent_type);

    // Build updated tool_input preserving every other field.
    let mut updated_input = parsed
        .get("tool_input")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    if let Some(obj) = updated_input.as_object_mut() {
        obj.insert("prompt".to_string(), serde_json::Value::String(briefed));
    }

    let decision = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "permissionDecisionReason": "tokenade subagent brief auto-prepended",
            "updatedInput": updated_input,
        }
    });
    println!("{decision}");
    tokenade_core::debug_log::log(
        "hookwrap-pretool",
        "subagent_briefed",
        serde_json::json!({
            "tool": tool,
            "subagent_type": subagent_type,
            "full_brief": skips_claude_md,
            "original_prompt_len": original_prompt.len(),
            "preamble_len": preamble_len,
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }),
    );
    0
}

/// The marker that says a prompt already carries the brief. One authority:
/// the hook, the CLI and every bridge test the same string.
pub(crate) const PREAMBLE_MARKER: &str = "[tokenade-subagent-brief]";

/// The brief itself, for a given `subagent_type`.
///
/// Extracted from the hook so the SAME text reaches an agent whose delegation
/// tool is not Claude Code's `Task`. Five bridges used to have no way to ask
/// for it, so a subagent on opencode inherited the folding and not the
/// instructions; they now shell out to `tokenade subagent-brief`, which calls
/// this. A second copy in TypeScript would be the "two lists describing one
/// fact" class again.
pub(crate) fn subagent_brief(subagent_type: &str) -> String {
    // Built-in `Explore` and `Plan` subagents are the ONLY ones that skip
    // the CLAUDE.md hierarchy (per Claude Code docs, subagents "What loads
    // at startup"); every other subagent loads it in full. So a normal
    // subagent ALREADY has the project scaffold's Explore-CLI block +
    // OUTPUT_BREVITY_RULES in context — re-injecting them here is pure
    // duplication. Give the full brief only to Explore/Plan (their sole
    // channel); to everyone else inject just the delta the scaffold doesn't
    // carry (thinking-terse, recovery commands, response language).
    //
    // Every token here is paid on every subagent call, so the wording is
    // tight. Deliberately omits the "fix causes not symptoms" principle:
    // subagents are typically exploration agents that don't apply fixes.
    let skips_claude_md = !loads_rules_file(subagent_type);
    let preamble = if skips_claude_md {
        format!(
            "{PREAMBLE_MARKER}\n\
For codebase questions, default to the tokenade CLI instead of grep/find/Read:\n\
- `tokenade semantic \"…\"` for natural-language (\"where is auth handled\")\n\
- `tokenade query <name>` for known identifiers (locates a symbol and its references)\n\
- `tokenade map` for a one-shot repo overview\n\
- `tokenade skeleton <file>` for signatures-only (cheaper than Read)\n\
- `tokenade impact <file>` before refactoring — lists dependents\n\
Fall back to `grep`/`find`/whole-file `Read` only when the query \
doesn't fit a structured shape. For a document or a big blob, ASK it instead of \
reading it whole: `tokenade read <file> --prompt \"…\"` (or `tokenade expand-ref \
<hash> --prompt \"…\"` for a folded ref). Several questions about the SAME source? \
Putting them ALL in ONE call, comma-separated — `--prompt \"q1, q2, q3\"` — is the \
CHEAPEST option available to you in tokens spent; it answers each \
under its own heading in a single round-trip; do not issue one call per question. \
For derived values from noisy output, \
use `tokenade exec` (think-in-code); for noisy bash transcripts, `tokenade exec-compact`. \
If a compactor folded bytes you need verbatim, recover them instantly \
via `tokenade expand-ref <hash>` (the hash is printed in the compactor's \
banner — no re-execution). Only fall back to `tokenade raw <cmd>` when \
you need to re-run a command WITHOUT compaction.\n\
An absolute path (`/usr/bin/git`) is intercepted exactly like `git` when \
hooks are installed; only a PATH-based interception needs the bare name.\n\
In your internal reasoning / `<thinking>` blocks, be terse: write \
compressed notes, not prose. Omit filler; think in telegrams.\n\
{brevity}\n\
Always respond and reason in the same language as the user's message. \
If the user writes in French, reply in French; in English, in English.\n\n\
---\n\n",
            brevity = tokenade_core::project_scaffold::OUTPUT_BREVITY_RULES,
        )
    } else {
        // Inherits the project CLAUDE.md → the Explore-CLI block and the
        // brevity rules are already in context. Only add what the scaffold
        // does NOT carry.
        format!(
            "{PREAMBLE_MARKER}\n\
In your internal reasoning / `<thinking>` blocks, be terse: write \
compressed notes, not prose. Omit filler; think in telegrams.\n\
For a document or a folded blob, ASK it rather than read it whole: \
`tokenade read <file> --prompt \"…\"` / `tokenade expand-ref <hash> --prompt \"…\"`. \
Several questions about the SAME source? Putting them ALL in ONE call is the \
CHEAPEST option in tokens spent — \
comma-separated — `--prompt \"q1, q2, q3\"` answers each under its own heading \
in a single round-trip; do not issue one call per question.\n\
For noisy bash transcripts use `tokenade exec-compact`; if a compactor \
folded bytes you need verbatim, recover them via `tokenade expand-ref \
<hash> --raw` (the hash is printed in the banner — no re-execution), and only \
`tokenade raw <cmd>` to re-run a command WITHOUT compaction.\n\
Always respond and reason in the same language as the user's message. \
If the user writes in French, reply in French; in English, in English.\n\n\
---\n\n"
        )
    };

    preamble
}

/// Does a subagent of this type load the CLAUDE.md hierarchy — tokenade's
/// block with it? Claude Code 2.1.299's agent table opts four built-ins out
/// (`omitClaudeMd: true`); a custom agent opts out in its frontmatter.
/// Everything else loads it, Claude Code's default.
pub(crate) fn loads_rules_file(subagent_type: &str) -> bool {
    if matches!(
        subagent_type,
        "Explore" | "Plan" | "web-fetch" | "comment-thread-analyst"
    ) {
        return false;
    }
    let file = format!("{subagent_type}.md");
    let dirs = [
        std::env::current_dir()
            .ok()
            .map(|c| c.join(".claude/agents")),
        tokenade_core::platform::home_dir().map(|h| h.join(".claude/agents")),
    ];
    // The project's definition shadows the user's: the first file found decides.
    let Some(text) = dirs
        .into_iter()
        .flatten()
        .find_map(|d| std::fs::read_to_string(d.join(&file)).ok())
    else {
        return true;
    };
    let front = text
        .strip_prefix("---")
        .and_then(|r| r.split("\n---").next())
        .unwrap_or("");
    !front.lines().any(|l| {
        let l = l.trim().replace(['"', '\''], "");
        matches!(l.as_str(), "omitClaudeMd: true" | "omitClaudeMd:true")
    })
}

/// `Some(brief + original)` when the brief should be injected, `None` when it
/// must not be: prompt already carrying it.
///
/// Books the overhead exactly once, here, so no caller can forget it and no
/// two callers can book it twice for one injection.
pub(crate) fn brief_prompt(subagent_type: &str, original_prompt: &str) -> Option<String> {
    if original_prompt.is_empty() || original_prompt.contains(PREAMBLE_MARKER) {
        return None;
    }
    let preamble = subagent_brief(subagent_type);
    // Overhead audit: this brief is INJECTED into the subagent's prompt
    // (143-558 tokens measured, not the "~80" the old comment claimed) and
    // was debited nowhere — it bypasses log_emitted_additional_context.
    //
    // It lives in the SUBAGENT's context, not the parent's: logged as the
    // parent's injection (`record_estimated`), it was re-billed on every parent
    // call until the parent compacted — a median 217 calls — while a subagent
    // makes 66 on average. Its re-reads are charged here instead
    // ([`tokenade_core::gain::SUBAGENT_BRIEF_REREADS`]), and kept out of the
    // parent's injection log.
    let n = tokenade_core::tokenizer::estimate(&preamble);
    tokenade_core::gain::record_estimated_io(
        "hook",
        "injected_context",
        "subagent-brief",
        "overhead",
        "input",
        tokenade_core::Savings::new(n, 0),
    );
    tokenade_core::gain::record_estimated_io(
        "hook",
        "overhead_rebill",
        tokenade_core::gain::PROXY_LABEL,
        "overhead",
        "cache",
        tokenade_core::Savings::new(
            n.saturating_mul(tokenade_core::gain::SUBAGENT_BRIEF_REREADS as usize),
            0,
        ),
    );
    // The rules file the subagent loads with CLAUDE.md is in its context too,
    // sent on its first call and re-read on every later one. The parent's
    // `scaffold_overhead` follows the parent's calls only.
    if loads_rules_file(subagent_type) {
        let agent = match tokenade_core::detect::from_env() {
            tokenade_core::detect::Agent::Unknown => tokenade_core::detect::Agent::ClaudeCode,
            a => a,
        };
        let cwd = std::env::current_dir().ok();
        let rules = tokenade_core::platform::home_dir()
            .map(|h| {
                tokenade_core::project_scaffold::loaded_block_tokens(agent, &h, cwd.as_deref())
            })
            .unwrap_or(0);
        if rules > 0 {
            tokenade_core::gain::record_estimated_io(
                "hook",
                "injected_context",
                "subagent-rules",
                "overhead",
                "input",
                tokenade_core::Savings::new(rules, 0),
            );
            tokenade_core::gain::record_estimated_io(
                "hook",
                "overhead_rebill",
                tokenade_core::gain::PROXY_LABEL,
                "overhead",
                "cache",
                tokenade_core::Savings::new(
                    rules.saturating_mul(tokenade_core::gain::SUBAGENT_RULES_REREADS as usize),
                    0,
                ),
            );
        }
    }

    Some(format!("{preamble}{original_prompt}"))
}

/// `tokenade subagent-brief [--type <subagent_type>]` — reads the delegation
/// prompt on stdin, writes the prompt the subagent should actually get.
///
/// The plumbing every bridge needs and Claude Code's hook does not: opencode,
/// kilo, cline, pi, hermes and openclaw all expose a "before the tool runs"
/// seam where the arguments are mutable, but none of them can build the brief
/// — it lives in Rust. They pipe the prompt
/// through here instead of carrying a second copy of the text.
///
/// Prints the prompt UNCHANGED when there is nothing to add (marker already
/// present, gate closed, empty prompt), so a caller can assign the output
/// without comparing anything.
pub(crate) fn cmd_subagent_brief(args: &[String]) -> i32 {
    let mut subagent_type = String::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--type" | "--subagent-type" => {
                subagent_type = it.next().cloned().unwrap_or_default();
            }
            _ => {}
        }
    }
    let mut original = String::new();
    use std::io::Read as _;
    let _ = crate::bounded_input::read_stdin_lossy(&mut original);
    let out = brief_prompt(&subagent_type, &original).unwrap_or_else(|| original.clone());
    print!("{out}");
    0
}

/// The subagent brief's envelope, decided by what the HOST sent us.
///
/// One handler now serves three hosts and they do not agree on the shape
/// of the answer, so the shape cannot be a constant:
///
/// - **Copilot** builds the payload as
///   `{sessionId, timestamp, cwd, transcriptPath, agentName, agentDisplayName,
///   agentDescription}` — no `hook_event_name` at all — and reads the reply
///   with `l?.additionalContext !== void 0 ? {additionalContext: …} : void 0`,
///   i.e. the TOP LEVEL and nothing else. Read off the shipped `app.js`.
/// - **Codex** sends `hook_event_name: "SubagentStart"` and `agent_type`, and
///   its `subagent-start.command.output` schema — shipped inside the binary —
///   is `additionalProperties: false` over `{continue, hookSpecificOutput,
///   stopReason, suppressOutput, systemMessage}` with
///   `hookEventName: {"const": "SubagentStart"}`. A top-level
///   `additionalContext` invalidates the whole reply, and the camelCase
///   spelling fails the const.
/// - **Grok** is Claude-shaped: its own embedded documentation gives
///   `{"hookSpecificOutput": {"hookEventName": "Stop", "additionalContext": …}}`.
///
/// So: echo back the name the host itself used, and put the brief where that
/// host reads it. Emitting BOTH is what this did before, and it is the one
/// option Codex refuses.
fn subagent_start_reply(parsed: &serde_json::Value, brief: &str) -> serde_json::Value {
    match parsed.get("hook_event_name").and_then(|s| s.as_str()) {
        Some(event) => serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": event,
                "additionalContext": brief,
            }
        }),
        None => serde_json::json!({ "additionalContext": brief }),
    }
}

/// The agent's name, under whichever key the host spells it.
///
/// Copilot says `agentName`; Codex says `agent_type` (its `subagent-start`
/// input requires `agent_id` and `agent_type` and has no `agentName`).
fn subagent_start_agent_name(parsed: &serde_json::Value) -> &str {
    for key in ["agentName", "agent_type"] {
        if let Some(v) = parsed.get(key).and_then(|s| s.as_str()) {
            if !v.is_empty() {
                return v;
            }
        }
    }
    ""
}

/// `hookwrap-subagentstart` — the `SubagentStart` seam on Copilot, Codex and
/// Grok.
///
/// Copilot has no seam where a delegation tool's arguments can be rewritten:
/// its subagents are named definitions, not a `Task` call carrying a prompt.
/// What it does have is `onSubagentStart`, whose `additionalContext` is joined
/// into the child's prompt before it runs — read off the shipped `app.js`,
/// where the three call sites all do
/// `b?.additionalContext && (d = b.additionalContext)` and then
/// `[V, c].filter(Boolean).join("\n")`.
///
/// Same brief, same gate, same one-time overhead booking as the Claude-family
/// `Task` path; only the envelope differs — see `subagent_start_reply`.
pub(crate) fn cmd_hookwrap_subagent_start() -> i32 {
    if hook_disabled_by_env() {
        return 0;
    }
    let mut raw = String::new();
    use std::io::Read as _;
    let _ = crate::bounded_input::read_stdin_lossy(&mut raw);
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null);
    let agent_name = subagent_start_agent_name(&parsed);
    // The brief is a PREFIX everywhere else; here the host owns the join, so
    // ask for it against an empty prompt and hand back what comes out.
    let Some(brief) = brief_prompt(agent_name, " ") else {
        return 0;
    };
    let brief = brief.trim_end().to_string();
    log_emitted_additional_context("hookwrap-subagentstart", "subagentStart", &brief);
    println!("{}", subagent_start_reply(&parsed, &brief));
    0
}

#[cfg(test)]
mod brief_tests {
    /// The marker is what makes the injection idempotent, and it is written in
    /// exactly one place. A second copy anywhere — a bridge, a test fixture —
    /// would let the two drift and the brief be injected twice.
    #[test]
    fn a_prompt_already_carrying_the_brief_is_left_alone() {
        let once = super::brief_prompt("", "find the bug").expect("first pass injects");
        assert!(once.starts_with(super::PREAMBLE_MARKER));
        assert!(
            super::brief_prompt("", &once).is_none(),
            "the brief was injected twice into one prompt"
        );
    }

    /// `Explore` and `Plan` skip the project scaffold, so they get the full
    /// brief; everyone else gets the delta. Two different texts, one builder:
    /// the six bridges and the hook all call this, and none carries a copy.
    #[test]
    fn the_two_briefs_differ_and_both_carry_the_marker() {
        let full = super::subagent_brief("Explore");
        let delta = super::subagent_brief("general-purpose");
        assert!(full.contains(super::PREAMBLE_MARKER) && delta.contains(super::PREAMBLE_MARKER));
        assert!(
            full.len() > delta.len(),
            "the scaffold-skipping brief ({}) should be the longer one, not {}",
            full.len(),
            delta.len()
        );
    }

    /// An empty prompt has nothing to brief, and the CLI hands it straight
    /// back so a bridge can assign the output without comparing anything.
    #[test]
    fn an_empty_prompt_is_not_briefed() {
        assert!(super::brief_prompt("", "").is_none());
    }

    /// Copilot's `subagentStart` payload carries no `hook_event_name`
    /// and its reader is `l?.additionalContext !== void 0` — the TOP LEVEL.
    /// Read off the shipped `app.js`.
    #[test]
    fn a_host_that_sends_no_event_name_gets_the_top_level_key() {
        let input = serde_json::json!({
            "sessionId": "s", "cwd": "/tmp", "transcriptPath": "/t",
            "agentName": "explore", "agentDisplayName": "Explore",
        });
        let out = super::subagent_start_reply(&input, "BRIEF");
        assert_eq!(
            out.get("additionalContext").and_then(|v| v.as_str()),
            Some("BRIEF")
        );
        assert!(
            out.get("hookSpecificOutput").is_none(),
            "Copilot reads the top level; the extra block is what Codex refuses"
        );
    }

    /// Codex's `subagent-start.command.output` is `additionalProperties: false`
    /// over {continue, hookSpecificOutput, stopReason, suppressOutput,
    /// systemMessage}, with `hookEventName: {"const": "SubagentStart"}` — both
    /// read out of the shipped binary's own JSON Schema. A top-level
    /// `additionalContext` invalidates the whole reply.
    #[test]
    fn codex_gets_only_the_block_and_its_own_spelling() {
        let input = serde_json::json!({
            "hook_event_name": "SubagentStart",
            "agent_id": "a1", "agent_type": "explore",
            "cwd": "/tmp", "session_id": "s", "turn_id": "t",
        });
        let out = super::subagent_start_reply(&input, "BRIEF");
        assert_eq!(
            out.pointer("/hookSpecificOutput/hookEventName")
                .and_then(|v| v.as_str()),
            Some("SubagentStart"),
            "the const is echoed from the host, never spelled here"
        );
        assert_eq!(
            out.pointer("/hookSpecificOutput/additionalContext")
                .and_then(|v| v.as_str()),
            Some("BRIEF")
        );
        assert!(
            out.get("additionalContext").is_none(),
            "a top-level key makes Codex reject the reply outright"
        );
        // Nothing outside Codex's allowed top-level set.
        let allowed = [
            "continue",
            "hookSpecificOutput",
            "stopReason",
            "suppressOutput",
            "systemMessage",
        ];
        for k in out.as_object().unwrap().keys() {
            assert!(
                allowed.contains(&k.as_str()),
                "Codex refuses top-level key {k:?}"
            );
        }
    }

    /// The agent's name is spelled differently by each host, and the brief is
    /// built from it. Codex's `subagent-start` input requires `agent_type` and
    /// has no `agentName`.
    #[test]
    fn the_agent_name_is_read_under_either_spelling() {
        assert_eq!(
            super::subagent_start_agent_name(&serde_json::json!({"agentName": "explore"})),
            "explore"
        );
        assert_eq!(
            super::subagent_start_agent_name(&serde_json::json!({"agent_type": "explore"})),
            "explore"
        );
        // An empty value must not shadow the other spelling.
        assert_eq!(
            super::subagent_start_agent_name(
                &serde_json::json!({"agentName": "", "agent_type": "explore"})
            ),
            "explore"
        );
        assert_eq!(super::subagent_start_agent_name(&serde_json::json!({})), "");
    }
}
