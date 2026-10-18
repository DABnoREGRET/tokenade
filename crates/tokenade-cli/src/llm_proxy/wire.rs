//! The two de-facto wires, and the ONE thing the LLM proxy does to a request.
//!
//! Rank 4 of the integration ladder. An agent that talks to an
//! API — rather than to a subscription — can be pointed at a local base URL,
//! and everything it sends passes through here.
//!
//! What this module does, and deliberately nothing else: walk a chat-completion
//! request body and replace the tool-RESULT blocks already sitting in the
//! history with their compacted form. It does not touch the model's reply, the
//! system prompt, the user's messages, or the tool DEFINITIONS.
//!
//! **The honest limit, which the product doc must repeat rather than hide.**
//! Compacting on the way out does NOT refund the turn where the raw result
//! first arrived — it removes that result's cost from every LATER turn. On a
//! long conversation that is most of the bill, because the same tool result is
//! re-sent on every turn; but the first occurrence is gone. A hook catches it
//! before it is ever sent even once. That is the technical reason rank 1 stays
//! above rank 4, and not a matter of taste.

use serde_json::Value;

/// Which provider wire a request body is written in.
///
/// Detected from the SHAPE of the body, not from the URL: an agent may proxy
/// through a path we do not recognise, and guessing from a route is how you end
/// up rewriting a body you did not understand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wire {
    /// `messages: [{role, content: [{type: "tool_result", content: …}]}]`
    Anthropic,
    /// `messages: [{role: "tool", content: "…", tool_call_id: "…"}]`
    OpenAiChat,
    /// Responses API: `input: [{type: "function_call_output", output: "…"}]`
    OpenAiResponses,
    /// Recognised as nothing. The caller must relay it byte-for-byte.
    Unknown,
}

/// Classify a request body.
///
/// Returns [`Wire::Unknown`] for anything not positively identified. That is
/// the whole contract: an unrecognised shape is relayed untouched, never
/// guessed at. The rule: "an unknown wire or an unrecognised shape is relayed
/// untouched, we do not guess."
/// [`detect`], told by the request path first. The body alone guessed wrong:
/// an Anthropic request with no `system` and no tool traffic was taken for
/// OpenAI chat (and given a `role: system` message Anthropic refuses), and
/// any `input` array — `/v1/embeddings` — for the Responses API. An endpoint
/// that is not a generation endpoint is not ours to rewrite.
pub fn detect_at(body: &Value, target: &str) -> Wire {
    let path = target.split('?').next().unwrap_or("");
    if path.ends_with("/embeddings")
        || path.ends_with("/models")
        || path.contains("/count_tokens")
        || path.ends_with("/moderations")
    {
        return Wire::Unknown;
    }
    let by_body = detect(body);
    let marked = by_body == Wire::OpenAiResponses
        || body
            .get("messages")
            .and_then(Value::as_array)
            .is_some_and(|ms| {
                ms.iter().any(|m| {
                    m.get("role").and_then(Value::as_str) == Some("tool")
                        || m.get("content")
                            .and_then(Value::as_array)
                            .is_some_and(|bs| {
                                bs.iter().any(|b| {
                                    matches!(
                                        b.get("type").and_then(Value::as_str),
                                        Some("tool_result") | Some("tool_use")
                                    )
                                })
                            })
                })
            });
    if marked {
        by_body
    } else if path.ends_with("/messages") {
        Wire::Anthropic
    } else if path.ends_with("/chat/completions") {
        Wire::OpenAiChat
    } else {
        by_body
    }
}

/// Index of the last item an assistant produced, in a history.
fn last_assistant(items: &[Value], wire: Wire) -> Option<usize> {
    items.iter().rposition(|m| match wire {
        Wire::OpenAiResponses => {
            matches!(
                m.get("type").and_then(Value::as_str),
                Some("function_call") | Some("reasoning")
            ) || m.get("role").and_then(Value::as_str) == Some("assistant")
        }
        _ => m.get("role").and_then(Value::as_str) == Some("assistant"),
    })
}

pub fn detect(body: &Value) -> Wire {
    if body.get("input").and_then(Value::as_array).is_some() {
        // The Responses API carries a flat `input` array of typed items.
        return Wire::OpenAiResponses;
    }
    let Some(messages) = body.get("messages").and_then(Value::as_array) else {
        return Wire::Unknown;
    };
    // `role: "tool"` exists only in the OpenAI chat wire; Anthropic puts tool
    // results in a USER message as a content block. Look for either marker
    // anywhere in the history rather than at a fixed index — a conversation may
    // legitimately have none yet, which is the `Unknown`-free ambiguous case
    // resolved below.
    for m in messages {
        if m.get("role").and_then(Value::as_str) == Some("tool") {
            return Wire::OpenAiChat;
        }
        if let Some(blocks) = m.get("content").and_then(Value::as_array) {
            for b in blocks {
                match b.get("type").and_then(Value::as_str) {
                    Some("tool_result") => return Wire::Anthropic,
                    Some("tool_use") => return Wire::Anthropic,
                    _ => {}
                }
            }
        }
    }
    // A `messages` array with no tool traffic yet. Anthropic requires
    // `max_tokens`; the OpenAI chat wire does not. This only decides which
    // WALKER runs, and both walkers are no-ops on a history with no tool
    // results, so a wrong answer here costs nothing.
    if body.get("max_tokens").is_some() && body.get("system").is_some() {
        Wire::Anthropic
    } else {
        Wire::OpenAiChat
    }
}

