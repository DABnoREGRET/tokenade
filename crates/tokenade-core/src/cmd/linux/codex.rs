//! `codex exec` — a whole sub-agent session, of which the caller needs the end.
//!
//! ```text
//! OpenAI Codex v0.147.0
//! --------
//! model: gpt-5.5
//! session id: 01a10b4e-…
//! --------
//! user
//! <the prompt the caller just wrote>
//! exec
//! /usr/bin/bash -lc "…" in /dir
//!  exited 1 in 0ms:
//! <the command's whole output>
//! apply patch
//! codex
//! <the final answer>
//! diff --git …          (the turn's diff, printed again after every patch)
//! tokens used
//! 28 342
//! ```
//!
//! On stderr Codex narrates the session; on stdout it prints the answer alone.
//! The calling agent reads both, merged: every command's output, the hooks'
//! chatter and the same diff up to four times (measured 2026-10-05 on a
//! one-line fix: 107 lines for a 51-byte answer). Across the fleet `codex`
//! was the largest uncovered command — 8.4 M tokens over 831 runs in 30 days.
//!
//! Kept: model, session id (what `codex exec resume` needs), token count, every
//! command with its status, the last lines of a failure no later run of the
//! same command repaired, errors and warnings, the turn's diff ONCE, and the
//! final answer. Everything else stays recoverable from the fold store.

use std::fmt::Write;

/// The lines Codex opens a section of the transcript with.
fn is_section(line: &str) -> bool {
    matches!(line, "user" | "codex" | "exec" | "thinking" | "apply patch" | "tokens used") || line.starts_with("hook: ")
}

/// A section, or a top-level line worth keeping on its own.
fn is_marker(line: &str) -> bool {
    is_section(line)
        || line.starts_with("ERROR: ")
        || line.starts_with("warning: ")
        || line.starts_with("diff --git ")
}

pub fn detect(text: &str) -> bool {
    let head: Vec<&str> = text.lines().take(16).collect();
    let banner = head.iter().any(|l| l.starts_with("OpenAI Codex v")) && head.iter().any(|l| l.starts_with("session id: "));
    banner || is_json_session(text)
}

