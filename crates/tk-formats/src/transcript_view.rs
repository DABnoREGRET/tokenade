//! **The VIEW of a session transcript, not its accounting.**
//!
//! `ndjson::compact` already extracts 94 to 99 % of these files, but it does not render
//! the SHAPE of the conversation: the agent gets "148 JSON lines / 18
//! keys / first 3 lines" instead of "the user said X, the assistant
//! ran tools A, B, C". This module renders a readable and
//! tight transcript: one line per turn, tool calls inline, session
//! metadata said once.
//!
//! **Why it left `claudecode_transcript`.** That 1,450-line file
//! did two jobs: reading a transcript's usage (tokens per day,
//! models, session estimate — what ten product modules ask of it,
//! 22 sites) and FOLDING a transcript (what `auto` asks of it, 2 sites).
//! The first is product, the second is a format. As long as they
//! shared a module, the formats router reached up two layers
//! to fold a file — and that is what blocked extracting the
//! formats crate.
//!
//! The module's name says which of the two lives here: a VIEW, like
//! `read_view`. It names only `tokenizer`.

use std::fmt::Write;

/// Cheap structural detector — true if `text` looks like a Claude
/// Code session transcript (JSONL where the first record has both
/// `sessionId` and a `type` of "user"/"assistant"/"tool_result").
/// Generic ndjson detection passes too, so call this BEFORE the
/// ndjson path in `auto::detect`.
pub fn detect(text: &str) -> bool {
    // Look at the first non-empty line; bail on the first JSON
    // parse failure (an actual transcript is line-after-line valid).
    let first = text.lines().find(|l| !l.trim().is_empty());
    let Some(first) = first else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(first) else {
        return false;
    };
    let has_session = v.get("sessionId").and_then(|x| x.as_str()).is_some();
    let kind = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
    let is_known_kind = matches!(kind, "user" | "assistant" | "tool_result" | "system");
    has_session && is_known_kind
}

/// Produce a compact human-readable rendering of a Claude Code
/// session transcript. Output shape:
///
/// ```text
/// [claude-transcript] session=<id> cwd=<path> entries=<N>
/// model=<from first assistant turn, if any>
/// [1] user: <text preview>
/// [2] assistant: <text preview>
/// [3] tool: <name>(<arg=value, …>)
/// [4] tool_result: <preview>
/// ...
/// ```
///
/// Limits per turn: 240 chars on user/assistant text, 120 chars on
/// tool args, 200 chars on tool_result. Anything longer is suffixed
/// with `…[+N chars]`. Empty/missing fields are silently skipped.
pub fn compact(text: &str) -> String {
    let mut out = String::with_capacity(text.len() / 20);
    let mut session = String::new();
    let mut cwd = String::new();
    let mut model = String::new();
    let mut entries = Vec::<String>::new();
    let mut entry_count = 0usize;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        entry_count += 1;
        if session.is_empty() {
            session = v
                .get("sessionId")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
        }
        if cwd.is_empty() {
            cwd = v
                .get("cwd")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
        }
        if model.is_empty() {
            model = v
                .pointer("/message/model")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
        }

        let kind = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
        match kind {
            "user" => {
                // user.message.content can be a string OR a list of
                // blocks (tool_result items live here too).
                let content = v.pointer("/message/content");
                if let Some(s) = content.and_then(|c| c.as_str()) {
                    let preview = truncate(s, 240);
                    entries.push(format!("[{entry_count}] user: {preview}"));
                } else if let Some(arr) = content.and_then(|c| c.as_array()) {
                    for block in arr {
                        let bt = block.get("type").and_then(|x| x.as_str()).unwrap_or("");
                        match bt {
                            "tool_result" => {
                                let preview = render_tool_result(block);
                                entries.push(format!("[{entry_count}] tool_result: {preview}"));
                            }
                            "text" => {
                                let s = block.get("text").and_then(|x| x.as_str()).unwrap_or("");
                                entries.push(format!("[{entry_count}] user: {}", truncate(s, 240)));
                            }
                            _ => {}
                        }
                    }
                }
            }
            "assistant" => {
                let content = v.pointer("/message/content");
                let Some(arr) = content.and_then(|c| c.as_array()) else {
                    continue;
                };
                for block in arr {
                    let bt = block.get("type").and_then(|x| x.as_str()).unwrap_or("");
                    match bt {
                        "text" => {
                            let s = block.get("text").and_then(|x| x.as_str()).unwrap_or("");
                            if !s.trim().is_empty() {
                                entries.push(format!(
                                    "[{entry_count}] assistant: {}",
                                    truncate(s, 240)
                                ));
                            }
                        }
                        "tool_use" => {
                            let name = block.get("name").and_then(|x| x.as_str()).unwrap_or("?");
                            let args = render_tool_input(block.get("input"));
                            entries.push(format!("[{entry_count}] tool: {name}({args})"));
                        }
                        _ => {}
                    }
                }
            }
            "tool_result" => {
                let preview = render_tool_result(&v);
                entries.push(format!("[{entry_count}] tool_result: {preview}"));
            }
            _ => {
                // system / unknown — drop
            }
        }
    }

    out.push_str("[claude-transcript]");
    if !session.is_empty() {
        let _ = write!(out, " session={}", short_id(&session));
    }
    if !cwd.is_empty() {
        let _ = write!(out, " cwd={cwd}");
    }
    let _ = writeln!(out, " entries={entry_count}");
    if !model.is_empty() {
        let _ = writeln!(out, "model={model}");
    }
    for e in entries {
        out.push_str(&e);
        out.push('\n');
    }
    out
}

