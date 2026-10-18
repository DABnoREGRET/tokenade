//! The one bounded stdin read every hook entry point goes through.
//!
//! There used to be two. `code_intel::read_hook_stdin_capped` served eight
//! entry points at a 1 MiB cap and, past it, returned `None` **in silence**;
//! `cmd_hookwrap` had its own copy at 32 MiB that logged both the stream error
//! and the oversize with the byte count. The channel carrying the LARGEST
//! payloads had the generous cap; the ones carrying a `Read`'s file contents or
//! a page of search results had the tight one, and a payload above it vanished
//! without a trace — the "a zero fallback makes a loss look like a non-use"
//! shape this repo has been bitten by before.
//!
//! Unifying kept the UNION of the two behaviours, not the poorer of them: one
//! reader, both caps, and every drop logged under the caller's own channel.
//! The two caps stay different — that difference is real — but they are now
//! named, adjacent, and each says what it is for.
//!
//! Distinct from `crate::bounded_input`, which bounds what an AGENT chose to
//! hand us (a path, a pipe) and REFUSES loudly above the cap. Here the drop
//! is silent for the command and loud for the log: a failing hook must be
//! transparent.

/// Cap for hook EVENT payloads: a JSON envelope describing what the agent is
/// about to do. Kilobytes in practice; a megabyte is already pathological.
pub(crate) const MAX_EVENT_STDIN_BYTES: usize = 1024 * 1024;

/// Cap for the PostToolUse channel, which carries a command's OUTPUT — the
/// thing tokenade exists to compact. A multi-megabyte build log or `grep -r`
/// is the normal case here, not an anomaly.
pub(crate) const MAX_OUTPUT_STDIN_BYTES: usize = 32 * 1024 * 1024;

/// Read stdin, bounded at `max`. `None` means "hand back no decision" — and
/// says why in the debug log, under `channel`, so a dropped payload is
/// countable instead of invisible.
pub(crate) fn read_capped(
    channel: &'static str,
    max: usize,
    started: std::time::Instant,
) -> Option<String> {
    use std::io::Read as _;
    let mut input = String::with_capacity(8192);
    // `+ 1` so an input of exactly `max` bytes is distinguishable from one that
    // was cut at the cap.
    let mut handle = std::io::stdin().take(max as u64 + 1);
    if handle.read_to_string(&mut input).is_err() {
        tokenade_core::debug_log::log(
            channel,
            "stdin_err",
            serde_json::json!({"elapsed_ms": started.elapsed().as_millis() as u64}),
        );
        return None;
    }
    if input.len() > max {
        tokenade_core::debug_log::log(
            channel,
            "oversized_passthrough",
            serde_json::json!({
                "input_bytes": input.len(),
                "cap_bytes": max,
                "elapsed_ms": started.elapsed().as_millis() as u64,
            }),
        );
        return None;
    }
    Some(input)
}

/// The common case: an event payload on `channel`, timed from now.
///
/// Exists so the seven event-channel callers do not each repeat the cap and an
/// `Instant::now()` — the two things none of them has an opinion about.
pub(crate) fn read_event(channel: &'static str) -> Option<String> {
    read_capped(channel, MAX_EVENT_STDIN_BYTES, std::time::Instant::now())
}

/// Parse a hook envelope and make it readable whatever case the host wrote it
/// in. Every entry point that reads an event payload goes through here, so the
/// answer to "does this handler see grok's fields?" is one call site, not
/// eleven — the same shape as when `fold_uniform_markers` had one caller and
/// the web route paid for it.
pub(crate) fn parse_event(input: &str) -> Option<serde_json::Value> {
    let mut v: serde_json::Value = serde_json::from_str(input).ok()?;
    normalise_envelope(&mut v);
    Some(v)
}