/// `codex exec --json`: one event per line, opening on `thread.started`.
fn is_json_session(text: &str) -> bool {
    text.lines()
        .find(|l| !l.trim().is_empty())
        .and_then(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .is_some_and(|v| v["type"] == "thread.started")
}

crate::archetypes::detect_gated_try_compact!();

struct Run {
    command: String,
    failed: Option<String>,
    output: Vec<String>,
}

/// One session, whichever way Codex printed it. Both readers fill this and one
/// renderer prints it, so the text and the `--json` forms cannot drift apart.
#[derive(Default)]
struct Session {
    model: Option<String>,
    session: Option<String>,
    tokens: Option<String>,
    notes: Vec<String>,
    runs: Vec<Run>,
    /// The turn's diff (text form), or the files it changed (`--json` names
    /// them without a diff).
    diff: Vec<String>,
    files: Vec<String>,
    answer: Vec<String>,
}

/// `/usr/bin/bash -lc "<cmd>" in /dir` → `<cmd>`, and a command an agent hook
/// wrapped (`… tokenade wrap '<cmd>'`) → the command itself. `--json` gives the
/// same shape without ` in /dir`.
fn command_of(line: &str) -> String {
    let mut cmd = line.to_string();
    if let Some(at) = cmd.rfind("\" in ") {
        cmd.truncate(at + 1);
    }
    if let Some(rest) = cmd.split_once(" -lc \"").map(|(_, r)| r) {
        let rest = rest.strip_suffix('"').unwrap_or(rest);
        cmd = rest.replace("\\\"", "\"").replace("\\\\", "\\");
    }
    // Every hook-wrapped command of a chain (`… tokenade wrap 'a' && … tokenade
    // wrap 'b'`), not only the first: a real `codex review` ran four such
    // chains and the report showed `git status --short' && TOKENADE_SESSION=…`.
    let wrapped = regex::Regex::new(r"(?:\S+=\S+ )*\S*tokenade wrap '((?:[^']|'\\'')*)'").ok();
    if let Some(re) = wrapped {
        cmd = re.replace_all(&cmd, |c: &regex::Captures| c[1].replace("'\\''", "'")).into_owned();
    }
    cmd.trim().to_string()
}

pub fn compact(text: &str) -> String {
    let session = if is_json_session(text) { read_json(text) } else { read_text(text) };
    render(&session)
}

fn read_text(text: &str) -> Session {
    let lines: Vec<&str> = text.lines().collect();
    let field = |key: &str| lines.iter().find_map(|l| l.strip_prefix(key)).map(|v| v.trim().to_string());
    let mut s = Session { model: field("model: "), session: field("session id: "), ..Session::default() };
    let mut answer: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let start = i + 1;
        let mut end = start;
        // A command's output ends at the next SECTION only: the `diff --git`
        // or `warning:` a command prints is its output, not the turn's diff.
        let ends = |l: &str| if line == "exec" { is_section(l) } else { is_marker(l) };
        while end < lines.len() && !ends(lines[end]) {
            end += 1;
        }
        let body = &lines[start..end];
        match line {
            "exec" if !body.is_empty() => {
                // The command runs to its status line: a command holding a
                // newline (`printf '%f\n'`) spans several lines (2026-10-05,
                // a real 18-command session).
                let at = body
                    .iter()
                    .position(|l| l.starts_with(" succeeded in") || l.starts_with(" exited "))
                    .unwrap_or(1.min(body.len() - 1));
                let status = body.get(at).map(|s| s.trim()).unwrap_or("");
                let failed = status
                    .strip_prefix("exited ")
                    .and_then(|s| s.split_whitespace().next())
                    .map(|code| format!("exit {code}"));
                let output = body.iter().skip(at + 1).filter(|l| !l.trim().is_empty()).map(|l| l.to_string()).collect();
                s.runs.push(Run { command: command_of(&body[..at.max(1)].join("\n")), failed, output });
            }
            "codex" => answer = body.to_vec(),
            "tokens used" => {
                s.tokens = body.first().map(|t| t.trim().to_string());
                // What follows the count is stdout: the answer printed again.
                let rest: Vec<&str> = body.iter().skip(1).copied().collect();
                if trimmed(&rest) != trimmed(&answer) {
                    answer.extend(rest);
                }
            }
            _ if line.starts_with("diff --git ") => {
                // The turn's diff is reprinted whole after every patch: keep
                // the last copy, which is the state the session ended in.
                let mut j = i;
                let mut block = Vec::new();
                while j < lines.len() && (j == i || !is_marker(lines[j]) || lines[j].starts_with("diff --git ")) {
                    block.push(lines[j]);
                    j += 1;
                }
                s.diff = trimmed(&block).iter().map(|l| l.to_string()).collect();
                end = j;
            }
            _ if (line.starts_with("ERROR: ") || line.starts_with("warning: ")) && !s.notes.iter().any(|n| n == line) => {
                s.notes.push(line.to_string())
            }
            _ => {}
        }
        i = end.max(start);
    }
    s.answer = once(trimmed(&answer)).iter().map(|l| l.to_string()).collect();
    s
}

/// The answer printed twice — on stderr after `codex`, then on stdout — with
/// no `tokens used` between them (a `codex review` has none): keep one copy.
fn once(lines: Vec<&str>) -> Vec<&str> {
    let first = match lines.first() {
        Some(f) if !f.trim().is_empty() => *f,
        _ => return lines,
    };
    for j in 1..lines.len() {
        if lines[j] == first {
            let (a, b) = (trimmed(&lines[..j]), trimmed(&lines[j..]));
            if a == b {
                return a;
            }
        }
    }
    lines
}

