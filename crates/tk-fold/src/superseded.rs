//! Tool results a LATER turn in the same session made obsolete.
//!
//! `session_compact` masks `tool_result` blocks by SIZE: big and idle. That is
//! the right rule when all we know is "this is heavy", but some blocks are
//! provably dead rather than merely heavy, and those deserve a lower bar:
//!
//!   * a file read, then EDITED later in the same session — the bytes in that
//!     result are no longer what the file says, so carrying them into a resume
//!     re-bills content that is not only stale but WRONG;
//!   * a shell command whose exact invocation was RE-RUN later — the fresher
//!     output answers whatever the first one was asked for.
//!
//! Idea taken from token-savior's `find_superseded` (commit b781cf1, 2026-07-28)
//! — same detector, different delivery: theirs reports stale refs through an MCP
//! tool, ours feeds the cold-resume mask, because Claude Code's `PreCompact`
//! schema accepts no model-facing payload at all (see `hooks::session_lifecycle`)
//! and a live transcript must not be rewritten under the agent.
//!
//! ## What is deliberately NOT a rule here
//!
//! **Read then re-read.** It looks like the obvious third case and it is a trap.
//! The re-read is where `hooks::read_diffs` earns its keep: the second result is
//! served as a DIFF against the first, so the first is not duplicate weight, it
//! is the base the agent needs to interpret what it was given. Masking it would
//! turn a cheap diff into an unreadable one. Nothing is left to reclaim there
//! anyway — the duplication was already prevented at delivery.
//!
//! **A result that FAILED.** A superseded read is stale bytes; a failed call is
//! the reason the session did what it did next. The re-run that supersedes it is
//! usually the fix attempt, and on resume the agent needs to know what it was
//! fixing — masking the failure leaves the correction unexplained and invites
//! the agent to re-run the broken command to find out. Errors are immune here
//! regardless of size or age, and they stay immune even when a later identical
//! invocation succeeds. (Every competitor that prunes context in-session lands
//! on the same rule; `hermes-context-manager` even carries a second regex guard
//! for `Traceback|FATAL|panic` on top of the structural flag.)
//!
//! Everything here is a pure function over parsed transcript lines: no clock, no
//! disk, no agent-specific paths, so the same detector works for any harness
//! whose transcript we can parse into `tool_use` / `tool_result` pairs.

use std::collections::{HashMap, HashSet};

/// Tools whose result is a view of a file's CONTENT at a point in time.
fn is_read_tool(name: &str) -> bool {
    matches!(name, "Read" | "NotebookRead")
}

/// Tools that CHANGE the file they name, invalidating any earlier view of it.
fn is_write_tool(name: &str) -> bool {
    matches!(name, "Edit" | "Write" | "NotebookEdit" | "MultiEdit")
}

