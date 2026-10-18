//! The light-agent loop. One-model-many-tools ReAct cycle, kept
//! deliberately small: the orchestrator delegates a *simple* task (code
//! exploration, output digestion, web lookup, info retrieval), so the
//! agent needs to gather → distil → return, not plan elaborately.
//!
//! Loop shape mirrors opencode's `runLoop()`
//! (`packages/opencode/src/session/prompt.ts`): call model → parse the
//! action → execute the (sandbox-gated) tool → feed the result back →
//! repeat until FINAL or the step budget is spent. No sub-agent spawning
//! is possible: the tool parser has no such verb.

use super::providers::{self, Msg};
use super::sandbox::Sandbox;
use super::tools;

pub struct AgentOutcome {
    pub answer: String,
    /// Tokens the (free) light-agent consumed, split by direction. This
    /// is the work the orchestrator did NOT pay for inline.
    pub agent_in: u64,
    pub agent_out: u64,
    pub steps: u32,
    pub provider: String,
    pub model: String,
}

pub struct AgentSpec<'a> {
    pub task: &'a str,
    pub sandbox: Sandbox,
    pub model: String,
    pub max_steps: u32,
    /// Path to the tokenade executable (for the agent's own tools).
    pub exe: String,
    /// Audit trace directory; when set, every turn is streamed there.
    pub trace: Option<std::path::PathBuf>,
}

/// Append an audit event to the run's trace directory, if one is set.
fn emit(spec: &AgentSpec, event: serde_json::Value) {
    if let Some(dir) = &spec.trace {
        super::trace::append_event(dir, event);
    }
}

/// The lines of `reply` AFTER the given action line — the file body for a
/// WRITE action.
fn write_body(reply: &str, action_line: &str) -> String {
    let mut it = reply.lines();
    let mut found = false;
    for l in it.by_ref() {
        if l.trim() == action_line.trim() {
            found = true;
            break;
        }
    }
    if !found {
        return String::new();
    }
    it.collect::<Vec<_>>().join("\n")
}

/// Extract the first actionable line from a model reply. Models may emit
/// reasoning first; we scan for the first line starting with a known verb.
fn extract_action(reply: &str) -> Option<(&str, bool)> {
    const VERBS: &[&str] = &[
        "FINAL", "READ", "TOKENADE", "TK", "BASH", "SH", "SHELL", "EXEC", "RUN", "WEB", "FETCH",
        "SEARCH", "GOOGLE", "DDG", "WRITE", "CREATE", "RM", "DELETE", "DEL",
    ];
    for line in reply.lines() {
        let t = line.trim();
        let head = t
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_uppercase();
        if VERBS.contains(&head.as_str()) {
            return Some((t, head == "FINAL"));
        }
    }
    None
}

pub fn run(spec: &AgentSpec, scaffold: &str) -> Result<AgentOutcome, String> {
    // Local model → native function-calling loop (the model calls tools in its
    // own trained format). Only in `--features local-llm` builds; the default
    // build ships the free-provider chain only.
    #[cfg(feature = "local-llm")]
    if super::local::wants_local(&spec.model) {
        return run_local_tools(spec);
    }
    // Production: each turn calls the free-provider chain. The completion
    // source is injected into `run_with` so the loop (tool dispatch, usage
    // accounting, step budget, FINAL detection) is unit-testable offline.
    let mut complete = |msgs: &[Msg]| providers::complete(&spec.model, msgs);
    run_with(spec, scaffold, &mut complete)
}

/// Strip `<think>…</think>` reasoning blocks from a hybrid-thinking model's
/// final answer, leaving just the answer.
#[cfg(feature = "local-llm")]
fn strip_think(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find("<think>") {
        out.push_str(&rest[..i]);
        rest = match rest[i..].find("</think>") {
            Some(j) => &rest[i + j + "</think>".len()..],
            None => "",
        };
    }
    out.push_str(rest);
    out.trim().to_string()
}

