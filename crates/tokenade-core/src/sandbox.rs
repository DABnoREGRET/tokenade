//! `tokenade execute` — sandboxed "think-in-code" runner.
//!
//! The mechanism: instead of reading raw data into the agent's context and
//! filtering it afterwards, the agent writes a script that runs in a
//! sandbox here and we return *only* what the script wrote to stdout. For
//! exploration tasks (count files matching X, list functions calling Y,
//! grep-then-summarise) this is strictly stronger than compression because
//! the raw data never enters the agent's context window.
//!
//! Design choices (clean-room — no code from any other project):
//!   * Subprocess executors only. Bash, Python, Node, Ruby, awk, jq, sh —
//!     whichever interpreter is on the host (probed via `availability`).
//!     Skipping WASM keeps the binary small and avoids a ~10 MB runtime
//!     dependency just to run one script.
//!   * Strict per-call resource caps: timeout, stdout cap, stderr cap.
//!   * Output is run through `redact::redact` before returning, so any
//!     accidentally-printed Bearer/AWS/Stripe token is scrubbed before it
//!     enters the agent's context.
//!   * Output is also offered to `auto::compact` so we get the full
//!     Tokenade compression stack on top of the script's own output.
//!   * Never inherits the parent env wholesale — passes a minimal env
//!     (`PATH`, `HOME`, `LANG`, `LC_ALL`, `TZ`, `USER`) by default. Callers
//!     can extend with explicit `env` entries.
//!   * Never escalates privileges; runs as the current user. No `sudo`,
//!     no `setuid`. The "sandbox" is the OS-level process boundary +
//!     resource caps + env scrubbing, not a real syscall jail. Good
//!     enough for cooperating-agent threat model; not a defence against
//!     a malicious agent (treat scripts as if your shell ran them).

use crate::{auto, redact, Savings};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// One supported interpreter. The `binary` is what we probe via
/// `availability::is_available` — if it's missing the lang is disabled at
/// the MCP/CLI level so the agent never sees an unsatisfiable choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lang {
    pub name: &'static str,
    pub binary: &'static str,
    /// Argv prefix to invoke the interpreter on a script string read from
    /// stdin (`-` placeholder) or `-c` style. The runtime substitutes the
    /// script content for the `{SCRIPT}` token.
    pub argv: &'static [&'static str],
    /// How the script reaches the interpreter:
    ///   `Stdin` — write to child's stdin
    ///   `Arg`   — substitute into the `{SCRIPT}` slot in argv
    pub mode: ScriptMode,
}

/// How the script content is delivered to the interpreter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptMode {
    Stdin,
    Arg,
}

/// The catalog of languages we know how to drive. Order matters only for
/// `lang_for_name` short-circuit lookups (we try matches in this order).
pub const LANGS: &[Lang] = &[
    Lang {
        name: "bash",
        binary: "bash",
        argv: &["bash", "-s"],
        mode: ScriptMode::Stdin,
    },
    Lang {
        name: "sh",
        binary: "sh",
        argv: &["sh", "-s"],
        mode: ScriptMode::Stdin,
    },
    Lang {
        name: "python",
        binary: "python3",
        argv: &["python3", "-c", "{SCRIPT}"],
        mode: ScriptMode::Arg,
    },
    Lang {
        name: "node",
        binary: "node",
        argv: &["node", "-e", "{SCRIPT}"],
        mode: ScriptMode::Arg,
    },
    Lang {
        name: "ruby",
        binary: "ruby",
        argv: &["ruby", "-e", "{SCRIPT}"],
        mode: ScriptMode::Arg,
    },
    Lang {
        name: "awk",
        binary: "awk",
        argv: &["awk", "{SCRIPT}"],
        mode: ScriptMode::Arg,
    },
    Lang {
        name: "jq",
        binary: "jq",
        // `-n`: run without input. The sandbox wires NO stdin/file to the
        // child, so plain `jq '.a'` read EOF and printed NOTHING with rc 0 —
        // silent false success on every jq script. With -n, programs must
        // load their data explicitly (`input`? no stdin either — use
        // `$__loc__`-free pure generation or `--slurpfile` via sh). The
        // honest contract: jq here is for GENERATING/TRANSFORMING literals;
        // reading files needs `--lang sh` with `jq . file`.
        argv: &["jq", "-n", "{SCRIPT}"],
        mode: ScriptMode::Arg,
    },
    Lang {
        name: "perl",
        binary: "perl",
        argv: &["perl", "-e", "{SCRIPT}"],
        mode: ScriptMode::Arg,
    },
];

