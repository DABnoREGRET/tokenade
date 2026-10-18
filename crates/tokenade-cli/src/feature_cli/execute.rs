//! `tokenade exec` — capped subprocess, only stdout comes back.
#![allow(unused_imports)]
use super::*;
use crate::hooks::*;
use crate::wrap::*;
use crate::*;

/// The script text read from stdin, or the exit code that says why not.
///
/// `--script -` and `--script-file -` both read it, word for word down to the
/// message. What differs is what each does NEXT (the file form also rejects an
/// empty script), so only the read is shared.
fn read_script_stdin() -> Result<String, i32> {
    let mut buf = String::with_capacity(4096);
    if crate::bounded_input::read_stdin_lossy(&mut buf).is_err() {
        eprintln!("tokenade execute: failed to read script from stdin");
        return Err(2);
    }
    Ok(buf)
}

/// The script to run: `--script-file <path|->` wins, else `--script <text|->`.
fn resolve_script(rest: &[String]) -> Result<String, i32> {
    let script_from_file: Option<String> = match get_flag(rest, "--script-file") {
        Some("-") => {
            let buf = read_script_stdin()?;
            if buf.trim().is_empty() {
                eprintln!("tokenade execute: --script-file - read an empty script from stdin");
                return Err(2);
            }
            Some(buf)
        }
        // Bounded like every other path an agent chooses: a script file that
        // never ends (`--script-file /dev/zero`) hung the command outright.
        Some(f) => match crate::bounded_input::read_path_bounded(std::path::Path::new(f))
            .map(|b| String::from_utf8_lossy(&b).into_owned())
        {
            Ok(s) if !s.trim().is_empty() => Some(s),
            Ok(_) => {
                eprintln!("tokenade execute: --script-file {f} is empty");
                return Err(2);
            }
            Err(e) => {
                eprintln!("tokenade execute: cannot read --script-file {f}: {e}");
                return Err(2);
            }
        },
        None => None,
    };
    let script: String = if let Some(s) = script_from_file {
        s
    } else {
        match get_flag(rest, "--script") {
            Some("-") => read_script_stdin()?,
            Some(s) => s.to_string(),
            None => {
                eprintln!(
            "tokenade execute: --script is required (use `--script -` for stdin, or --script-file <path>)"
        );
                return Err(2);
            }
        }
    };
    Ok(script)
}
const EXEC_USAGE: &str = "usage: tokenade execute --lang <name> [--script CODE | -] [--cwd DIR] \\\n               [--timeout SECS] [--cap BYTES] [--no-compact] [--no-redact]\n       tokenade execute --list   # show supported interpreters\n\nThe think-in-code runner: runs `--script` in `--lang` and returns only\nits stdout (redacted + compacted + capped). Use when the answer is a\nderived value (count, filter, sum) — keeps raw data OUT of your\nagent's context.\n\nData you PIPE in reaches the script's stdin (`cmd | tokenade exec …`).\nTwo exceptions: `--script -` spends that channel on the script itself,\nand a bash/sh script over 64 KiB keeps the delivery that needs stdin.\n\nScope of \"sandbox\": a separate process with a scrubbed environment, a\ntimeout and output caps. It is NOT a syscall jail — the script runs as\nyou, with your filesystem. Treat a script here exactly as you would\ntreat running it in your own shell.\n\nFlags:\n  --lang <name>     interpreter (bash, sh, python, node, ruby, awk, jq, perl)\n  --script CODE     source to run (positional alt: `--script -` reads stdin)\n  --script-file F   read the script from a file — no shell quoting to get wrong\n  --cwd DIR         working directory (default: cwd)\n  --timeout SECS    kill after this many seconds (default 30, max 600)\n  --cap BYTES       stdout cap (default 1 MiB, max 16 MiB)\n  --no-compact      skip the post-run compactor\n  --no-redact       skip secret redaction (NOT recommended)\n  --sandbox         run under the OS sandbox (also TOKENADE_EXEC_SANDBOX=1)\n  --sandbox-net     allow network from inside it (also TOKENADE_EXEC_SANDBOX_NET=1)\n  --list            print supported interpreters and exit\n  --json            with --list, print the interpreters as JSON";
const EXEC_USAGE_SHORT: &str = "usage: tokenade execute --lang <name> [--script CODE | -] [--cwd DIR] \\\n               [--timeout SECS] [--cap BYTES] [--no-compact] [--no-redact]\n       tokenade execute --list   # show supported interpreters";