/// Native function-calling loop for a local model. The model receives tool
/// SCHEMAS (via its chat template) and emits tool calls in its own format; we
/// parse, execute them sandboxed, feed results back as `tool` messages, and
/// loop until it replies with plain text (the final answer).
#[cfg(feature = "local-llm")]
fn run_local_tools(spec: &AgentSpec) -> Result<AgentOutcome, String> {
    use serde_json::json;
    // Headroom for hybrid-thinking models that reason in <think> before acting.
    const GEN_CAP: usize = 1024;

    let tools = super::toolcall::tool_specs(&spec.sandbox);
    let sys = format!(
        "You are a tokenade light-agent solving ONE scoped task for an \
orchestrator. Use the provided tools to GATHER what you need (read files, run \
commands, search), then reply with the final answer as PLAIN TEXT and no tool \
call. Be terse — no preamble. Working directory: {}.",
        spec.sandbox.primary_root()
    );
    let mut messages = json!([
        {"role": "system", "content": sys},
        {"role": "user", "content": spec.task},
    ]);
    let arr = |m: &mut serde_json::Value, v: serde_json::Value| {
        if let Some(a) = m.as_array_mut() {
            a.push(v);
        }
    };

    let mut owned: std::collections::HashSet<std::path::PathBuf> = std::collections::HashSet::new();
    let (mut agent_in, mut agent_out) = (0u64, 0u64);
    let mut last_text = String::new();

    for step in 1..=spec.max_steps {
        let (text, pin, pout) = super::local::complete_tools(&messages, &tools, GEN_CAP)?;
        agent_in += pin;
        agent_out += pout;
        last_text = text.clone();
        emit(
            spec,
            serde_json::json!({
                "kind": "turn", "step": step, "provider": "local",
                "in": pin, "out": pout,
                "reply": text.chars().take(4000).collect::<String>(),
            }),
        );

        let calls = super::toolcall::parse_tool_calls(&text);
        if calls.is_empty() {
            let answer = strip_think(&text);
            emit(
                spec,
                serde_json::json!({"kind": "final", "step": step, "answer": answer}),
            );
            return Ok(AgentOutcome {
                answer,
                agent_in,
                agent_out,
                steps: step,
                provider: "local".into(),
                model: "local".into(),
            });
        }

        arr(&mut messages, json!({"role": "assistant", "content": text}));
        for tc in &calls {
            let result = match super::toolcall::call_to_tool(tc) {
                Ok(tool) => super::tools::execute(&tool, &spec.sandbox, &spec.exe, &mut owned),
                Err(e) => format!("ERROR: {e}"),
            };
            emit(
                spec,
                serde_json::json!({
                    "kind": "tool", "step": step, "action": tc.name,
                    "result": result.chars().take(2000).collect::<String>(),
                }),
            );
            arr(
                &mut messages,
                json!({
                    "role": "tool", "name": tc.name,
                    "content": result.chars().take(4000).collect::<String>(),
                }),
            );
        }
    }

    // Budget spent without a plain-text answer: return the last output.
    Ok(AgentOutcome {
        answer: strip_think(&last_text),
        agent_in,
        agent_out,
        steps: spec.max_steps,
        provider: "local".into(),
        model: "local".into(),
    })
}