/// Resolve `name` (e.g. "python", "py", "js") to a [`Lang`]. Returns `None`
/// for unknown names. Accepts common aliases.
pub fn lang_for_name(name: &str) -> Option<&'static Lang> {
    let n = name.trim().to_ascii_lowercase();
    let alias = match n.as_str() {
        "py" | "python3" => "python",
        "js" | "javascript" => "node",
        "shell" | "/bin/sh" => "sh",
        "rb" => "ruby",
        other => other,
    };
    LANGS.iter().find(|l| l.name == alias)
}

/// Defaults — generous enough for real exploratory scripts, tight enough
/// that a runaway script (infinite loop, accidental fork bomb) can't hose
/// the user's machine.
pub const DEFAULT_TIMEOUT_SECS: u64 = 30;
pub const DEFAULT_STDOUT_CAP_BYTES: usize = 1024 * 1024; // 1 MiB
pub const DEFAULT_STDERR_CAP_BYTES: usize = 64 * 1024; // 64 KiB

/// Options for a single `execute` call. Defaults via `Default::default()`.
#[derive(Debug, Clone)]
pub struct ExecOptions {
    pub timeout: Duration,
    pub stdout_cap: usize,
    pub stderr_cap: usize,
    /// Working directory. `None` ⇒ current dir.
    pub cwd: Option<PathBuf>,
    /// Extra env vars layered onto the minimal default env.
    pub env: HashMap<String, String>,
    /// If true, run the output through `auto::compact` before returning.
    /// Defaults to true — agents asking for execution almost always want
    /// the answer, not the verbose intermediate.
    pub compact_output: bool,
    /// If true, run `redact::redact` over stdout/stderr before returning.
    /// Defaults to true — a script may accidentally echo a token.
    pub redact_output: bool,
    /// If true, run the interpreter under an OS sandbox (bubblewrap): the
    /// whole filesystem read-only, only a fresh tmpfs `/tmp` writable, so a
    /// script cannot modify any file on disk. Used when an untrusted
    /// sub-agent runs code (see `tokenade agent`); the trusted orchestrator
    /// leaves it false. If requested but bwrap is unavailable, `execute`
    /// errors rather than run unconfined.
    pub sandbox: bool,
    /// When sandboxed, allow network (default: network is unshared/blocked).
    pub sandbox_net: bool,
}

impl Default for ExecOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
            stdout_cap: DEFAULT_STDOUT_CAP_BYTES,
            stderr_cap: DEFAULT_STDERR_CAP_BYTES,
            cwd: None,
            env: HashMap::new(),
            compact_output: true,
            redact_output: true,
            sandbox: false,
            sandbox_net: false,
        }
    }
}

/// What we send back to the caller.
#[derive(Debug, Clone)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub timed_out: bool,
    pub elapsed_ms: u64,
    /// Tokens saved by compaction (if `compact_output` was on). `before`
    /// is the script's raw stdout; `after` is what we returned.
    pub savings: Savings,
    /// The stdout as the script produced it, kept ONLY when compaction
    /// changed it. `exec`'s caller needs it to offer a way back to the bytes
    /// the fold dropped — `wrap` and the MCP proxy both do, and `exec`, which
    /// carries 41 % of all invocations, was the one path that could not.
    pub raw_stdout: Option<String>,
    /// Lang we actually executed under (after alias resolution).
    pub lang: &'static Lang,
    /// The caller piped data in and the script was too large to travel any
    /// way but ON that pipe, so the script ran against an EMPTY stdin. The
    /// comment on [`MAX_SCRIPT_AS_ARG`] says this case "is stated rather than
    /// silently mishandled", and it was stated in `--help` only: measured on
    /// a 72 121-byte bash script reading its stdin, `printf 'DONNEE\\n' |
    /// tokenade exec --lang bash --script-file big.sh` returned **empty
    /// stdout, empty stderr and exit 0**. An answer computed on no input, and
    /// nothing to tell it apart from an answer that is simply empty.
    pub stdin_spent_on_script: bool,
}