const EXEC_BOOL_FLAGS: &[&str] = &[
    "--help",
    "-h",
    "--list",
    "--json",
    "--no-compact",
    "--no-redact",
    "--sandbox",
    "--sandbox-net",
];
/// The flags that take a VALUE. An agent has to spell these exactly, so each
/// one must appear in the usage text — `--script-file` worked and was absent
/// from it, which is the same defect as naming a flag that does not exist,
/// pointing the other way.
const EXEC_VALUE_FLAGS: &[&str] = &[
    "--lang",
    "--script",
    "--script-file",
    "--cwd",
    "--timeout",
    "--cap",
];

/// `tokenade execute --lang <name> [--script "code" | -] [--cwd DIR]
///                 [--timeout SECS] [--cap BYTES] [--no-compact] [--no-redact]
///                 [--list]`
///
/// The "think-in-code" tool. Instead of asking the agent to read raw data
/// into its context, the agent calls `tokenade execute` with a small script
/// that does the analysis locally; we return only the script's stdout
/// (redacted, compacted, capped). Useful for: counting things, picking
/// fields out of JSON via jq, summarising file lists, running ad-hoc
/// SQL through sqlite3, etc.
///
/// Script source is positional **or** read from stdin if you pass `-`:
///
///   tokenade execute --lang python --script 'print(2+2)'
///   echo 'console.log(2+2)' | tokenade execute --lang node --script -
pub(crate) fn cmd_execute(rest: &[String]) -> i32 {
    // #634 — `execute` is in the passthrough_cmd exemption list at
    // the top-level dispatch (so `--help` isn't intercepted by the
    // generic helper); without this branch, `tokenade execute --help`
    // falls into the no-lang path below and returns exit 2 (usage
    // error) — wrong: --help is the user asking for docs, not an
    // error. Print the same usage but with exit 0.
    if crate::help_routing::asks_for_help(rest) {
        println!("{EXEC_USAGE}");
        return 0;
    }
    if rest.iter().any(|a| a == "--list") {
        let langs = sandbox::available_langs();
        // #648 — `--list --json` for CI / install scripts that want
        // to detect what's available without parsing the human text.
        if rest.iter().any(|a| a == "--json") {
            let arr: Vec<serde_json::Value> = langs
                .iter()
                .map(|l| serde_json::json!({"name": l.name, "binary": l.binary}))
                .collect();
            let payload = serde_json::json!({
                "version": env!("CARGO_PKG_VERSION"),
                "available": arr,
                "count": langs.len(),
            });
            println!("{}", serde_json::to_string_pretty(&payload).unwrap());
            return 0;
        }
        if langs.is_empty() {
            println!("No supported interpreters found on PATH.");
            println!("Install one of: {}", sandbox::known_lang_names());
            return 0;
        }
        println!("Available interpreters:");
        for l in langs {
            println!("  {:<8} (binary: {})", l.name, l.binary);
        }
        return 0;
    }

    // Silent-accept sweep (#812 follow-up): cmd_execute had the same
    // class bug as the compactor family — `tokenade execute --lang
    // bash --script foo --no-compactt` silently dropped `--no-compactt`
    // (typo for `--no-compact`) and ran with compaction ON when the
    // user intended OFF. Reject unknown flags up front, after the
    // --help / --list early-exits so those still print their help.
    {
        let mut i = 0;
        while i < rest.len() {
            let a = &rest[i];
            if a.starts_with("--") || (a.starts_with('-') && a.len() > 1 && a != "-") {
                if EXEC_VALUE_FLAGS.contains(&a.as_str()) {
                    i += 2;
                    continue;
                }
                if !EXEC_BOOL_FLAGS.contains(&a.as_str()) {
                    eprintln!(
                        "tokenade execute: unknown flag '{a}'. Try `tokenade execute --help`."
                    );
                    return 2;
                }
            }
            i += 1;
        }
    }

    let lang_name = match get_flag(rest, "--lang") {
        Some(v) => v,
        None => {
            eprintln!("{EXEC_USAGE_SHORT}");
            return 2;
        }
    };
    // #942 — `--lang ""` / `--lang "   "` fell through to
    // `sandbox::execute_named("")` which returned the "unknown lang
    // ''" error, hitting the catch-all `127` exit. That's
    // command-not-found semantics applied to a usage error.
    // Reject upfront with exit 2 + the known-lang list (so the
    // agent can self-correct without a second `--list` call).
    if lang_name.trim().is_empty() {
        eprintln!(
            "tokenade execute: --lang must not be empty/whitespace. \
             Known: {}. Run `tokenade execute --list` for details.",
            sandbox::known_lang_names()
        );
        return 2;
    }

    // Script: from --script flag, or from stdin if the value is "-",
    // or fall back to reading the first positional after --script.
    // --script-file: read the program from a file — the robust channel for
    // long/quote-heavy scripts (an inline single-quoted --script arg breaks
    // at the first embedded apostrophe, at SHELL parse time). The pretool
    // hook auto-lifts broken inline scripts to this flag.
    //
    // `-` means stdin here exactly as it does for `--script`. Without this the
    // two flags disagree on the one convention every UNIX tool shares, and the
    // failure is a liar: `--script-file -` reported `cannot read --script-file
    // -: No such file or directory`, which reads as a missing file rather than
    // an unsupported spelling. Measured cost is one wasted turn per agent that
    // reaches for the wrong flag — the scaffold documents both on the same line.
    let script = match resolve_script(rest) {
        Ok(s) => s,
        Err(rc) => return rc,
    };
    // #984 — `--script ""` (or `--script "   "` from a `--script
    // $EMPTY_VAR` shell expansion) used to fall straight through to
    // sandbox::execute_named with an empty program. Most interpreters
    // happily run zero bytes and exit 0 with no output, so the user
    // got NO signal their invocation was malformed. The MCP exec_script
    // handler already validates both lang AND script for emptiness
    // upfront (`if lang_name.is_empty() || script.is_empty()`); mirror
    // the missing half here so the CLI / MCP surfaces stay in sync.
    // Same trim-reject pattern as #942 (empty --lang) / #946 (empty
    // text input across compactors).
    if script.trim().is_empty() {
        eprintln!(
            "tokenade execute: --script must not be empty/whitespace. \
             Pass a non-empty program (or `--script -` to read source from stdin)."
        );
        return 2;
    }

    let mut opts = sandbox::ExecOptions::default();
    // #642 — surface invalid integers instead of silently falling back
    // to the default (same class as #636 hooks-tail). User typing
    // `--timeout abc` or `--cap 5MB` (forgot `--cap` wants bytes only)
    // used to get the default with no signal their value was dropped.
    let explicit_timeout = get_flag(rest, "--timeout").is_some();
    if let Some(raw) = get_flag(rest, "--timeout") {
        match raw.parse::<u64>() {
            Ok(t) => opts.timeout = std::time::Duration::from_secs(t.min(600)),
            Err(_) => {
                eprintln!(
                    "tokenade execute: --timeout '{raw}' is not a non-negative integer (seconds, max 600)."
                );
                return 2;
            }
        }
    }
    if let Some(raw) = get_flag(rest, "--cap") {
        match raw.parse::<usize>() {
            // Hard upper bound: 16 MiB. Anything larger and the agent should
            // be using a different tool (write to file, then `tokenade read
            // <file>`).
            Ok(c) => opts.stdout_cap = c.min(16 * 1024 * 1024),
            Err(_) => {
                eprintln!(
                    "tokenade execute: --cap '{raw}' is not a non-negative integer (bytes, max 16777216 = 16 MiB)."
                );
                return 2;
            }
        }
    }
    if let Some(cwd) = get_flag(rest, "--cwd") {
        let p = PathBuf::from(cwd);
        if let Err(e) = sandbox::check_cwd(&p) {
            eprintln!("tokenade execute: {e}");
            return 2;
        }
        opts.cwd = Some(p);
    }
    // OS sandbox: `--sandbox` (or env TOKENADE_EXEC_SANDBOX=1) runs the
    // interpreter under bubblewrap (read-only FS, writable /tmp only). Meant
    // for untrusted sub-agent code — the trusted orchestrator omits it.
    // `--sandbox-net` (or TOKENADE_EXEC_SANDBOX_NET=1) keeps network up.
    if rest.iter().any(|a| a == "--sandbox")
        || std::env::var("TOKENADE_EXEC_SANDBOX").ok().as_deref() == Some("1")
    {
        opts.sandbox = true;
    }
    if rest.iter().any(|a| a == "--sandbox-net")
        || std::env::var("TOKENADE_EXEC_SANDBOX_NET").ok().as_deref() == Some("1")
    {
        opts.sandbox_net = true;
    }
    if rest.iter().any(|a| a == "--no-compact") {
        opts.compact_output = false;
    }
    if rest.iter().any(|a| a == "--no-redact") {
        opts.redact_output = false;
    }

    match sandbox::execute_named(lang_name, &script, &opts) {
        Ok(r) => {
            // stdout to the user; stderr goes to *our* stderr so callers
            // can separate them with shell redirection.
            // NOT `println!`: this subcommand is excluded from the SIGPIPE
            // restore because it feeds a script to an interpreter's stdin, and
            // that exclusion leaves SIG_IGN on OUR stdout too — so a reader
            // that quit early (`| head -1`) turned into a Rust panic instead
            // of the silent exit every other CLI performs. See
            // `crash_guard::write_stdout_or_exit_on_broken_pipe`.
            // The stash, which every other delivery surface applies and this
            // one did not. `execute` compacts (`auto::compact` inside the
            // sandbox) and CAPS at 1 MiB, and between those two there was
            // nothing: output the compactors could not fold — a log whose
            // every line differs — was delivered at the cap. Measured
            // 2026-08-27 on one script printing 9 368 890 bytes of varied log
            // lines: `exec` returned 1 048 678 (~260 k tokens, a whole context
            // window) where `wrap` on the SAME bytes returned 3 013.
            //
            // `SourceHint::Shell` for the same reason `wrap` uses it when the
            // command is not a single-file read: the bytes came from a process,
            // not from a path.
            // Same swallow as `wrap`, and this is the busier command: a script
            // ending `cargo test | tail -5` exits 0 here too. Read from the
            // RAW output, before compaction: a marker the fold removed is
            // exactly the case where the agent has nothing else to go on.
            crate::wrap::disclose_swallowed_failure(r.exit_code, &r.stdout, &r.stderr, &script);
            let mut stdout = r.stdout;
            // `--no-compact` means what it says. The stash IS a compaction, so
            // it answers to the same opt-out as the compactors do — a caller
            // asking for the script's own bytes must get them, and one of them
            // is the SIGPIPE guard's fixture, which needs a write big enough to
            // fill several pipe buffers.
            let stash_floor = if opts.compact_output {
                tokenade_core::disclose::threshold_tokens()
            } else {
                usize::MAX
            };
            // A token is at least one byte, so a string shorter than the
            // threshold can never exceed it in tokens — the same exact floor
            // `wrap` uses to keep the estimate off the small-output path.
            let pre_stash =
                (stdout.len() >= stash_floor).then(|| tokenade_core::tokenizer::estimate(&stdout));
            if pre_stash.is_some() {
                stdout = tokenade_core::disclose::maybe_stash(
                    stdout,
                    tokenade_core::disclose::SourceHint::Shell,
                );
            }
            // A way back to the bytes the FOLD dropped, which is a different
            // question from the one above: `maybe_stash` looks at how big the
            // surviving text is, so the better the fold worked the less likely
            // it fires. A 400-path `ls -laR` becomes a two-line rollup and
            // fell under its floor with every path destroyed and nothing to
            // recover them by. `wrap` and the MCP proxy have called this since
            // it was written; `exec` — 41 % of all invocations — never did.
            // Measured over our own sessions: 16 such rollups served, 2 with
            // any route back.
            if let Some(raw) = r.raw_stdout.as_deref() {
                let dropped = r
                    .savings
                    .tokens_before
                    .saturating_sub(r.savings.tokens_after);
                stdout = crate::wrap::attach_drop_recovery(
                    stdout,
                    raw,
                    dropped,
                    r.savings.tokens_before,
                    &tokenade_core::disclose::SourceHint::Shell,
                    "exec",
                );
            }
            // The caller piped data in and the script was too large to
            // travel any way but on that pipe, so it ran against an EMPTY
            // stdin. Said HERE, at the moment it happens: the ceiling's own
            // comment claims this case "is stated rather than silently
            // mishandled", and it was stated in `--help` alone. Measured on a
            // 72 121-byte bash script whose last line is `cat`: empty stdout,
            // empty stderr, exit 0 — an answer computed on no input, and
            // nothing to tell it apart from an answer that is simply empty.
            if r.stdin_spent_on_script {
                tokenade_core::crash_guard::write_stderr_or_exit_on_broken_pipe(
                    "[tokenade:exec] the data on stdin did NOT reach this script: a \
                     bash/sh script this large can only be delivered ON stdin, which \
                     is the same channel the data came in on, so the script ran \
                     against an empty stdin. Shorten the script (they are meant to \
                     be a few lines), or have it READ the data from a file instead \
                     of a pipe.\n",
                );
            }
            if !stdout.is_empty() {
                tokenade_core::crash_guard::write_stdout_or_exit_on_broken_pipe(&format!(
                    "{}\n",
                    stdout
                ));
            }
            if !r.stderr.is_empty() {
                tokenade_core::crash_guard::write_stderr_or_exit_on_broken_pipe(&format!(
                    "{}\n",
                    r.stderr
                ));
            }
            if let Some(note) = shell_escape_message(&script, &r.stderr) {
                eprintln!("{note}");
            }
            if r.timed_out {
                eprintln!(
                    "{}",
                    timeout_message(r.elapsed_ms, opts.timeout.as_secs(), explicit_timeout)
                );
            }
            // Log savings so `tokenade gain` accounts for what we compacted out
            // of the script's output.
            log_gain(&format!("exec:{}", r.lang.name), "execute", r.savings);
            // The two intervals tile and cannot overlap: the row above spans
            // raw → compacted, this one compacted → delivered. Without it a
            // 99 % delivery is booked as nothing at all.
            if let Some(before) = pre_stash {
                let after = tokenade_core::tokenizer::estimate(&stdout);
                log_gain(
                    &format!("exec:{}", r.lang.name),
                    "disclose:stash",
                    tokenade_core::Savings::new(before, after),
                );
            }
            r.exit_code
        }
        Err(e) => {
            eprintln!("tokenade execute: {e}");
            127
        }
    }
}