/// What one rewrite pass did. Returned rather than logged so the caller decides
/// what to do with it — the proxy must not persist request bodies.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Rewrite {
    /// A system preamble was prepended.
    ///
    /// Separate from `folded` because it is the opposite operation — it makes
    /// the body longer — and because a body that gained ONLY a preamble is
    /// still a body we must send. `is_noop` therefore asks about both.
    ///
    /// It does not spoil prompt caching: the preamble is idempotent by content
    /// and the agent re-sends the same system prompt each turn, so the prefix
    /// the provider caches is byte-identical from the second turn on. It is a
    /// DIFFERENT prefix from the one the agent would have had, which costs one
    /// cache write on the first turn and nothing after.
    pub preambled: bool,
    /// Tool-result blocks examined.
    pub seen: usize,
    /// Blocks actually replaced with a shorter version.
    pub folded: usize,
    /// Bytes before, over the blocks that were folded.
    pub bytes_before: usize,
    /// Bytes after, over the same blocks.
    pub bytes_after: usize,
    /// Tool-result bytes sitting BEFORE the first block that was folded.
    ///
    /// Exists for [`super::cache_gate`], which needs to know how much of the
    /// provider's cached prefix a rewrite invalidates: everything from the
    /// first rewritten block to the end of the request. The caller derives that
    /// suffix as `total request bytes - bytes_before_first_fold`, which
    /// deliberately OVER-counts — the system prompt, the tool definitions and
    /// every non-tool-result message before the fold land in the suffix too.
    ///
    /// Over-counting the invalidated prefix makes a rewrite look more expensive
    /// than it is, so the gate declines more than strictly necessary. That is
    /// the one direction that is safe: a declined fold costs tokens we could
    /// have saved, an approved one that should not have been costs a cache
    /// write the user never asked for.
    pub bytes_before_first_fold: usize,
    /// True when at least one replacement scrubbed a credential.
    ///
    /// Carried separately from `folded` because the two answer different
    /// questions and only one of them may be traded for bytes: a fold that does
    /// not shrink the body is pointless, a redaction that does not shrink it is
    /// the whole point.
    pub redacted: bool,
    /// One entry per folded block: (identity of the ORIGINAL text, bytes
    /// before, bytes after, new in this turn). The agent re-sends its whole
    /// history every call, so the same block is folded again on each one: a
    /// block is a NEW saving only when it arrived after the last assistant
    /// message (and was never credited before); every other is a re-read the
    /// fold spared. Positional, so a proxy restart — which empties the credit
    /// memory — cannot re-credit a whole history as fresh folds. The last
    /// field: the fold left a recovery pointer (an `expand-ref` offer).
    pub blocks: Vec<(u64, usize, usize, bool, bool)>,
    /// Set by the walker while it is past the last assistant message.
    pub(crate) in_tail: bool,
    /// The request's `model`, when the body names one.
    pub model: Option<String>,
}

/// Below this, a repeated tool result costs less than the pointer explaining it.
pub const DEDUP_MIN_BYTES: usize = 512;

/// Wrap a fold so that a tool result IDENTICAL to an earlier one in the same
/// request goes out as a pointer to it: the agent re-read an unchanged file or
/// re-ran a command with the same output, and the model already has it. The
/// pointer names the earlier result by its position among the tool results,
/// which the history keeps stable from one call to the next, so the prefix the
/// provider caches stays byte-identical.
pub fn dedup_identical<'a, F>(
    enabled: bool,
    fold: &'a mut F,
) -> impl FnMut(&str, Option<&str>) -> Option<String> + 'a
where
    F: FnMut(&str, Option<&str>) -> Option<String>,
{
    let mut first: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    let mut n = 0usize;
    move |s: &str, h: Option<&str>| {
        n += 1;
        let (head, body) = split_run_header(s);
        if enabled && body.len() >= DEDUP_MIN_BYTES {
            let id = block_identity(body);
            if let Some(at) = first.get(&id) {
                return Some(format!(
                    "{head}[tokenade: output identical to tool result #{at} above ({} bytes) — not repeated]",
                    body.len()
                ));
            }
            first.insert(id, n);
        }
        fold(s, h)
    }
}

/// Codex prefixes every command's output with lines that change on each run
/// (`Chunk ID: 4ed39d`, `Wall time: 0.0000 seconds`, the exit code, a token
/// count) and then `Output:` — measured on codex-cli 0.147, 2026-10-06. Two
/// reads of the same unchanged file never compare equal as a whole, so the
/// comparison is made on what follows, and the header is kept as it was.
fn split_run_header(s: &str) -> (&str, &str) {
    const KEYS: [&str; 5] = [
        "Chunk ID:",
        "Wall time:",
        "Process exited",
        "Exit code:",
        "Original token count:",
    ];
    let Some(at) = s.find("Output:\n") else {
        return ("", s);
    };
    let head = &s[..at + "Output:\n".len()];
    let known = s[..at]
        .lines()
        .all(|l| l.is_empty() || KEYS.iter().any(|k| l.starts_with(k)));
    if known && !s[..at].trim().is_empty() {
        (head, &s[head.len()..])
    } else {
        ("", s)
    }
}