/// Is there anything on stdin for a script to lose?
///
/// `forward_stdin` only asks whether stdin is a terminal, which is the right
/// question for FORWARDING — `tokenade exec … < /dev/null` and `cmd |
/// tokenade exec …` are the same fd situation to the child. It is the wrong
/// question for a WARNING: telling somebody their piped data was swallowed
/// when they redirected `/dev/null` is a false alarm on every such run, and a
/// diagnostic that cries wolf is one an agent learns to skip past. A pipe or
/// FIFO always warrants it; a regular file only when it is non-empty. Any
/// other shape (character device, socket, or a platform we cannot ask) keeps
/// the warning, because a missed warning costs a wrong answer and a spurious
/// one costs a line.
fn stdin_carries_data() -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        let Ok(md) = std::fs::metadata("/dev/stdin") else {
            return true;
        };
        let ft = md.file_type();
        if ft.is_fifo() {
            return true;
        }
        if ft.is_file() {
            return md.len() > 0;
        }
        !ft.is_char_device()
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Build the minimal environment we hand the child. Inherits only the
/// variables that real programs need to find files and format output —
/// no AWS_*, no GH_*, no API keys, no PATH overrides from the user shell.
fn minimal_env() -> HashMap<String, String> {
    const PASSTHROUGH: &[&str] = &["PATH", "HOME", "LANG", "LC_ALL", "TZ", "USER", "LOGNAME"];
    let mut env = HashMap::with_capacity(PASSTHROUGH.len());
    for k in PASSTHROUGH {
        if let Ok(v) = std::env::var(k) {
            env.insert((*k).to_string(), v);
        }
    }
    env
}