/// What the agent is told when the interpreter refused the script over a
/// backslash the SHELL was supposed to remove.
///
/// `--script "print(\"hi\")"` works: a double-quoted shell argument drops the
/// backslashes, so the interpreter sees `print("hi")`. `--script 'print(\"hi\")'`
/// does not: a single-quoted argument passes every byte through, the
/// interpreter sees a backslash outside a string, and Python answers
/// `SyntaxError: unexpected character after line continuation character` —
/// which names a line continuation, a thing the author never wrote.
///
/// Measured over 90 days of this machine's transcripts: **62 `tokenade exec`
/// calls died this way, 59 of them with a backslash-quote in the script**, and
/// the count is rising (13 in June, 30 in July, 45 in August). Each costs a
/// whole turn, because nothing runs — the script never parses.
///
/// The gate is the script AS RECEIVED, which is exact rather than heuristic:
/// in the working form the shell already removed the backslash, so tokenade
/// never sees one. Of 514 calls whose COMMAND carried `\"`, 433 ran fine — and
/// in every one of those the script tokenade got had plain quotes.
///
/// Keyed on Python's wording because that is where every measured case came
/// from (73 of 73 with a `--lang`); another interpreter's phrasing would need
/// its own evidence before being claimed here.
fn shell_escape_message(script: &str, stderr: &str) -> Option<String> {
    if !stderr.contains("line continuation") {
        return None;
    }
    let escaped = script
        .as_bytes()
        .windows(2)
        .filter(|w| w[0] == b'\\' && (w[1] == b'"' || w[1] == b'\''))
        .count();
    if escaped == 0 {
        return None;
    }
    Some(format!(
        "tokenade execute: the script reached the interpreter with {escaped} backslash-escaped \
         quote(s) still in it, and that is what it refused — the backslash is read as a line \
         continuation, not as an escape.\n  \
         A single-quoted shell argument passes `\\` through unchanged; only a double-quoted one \
         removes it. Nothing ran: the script never parsed.\n  \
         Send it without shell quoting at all — `--script-file <path>`, or `--script -` to read \
         it from stdin."
    ))
}