/// The identity of a block, for [`Rewrite::blocks`].
pub fn block_identity(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

impl Rewrite {
    /// True when nothing changed, so the caller can relay the ORIGINAL bytes
    /// instead of a re-serialised body. Re-serialising is not free and is not
    /// neutral: key order and number formatting can change, and a provider that
    /// hashes the prefix for its cache would see a different request.
    pub fn is_noop(&self) -> bool {
        // A scrub is a change: a credential left in a cached block still goes.
        self.folded == 0 && !self.preambled && !self.redacted
    }
}

/// Rewrite every tool-result payload in `body`, in place.
///
/// `fold` is the compactor, injected rather than called directly so this module
/// stays a pure transformation over JSON and can be tested without a binary,
/// a network, or a HOME. It must return `None` when it has nothing better to
/// offer — the caller then keeps the original text.
/// The COMMAND that produced each result, when the payload carries it.
///
/// tokenade picks its compactor by the command name — that is the whole point
/// of the `--cmd` hint the bridges already pass. The wire carries it too: a
/// `tool_result` refers to its `tool_use`, whose `input` contains the
/// command. Nobody read it, and the proxy folded blind: measured 2026-08-21,
/// the same listing yields 0.47 without a hint and a fraction of that with
/// one.
fn command_hints(body: &Value, wire: Wire) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let take = |v: &Value| -> Option<String> {
        for k in [
            "command",
            "cmd",
            "script",
            "query",
            "pattern",
            "file_path",
            "path",
        ] {
            if let Some(s) = v.get(k).and_then(Value::as_str) {
                if !s.trim().is_empty() {
                    return Some(s.chars().take(400).collect());
                }
            }
            // Codex's `shell` tool: `"command": ["bash", "-lc", "git status"]`.
            if let Some(argv) = v.get(k).and_then(Value::as_array) {
                let parts: Vec<&str> = argv.iter().filter_map(Value::as_str).collect();
                let line = match parts.as_slice() {
                    [sh, flag, script]
                        if flag.starts_with('-') && flag.contains('c') && sh.ends_with("sh") =>
                    {
                        script.to_string()
                    }
                    _ => parts.join(" "),
                };
                if !line.trim().is_empty() {
                    return Some(line.chars().take(400).collect());
                }
            }
        }
        None
    };
    match wire {
        Wire::Anthropic => {
            for m in body
                .get("messages")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                for b in m
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if b.get("type").and_then(Value::as_str) != Some("tool_use") {
                        continue;
                    }
                    let (Some(id), Some(input)) =
                        (b.get("id").and_then(Value::as_str), b.get("input"))
                    else {
                        continue;
                    };
                    // A tool name alone is still a useful hint when the input
                    // carries no recognised field.
                    let hint = take(input)
                        .or_else(|| b.get("name").and_then(Value::as_str).map(str::to_string));
                    if let Some(h) = hint {
                        out.insert(id.to_string(), h);
                    }
                }
            }
        }
        Wire::OpenAiResponses => {
            for it in body
                .get("input")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let args = match it.get("type").and_then(Value::as_str) {
                    Some("function_call") => it
                        .get("arguments")
                        .and_then(Value::as_str)
                        .and_then(|s| serde_json::from_str::<Value>(s).ok()),
                    Some("custom_tool_call") => it
                        .get("input")
                        .and_then(Value::as_str)
                        .map(|s| serde_json::json!({ "command": s })),
                    _ => continue,
                };
                let Some(id) = it.get("call_id").and_then(Value::as_str) else {
                    continue;
                };
                let hint = args
                    .as_ref()
                    .and_then(take)
                    .or_else(|| it.get("name").and_then(Value::as_str).map(str::to_string));
                if let Some(h) = hint {
                    out.insert(id.to_string(), h);
                }
            }
        }
        Wire::OpenAiChat => {
            for m in body
                .get("messages")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                for c in m
                    .get("tool_calls")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let Some(id) = c.get("id").and_then(Value::as_str) else {
                        continue;
                    };
                    let args = c.get("function").and_then(|f| f.get("arguments"));
                    let parsed = args
                        .and_then(Value::as_str)
                        .and_then(|s| serde_json::from_str::<Value>(s).ok());
                    let hint = parsed.as_ref().and_then(take).or_else(|| {
                        c.get("function")
                            .and_then(|f| f.get("name"))
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    });
                    if let Some(h) = hint {
                        out.insert(id.to_string(), h);
                    }
                }
            }
        }
        _ => {}
    }
    out
}

#[cfg(test)]
pub fn rewrite_tool_results<F>(body: &mut Value, wire: Wire, fold: F) -> Rewrite
where
    F: FnMut(&str, Option<&str>) -> Option<String>,
{
    rewrite_tool_results_settled(body, wire, fold, &|_| true)
}