/// Keep the free agent's own context bounded on long runs: when the whole
/// conversation exceeds ~100k chars (≈25k tokens), shrink the OLDEST
/// `TOOL RESULT:` turns — the model has already acted on them — down to a
/// short head + fold note. The system prompt, the task and the most recent
/// exchanges are never touched, so the model keeps what it needs to conclude.
fn fold_old_tool_results(messages: &mut [Msg]) {
    const BUDGET: usize = 100_000;
    const KEEP_LAST: usize = 6; // most recent turns stay verbatim
    const FOLD_TO: usize = 600;
    let total: usize = messages.iter().map(|m| m.content.len()).sum();
    if total <= BUDGET {
        return;
    }
    let mut excess = total - BUDGET;
    let n = messages.len();
    // Index 0 = system, 1 = task: never folded.
    for m in messages
        .iter_mut()
        .take(n.saturating_sub(KEEP_LAST))
        .skip(2)
    {
        if excess == 0 {
            break;
        }
        if m.role == "user" && m.content.starts_with("TOOL RESULT:") && m.content.len() > FOLD_TO {
            let saved = m.content.len() - FOLD_TO;
            let mut head: String = m.content.chars().take(FOLD_TO).collect();
            head.push_str("\n…[older tool result folded — already used above]");
            m.content = head;
            excess = excess.saturating_sub(saved);
        }
    }
}

/// The agent's OWN system prompt. NOTE: this is not the orchestrator
/// scaffold — the goal-writer asked that the *feature description* live in
/// the orchestrator's prompt, not be re-injected into every delegate.
/// What we DO inject is the tokenade explore/brevity scaffold, so the
/// delegate reaches for the compacting tools and stays terse (its free
/// tokens are finite per-request).
fn system_prompt(spec: &AgentSpec, scaffold: &str) -> String {
    format!(
        "You are a tokenade light-agent: a fast, cheap delegate solving ONE \
scoped task for an orchestrator. Gather/do what's needed, then STOP and return \
a distilled answer — no preamble, no narration. Prefer the compacting tokenade \
tools over reading whole files.\n\n\
YOUR SANDBOX — capabilities and limits (set by the orchestrator, you CANNOT \
widen them):\n  {sandbox}\n\n\
{tools}\n\n\
PROTOCOL: think briefly, then emit exactly ONE action. Most actions are a \
single line; WRITE and EXEC are special — put the header line (`WRITE <path>` \
or `EXEC <lang>`) on its own line and the body (file content / code) on the \
lines AFTER it. Emit `FINAL <answer>` when done. Never emit more than one \
action per turn. Respect your limits: BASH/EXEC run SANDBOXED (read-only \
filesystem + writable /tmp only) so scripts CANNOT persist files — to \
create/change/delete real files use WRITE/RM, which only work on files YOU \
created. Stay inside the jail dir; you cannot spawn sub-agents. When you have \
enough to answer, emit FINAL and stop.\n\n\
Your FIRST line MUST be exactly one action verb \
(READ/BASH/EXEC/SEARCH/WRITE/RM/FINAL) followed by its argument — never prose, \
markdown, code fences, or shell `$(...)`.\n\n\
EXAMPLE (task: \"what version is in Cargo.toml?\"):\n\
  turn 1, you emit →  READ Cargo.toml\n\
  you then receive →  TOOL RESULT: [package] ... version = \"1.2.3\" ...\n\
  turn 2, you emit →  FINAL 1.2.3\n\n\
EXAMPLE (task: \"how many .rs files in src?\"):\n\
  turn 1, you emit →  BASH ls src | grep -c '\\.rs$'\n\
  you then receive →  TOOL RESULT: 12\n\
  turn 2, you emit →  FINAL 12\n\n\
--- tokenade tool notes ---\n{scaffold}",
        sandbox = spec.sandbox.describe(),
        tools = tools::catalogue(&spec.sandbox),
        scaffold = scaffold,
    )
}