/// What the agent is told when the sandbox kills its script.
///
/// Names the remedy, not just the fact. `--timeout` exists and is in
/// `--help`, but an agent that hits the wall has no reason to go looking:
/// measured over this machine's transcripts, the timeout is the LARGEST
/// `exec` failure mode (37 of 163) on the command carrying 41 % of all
/// tokenade usage. The reaction it produces is to abandon `exec` for a plain
/// interpreter — which abandons the compaction of the output too, so one
/// unhelpful refusal costs the saving as well as the turn.
///
/// Split out so a test can reach it: the message used to live inline in a
/// hundred-line match arm, where nothing exercised it (the third reading
/// of a green mutation — no test at all).
fn timeout_message(elapsed_ms: u64, ceiling_secs: u64, explicit: bool) -> String {
    format!(
        "tokenade execute: timed out after {:.2}s (ceiling {ceiling_secs}s{}). \
         Raise it with `--timeout <secs>` (max 600), or narrow the script. \
         Any stdout above is what the script had produced before the kill.",
        elapsed_ms as f64 / 1000.0,
        if explicit { "" } else { ", the default" }
    )
}

#[cfg(test)]
mod shell_escape_notice {
    use super::shell_escape_message;

    const PY: &str = "SyntaxError: unexpected character after line continuation character";