/// One `tool_use` seen in the transcript, in appearance order.
struct Use {
    id: String,
    /// What the call is keyed on: the file path, or the exact command line.
    key: String,
    kind: Kind,
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Kind {
    Read,
    Write,
    Bash,
}

/// Canonical spelling of a path, so the same file written two ways keys the
/// same. Only the lexical noise an agent actually produces is removed — no
/// filesystem access, no `..` resolution (which would need a cwd we do not have
/// and cannot invent from a transcript).
fn normalize_path(p: &str) -> String {
    let mut s = p.trim();
    while let Some(rest) = s.strip_prefix("./") {
        s = rest;
    }
    let mut out = String::with_capacity(s.len());
    let mut prev_slash = false;
    for c in s.chars() {
        if c == '/' {
            if prev_slash {
                continue; // collapse `//`
            }
            prev_slash = true;
        } else {
            prev_slash = false;
        }
        out.push(c);
    }
    while out.len() > 1 && out.ends_with('/') {
        out.pop();
    }
    out
}

/// Last path component — the bucket key, so matching stays O(1) per call
/// instead of scanning every pending read.
fn basename(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// Do two normalized paths name the same file?
///
/// An agent reads `src/x.rs` and edits `/home/u/proj/src/x.rs`, or the reverse;
/// keying on the raw string misses the pair and the stale read rides into the
/// resume. So a relative path matches an absolute one that ENDS with it at a
/// component boundary.
///
/// The guard that keeps this honest: a bare filename (`Cargo.toml`, `mod.rs`)
/// only ever matches exactly. Without it, editing any `mod.rs` would invalidate
/// the read of every other `mod.rs` in the tree — the ambiguous case, where
/// `pi-dcp` (`frontier.ts:293-295`) also chooses to protect rather than mask.
fn paths_match(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let (short, long) = if a.len() < b.len() { (a, b) } else { (b, a) };
    // Ambiguous: one component only, and the two spellings already differ.
    if !short.contains('/') {
        return false;
    }
    long.len() > short.len()
        && long.ends_with(short)
        && long.as_bytes()[long.len() - short.len() - 1] == b'/'
}

/// Classify one `tool_use` block, or `None` for tools this detector says nothing
/// about. Being silent is the safe direction: an unrecognised tool's result is
/// simply judged on size like everything else.
fn classify(name: &str, input: &serde_json::Value) -> Option<(String, Kind)> {
    if is_read_tool(name) {
        let path = input.get("file_path")?.as_str()?;
        return Some((normalize_path(path), Kind::Read));
    }
    if is_write_tool(name) {
        let path = input.get("file_path")?.as_str()?;
        return Some((normalize_path(path), Kind::Write));
    }
    if name == "Bash" {
        let cmd = input.get("command")?.as_str()?.trim();
        if cmd.is_empty() {
            return None;
        }
        // The exact invocation, so `cargo test -p a` never supersedes
        // `cargo test -p b`. A re-run with any argument changed is a
        // DIFFERENT question and both answers stay.
        return Some((cmd.to_string(), Kind::Bash));
    }
    None
}

/// Collect the `tool_use` calls of one transcript line, in order.
fn uses_in_line(line: &serde_json::Value) -> Vec<Use> {
    let mut out = Vec::new();
    let Some(content) = line.pointer("/message/content").and_then(|c| c.as_array()) else {
        return out;
    };
    for block in content {
        if block.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
            continue;
        }
        let (Some(id), Some(name)) = (
            block.get("id").and_then(|i| i.as_str()),
            block.get("name").and_then(|n| n.as_str()),
        ) else {
            continue;
        };
        let input = block
            .get("input")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        if let Some((key, kind)) = classify(name, &input) {
            out.push(Use {
                id: id.to_string(),
                key,
                kind,
            });
        }
    }
    out
}

/// The `tool_use_id`s of DELEGATED work — a `Task` / `Agent` dispatch.
///
/// Their result is the only copy there is. Every other tool_result names
/// something that can be had again: a file is on disk, a command can be
/// re-run, a page can be re-fetched. A subagent's report is the distilled
/// output of a run that will never happen the same way twice, so masking it to
/// a bare pointer removes the deliverable itself.
///
/// MEASURED 2026-09-05 across this machine's transcripts: 6 subagent reports
/// were masked whole on cold resume — 2 173, 2 241, 2 538 and 2 676 tokens
/// among them, all just over the 2 000-token bar, so it was the generic size
/// gate treating a deliverable like any other output. They keep their opening
/// now; the bulk still goes.
pub fn delegated_report_ids(lines: &[serde_json::Value]) -> HashSet<String> {
    let mut ids = HashSet::new();
    for line in lines {
        let Some(content) = line.pointer("/message/content").and_then(|c| c.as_array()) else {
            continue;
        };
        for block in content {
            if block.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
                continue;
            }
            let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("");
            if name != "Task" && name != "Agent" {
                continue;
            }
            if let Some(id) = block.get("id").and_then(|i| i.as_str()) {
                ids.insert(id.to_string());
            }
        }
    }
    ids
}