/// The argv that actually runs: the interpreter call, wrapped in the platform's
/// sandbox launcher when one is asked for and available.
///
/// 46 lines of launcher selection lifted out of `execute`, which is about
/// running a child and draining it — not about which jail exists on this box.
fn sandboxed_argv(argv: Vec<String>, opts: &ExecOptions) -> Result<Vec<String>, String> {
    if opts.sandbox {
        if !crate::availability::is_available("bwrap") {
            return Err(
                "sandbox requested but bubblewrap (`bwrap`) is not available — refusing to run \
             untrusted code unconfined"
                    .to_string(),
            );
        }
        let mut bw: Vec<String> = [
            "bwrap",
            "--ro-bind",
            "/",
            "/",
            "--dev",
            "/dev",
            "--proc",
            "/proc",
            "--tmpfs",
            "/tmp",
            "--die-with-parent",
            "--unshare-pid",
            "--unshare-uts",
            "--unshare-ipc",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        if let Some(cwd) = opts.cwd.as_deref() {
            let c = cwd.display().to_string();
            // Re-expose the working dir read-only AFTER the tmpfs, so a jail
            // that lives under /tmp isn't shadowed by the writable /tmp overlay
            // (otherwise bwrap can't chdir into it).
            bw.push("--ro-bind".into());
            bw.push(c.clone());
            bw.push(c.clone());
            bw.push("--chdir".into());
            bw.push(c);
        }
        if !opts.sandbox_net {
            bw.push("--unshare-net".into());
        }
        bw.push("--".into());
        bw.extend(argv);
        Ok(bw)
    } else {
        Ok(argv)
    }
}

/// Execute `script` in `lang`. Returns once the child exits OR the timeout
/// fires (whichever comes first); a timed-out child is killed.
///
/// Errors:
///   * Returns `Err(String)` only on hard failures (binary missing, spawn
///     failure, IO error). A non-zero exit code is **not** an error —
///     it's reflected in `ExecResult.exit_code`.
pub fn execute(
    lang: &'static Lang,
    script: &str,
    opts: &ExecOptions,
) -> Result<ExecResult, String> {
    if !crate::availability::is_available(lang.binary) {
        let msg = format!(
            "{} is not installed (binary `{}` not found on PATH)",
            lang.name, lang.binary
        );
        crate::debug_log::log(
            "sandbox",
            "binary_missing",
            serde_json::json!({"lang": lang.name, "binary": lang.binary, "error": msg}),
        );
        return Err(msg);
    }
    let start = Instant::now();
    crate::debug_log::log(
        "sandbox",
        "start",
        serde_json::json!({
            "lang": lang.name,
            "script": script,
            "timeout_secs": opts.timeout.as_secs(),
            "stdout_cap_bytes": opts.stdout_cap,
            "cwd": opts.cwd.as_ref().map(|p| p.display().to_string()),
        }),
    );

    // Is the caller PIPING data in? `printf 'a\nb\n' | tokenade exec --lang sh
    // --script 'wc -l'` printed `0`, not `2`, on every version until now: the
    // `Stdin`-mode interpreters take the script on stdin, and `Arg`-mode ones
    // had their stdin CLOSED outright a few lines below. Neither errored, so a
    // pipe produced a confident wrong number rather than a complaint.
    //
    // A terminal is excluded deliberately: forwarding it would make a script
    // that happens to read stdin sit and wait for a human who has no idea it
    // is waiting. Only an actual redirect/pipe is handed through. If the
    // script itself arrived on stdin (`--script -`), the child simply sees
    // EOF, which is harmless rather than wrong.
    let forward_stdin = {
        use std::io::IsTerminal;
        !std::io::stdin().is_terminal()
    };

    /// Ceiling for routing a `Stdin`-mode script through `-c` instead.
    ///
    /// `-s` puts no limit on script size; `-c` spends `ARG_MAX` (2 MiB here),
    /// and `--script-file` / `--script -` can carry a script far larger than
    /// anything `--script` could. Since an agent's stdin is never a terminal,
    /// an unbounded rule would send EVERY such call down the `-c` path and
    /// break scripts that `-s` handled: measured, `sh -c` with a 500 KiB
    /// script fails `Argument list too long`.
    ///
    /// Above the ceiling `-s` is kept, which means data on stdin still cannot
    /// reach the script — the documented scripts are SHORT (≤ ~20 lines), so
    /// the combination "enormous script AND piped data" is the one case left
    /// uncovered, and it is stated rather than silently mishandled.
    const MAX_SCRIPT_AS_ARG: usize = 64 * 1024;

    // ONE computation, two uses, because they must agree: `-s` needs the
    // child's stdin to deliver the SCRIPT, so inheriting stdin there hands the
    // interpreter the user's data as its program. Measured while writing this:
    // a 200 KiB `--script-file` plus a pipe ran `a` as a command.
    let script_as_arg = matches!(lang.mode, ScriptMode::Stdin)
        && forward_stdin
        && script.len() <= MAX_SCRIPT_AS_ARG;
    // The one uncovered combination, named at the moment it happens: a pipe
    // is present, the interpreter takes its script on stdin, and the script
    // is too large to travel as an argument instead. The data is lost either
    // way; what changes is whether the caller is told.
    let stdin_spent_on_script = matches!(lang.mode, ScriptMode::Stdin)
        && forward_stdin
        && script.len() > MAX_SCRIPT_AS_ARG
        && stdin_carries_data();
    let forward_stdin = forward_stdin && (matches!(lang.mode, ScriptMode::Arg) || script_as_arg);

    // Build argv. For `Arg` mode we substitute `{SCRIPT}` with the script
    // content directly — no shell interpolation, no quoting risk.
    let argv: Vec<String> = match lang.mode {
        // `-s` reads the script FROM stdin, which is exactly the channel the
        // data needs. `-c` takes it as one argv element instead — identical
        // semantics, no quoting risk (it is a single arg, never re-parsed) —
        // and only when a pipe is actually present, so the default path keeps
        // its existing `-s` behaviour and its tolerance for a huge script.
        ScriptMode::Stdin if script_as_arg => {
            let mut a: Vec<String> = vec![lang.argv[0].to_string(), "-c".into()];
            a.push(script.to_string());
            a
        }
        ScriptMode::Stdin => lang.argv.iter().map(|s| s.to_string()).collect(),
        ScriptMode::Arg => lang
            .argv
            .iter()
            .map(|s| {
                if *s == "{SCRIPT}" {
                    script.to_string()
                } else {
                    s.to_string()
                }
            })
            .collect(),
    };

    // OS confinement: when requested, wrap the interpreter in bubblewrap so a
    // script can read the machine but write nothing on disk except a fresh
    // tmpfs /tmp. This is what makes it safe to let an untrusted sub-agent run
    // arbitrary code. If bwrap isn't available we refuse rather than run the
    // code unconfined.
    let argv = sandboxed_argv(argv, opts)?;

    let mut env = minimal_env();
    for (k, v) in &opts.env {
        env.insert(k.clone(), v.clone());
    }

    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .env_clear()
        .envs(&env)
        .stdin(if forward_stdin {
            Stdio::inherit()
        } else {
            Stdio::piped()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = opts.cwd.as_deref() {
        cmd.current_dir(cwd);
    }

    // Make the child its own process-group leader on Unix so a timeout
    // can `kill(-pgid)` the entire descendant tree. Without this, a
    // `bash -c "sleep 100"` whose parent we kill leaves `sleep` orphaned,
    // still holding the stdout pipe — our reader threads then block for
    // up to the full sleep duration. Process-group kill is the only
    // portable way to terminate the whole tree without a more exotic
    // (Linux-only) PR_SET_CHILD_SUBREAPER setup.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    // Sandbox stdio is fully piped; on Windows a console-subsystem
    // interpreter would otherwise flash a window when tokenade has none.
    crate::process_util::configure_no_window(&mut cmd);
    crate::process_util::configure_die_with_parent(&mut cmd);

    let mut child = cmd.spawn().map_err(|e| format!("spawn failed: {e}"))?;

    // The group above already lets a TIMEOUT kill the tree. It did nothing for
    // a signal arriving from OUTSIDE: kill `tokenade exec` and the interpreter
    // (plus anything it spawned) kept running, since its group is not ours.
    // Unconditional here, unlike `wrap`: this group exists whether or not a
    // terminal is attached, and the sandbox pipes all three stdio streams, so
    // there is no foreground-group case to defer to.
    crate::process_util::forward_termination_to(child.id());
    // And the kill we cannot catch: a script that outlives `tokenade exec` is
    // the same orphan as a wrapped build, minus the banner that would show it.
    let reaper = crate::process_util::guard_group_against_our_death(child.id());

    // Write the script to stdin only in Stdin mode. Drop stdin afterwards
    // so the child sees EOF (otherwise interpreters like `bash -s` will
    // block forever).
    //
    // When the caller piped data in, `child.stdin` is None (inherited, not
    // piped) and the script travelled by `-c`, so both branches below are
    // no-ops. Spelled out because "it happens to work" is how the next reader
    // gets it wrong.
    if lang.mode == ScriptMode::Stdin {
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            // Best-effort: a broken pipe just means the child has already
            // exited or crashed — `wait` will surface the real reason.
            let _ = stdin.write_all(script.as_bytes());
        }
    } else {
        // Close stdin immediately — we don't pipe anything to `-e`-style scripts.
        drop(child.stdin.take());
    }

    // Read stdout/stderr in background threads so the child can't deadlock
    // on a full pipe buffer. We cap reads at `stdout_cap` / `stderr_cap`
    // to bound memory; anything beyond the cap is dropped with a marker.
    let stdout_buf = Arc::new(Mutex::new(Vec::with_capacity(8192)));
    let stderr_buf = Arc::new(Mutex::new(Vec::with_capacity(2048)));
    let stdout_cap = opts.stdout_cap;
    let stderr_cap = opts.stderr_cap;

    let so_buf = Arc::clone(&stdout_buf);
    let se_buf = Arc::clone(&stderr_buf);
    let truncated = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let truncated_flag = Arc::clone(&truncated);

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let so_thr = thread::spawn(move || {
        if let Some(mut s) = stdout {
            use std::io::Read;
            let mut chunk = [0u8; 8192];
            loop {
                match s.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut b = so_buf.lock().unwrap_or_else(|p| p.into_inner());
                        let remaining = stdout_cap.saturating_sub(b.len());
                        if remaining == 0 {
                            // Drop the rest — we already hit the cap.
                            truncated_flag.store(true, std::sync::atomic::Ordering::Relaxed);
                            continue;
                        }
                        let take = n.min(remaining);
                        if take < n {
                            truncated_flag.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                        b.extend_from_slice(&chunk[..take]);
                    }
                    Err(_) => break,
                }
            }
        }
    });
    let se_thr = thread::spawn(move || {
        if let Some(mut s) = stderr {
            use std::io::Read;
            let mut chunk = [0u8; 4096];
            loop {
                match s.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut b = se_buf.lock().unwrap_or_else(|p| p.into_inner());
                        let remaining = stderr_cap.saturating_sub(b.len());
                        if remaining == 0 {
                            continue;
                        }
                        let take = n.min(remaining);
                        b.extend_from_slice(&chunk[..take]);
                    }
                    Err(_) => break,
                }
            }
        }
    });

    // Poll for completion until the timeout. `try_wait` is non-blocking;
    // we sleep a short interval between polls to keep CPU use trivial.
    let deadline = start + opts.timeout;
    let mut timed_out = false;
    let exit_code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code().unwrap_or(-1),
            Ok(None) => {
                if Instant::now() >= deadline {
                    timed_out = true;
                    kill_tree(&mut child);
                    let _ = child.wait();
                    break -1;
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(_) => break -1,
        }
    };

    // Reaped: the pid could be recycled, so stop aiming signals at it.
    crate::process_util::stop_forwarding_termination();
    if let Some(r) = reaper {
        r.stand_down();
    }

    // Reader threads should finish promptly once the child exits / is killed.
    let _ = so_thr.join();
    let _ = se_thr.join();

    // Decode rather than replace: a script that prints a latin-1 / Shift-JIS /
    // GBK line had every accented byte turned into U+FFFD here, exactly the
    // defect `text_encoding` was written for and was never wired to command
    // output. See `process_util::decode_child_output`.
    let (raw_stdout, _so_faithful) = crate::process_util::decode_child_output(
        &Arc::try_unwrap(stdout_buf)
            .map(|m| m.into_inner().unwrap_or_default())
            .unwrap_or_default(),
    );
    let (raw_stderr, _se_faithful) = crate::process_util::decode_child_output(
        &Arc::try_unwrap(stderr_buf)
            .map(|m| m.into_inner().unwrap_or_default())
            .unwrap_or_default(),
    );

    // Redact secrets before any further processing. We want compaction to
    // see redacted text (so it doesn't try to dedupe around a redacted
    // line in a confusing way).
    let stdout_redacted = if opts.redact_output {
        redact::redact(&raw_stdout)
    } else {
        raw_stdout.clone()
    };
    let stderr_redacted = if opts.redact_output {
        redact::redact(&raw_stderr)
    } else {
        raw_stderr
    };

    // Compaction is a "first try; fall back if nothing changed" — auto
    // returns `(Kind, body, savings)` where body is the compacted output
    // or, when it didn't recognise the format, the original text.
    let mut raw_stdout: Option<String> = None;
    let (final_stdout, savings) = if opts.compact_output && !stdout_redacted.is_empty() {
        let (_kind, body, sav) = auto::compact(&stdout_redacted);
        if body != stdout_redacted {
            raw_stdout = Some(stdout_redacted.clone());
        }
        // L81 — anti-inflation guard via shared helper. See
        // [`crate::process_util::pick_compact_or_raw`] for rationale.
        let raw = stdout_redacted;
        crate::fold_count::pick_compact_or_raw(raw.clone(), body, sav, || {
            crate::tokenizer::estimate(&raw)
        })
    } else {
        let n = crate::tokenizer::estimate(&stdout_redacted);
        (stdout_redacted, Savings::new(n, n))
    };

    // Surface the truncation: an agent given a silently-cut answer treats
    // it as complete (the doc always claimed a marker; none was emitted).
    let mut final_stdout = final_stdout;
    if truncated.load(std::sync::atomic::Ordering::Relaxed) {
        final_stdout.push_str(&format!(
            // `--cap`, which is what the binary takes. It said `--stdout-cap`
            // for the life of the flag, and the parser answers that with
            // `unknown flag` and exit 2 — so an agent that did what the
            // truncation notice told it lost the turn AND the output, on the
            // command that carries 41 % of all invocations. Same shape as the
            // timeout notice, which names `--timeout` and its ceiling.
            "\n[tokenade:exec] stdout truncated at {} bytes (cap) — narrow the output, or raise it with `--cap N` (max 16 MiB)\n",
            stdout_cap
        ));
    }
    let elapsed_ms = start.elapsed().as_millis() as u64;
    crate::debug_log::log(
        "sandbox",
        if timed_out { "timeout" } else { "done" },
        serde_json::json!({
            "lang": lang.name,
            "exit_code": exit_code,
            "timed_out": timed_out,
            "elapsed_ms": elapsed_ms,
            "tokens_before": savings.tokens_before,
            "tokens_after": savings.tokens_after,
            "stdout_bytes": final_stdout.len(),
            "stderr_bytes": stderr_redacted.len(),
        }),
    );
    Ok(ExecResult {
        stdout: final_stdout,
        stderr: stderr_redacted,
        exit_code,
        timed_out,
        elapsed_ms,
        savings,
        raw_stdout,
        lang,
        stdin_spent_on_script,
    })
}