/// **Two spellings for one and the same envelope.**
///
/// grok delivers its hook payload in camelCase — proven on the installed
/// binary (1.0.0), whose field table lists `toolUseId`, `toolInput`,
/// `toolResult`, `hookEventName`, `sessionId`, `transcriptPath`,
/// `permissionMode`, and on its embedded documentation, whose `PreToolUse`
/// example is literally
/// `{"hookEventName":"pre_tool_use", …, "toolName":"run_terminal_command",
/// "toolInput":{"command":"npm test"}}`. Our handlers read `tool_name`,
/// `hook_event_name`, `tool_response`: on grok they find NOTHING and answer
/// that they have no opinion — indistinguishable, from the outside, from not
/// being wired at all. It is the same trap the Antigravity note already names
/// in `hooks/mod.rs`, on one more agent.
///
/// Normalisation only ADDS what is missing: a snake_case key already present
/// wins, so Claude, Codex and Droid do not change by a single byte. The two
/// agents that have their own dialect branch are not affected either:
/// Antigravity carries `toolCall.name` (not `toolName`), and Cursor already
/// writes snake_case.
///
/// What this revives on grok: the PASSIVE hooks already installed for it —
/// session brief, prompt nudges, write barrier before compaction. They all
/// return `additionalContext`, vocabulary grok declares it accepts as is. The
/// Bash path is deliberately NOT targeted: grok's `PreToolUse` output only
/// accepts `allow`/`deny`, so substituting there would mean refusing the
/// command, and its commands are already covered by the shell proxy.
pub(crate) fn normalise_envelope(v: &mut serde_json::Value) {
    /// `(camelCase as delivered, snake_case expected by our handlers)`.
    const ALIASES: [(&str, &str); 10] = [
        ("hookEventName", "hook_event_name"),
        // Reasonix names the event plainly `event` and the arguments
        // `toolArgs`. Read out of the shipped 1.8.1 binary's own hook-payload
        // table: `{"event":"PreToolUse","cwd":"/repo","toolName":"bash",
        // "toolArgs":{"command":"go test ./..."}}`. Our installer's doc comment
        // claimed a Claude-shaped `tool_name`/`tool_input` payload from a
        // 2026-07 repo read; the binary that ships today says otherwise.
        ("event", "hook_event_name"),
        ("toolName", "tool_name"),
        ("toolInput", "tool_input"),
        ("toolArgs", "tool_input"),
        ("toolResult", "tool_response"),
        ("toolUseId", "tool_use_id"),
        ("sessionId", "session_id"),
        ("transcriptPath", "transcript_path"),
        ("permissionMode", "permission_mode"),
    ];
    let Some(obj) = v.as_object_mut() else {
        return;
    };
    for (camel, snake) in ALIASES {
        if obj.contains_key(snake) {
            continue;
        }
        if let Some(val) = obj.get(camel).cloned() {
            obj.insert(snake.to_string(), val);
        }
    }
}

#[cfg(test)]
mod an_envelope_reads_in_both_spellings {
    use super::normalise_envelope;

    /// The payload grok DOCUMENTS, word for word.
    #[test]
    fn the_payload_documented_by_grok_becomes_readable() {
        let mut v = serde_json::json!({
            "hookEventName": "pre_tool_use",
            "sessionId": "abc-123",
            "cwd": "/Users/you/project",
            "workspaceRoot": "/Users/you/project",
            "permissionMode": "default",
            "toolName": "run_terminal_command",
            "toolInput": { "command": "npm test" },
            "timestamp": "2026-04-14T12:00:00Z"
        });
        normalise_envelope(&mut v);
        assert_eq!(v["tool_name"], "run_terminal_command");
        assert_eq!(v["tool_input"]["command"], "npm test");
        assert_eq!(v["hook_event_name"], "pre_tool_use");
        assert_eq!(v["session_id"], "abc-123");
        // The original form stays: we add, we do not move.
        assert_eq!(v["toolName"], "run_terminal_command");
    }

    /// The payload REASONIX documents, word for word — three more keys, two of
    /// which no other host writes (`event`, `toolArgs`).
    #[test]
    fn the_payload_documented_by_reasonix_becomes_readable() {
        let mut v = serde_json::json!({
            "event": "PreToolUse",
            "cwd": "/repo",
            "toolName": "bash",
            "toolArgs": { "command": "go test ./..." }
        });
        normalise_envelope(&mut v);
        assert_eq!(v["hook_event_name"], "PreToolUse");
        assert_eq!(v["tool_name"], "bash");
        assert_eq!(v["tool_input"]["command"], "go test ./...");
    }

    /// And its PostToolUse form, whose result is a STRING and not a
    /// `{stdout, stderr}` object: the renaming must pass it through as is, not
    /// invent it.
    #[test]
    fn the_reasonix_result_arrives_as_is() {
        let mut v = serde_json::json!({
            "event": "PostToolUse",
            "toolName": "bash",
            "toolArgs": { "command": "go test ./..." },
            "toolResult": "ok"
        });
        normalise_envelope(&mut v);
        assert_eq!(v["tool_response"], "ok");
    }

    /// **A snake_case key already present WINS.** Without this rule, an agent
    /// delivering both spellings with different values would see ours
    /// overwritten by its own — and Claude, Codex and Droid must not change by
    /// a single byte.
    #[test]
    fn an_existing_snake_case_key_is_never_overwritten() {
        let mut v = serde_json::json!({"tool_name": "Bash", "toolName": "autre"});
        normalise_envelope(&mut v);
        assert_eq!(v["tool_name"], "Bash");
    }

    /// The two agents that have their own dialect branch must not be touched:
    /// Antigravity carries `toolCall.name`, not `toolName`.
    #[test]
    fn the_antigravity_envelope_is_not_touched() {
        let mut v = serde_json::json!({"toolCall": {"name": "run_command"}});
        let before = v.clone();
        normalise_envelope(&mut v);
        assert_eq!(v, before);
    }

    /// An input that is not an object must not make a hook panic.
    #[test]
    fn an_input_that_is_not_an_object_does_not_panic() {
        let mut v = serde_json::json!([1, 2, 3]);
        normalise_envelope(&mut v);
        assert!(v.is_array());
    }
}