/// [`rewrite_tool_results`], folding a block BEFORE the last assistant
/// message only when `settled(identity)` — the block went out folded before,
/// so the provider's cached prefix already holds the folded bytes. A block
/// sent raw on an earlier call sits raw in that prefix: folding it now would
/// make the provider write the rest of the history to cache again. It is only
/// scrubbed of credentials.
pub fn rewrite_tool_results_settled<F>(
    body: &mut Value,
    wire: Wire,
    mut fold: F,
    settled: &dyn Fn(u64) -> bool,
) -> Rewrite
where
    F: FnMut(&str, Option<&str>) -> Option<String>,
{
    let hints = command_hints(body, wire);
    let mut r = Rewrite::default();
    match wire {
        Wire::Unknown => {}
        Wire::OpenAiResponses => {
            if let Some(items) = body.get_mut("input").and_then(Value::as_array_mut) {
                let last = last_assistant(items, wire);
                for (i, it) in items.iter_mut().enumerate() {
                    r.in_tail = last.is_none_or(|l| i > l);
                    // `custom_tool_call_output` is Codex's `apply_patch` and
                    // other freeform tools: tool output all the same.
                    if !matches!(
                        it.get("type").and_then(Value::as_str),
                        Some("function_call_output") | Some("custom_tool_call_output")
                    ) {
                        continue;
                    }
                    let hint = it
                        .get("call_id")
                        .and_then(Value::as_str)
                        .and_then(|id| hints.get(id))
                        .cloned();
                    if let Some(out) = it.get_mut("output") {
                        fold_text_or_parts(out, hint.as_deref(), &mut fold, &mut r, settled);
                    }
                }
            }
        }
        Wire::OpenAiChat => {
            if let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) {
                let last = last_assistant(messages, wire);
                for (i, m) in messages.iter_mut().enumerate() {
                    r.in_tail = last.is_none_or(|l| i > l);
                    if m.get("role").and_then(Value::as_str) != Some("tool") {
                        continue;
                    }
                    let hint = m
                        .get("tool_call_id")
                        .and_then(Value::as_str)
                        .and_then(|id| hints.get(id))
                        .cloned();
                    if let Some(c) = m.get_mut("content") {
                        fold_text_or_parts(c, hint.as_deref(), &mut fold, &mut r, settled);
                    }
                }
            }
        }
        Wire::Anthropic => {
            if let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) {
                let last = last_assistant(messages, wire);
                for (i, m) in messages.iter_mut().enumerate() {
                    r.in_tail = last.is_none_or(|l| i > l);
                    let Some(blocks) = m.get_mut("content").and_then(Value::as_array_mut) else {
                        continue;
                    };
                    for b in blocks {
                        if b.get("type").and_then(Value::as_str) != Some("tool_result") {
                            continue;
                        }
                        // Anthropic's `content` is either a string or a list of
                        // blocks. Both shapes are real and both carry tool
                        // output; handling only the string one would silently
                        // skip every structured result.
                        let hint = b
                            .get("tool_use_id")
                            .and_then(Value::as_str)
                            .and_then(|id| hints.get(id))
                            .cloned();
                        match b.get_mut("content") {
                            Some(Value::String(_)) => {
                                fold_string_field(
                                    b.get_mut("content").unwrap(),
                                    hint.as_deref(),
                                    &mut fold,
                                    &mut r,
                                    settled,
                                );
                            }
                            Some(Value::Array(inner)) => {
                                for ib in inner {
                                    if ib.get("type").and_then(Value::as_str) != Some("text") {
                                        // An image block is not text and must
                                        // not be handed to a text compactor.
                                        continue;
                                    }
                                    if let Some(t) = ib.get_mut("text") {
                                        fold_string_field(
                                            t,
                                            hint.as_deref(),
                                            &mut fold,
                                            &mut r,
                                            settled,
                                        );
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }
    r
}

/// A tool output that is either a string or a list of text parts
/// (`[{"type":"text"|"input_text"|"output_text","text":…}]`). Only the string
/// shape was handled: a list went out unfolded and unscrubbed.
fn fold_text_or_parts<F>(
    v: &mut Value,
    hint: Option<&str>,
    fold: &mut F,
    r: &mut Rewrite,
    settled: &dyn Fn(u64) -> bool,
) where
    F: FnMut(&str, Option<&str>) -> Option<String>,
{
    match v {
        Value::String(_) => fold_string_field(v, hint, fold, r, settled),
        Value::Array(parts) => {
            for p in parts {
                if matches!(
                    p.get("type").and_then(Value::as_str),
                    Some("text") | Some("input_text") | Some("output_text")
                ) {
                    if let Some(t) = p.get_mut("text") {
                        fold_string_field(t, hint, fold, r, settled);
                    }
                }
            }
        }
        _ => {}
    }
}

/// Fold one JSON string field in place, counting what happened.
fn fold_string_field<F>(
    v: &mut Value,
    hint: Option<&str>,
    fold: &mut F,
    r: &mut Rewrite,
    settled: &dyn Fn(u64) -> bool,
) where
    F: FnMut(&str, Option<&str>) -> Option<String>,
{
    let Some(s) = v.as_str() else { return };
    r.seen += 1;
    let before = s.len();
    // Everything examined while nothing has folded yet is still inside the
    // cached prefix the provider will keep. Counted here, on every path that
    // leaves the block alone, so the gate can price the suffix.
    let untouched = |r: &mut Rewrite| {
        if r.folded == 0 {
            r.bytes_before_first_fold += before;
        }
    };
    // Raw in the provider's cache: left as sent, credentials aside.
    if !r.in_tail && !settled(block_identity(s)) {
        let clean = tokenade_core::redact::redact_for_model(s);
        if clean == s {
            untouched(r);
            return;
        }
        r.redacted = true;
        *v = Value::String(clean);
        return;
    }
    let Some(next) = fold(s, hint) else {
        untouched(r);
        return;
    };
    // Never accept a REWRITE that grows the history: the whole point is to
    // shrink what gets re-sent, and a longer block also costs a cache write.
    //
    // Except when the growth IS the safety. `API_KEY=AKIA…` becoming
    // `API_KEY=<redacted>` is longer than what it replaced, and a length-only
    // rule drops the safe version and forwards the key. Asked of the ORIGINAL
    // rather than pattern-matched on the replacement: "did this block carry a
    // credential" is a question `redact` can answer exactly, where "does the
    // output look redacted" is a guess about our own formatting.
    if next.len() >= before {
        let carried_a_secret = tokenade_core::redact::redact_for_model(s) != s;
        if !carried_a_secret {
            untouched(r);
            return;
        }
        r.redacted = true;
    }
    r.folded += 1;
    r.bytes_before += before;
    r.bytes_after += next.len();
    let offered = !tokenade_core::fold_marker::already_folded(s)
        && tokenade_core::fold_marker::already_folded(&next);
    r.blocks
        .push((block_identity(s), before, next.len(), r.in_tail, offered));
    if !r.redacted && tokenade_core::redact::redact_for_model(s) != s {
        r.redacted = true;
    }
    *v = Value::String(next);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A compactor stand-in: halves anything long enough to be worth it.
    fn halve(s: &str, _hint: Option<&str>) -> Option<String> {
        (s.len() > 20).then(|| s[..s.len() / 2].to_string())
    }

    #[test]
    fn detects_each_wire_from_its_shape_not_its_url() {
        let anthropic = json!({"messages":[{"role":"user","content":[
            {"type":"tool_result","tool_use_id":"t1","content":"x"}]}]});
        assert_eq!(detect(&anthropic), Wire::Anthropic);

        let openai = json!({"messages":[
            {"role":"assistant","content":null},
            {"role":"tool","tool_call_id":"c1","content":"x"}]});
        assert_eq!(detect(&openai), Wire::OpenAiChat);

        let responses = json!({"input":[{"type":"function_call_output","output":"x"}]});
        assert_eq!(detect(&responses), Wire::OpenAiResponses);

        // Not a chat request at all.
        assert_eq!(detect(&json!({"prompt":"hello"})), Wire::Unknown);
    }

    #[test]
    fn an_unknown_wire_is_never_touched() {
        // The rule that keeps the proxy safe: a body we did not recognise is
        // relayed byte-for-byte. If this ever rewrites something, the proxy has
        // started guessing.
        let mut body = json!({"prompt":"a very long tool output ".repeat(50)});
        let before = body.clone();
        let r = rewrite_tool_results(&mut body, Wire::Unknown, halve);
        assert!(r.is_noop());
        assert_eq!(body, before);
    }

    #[test]
    fn folds_tool_results_and_leaves_everything_else_alone() {
        let long = "tool output line that is definitely long enough".to_string();
        let mut body = json!({
            "system": "you are helpful",
            "messages": [
                {"role":"user","content":"question that is also quite long here"},
                {"role":"user","content":[
                    {"type":"tool_result","tool_use_id":"t1","content": long},
                    {"type":"text","text":"a user comment long enough to fold too"}
                ]}
            ]
        });
        let r = rewrite_tool_results(&mut body, Wire::Anthropic, halve);
        assert_eq!((r.seen, r.folded), (1, 1));
        // The user's own message and the sibling text block are untouched:
        // the proxy compacts tool OUTPUT, not the conversation.
        assert_eq!(
            body["messages"][0]["content"],
            json!("question that is also quite long here")
        );
        assert_eq!(
            body["messages"][1]["content"][1]["text"],
            json!("a user comment long enough to fold too")
        );
        assert!(
            body["messages"][1]["content"][0]["content"]
                .as_str()
                .unwrap()
                .len()
                < long.len()
        );
    }

    #[test]
    fn anthropic_structured_tool_results_are_folded_too() {
        // `content` as a LIST of blocks is as real as the string form. Handling
        // only the string would skip every structured result, silently.
        let mut body = json!({"messages":[{"role":"user","content":[
        {"type":"tool_result","tool_use_id":"t1","content":[
            {"type":"text","text":"a long structured tool output block here"},
            {"type":"image","source":{"data":"AAAA"}}
        ]}]}]});
        let r = rewrite_tool_results(&mut body, Wire::Anthropic, halve);
        assert_eq!((r.seen, r.folded), (1, 1));
        // The image block is not text and must never reach a text compactor.
        assert_eq!(
            body["messages"][0]["content"][0]["content"][1]["source"]["data"],
            json!("AAAA")
        );
    }

    #[test]
    fn openai_chat_and_responses_shapes_both_fold() {
        let mut chat = json!({"messages":[
            {"role":"tool","tool_call_id":"c1","content":"a long tool result for the chat wire"}]});
        assert_eq!(
            rewrite_tool_results(&mut chat, Wire::OpenAiChat, halve).folded,
            1
        );

        let mut resp = json!({"input":[
            {"type":"function_call_output","call_id":"c1","output":"a long tool result again"},
            {"type":"message","content":"a long assistant message not to touch"}]});
        let r = rewrite_tool_results(&mut resp, Wire::OpenAiResponses, halve);
        assert_eq!((r.seen, r.folded), (1, 1));
        assert_eq!(
            resp["input"][1]["content"],
            json!("a long assistant message not to touch")
        );
    }

    #[test]
    fn a_fold_that_grows_the_payload_is_refused() {
        // Redaction can make text LONGER, and the bridges accept that because a
        // scrubbed secret is worth the bytes. Here it is not the same trade:
        // the block is being RE-SENT, and a longer one also invalidates the
        // provider's cached prefix. So the rule differs on purpose.
        let mut body = json!({"messages":[{"role":"tool","content":"short"}]});
        let r = rewrite_tool_results(&mut body, Wire::OpenAiChat, |s, _h| {
            Some(format!("{s} plus a great deal more text than before"))
        });
        assert!(r.is_noop());
        assert_eq!(body["messages"][0]["content"], json!("short"));
    }
}

/// Put a preamble at the FRONT of the request's system prompt.
///
/// The one thing rank 4 can do that has nothing to do with folding: change
/// what the model is told before it decides anything. Rank 1 does it with a
/// prompt-time hook, and eleven agents on the roster have no such hook — their
/// `sobriety` cell reads `Partial bin` for exactly that reason, with the
/// preamble sitting in a file nothing reads at the right moment.
///
/// Three shapes, because the three wires spell the system prompt differently:
///
///   * Anthropic — a top-level `system`, either a string or an array of text
///     blocks. Both forms occur in the wild and both are handled.
///   * OpenAI chat — a first message with `role: "system"` (or `"developer"`,
///     which the newer models take instead).
///   * OpenAI Responses — a top-level `instructions` string.
///
/// Prepended rather than appended: the agent's own prompt usually ends with
/// task-specific instructions, and a line about which CLI to prefer belongs
/// with the standing rules at the top.
///
/// Idempotent by content. The proxy sees every turn of a conversation and the
/// system prompt is re-sent whole each time, so appending blindly would grow
/// it without bound — measured on a fifty-turn session, that is fifty copies.
/// Returns `true` only when the body actually changed.
pub fn prepend_system_preamble(body: &mut Value, wire: Wire, preamble: &str) -> bool {
    if preamble.trim().is_empty() {
        return false;
    }
    // The double-declaration guard, and it looks at the WHOLE request rather
    // than at the system prompt alone.
    //
    // A rank-1 prompt hook does not necessarily put the preamble where we
    // would: Claude Code's `UserPromptSubmit` returns
    // `hookSpecificOutput.additionalContext`, which the agent folds into the
    // CONVERSATION, not into `system`. Checking only the field we write would
    // miss that and send the model the same instructions twice, in two
    // different places — the positive-gain-counted-twice failure, wearing the
    // shape of a prompt instead of a tool result.
    //
    // So the question is "is this text already in this request", exactly as
    // the already-folded rule asks "has anyone already folded this payload". Both are
    // content questions and neither is a question about which agent is in
    // front, which is the classification that was wrong twice today.
    let needle = preamble.trim();
    // The serialised body is JSON-ESCAPED: a multi-line preamble (`\n`,
    // quotes) never matched its raw text there, and a copy a hook injected
    // elsewhere in the conversation got a second one. Searched escaped, by its
    // first line — what any copy, however re-wrapped, starts with.
    let escaped = |s: &str| {
        serde_json::to_string(s)
            .map(|q| q[1..q.len() - 1].to_string())
            .unwrap_or_default()
    };
    let head = needle
        .lines()
        .map(str::trim)
        .find(|l| l.len() >= 20)
        .unwrap_or(needle);
    if serde_json::to_string(&*body)
        .map(|whole| whole.contains(&escaped(needle)) || whole.contains(&escaped(head)))
        .unwrap_or(false)
    {
        return false;
    }
    let already = |s: &str| s.contains(needle);
    match wire {
        Wire::Anthropic => match body.get_mut("system") {
            Some(Value::String(s)) => {
                if already(s) {
                    return false;
                }
                *s = format!("{preamble}\n\n{s}");
                true
            }
            Some(Value::Array(blocks)) => {
                let joined: String = blocks
                    .iter()
                    .filter_map(|b| b.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n");
                if already(&joined) {
                    return false;
                }
                blocks.insert(0, serde_json::json!({ "type": "text", "text": preamble }));
                true
            }
            // No system prompt at all is still a request we can answer: give it
            // one. An agent that sends none is the case where the preamble has
            // the most room to matter.
            _ => {
                body["system"] = Value::String(preamble.to_string());
                true
            }
        },
        Wire::OpenAiChat => {
            let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) else {
                return false;
            };
            let first_is_system = messages
                .first()
                .and_then(|m| m.get("role"))
                .and_then(Value::as_str)
                .is_some_and(|r| r == "system" || r == "developer");
            if first_is_system {
                let m = &mut messages[0];
                match m.get_mut("content") {
                    Some(Value::String(s)) => {
                        if already(s) {
                            return false;
                        }
                        *s = format!("{preamble}\n\n{s}");
                        true
                    }
                    // The array form of a system message, same as Anthropic's.
                    Some(Value::Array(parts)) => {
                        let joined: String = parts
                            .iter()
                            .filter_map(|b| b.get("text").and_then(Value::as_str))
                            .collect::<Vec<_>>()
                            .join("\n");
                        if already(&joined) {
                            return false;
                        }
                        parts.insert(0, serde_json::json!({ "type": "text", "text": preamble }));
                        true
                    }
                    _ => false,
                }
            } else {
                messages.insert(
                    0,
                    serde_json::json!({ "role": "system", "content": preamble }),
                );
                true
            }
        }
        Wire::OpenAiResponses => match body.get_mut("instructions") {
            Some(Value::String(s)) => {
                if already(s) {
                    return false;
                }
                *s = format!("{preamble}\n\n{s}");
                true
            }
            _ => {
                body["instructions"] = Value::String(preamble.to_string());
                true
            }
        },
        // An unknown shape has no system prompt we can name, and
        // inventing a field in somebody else's schema is how a proxy breaks a
        // request it was only supposed to relay.
        Wire::Unknown => false,
    }
}

#[cfg(test)]
mod preamble_tests {
    use super::*;

    const P: &str = "Prefer `tokenade exec-compact` for noisy commands.";

    #[test]
    fn every_wire_we_recognise_can_carry_a_preamble() {
        // The positive half, one shape per wire, so a fourth wire cannot be
        // added without answering the question.
        let mut a = serde_json::json!({"system": "be helpful", "messages": []});
        assert!(prepend_system_preamble(&mut a, Wire::Anthropic, P));
        assert!(a["system"].as_str().unwrap().starts_with(P));
        assert!(a["system"].as_str().unwrap().contains("be helpful"));

        let mut b = serde_json::json!({"messages": [{"role": "system", "content": "be helpful"}]});
        assert!(prepend_system_preamble(&mut b, Wire::OpenAiChat, P));
        assert!(b["messages"][0]["content"].as_str().unwrap().starts_with(P));

        let mut c = serde_json::json!({"instructions": "be helpful", "input": []});
        assert!(prepend_system_preamble(&mut c, Wire::OpenAiResponses, P));
        assert!(c["instructions"].as_str().unwrap().starts_with(P));
    }

    #[test]
    fn a_request_with_no_system_prompt_gets_one() {
        let mut a = serde_json::json!({"messages": []});
        assert!(prepend_system_preamble(&mut a, Wire::Anthropic, P));
        assert_eq!(a["system"], P);

        let mut b = serde_json::json!({"messages": [{"role": "user", "content": "hi"}]});
        assert!(prepend_system_preamble(&mut b, Wire::OpenAiChat, P));
        assert_eq!(b["messages"][0]["role"], "system");
        assert_eq!(b["messages"][1]["role"], "user");
    }

    #[test]
    fn the_array_form_of_a_system_prompt_is_not_flattened() {
        // Anthropic's array form carries cache_control on individual blocks.
        // Rewriting it into one string would drop those and silently disable
        // prompt caching, which costs far more than the preamble saves.
        let mut a = serde_json::json!({"system": [
            {"type": "text", "text": "be helpful", "cache_control": {"type": "ephemeral"}}
        ]});
        assert!(prepend_system_preamble(&mut a, Wire::Anthropic, P));
        let blocks = a["system"].as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["text"], P);
        assert_eq!(blocks[1]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn a_second_turn_does_not_stack_a_second_copy() {
        // The proxy sees every turn and the system prompt is re-sent whole.
        // Measured against a fifty-turn session, a blind append is fifty
        // copies of the preamble and a prompt cache that never hits.
        for wire in [Wire::Anthropic, Wire::OpenAiChat, Wire::OpenAiResponses] {
            let mut body = match wire {
                Wire::Anthropic => serde_json::json!({"system": "be helpful"}),
                Wire::OpenAiChat => {
                    serde_json::json!({"messages": [{"role": "system", "content": "be helpful"}]})
                }
                _ => serde_json::json!({"instructions": "be helpful"}),
            };
            assert!(prepend_system_preamble(&mut body, wire, P), "{wire:?}");
            let after_first = body.clone();
            assert!(
                !prepend_system_preamble(&mut body, wire, P),
                "{wire:?} stacked"
            );
            assert_eq!(body, after_first, "{wire:?} changed on the second call");
        }
    }

    #[test]
    fn the_path_settles_an_ambiguous_body_and_never_overrides_a_marked_one() {
        let bare: Value =
            serde_json::from_str(r#"{"max_tokens":5,"messages":[{"role":"user","content":"hi"}]}"#)
                .unwrap();
        assert_eq!(
            detect_at(&bare, "/v1/messages"),
            Wire::Anthropic,
            "no `system` yet: still Anthropic"
        );
        assert_eq!(detect_at(&bare, "/v1/chat/completions"), Wire::OpenAiChat);
        let emb: Value = serde_json::from_str(r#"{"model":"m","input":["a","b"]}"#).unwrap();
        assert_eq!(
            detect_at(&emb, "/v1/embeddings"),
            Wire::Unknown,
            "not a generation endpoint"
        );
        let chat: Value =
            serde_json::from_str(r#"{"messages":[{"role":"tool","content":"x"}]}"#).unwrap();
        assert_eq!(
            detect_at(&chat, "/v1/messages"),
            Wire::OpenAiChat,
            "the body's own marker wins"
        );
    }

    /// A block is a NEW fold only past the last assistant message; earlier
    /// ones are re-sends — whatever the proxy remembers (a restart forgets).
    #[test]
    fn only_a_block_after_the_last_answer_is_new() {
        let mut body: Value = serde_json::from_str(&format!(
            r#"{{"messages":[{{"role":"tool","content":"{a}"}},{{"role":"assistant","content":"k"}},{{"role":"tool","content":"{b}"}}]}}"#,
            a = "a".repeat(100),
            b = "b".repeat(100)
        ))
        .unwrap();
        let r = rewrite_tool_results(&mut body, Wire::OpenAiChat, |s: &str, _: Option<&str>| {
            Some(s[..10].to_string())
        });
        assert_eq!(
            r.blocks.iter().map(|b| b.3).collect::<Vec<_>>(),
            vec![false, true]
        );
    }

    /// Codex's shapes: a `shell` call whose command is an argv, its output as
    /// a list of parts, and a freeform `custom_tool_call_output`; and a chat
    /// tool message whose content is a list of parts. All were sent unfolded.
    #[test]
    fn every_tool_output_shape_is_folded_with_its_command() {
        let big = "x".repeat(200);
        let mut body: Value = serde_json::from_str(&format!(
            r#"{{"input":[{{"type":"function_call","call_id":"c1","name":"shell","arguments":"{{\"command\":[\"bash\",\"-lc\",\"git status\"]}}"}},{{"type":"function_call_output","call_id":"c1","output":[{{"type":"input_text","text":"{big}"}}]}},{{"type":"custom_tool_call_output","call_id":"c2","output":"{big}"}}]}}"#
        ))
        .unwrap();
        let mut seen = Vec::new();
        let r = rewrite_tool_results(
            &mut body,
            Wire::OpenAiResponses,
            |s: &str, h: Option<&str>| {
                seen.push(h.map(str::to_string));
                Some(s[..10].to_string())
            },
        );
        assert_eq!(r.folded, 2);
        assert_eq!(seen[0].as_deref(), Some("git status"));
        let mut chat: Value = serde_json::from_str(&format!(r#"{{"messages":[{{"role":"tool","tool_call_id":"t","content":[{{"type":"text","text":"{big}"}}]}}]}}"#)).unwrap();
        assert_eq!(
            rewrite_tool_results(
                &mut chat,
                Wire::OpenAiChat,
                |s: &str, _: Option<&str>| Some(s[..10].to_string())
            )
            .folded,
            1
        );
    }

    /// A multi-line preamble a hook already put in a user message: found in
    /// the escaped body, not added twice.
    #[test]
    fn a_multi_line_preamble_already_injected_is_found() {
        let pre = "Style: answer tersely and \"directly\".\nNo recaps.";
        let mut body = serde_json::json!({"system":"sys","max_tokens":5,"messages":[{"role":"user","content":format!("{pre}\n\nhello")}]});
        assert!(!prepend_system_preamble(&mut body, Wire::Anthropic, pre));
    }

    #[test]
    fn an_unknown_wire_is_never_given_a_field_we_invented() {
        // We do not know where this schema keeps its instructions, and
        // guessing writes a key the provider will reject or ignore.
        let mut u = serde_json::json!({"prompt": "hello", "opts": {}});
        let before = u.clone();
        assert!(!prepend_system_preamble(&mut u, Wire::Unknown, P));
        assert_eq!(u, before);
    }

    #[test]
    fn an_empty_preamble_is_not_a_rewrite() {
        let mut a = serde_json::json!({"system": "be helpful"});
        assert!(!prepend_system_preamble(&mut a, Wire::Anthropic, "   "));
        assert_eq!(a["system"], "be helpful");
    }
}

#[cfg(test)]
mod preamble_no_double_declaration {
    use super::*;

    const P: &str = "[tokenade:style=coding] Multi-tool coding workflow.";

    #[test]
    fn a_preamble_a_rank_one_hook_put_in_the_conversation_is_not_added_again() {
        // Claude Code's `UserPromptSubmit` hands back `additionalContext`, and
        // the agent folds that into a message — not into `system`. A guard
        // that looked only at `system` would inject a second copy and the
        // model would read the same standing instructions twice.
        let mut body = serde_json::json!({
            "system": "you are helpful",
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": format!("{P}\n\nnow do the thing")}]}
            ]
        });
        let before = body.clone();
        assert!(
            !prepend_system_preamble(&mut body, Wire::Anthropic, P),
            "the preamble is already in the conversation"
        );
        assert_eq!(body, before);
    }

    #[test]
    fn a_request_that_does_not_carry_it_yet_gets_it_exactly_once() {
        let mut body = serde_json::json!({
            "system": "you are helpful",
            "messages": [{"role": "user", "content": "hello"}]
        });
        assert!(prepend_system_preamble(&mut body, Wire::Anthropic, P));
        let whole = serde_json::to_string(&body).unwrap();
        assert_eq!(whole.matches("style=coding").count(), 1, "{whole}");
        // And a second pass over the SAME body adds nothing.
        assert!(!prepend_system_preamble(&mut body, Wire::Anthropic, P));
        let whole2 = serde_json::to_string(&body).unwrap();
        assert_eq!(whole2.matches("style=coding").count(), 1);
    }

    #[test]
    fn the_guard_reads_the_whole_body_on_every_wire() {
        for (wire, mut body) in [
            (
                Wire::OpenAiChat,
                serde_json::json!({"messages": [{"role": "user", "content": P}]}),
            ),
            (
                Wire::OpenAiResponses,
                serde_json::json!({"input": [{"type": "message", "content": P}]}),
            ),
        ] {
            let before = body.clone();
            assert!(!prepend_system_preamble(&mut body, wire, P), "{wire:?}");
            assert_eq!(body, before, "{wire:?}");
        }
    }

    /// A tool result repeated verbatim goes out once; the copy is a pointer
    /// naming the first by position, and the same history gives the same
    /// pointer on the next call. Short repeats and a disabled pass are left.
    #[test]
    fn an_identical_tool_result_is_sent_once() {
        let big = "line of a file that did not change\n".repeat(40);
        let body = |extra: &str| {
            serde_json::json!({"messages":[
            {"role":"tool","tool_call_id":"a","content": big},
            {"role":"tool","tool_call_id":"b","content":"ok"},
            {"role":"tool","tool_call_id":"c","content": big},
            {"role":"tool","tool_call_id":"d","content": extra}]})
        };
        let run = |mut b: Value, on: bool| {
            let mut keep = |_: &str, _: Option<&str>| None;
            let r = rewrite_tool_results(&mut b, Wire::OpenAiChat, dedup_identical(on, &mut keep));
            (b, r.folded)
        };
        let (out, folded) = run(body("ok"), true);
        assert_eq!(folded, 1);
        assert_eq!(out["messages"][0]["content"], big);
        let ptr = out["messages"][2]["content"].as_str().unwrap().to_string();
        assert!(ptr.contains("#1"), "{ptr}");
        assert_eq!(
            run(body("ok"), true).0["messages"][2]["content"],
            ptr.as_str()
        );
        assert_eq!(run(body("ok"), false).1, 0);
        assert_eq!(run(serde_json::json!({"messages":[{"role":"tool","content":"x"},{"role":"tool","content":"x"}]}), true).1, 0);
        // Codex's run header differs on every run; the output does not.
        let codex = |id: &str| {
            format!("Chunk ID: {id}\nWall time: 0.0001 seconds\nProcess exited with code 0\nOutput:\n{big}")
        };
        let (out, folded) = run(
            serde_json::json!({"messages":[
            {"role":"tool","content": codex("098941")},
            {"role":"tool","content": codex("4ed39d")}]}),
            true,
        );
        assert_eq!(folded, 1);
        let ptr = out["messages"][1]["content"].as_str().unwrap();
        assert!(
            ptr.starts_with("Chunk ID: 4ed39d\n") && ptr.contains("#1"),
            "{ptr}"
        );
    }
}