/// Variant returning `Savings` for the auto-compact ledger.
pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    let before = crate::tokenizer::estimate(text);
    let compact = compact(text);
    let after = crate::tokenizer::estimate(&compact);
    (compact, crate::Savings::new(before, after))
}

// ───── compact() helpers (private) ──────────────────────────────────

fn short_id(s: &str) -> String {
    // First 8 hex chars of a UUID — enough to dedup within a project.
    s.chars().take(8).collect()
}

fn truncate(s: &str, max_chars: usize) -> String {
    let len = s.chars().count();
    if len <= max_chars {
        // Compress runs of whitespace so multi-line messages don't
        // dominate the output.
        return s.split_whitespace().collect::<Vec<_>>().join(" ");
    }
    let head: String = s.chars().take(max_chars).collect();
    let head = head.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("{head}…[+{} chars]", len - max_chars)
}

fn render_tool_input(input: Option<&serde_json::Value>) -> String {
    let Some(input) = input else {
        return String::new();
    };
    let Some(obj) = input.as_object() else {
        let s = input.to_string();
        return truncate(&s, 120);
    };
    let mut parts = Vec::new();
    for (k, v) in obj {
        let val_preview = match v {
            serde_json::Value::String(s) => format!("\"{}\"", truncate(s, 60)),
            other => truncate(&other.to_string(), 60),
        };
        parts.push(format!("{k}={val_preview}"));
    }
    truncate(&parts.join(", "), 120)
}

fn render_tool_result(block: &serde_json::Value) -> String {
    // Could be: { content: "..." } OR { content: [ {type: "text", text: "..."} ] }
    // OR (for top-level tool_result entries) { toolUseResult: ... }
    let c = block
        .get("content")
        .or_else(|| block.get("toolUseResult"))
        .or_else(|| block.pointer("/message/content"));
    let s = match c {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(arr)) => {
            let texts: Vec<String> = arr
                .iter()
                .filter_map(|b| {
                    b.get("text")
                        .and_then(|t| t.as_str())
                        .map(|s| s.to_string())
                })
                .collect();
            texts.join(" ")
        }
        Some(v) => v.to_string(),
        None => String::new(),
    };
    head_and_tail_flat(&s, 200)
}

/// A tool result's verdict sits at its END — `test result: FAILED`, the
/// compiler's `error[E…]` count, a traceback's raising frame, an exit code —
/// so its one-line preview keeps both ends, half the budget each.
fn head_and_tail_flat(s: &str, max_chars: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let len = flat.chars().count();
    if len <= max_chars {
        return flat;
    }
    let half = max_chars / 2;
    let head: String = flat.chars().take(half).collect();
    let tail: String = flat.chars().skip(len - half).collect();
    format!(
        "{}…[+{} chars]…{}",
        head.trim_end(),
        len - 2 * half,
        tail.trim_start()
    )
}
#[cfg(test)]
include!("transcript_view_tests.rs");