/// The `tool_use_id`s whose results a later turn superseded.
///
/// `lines` is the transcript in order; entries that do not parse are skipped by
/// the caller. The returned ids are the ones a compactor may treat as dead
/// weight — the CONTENT is still recoverable from the stash either way, so a
/// false positive costs an `expand-ref`, never the bytes.
pub fn superseded_tool_use_ids(lines: &[serde_json::Value]) -> HashSet<String> {
    // Reads awaiting a write, bucketed by BASENAME so a write only compares
    // against the handful of pending reads that could plausibly name the same
    // file — `paths_match` then decides. Bash keeps its own map: its key is a
    // command line, which must match exactly.
    let mut pending_reads: HashMap<String, Vec<(String, String)>> = HashMap::new();
    let mut pending_bash: HashMap<String, Vec<String>> = HashMap::new();
    let mut dead = HashSet::new();

    for line in lines {
        for use_ in uses_in_line(line) {
            match use_.kind {
                // A write invalidates every earlier view of that path — and
                // itself carries no reusable content, so it is not queued.
                Kind::Write => {
                    let Some(slot) = pending_reads.get_mut(basename(&use_.key)) else {
                        continue;
                    };
                    slot.retain(|(id, path)| {
                        if paths_match(path, &use_.key) {
                            dead.insert(id.clone());
                            false
                        } else {
                            true
                        }
                    });
                }
                // A re-run of the identical command supersedes the earlier one.
                Kind::Bash => {
                    let slot = pending_bash.entry(use_.key).or_default();
                    for id in slot.drain(..) {
                        dead.insert(id);
                    }
                    slot.push(use_.id);
                }
                // Reads accumulate: a re-read does NOT supersede its
                // predecessor (see the module docs — the diff needs it).
                Kind::Read => pending_reads
                    .entry(basename(&use_.key).to_string())
                    .or_default()
                    .push((use_.id, use_.key)),
            }
        }
    }
    // A failed call is never dead weight, however stale (see the module docs).
    for id in errored_tool_use_ids(lines) {
        dead.remove(&id);
    }
    dead
}

/// The `tool_use_id`s whose `tool_result` reports a failure.
///
/// Two signals, because harnesses disagree: the structural `is_error` flag that
/// Claude Code sets on the result block, and — for the ones that only bake the
/// failure into the text — a narrow set of unambiguous crash markers. The
/// regexless substring scan is capped so a 300 KB dump costs nothing.
pub fn errored_tool_use_ids(lines: &[serde_json::Value]) -> HashSet<String> {
    /// Only markers that cannot plausibly appear in a healthy output. `error`
    /// alone would match a grep for the word, `FAILED` a passing test name.
    const CRASH_MARKERS: [&str; 5] = [
        "Traceback (most recent call last)",
        "panicked at",
        "Segmentation fault",
        "FATAL ERROR",
        "command not found",
    ];
    /// A crash marker lands in the first lines of a failure; scanning the whole
    /// blob would make this pass quadratic on a big transcript for no gain.
    const SCAN_BYTES: usize = 4_096;

    let mut out = HashSet::new();
    for line in lines {
        let Some(content) = line.pointer("/message/content").and_then(|c| c.as_array()) else {
            continue;
        };
        for block in content {
            if block.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
                continue;
            }
            let Some(id) = block.get("tool_use_id").and_then(|i| i.as_str()) else {
                continue;
            };
            if block.get("is_error").and_then(|e| e.as_bool()) == Some(true) {
                out.insert(id.to_string());
                continue;
            }
            let text = match block.get("content") {
                Some(serde_json::Value::String(s)) => s.as_str(),
                _ => continue,
            };
            // Floor to a char boundary: slicing a UTF-8 string at a raw byte
            // offset panics, and tool output is arbitrary bytes from the world.
            let mut end = text.len().min(SCAN_BYTES);
            while end > 0 && !text.is_char_boundary(end) {
                end -= 1;
            }
            if CRASH_MARKERS.iter().any(|m| text[..end].contains(m)) {
                out.insert(id.to_string());
            }
        }
    }
    out
}

/// Parse a transcript's lines, keeping only the ones that are JSON objects.
pub fn parse_lines(transcript: &str) -> Vec<serde_json::Value> {
    transcript
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .collect()
}