fn run_with(
    spec: &AgentSpec,
    scaffold: &str,
    complete: &mut dyn FnMut(&[Msg]) -> Result<providers::Completion, String>,
) -> Result<AgentOutcome, String> {
    let mut messages = vec![
        Msg::system(system_prompt(spec, scaffold)),
        Msg::user(format!("TASK: {}", spec.task)),
    ];
    let mut agent_in = 0u64;
    let mut agent_out = 0u64;
    // Files the agent created this run — the provenance set that lets it
    // overwrite/delete only what it made itself.
    let mut owned: std::collections::HashSet<std::path::PathBuf> = std::collections::HashSet::new();
    let (mut provider, mut model) = (String::new(), String::new());

    for step in 1..=spec.max_steps {
        // Long runs accumulate tool results (24k chars each) — unbounded, a
        // 30-step run would overflow the model's context. Fold the OLDEST
        // tool results (already acted upon) once the conversation passes the
        // budget; the task, recent turns and the model's own replies survive.
        fold_old_tool_results(&mut messages);
        let c = complete(&messages)?;
        provider = c.provider.clone();
        model = c.model.clone();
        // Account tokens (fall back to estimate when the endpoint omits usage).
        let (pin, pout) = if c.usage.prompt_tokens + c.usage.completion_tokens > 0 {
            (c.usage.prompt_tokens, c.usage.completion_tokens)
        } else {
            let inb: u64 = messages
                .iter()
                .map(|m| providers::est_tokens(&m.content))
                .sum();
            (inb, providers::est_tokens(&c.text))
        };
        agent_in += pin;
        agent_out += pout;

        let reply = c.text.trim().to_string();
        emit(
            spec,
            serde_json::json!({
                "kind": "turn", "step": step, "provider": provider, "model": model,
                "in": pin, "out": pout,
                "reply": reply.chars().take(4000).collect::<String>(),
            }),
        );
        match extract_action(&reply) {
            Some((line, true)) => {
                // FINAL <answer>
                let answer = line
                    .split_once(char::is_whitespace)
                    .map(|(_, a)| a.trim().to_string())
                    .unwrap_or_default();
                let answer = if answer.is_empty() {
                    reply.clone()
                } else {
                    answer
                };
                emit(
                    spec,
                    serde_json::json!({"kind": "final", "step": step, "answer": answer}),
                );
                return Ok(AgentOutcome {
                    answer,
                    agent_in,
                    agent_out,
                    steps: step,
                    provider,
                    model,
                });
            }
            Some((line, false)) => match tools::parse(line) {
                Ok(mut tool) => {
                    // WRITE / EXEC carry their body on the lines AFTER the action.
                    match &mut tool {
                        tools::Tool::Write { content, .. } => *content = write_body(&reply, line),
                        tools::Tool::Exec { code, .. } => *code = write_body(&reply, line),
                        _ => {}
                    }
                    let result = tools::execute(&tool, &spec.sandbox, &spec.exe, &mut owned);
                    emit(
                        spec,
                        serde_json::json!({
                            "kind": "tool", "step": step, "action": line,
                            "result": result.chars().take(2000).collect::<String>(),
                        }),
                    );
                    messages.push(Msg::assistant(reply));
                    messages.push(Msg::user(format!("TOOL RESULT:\n{result}")));
                }
                Err(e) => {
                    messages.push(Msg::assistant(reply));
                    messages.push(Msg::user(format!(
                        "TOOL ERROR: {e}. Emit a valid action or FINAL."
                    )));
                }
            },
            None => {
                // No recognized action line. Distinguish a genuine plain-text
                // answer from a BOTCHED action (the model inventing a verb the
                // parser doesn't know — `SEMANTIC "retry"`, `READcobra.go` —
                // which, if accepted as the answer, hands the orchestrator
                // garbage and torches trust in delegation). An action attempt
                // is a short single line starting with an ALL-CAPS token;
                // steer the model back instead of finishing on it.
                let t = reply.trim();
                let head = t.split_whitespace().next().unwrap_or("");
                let leading_caps = head.chars().take_while(|c| c.is_ascii_uppercase()).count();
                let looks_botched_action = t.lines().count() <= 1
                    && !t.is_empty()
                    && t.len() < 120
                    // fully-caps token (`SEMANTIC "retry"`) or a verb glued to
                    // its argument (`READcobra.go`).
                    && (head.chars().all(|c| c.is_ascii_uppercase() || c == '_') && head.len() >= 2
                        || leading_caps >= 4);
                if looks_botched_action && step < spec.max_steps {
                    let nudge = format!(
                        "`{head}` is not a valid action. Valid actions: READ / BASH / WEB / \
                         SEARCH / WRITE / RM / TOKENADE — or reply `FINAL <your answer>` \
                         when done. Try again."
                    );
                    messages.push(Msg::assistant(reply));
                    messages.push(Msg::user(nudge));
                    continue;
                }
                // Genuine plain-text reply — treat it as the answer.
                return Ok(AgentOutcome {
                    answer: reply,
                    agent_in,
                    agent_out,
                    steps: step,
                    provider,
                    model,
                });
            }
        }
    }
    // Budget spent without FINAL: return the transcript tail as best-effort.
    let tail = messages
        .iter()
        .rev()
        .find(|m| m.role == "assistant")
        .map(|m| m.content.clone())
        .unwrap_or_else(|| "light-agent exhausted its step budget without answering".into());
    Ok(AgentOutcome {
        answer: tail,
        agent_in,
        agent_out,
        steps: spec.max_steps,
        provider,
        model,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_final_after_reasoning() {
        let r = "let me think\nI have the answer now\nFINAL the value is 42";
        let (line, is_final) = extract_action(r).unwrap();
        assert!(is_final);
        assert!(line.starts_with("FINAL"));
    }

    #[test]
    fn finds_first_action() {
        let r = "I'll look at the file.\nREAD src/lib.rs 1:20";
        let (line, is_final) = extract_action(r).unwrap();
        assert!(!is_final);
        assert_eq!(line, "READ src/lib.rs 1:20");
    }

    #[test]
    fn recognizes_every_tool_verb_as_action() {
        // Regression: EXEC/RUN were missing from VERBS, so EXEC actions were
        // silently treated as a FINAL answer and never executed.
        for v in [
            "EXEC python",
            "RUN bash",
            "SEARCH x",
            "WRITE f",
            "RM f",
            "BASH ls",
        ] {
            let (line, is_final) = extract_action(v).expect(v);
            assert!(!is_final, "{v} must be an action, not FINAL");
            assert_eq!(line, v);
        }
    }

    fn spec_for(max_steps: u32) -> AgentSpec<'static> {
        use super::super::sandbox::Sandbox;
        use std::path::Path;
        AgentSpec {
            task: "do it",
            sandbox: Sandbox::read_only(Path::new(".")),
            model: "x".into(),
            max_steps,
            exe: "tokenade".into(),
            trace: None,
        }
    }

    fn completion(text: &str) -> providers::Completion {
        providers::Completion {
            text: text.to_string(),
            usage: providers::Usage {
                prompt_tokens: 100,
                completion_tokens: 10,
            },
            provider: "test".into(),
            model: "x".into(),
            from_reasoning: false,
            truncated: false,
        }
    }

    #[test]
    fn loop_executes_tool_then_final_and_accounts_usage() {
        let spec = spec_for(4);
        // Turn 1: a BASH the read-only sandbox DENIES (hermetic — no process
        // spawns). Turn 2: FINAL. The loop must thread the denial back and
        // continue, then stop on FINAL.
        let scripted = ["BASH ls", "FINAL all done"];
        let mut i = 0;
        let mut complete = |_m: &[Msg]| {
            let c = completion(scripted[i]);
            i += 1;
            Ok(c)
        };
        let out = run_with(&spec, "scaffold", &mut complete).unwrap();
        assert_eq!(out.answer, "all done");
        assert_eq!(out.steps, 2);
        assert_eq!(i, 2, "exactly two model turns");
        assert_eq!(out.agent_in, 200); // 2 turns × 100
        assert_eq!(out.agent_out, 20); // 2 turns × 10
    }

    #[test]
    fn fold_keeps_task_and_recent_folds_old_tool_results() {
        let big = format!("TOOL RESULT:\n{}", "x".repeat(30_000));
        let mut msgs = vec![Msg::system("sys"), Msg::user("TASK: find it")];
        for _ in 0..5 {
            msgs.push(Msg::assistant("READ foo"));
            msgs.push(Msg::user(big.clone()));
        }
        let before: usize = msgs.iter().map(|m| m.content.len()).sum();
        assert!(before > 100_000);
        fold_old_tool_results(&mut msgs);
        let after: usize = msgs.iter().map(|m| m.content.len()).sum();
        assert!(after < before, "shrunk: {after} < {before}");
        // system + task untouched, most recent tool result untouched
        assert_eq!(msgs[0].content, "sys");
        assert_eq!(msgs[1].content, "TASK: find it");
        assert!(msgs.last().unwrap().content.len() > 29_000, "recent kept");
        // an old one is folded with the note
        assert!(
            msgs[3].content.contains("folded"),
            "old folded: {}",
            &msgs[3].content[..80]
        );
    }

    #[test]
    fn fold_noop_under_budget() {
        let mut msgs = vec![
            Msg::system("sys"),
            Msg::user("TASK: t"),
            Msg::assistant("READ a"),
            Msg::user("TOOL RESULT:\nshort"),
        ];
        let orig = msgs[3].content.clone();
        fold_old_tool_results(&mut msgs);
        assert_eq!(msgs[3].content, orig);
    }

    #[test]
    fn botched_action_is_steered_not_returned() {
        // Model emits an invented verb (`SEMANTIC "retry"`) then, after the
        // corrective nudge, a proper FINAL. The junk line must NOT be the answer.
        let spec = spec_for(5);
        let replies = std::cell::RefCell::new(vec![
            "SEMANTIC \"retry\"".to_string(),
            "FINAL net.py:retry".to_string(),
        ]);
        let mut complete = |_msgs: &[Msg]| {
            let mut r = replies.borrow_mut();
            Ok(completion(&r.remove(0)))
        };
        let out = run_with(&spec, "", &mut complete).unwrap();
        assert_eq!(out.answer, "net.py:retry");
        assert_eq!(out.steps, 2);
    }

    #[test]
    fn glued_verb_is_steered_not_returned() {
        let spec = spec_for(5);
        let replies = std::cell::RefCell::new(vec![
            "READcobra.go".to_string(),
            "FINAL command.go holds the Command struct".to_string(),
        ]);
        let mut complete = |_msgs: &[Msg]| {
            let mut r = replies.borrow_mut();
            Ok(completion(&r.remove(0)))
        };
        let out = run_with(&spec, "", &mut complete).unwrap();
        assert_eq!(out.answer, "command.go holds the Command struct");
    }

    #[test]
    fn plain_text_answer_still_accepted() {
        let spec = spec_for(5);
        let mut complete = |_msgs: &[Msg]| {
            Ok(completion(
                "The retry logic lives in net.py, function retry().",
            ))
        };
        let out = run_with(&spec, "", &mut complete).unwrap();
        assert!(out.answer.contains("net.py"));
        assert_eq!(out.steps, 1);
    }

    #[test]
    fn loop_respects_step_budget() {
        let spec = spec_for(3);
        // Never emits FINAL → must stop at max_steps, not spin forever.
        let mut turns = 0;
        let mut complete = |_m: &[Msg]| {
            turns += 1;
            Ok(completion("BASH ls"))
        };
        let out = run_with(&spec, "scaffold", &mut complete).unwrap();
        assert_eq!(out.steps, 3);
        assert_eq!(turns, 3);
    }

    #[test]
    fn loop_propagates_provider_error() {
        let spec = spec_for(4);
        let mut complete = |_m: &[Msg]| Err("all free providers failed".to_string());
        assert!(run_with(&spec, "scaffold", &mut complete).is_err());
    }
}