/// `codex exec --json`. A command is reported twice (`item.started`, then
/// `item.completed` with its output): only the completion counts.
fn read_json(text: &str) -> Session {
    let mut s = Session::default();
    let (mut input, mut output) = (0u64, 0u64);
    for line in text.lines() {
        let Ok(e) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let item = &e["item"];
        match (e["type"].as_str().unwrap_or(""), item["type"].as_str().unwrap_or("")) {
            ("thread.started", _) => s.session = e["thread_id"].as_str().map(str::to_string),
            ("turn.completed", _) => {
                input += e["usage"]["input_tokens"].as_u64().unwrap_or(0);
                output += e["usage"]["output_tokens"].as_u64().unwrap_or(0);
            }
            ("item.completed", "command_execution") => {
                let code = item["exit_code"].as_i64().unwrap_or(0);
                s.runs.push(Run {
                    command: command_of(item["command"].as_str().unwrap_or("")),
                    failed: (code != 0).then(|| format!("exit {code}")),
                    output: item["aggregated_output"]
                        .as_str()
                        .unwrap_or("")
                        .lines()
                        .filter(|l| !l.trim().is_empty())
                        .map(str::to_string)
                        .collect(),
                });
            }
            ("item.completed", "file_change") => {
                for c in item["changes"].as_array().into_iter().flatten() {
                    let f = format!("{} {}", c["kind"].as_str().unwrap_or("change"), c["path"].as_str().unwrap_or("?"));
                    if !s.files.contains(&f) {
                        s.files.push(f);
                    }
                }
            }
            ("item.completed", "agent_message") => {
                s.answer = item["text"].as_str().unwrap_or("").lines().map(str::to_string).collect();
            }
            ("item.completed", "error") | ("error", _) | ("turn.failed", _) => {
                let msg = item["message"].as_str().or(e["message"].as_str()).or(e["error"]["message"].as_str());
                let note = format!("ERROR: {}", msg.unwrap_or(line));
                if !s.notes.contains(&note) {
                    s.notes.push(note);
                }
            }
            _ => {}
        }
    }
    if input + output > 0 {
        s.tokens = Some(format!("{input} in / {output} out"));
    }
    s
}

fn render(s: &Session) -> String {
    let mut out = String::from("codex");
    for (label, v) in [("model", &s.model), ("session", &s.session)] {
        if let Some(v) = v {
            let _ = write!(out, " · {label} {v}");
        }
    }
    if let Some(t) = &s.tokens {
        let _ = write!(out, " · {t} tokens");
    }
    out.push('\n');
    for n in &s.notes {
        out.push_str(n);
        out.push('\n');
    }
    if !s.runs.is_empty() {
        let _ = writeln!(out, "ran {} command(s):", s.runs.len());
        for (k, r) in s.runs.iter().enumerate() {
            match &r.failed {
                None => {
                    let _ = writeln!(out, "  ✓ {}", r.command.replace('\n', "⏎"));
                }
                Some(code) => {
                    let repaired = s.runs[k + 1..].iter().any(|later| later.command == r.command && later.failed.is_none());
                    if repaired {
                        let _ = writeln!(out, "  ✗ {code} · {}  (passed on a later run)", r.command.replace('\n', "⏎"));
                    } else {
                        let _ = writeln!(out, "  ✗ {code} · {}", r.command.replace('\n', "⏎"));
                        for l in r.output.iter().skip(r.output.len().saturating_sub(3)) {
                            let _ = writeln!(out, "      {l}");
                        }
                    }
                }
            }
        }
    }
    if !s.files.is_empty() {
        let _ = writeln!(out, "changed: {}", s.files.join(", "));
    }
    if !s.diff.is_empty() {
        out.push_str(&s.diff.join("\n"));
        out.push('\n');
    }
    if !s.answer.is_empty() {
        out.push_str("answer:\n");
        out.push_str(&s.answer.join("\n"));
    }
    out.trim_end().to_string()
}