/// Every `tool_use_id` a `tool_result` block in this line answers.
pub fn result_ids_in_line(line: &serde_json::Value) -> Vec<String> {
    let Some(content) = line.pointer("/message/content").and_then(|c| c.as_array()) else {
        return Vec::new();
    };
    content
        .iter()
        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
        .filter_map(|b| b.get("tool_use_id").and_then(|i| i.as_str()))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool_use(id: &str, name: &str, input: serde_json::Value) -> serde_json::Value {
        json!({"type": "assistant", "message": {"content": [
            {"type": "tool_use", "id": id, "name": name, "input": input}
        ]}})
    }

    fn tool_result(id: &str) -> serde_json::Value {
        json!({"type": "user", "message": {"content": [
            {"type": "tool_result", "tool_use_id": id, "content": "…"}
        ]}})
    }

    #[test]
    fn a_read_followed_by_an_edit_of_the_same_file_is_superseded() {
        let lines = vec![
            tool_use("t1", "Read", json!({"file_path": "/a/b.rs"})),
            tool_result("t1"),
            tool_use("t2", "Edit", json!({"file_path": "/a/b.rs"})),
        ];
        let dead = superseded_tool_use_ids(&lines);
        assert!(dead.contains("t1"));
        assert!(!dead.contains("t2"));
    }

    #[test]
    fn an_edit_to_a_different_file_supersedes_nothing() {
        let lines = vec![
            tool_use("t1", "Read", json!({"file_path": "/a/b.rs"})),
            tool_use("t2", "Edit", json!({"file_path": "/a/other.rs"})),
        ];
        assert!(superseded_tool_use_ids(&lines).is_empty());
    }

    #[test]
    fn a_read_after_the_edit_survives_it() {
        // Order matters: only views taken BEFORE the write go stale.
        let lines = vec![
            tool_use("t1", "Edit", json!({"file_path": "/a/b.rs"})),
            tool_use("t2", "Read", json!({"file_path": "/a/b.rs"})),
        ];
        assert!(superseded_tool_use_ids(&lines).is_empty());
    }

    #[test]
    fn a_re_read_does_not_supersede_the_read_its_diff_is_built_against() {
        let lines = vec![
            tool_use("t1", "Read", json!({"file_path": "/a/b.rs"})),
            tool_use("t2", "Read", json!({"file_path": "/a/b.rs"})),
        ];
        assert!(
            superseded_tool_use_ids(&lines).is_empty(),
            "read_diffs serves the second as a diff against the first"
        );
    }

    #[test]
    fn every_read_before_a_write_goes_stale_not_just_the_last() {
        let lines = vec![
            tool_use("t1", "Read", json!({"file_path": "/a/b.rs"})),
            tool_use("t2", "Read", json!({"file_path": "/a/b.rs"})),
            tool_use("t3", "Write", json!({"file_path": "/a/b.rs"})),
        ];
        let dead = superseded_tool_use_ids(&lines);
        assert!(dead.contains("t1") && dead.contains("t2"));
    }

    #[test]
    fn an_identical_command_rerun_supersedes_the_earlier_output() {
        let lines = vec![
            tool_use("t1", "Bash", json!({"command": "cargo test -p core"})),
            tool_use("t2", "Bash", json!({"command": "cargo test -p core"})),
        ];
        let dead = superseded_tool_use_ids(&lines);
        assert!(dead.contains("t1"));
        assert!(!dead.contains("t2"), "the freshest run is the live answer");
    }

    #[test]
    fn a_command_with_any_argument_changed_is_a_different_question() {
        let lines = vec![
            tool_use("t1", "Bash", json!({"command": "cargo test -p a"})),
            tool_use("t2", "Bash", json!({"command": "cargo test -p b"})),
        ];
        assert!(superseded_tool_use_ids(&lines).is_empty());
    }

    #[test]
    fn tools_this_detector_does_not_know_are_left_alone() {
        let lines = vec![
            tool_use("t1", "WebFetch", json!({"url": "https://example.com"})),
            tool_use("t2", "WebFetch", json!({"url": "https://example.com"})),
            tool_use("t3", "Grep", json!({"pattern": "x"})),
        ];
        assert!(superseded_tool_use_ids(&lines).is_empty());
    }

    #[test]
    fn malformed_lines_and_missing_fields_are_skipped() {
        let lines = vec![
            json!({"type": "assistant"}),
            json!({"message": {"content": "not an array"}}),
            // A Read with no path cannot be keyed, so it is silent.
            tool_use("t1", "Read", json!({})),
            tool_use("t2", "Edit", json!({"file_path": "/a/b.rs"})),
        ];
        assert!(superseded_tool_use_ids(&lines).is_empty());
    }

    fn tool_result_err(id: &str, text: &str) -> serde_json::Value {
        json!({"type": "user", "message": {"content": [
            {"type": "tool_result", "tool_use_id": id, "content": text, "is_error": true}
        ]}})
    }

    #[test]
    fn a_relative_read_is_superseded_by_an_absolute_edit_of_the_same_file() {
        let lines = vec![
            tool_use("t1", "Read", json!({"file_path": "src/x.rs"})),
            tool_use("t2", "Edit", json!({"file_path": "/home/u/proj/src/x.rs"})),
        ];
        assert!(superseded_tool_use_ids(&lines).contains("t1"));
    }

    #[test]
    fn a_dot_slash_prefix_is_not_a_different_file() {
        let lines = vec![
            tool_use("t1", "Read", json!({"file_path": "./src/x.rs"})),
            tool_use("t2", "Edit", json!({"file_path": "src/x.rs"})),
        ];
        assert!(superseded_tool_use_ids(&lines).contains("t1"));
    }

    #[test]
    fn a_bare_filename_never_matches_a_different_spelling() {
        // Editing one `mod.rs` must not invalidate the read of another. The
        // ambiguous case protects instead of masking.
        let lines = vec![
            tool_use("t1", "Read", json!({"file_path": "mod.rs"})),
            tool_use("t2", "Edit", json!({"file_path": "/a/deep/mod.rs"})),
        ];
        assert!(superseded_tool_use_ids(&lines).is_empty());
    }

    #[test]
    fn a_shared_basename_in_a_different_directory_is_a_different_file() {
        let lines = vec![
            tool_use("t1", "Read", json!({"file_path": "/a/src/mod.rs"})),
            tool_use("t2", "Edit", json!({"file_path": "/b/src/mod.rs"})),
        ];
        assert!(superseded_tool_use_ids(&lines).is_empty());
    }

    #[test]
    fn a_failed_result_is_never_superseded_even_when_re_run() {
        let lines = vec![
            tool_use("t1", "Bash", json!({"command": "cargo test"})),
            tool_result_err("t1", "error: could not compile"),
            tool_use("t2", "Bash", json!({"command": "cargo test"})),
            tool_result("t2"),
        ];
        let dead = superseded_tool_use_ids(&lines);
        assert!(
            !dead.contains("t1"),
            "the failure explains why the session re-ran the command"
        );
    }

    #[test]
    fn a_crash_marker_in_the_text_is_a_failure_even_without_the_flag() {
        let lines = vec![
            tool_use("t1", "Bash", json!({"command": "python x.py"})),
            json!({"type": "user", "message": {"content": [{
                "type": "tool_result", "tool_use_id": "t1",
                "content": "Traceback (most recent call last):\n  File \"x.py\""
            }]}}),
            tool_use("t2", "Bash", json!({"command": "python x.py"})),
        ];
        assert!(superseded_tool_use_ids(&lines).is_empty());
    }

    #[test]
    fn the_word_error_alone_does_not_immunise_a_result() {
        // Otherwise a grep for "error" would pin every output it ever produced.
        let lines = vec![
            tool_use("t1", "Bash", json!({"command": "rg error"})),
            json!({"type": "user", "message": {"content": [{
                "type": "tool_result", "tool_use_id": "t1",
                "content": "src/a.rs:12: // error handling is done upstream"
            }]}}),
            tool_use("t2", "Bash", json!({"command": "rg error"})),
        ];
        assert!(superseded_tool_use_ids(&lines).contains("t1"));
    }

    #[test]
    fn a_multibyte_result_does_not_panic_the_crash_scan() {
        let big = "é".repeat(8_000); // straddles the 4 KiB scan cap mid-char
        let lines = vec![
            tool_use("t1", "Bash", json!({"command": "cat f"})),
            json!({"type": "user", "message": {"content": [{
                "type": "tool_result", "tool_use_id": "t1", "content": big
            }]}}),
            tool_use("t2", "Bash", json!({"command": "cat f"})),
        ];
        assert!(superseded_tool_use_ids(&lines).contains("t1"));
    }

    #[test]
    fn result_ids_are_read_off_a_user_line() {
        let line = json!({"type": "user", "message": {"content": [
            {"type": "tool_result", "tool_use_id": "t1", "content": "x"},
            {"type": "text", "text": "ignored"},
            {"type": "tool_result", "tool_use_id": "t2", "content": "y"}
        ]}});
        assert_eq!(result_ids_in_line(&line), vec!["t1", "t2"]);
    }
}