/// Kill the entire descendant process group of `child`. Unix-only — falls
/// back to a plain `child.kill()` (which only signals the direct child)
/// on other platforms. Required to terminate scripts that spawned long-
/// running subprocesses (e.g. `bash -c "sleep 100"`).
fn kill_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let pid = child.id() as i32;
        // SAFETY: we pass a valid pid we own and SIGKILL is always defined.
        // `kill(-pgid, SIGKILL)` signals every process in pgid (which equals
        // the child pid since we did process_group(0)). Errors are ignored:
        // the worst case is the child died between our `try_wait` and the
        // signal, which is harmless.
        unsafe {
            libc_kill(-pid, 9);
        }
        return;
    }
    #[allow(unreachable_code)]
    {
        let _ = child.kill();
    }
}

#[cfg(unix)]
extern "C" {
    /// Bare libc `kill(2)` — pulling in the `libc` crate just for this
    /// would be heavy; we declare the prototype directly. The function is
    /// in libc (linked by default), and the signature is stable across
    /// every Unix we ship on.
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

/// Convenience: same as `execute` but accepts a lang name and resolves it.
pub fn execute_named(
    lang_name: &str,
    script: &str,
    opts: &ExecOptions,
) -> Result<ExecResult, String> {
    let lang = lang_for_name(lang_name)
        .ok_or_else(|| format!("unknown lang '{lang_name}'; known: {}", known_lang_names()))?;
    execute(lang, script, opts)
}

/// Comma-separated list of supported lang names, for error messages.
pub fn known_lang_names() -> String {
    LANGS.iter().map(|l| l.name).collect::<Vec<_>>().join(", ")
}

/// Probe the host once and return the langs whose interpreter is installed.
/// Used by `tools/list` filtering and `tokenade execute --list` UX.
pub fn available_langs() -> Vec<&'static Lang> {
    LANGS
        .iter()
        .filter(|l| crate::availability::is_available(l.binary))
        .collect()
}