fn trimmed<'a>(lines: &[&'a str]) -> Vec<&'a str> {
    let start = lines.iter().position(|l| !l.trim().is_empty()).unwrap_or(lines.len());
    let end = lines.iter().rposition(|l| !l.trim().is_empty()).map_or(start, |e| e + 1);
    lines[start..end].to_vec()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIX: &str = include_str!("../../../tests/fixtures/linux/codex_exec_fix.txt");
    const ERROR: &str = include_str!("../../../tests/fixtures/linux/codex_exec_error.txt");
    const JSON: &str = include_str!("../../../tests/fixtures/linux/codex_exec_json.jsonl");
    const REVIEW: &str = include_str!("../../../tests/fixtures/linux/codex_review.txt");

    #[test]
    fn a_session_folds_to_its_commands_its_diff_and_its_answer() {
        assert!(detect(FIX));
        let out = compact(FIX);
        assert!(out.starts_with("codex · model gpt-5.5 · session 01a10b4e-f870-7f91-8752-b2460b6d2b09 · 28\u{202f}342 tokens\n"), "{out}");
        assert!(out.contains("ran 4 command(s):\n  ✗ exit 1 · python3 test_calc.py  (passed on a later run)\n  ✓ sed -n '1,160p' calc.py\n"), "{out}");
        assert_eq!(out.matches("diff --git a/calc.py b/calc.py").count(), 1, "{out}");
        assert!(out.contains("+    return a + b"), "{out}");
        assert!(out.ends_with("answer:\nFixed `calc.py`; `python3 test_calc.py` now passes."), "{out}");
        // Neither the hooks' chatter nor a command's output nor the prompt.
        for gone in ["hook: ", "Traceback", "Reading additional input", "Run `python3"] {
            assert!(!out.contains(gone), "{gone} in {out}");
        }
        assert!(out.len() * 4 < FIX.len(), "{} vs {}", out.len(), FIX.len());
    }

    #[test]
    fn a_failure_nothing_repaired_keeps_its_last_lines() {
        let text = FIX.replace(" succeeded in 0ms:\nok", " exited 1 in 0ms:\nStill broken");
        let out = compact(&text);
        assert!(out.contains("  ✗ exit 1 · python3 test_calc.py\n      test_calc.py:2 in <module>   assert add(2, 3) == 5"), "{out}");
        assert!(out.contains("  ✗ exit 1 · python3 test_calc.py\n      Still broken"), "{out}");
    }

    #[test]
    fn an_error_is_kept_once() {
        let out = compact(ERROR);
        assert_eq!(out.matches("ERROR: ").count(), 1, "{out}");
        assert!(out.contains("is not supported when using Codex with a ChatGPT account"), "{out}");
        assert!(out.contains("warning: Model metadata for `no-such-model` not found"), "{out}");
    }

    #[test]
    fn the_json_form_folds_to_the_same_report() {
        assert!(detect(JSON));
        let out = compact(JSON);
        assert!(out.starts_with("codex · session 01a10b7c-889b-7262-a233-0cd902313137 · 76189 in / 467 out tokens\n"), "{out}");
        assert!(out.contains("ran 4 command(s):\n  ✗ exit 1 · python3 test_calc.py  (passed on a later run)\n  ✓ sed -n '1,120p' calc.py\n"), "{out}");
        assert!(out.contains("changed: update /home/user/proj/calc.py\n"), "{out}");
        assert!(out.ends_with("answer:\nFixed `calc.py`; `python3 test_calc.py` now passes."), "{out}");
        for gone in ["item.started", "aggregated_output", "Traceback", "Running the test first"] {
            assert!(!out.contains(gone), "{gone} in {out}");
        }
        // A plain JSON stream is not a Codex session.
        assert!(!detect("{\"type\":\"turn.started\"}\n"));
    }

    /// A real `codex review`: chained hook-wrapped commands read as typed, the
    /// `git diff` a command printed is that command's output, and the review —
    /// printed on stderr then on stdout — is kept once.
    #[test]
    fn a_review_keeps_its_findings_once() {
        assert!(detect(REVIEW));
        let out = compact(REVIEW);
        assert_eq!(out.matches("[P2] Return the actual average from avg").count(), 1, "{out}");
        assert!(out.contains("  ✓ git status --short && git diff --staged"), "{out}");
        assert!(!out.contains("TOKENADE_SESSION"), "{out}");
        assert!(!out.contains("diff --git"), "a command's output is not the turn's diff: {out}");
        assert!(out.len() * 4 < REVIEW.len(), "{} vs {}", out.len(), REVIEW.len());
    }

    #[test]
    fn a_wrapped_command_reads_as_typed() {
        let line = r#"/usr/bin/bash -lc "TOKENADE_SESSION=1 /bin/tokenade wrap 'sed -n '\\''1,9p'\\'' a.py'" in /w"#;
        assert_eq!(command_of(line), "sed -n '1,9p' a.py");
        assert_eq!(command_of(r#"/bin/zsh -lc "rg -n \"fn main\" src" in /w"#), r#"rg -n "fn main" src"#);
        assert_eq!(command_of(r#"/usr/bin/bash -lc "TOKENADE_SESSION=1 /bin/tokenade wrap 'ls crates'""#), "ls crates");
    }

    /// A command holding a newline spans two transcript lines (a real
    /// `-printf '%f\n'`): the whole command is read, up to its status line.
    #[test]
    fn a_command_with_a_newline_is_read_whole() {
        let text = "OpenAI Codex v0.147.0\nsession id: s\nexec\n/usr/bin/bash -lc \"T=1 /bin/tokenade wrap 'find crates -type d -printf '\\\\''%f\n'\\\\'' | sort'\" in /w\n succeeded in 0ms:\ntk-base\ncodex\ndone";
        let out = compact(text);
        assert!(out.contains("  ✓ find crates -type d -printf '%f⏎' | sort\n"), "{out}");
    }
}