    /// Both halves of the gate are required, and the test says why each is
    /// there: the interpreter's own words are the FACT (tokenade does not
    /// judge whether a script is valid), and the backslash in the script AS
    /// RECEIVED is what separates this mistake from the 433 measured calls
    /// whose command carried `\"` and ran fine.
    #[test]
    fn it_fires_only_when_the_interpreter_blamed_a_continuation_and_the_script_carries_one() {
        let bad = r#"print(\"hi\")"#;
        let note = shell_escape_message(bad, PY).expect("the measured shape must be named");
        assert!(
            note.contains("2 backslash-escaped"),
            "counts the actual quotes: {note}"
        );
        assert!(
            note.contains("--script-file"),
            "and names a route that avoids shell quoting: {note}"
        );

        assert!(
            shell_escape_message(bad, "NameError: name 'x' is not defined").is_none(),
            "a script that carries the backslash but died of something else is not this"
        );
        assert!(
            shell_escape_message("print(\"hi\")", PY).is_none(),
            "the working form reaches us with plain quotes — nothing to report"
        );
        assert!(shell_escape_message("", PY).is_none());
    }

    /// A single-quoted script is the other half of the same mistake.
    #[test]
    fn a_backslashed_single_quote_counts_too() {
        let note = shell_escape_message(r"print(\'hi\')", PY).expect("same shape, other quote");
        assert!(note.contains("2 backslash-escaped"), "{note}");
    }
}