/// Quick sanity: is the path acceptable as a `cwd` (exists, is dir)?
pub fn check_cwd(p: &Path) -> Result<(), String> {
    if !p.exists() {
        return Err(format!("cwd does not exist: {}", p.display()));
    }
    if !p.is_dir() {
        return Err(format!("cwd is not a directory: {}", p.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skip_if_no(binary: &str) -> bool {
        if !crate::availability::is_available(binary) {
            eprintln!("[skip] {binary} not installed");
            return true;
        }
        false
    }

    #[test]
    fn lang_lookup_resolves_aliases() {
        assert_eq!(lang_for_name("py").map(|l| l.name), Some("python"));
        assert_eq!(lang_for_name("js").map(|l| l.name), Some("node"));
        assert_eq!(lang_for_name("Python3").map(|l| l.name), Some("python"));
        assert_eq!(lang_for_name("unknown-lang"), None);
    }

    #[test]
    fn bash_echo_returns_stdout() {
        if skip_if_no("bash") {
            return;
        }
        let lang = lang_for_name("bash").unwrap();
        let opts = ExecOptions {
            compact_output: false,
            ..Default::default()
        };
        let r = execute(lang, "echo hello", &opts).unwrap();
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("hello"));
        assert!(!r.timed_out);
    }

    #[test]
    fn nonzero_exit_is_not_an_error() {
        if skip_if_no("bash") {
            return;
        }
        let lang = lang_for_name("bash").unwrap();
        let r = execute(lang, "exit 42", &ExecOptions::default()).unwrap();
        assert_eq!(r.exit_code, 42);
    }

    #[test]
    fn timeout_kills_runaway_child() {
        if skip_if_no("bash") {
            return;
        }
        let lang = lang_for_name("bash").unwrap();
        let opts = ExecOptions {
            timeout: Duration::from_millis(200),
            ..Default::default()
        };
        let r = execute(lang, "sleep 10", &opts).unwrap();
        assert!(r.timed_out, "expected timeout flag");
        assert!(r.elapsed_ms < 1000, "killed quickly");
    }

    #[test]
    fn output_is_capped() {
        if skip_if_no("bash") {
            return;
        }
        let lang = lang_for_name("bash").unwrap();
        let opts = ExecOptions {
            stdout_cap: 64,
            compact_output: false,
            ..Default::default()
        };
        // Spit 10 KiB; we should see at most 64 bytes back.
        let r = execute(lang, "yes 'xxxxxxxx' | head -c 10000", &opts).unwrap();
        // Cap applies to the payload; a truncation MARKER is appended so
        // the agent knows the answer was cut (it used to be silent).
        assert!(
            r.stdout.contains("[tokenade:exec] stdout truncated"),
            "missing truncation marker: {}",
            r.stdout
        );
        let payload = r.stdout.split("\n[tokenade:exec]").next().unwrap();
        assert!(
            payload.len() <= 64,
            "stdout cap violated, got {} bytes",
            payload.len()
        );
    }

    #[test]
    fn secrets_are_redacted_in_output() {
        if skip_if_no("bash") {
            return;
        }
        let lang = lang_for_name("bash").unwrap();
        // GitHub PAT pattern.
        let script = "echo 'token=ghp_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA'";
        let r = execute(lang, script, &ExecOptions::default()).unwrap();
        assert!(
            !r.stdout
                .contains("ghp_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
            "PAT not redacted: {:?}",
            r.stdout
        );
    }

    #[test]
    fn env_is_minimal_no_aws_leak() {
        if skip_if_no("bash") {
            return;
        }
        // SAFETY: process-wide env; we restore.
        std::env::set_var("AWS_SECRET_ACCESS_KEY", "test-no-leak");
        let lang = lang_for_name("bash").unwrap();
        let opts = ExecOptions {
            compact_output: false,
            redact_output: false,
            ..Default::default()
        };
        let r = execute(lang, "echo $AWS_SECRET_ACCESS_KEY", &opts).unwrap();
        std::env::remove_var("AWS_SECRET_ACCESS_KEY");
        assert!(
            !r.stdout.contains("test-no-leak"),
            "AWS_SECRET leaked into child env: {:?}",
            r.stdout
        );
    }

    #[test]
    fn unknown_lang_is_an_error() {
        let r = execute_named("perl6", "say 1", &ExecOptions::default());
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("unknown lang"));
    }
}