#[cfg(test)]
mod timeout_notice {
    use super::timeout_message;

    /// A message that states a limit must state the knob that moves it.
    #[test]
    fn the_message_names_the_flag_and_its_ceiling() {
        let m = timeout_message(30_020, 30, false);
        assert!(m.contains("--timeout"), "the remedy is not named: {m}");
        assert!(m.contains("600"), "the maximum is not named: {m}");
        assert!(m.contains("30.02s"), "the elapsed time is wrong: {m}");
        assert!(m.contains("ceiling 30s"), "the ceiling is not named: {m}");
    }

    /// "the default" is a claim about the caller, and it is false once they
    /// have passed `--timeout` themselves — telling somebody their own value
    /// is the default reads as the flag having been ignored.
    #[test]
    fn the_default_is_only_called_default_when_it_is_one() {
        assert!(timeout_message(1_000, 30, false).contains(", the default"));
        assert!(
            !timeout_message(1_000, 1, true).contains("default"),
            "an explicit --timeout was reported as the default"
        );
    }

    /// How `explicit` is COMPUTED, not just what it renders. Setting it to a
    /// constant left every message test green — the three above pass the flag
    /// in by hand and never touch the call site's expression.
    #[test]
    fn an_explicit_timeout_flag_is_detected() {
        let argv = |xs: &[&str]| -> Vec<String> { xs.iter().map(|s| s.to_string()).collect() };
        let with = argv(&["--lang", "bash", "--timeout", "5", "--script", "x"]);
        let without = argv(&["--lang", "bash", "--script", "x"]);
        assert!(
            super::get_flag(&with, "--timeout").is_some(),
            "the flag was missed"
        );
        assert!(
            super::get_flag(&without, "--timeout").is_none(),
            "a flag that was never passed was reported as explicit"
        );
    }

    /// The partial-output promise is load-bearing: stdout IS written before
    /// this line, and a message claiming otherwise would send the agent to
    /// re-run something it already has.
    #[test]
    fn the_message_says_the_partial_output_is_above() {
        let m = timeout_message(1_000, 1, true);
        assert!(m.contains("stdout above"), "{m}");
    }
}

/// The names an agent reads must be the names the binary takes.
///
/// `exec` carries 41 % of all invocations, and it broke this in both
/// directions at once: the stdout-truncation notice told the agent to raise
/// `--stdout-cap`, which the parser answers with `unknown flag` and exit 2,
/// while `--script-file` worked and appeared nowhere in the usage text. One
/// sends the agent into a refusal, the other hides a door.
#[cfg(test)]
mod the_names_an_agent_reads {
    use super::{EXEC_BOOL_FLAGS, EXEC_VALUE_FLAGS};

    /// The usage string the command prints, taken from the command itself.
    fn usage() -> String {
        format!("{}\n{}", super::EXEC_USAGE, super::EXEC_USAGE_SHORT)
    }

    #[test]
    fn every_value_flag_is_in_the_usage_text() {
        let u = usage();
        let missing: Vec<&str> = EXEC_VALUE_FLAGS
            .iter()
            .copied()
            .filter(|f| !u.contains(f))
            .collect();
        assert!(
            missing.is_empty(),
            "accepted but undocumented, so an agent cannot find them: {missing:?}"
        );
    }

    /// ...and nothing the usage text names is a flag the parser refuses.
    #[test]
    fn every_flag_named_in_the_usage_text_is_accepted() {
        let u = usage();
        let mut named: Vec<String> = Vec::new();
        let bytes = u.as_bytes();
        let mut i = 0;
        while i + 1 < bytes.len() {
            if bytes[i] == b'-' && bytes[i + 1] == b'-' {
                let start = i;
                i += 2;
                while i < bytes.len()
                    && (bytes[i].is_ascii_lowercase()
                        || bytes[i] == b'-'
                        || bytes[i].is_ascii_digit())
                {
                    i += 1;
                }
                let tok = &u[start..i];
                if tok.len() > 2 {
                    named.push(tok.to_string());
                }
            } else {
                i += 1;
            }
        }
        let unknown: Vec<&String> = named
            .iter()
            .filter(|f| {
                !EXEC_VALUE_FLAGS.contains(&f.as_str()) && !EXEC_BOOL_FLAGS.contains(&f.as_str())
            })
            .collect();
        assert!(
            unknown.is_empty(),
            "the usage text names flags the parser refuses: {unknown:?}"
        );
    }
}
