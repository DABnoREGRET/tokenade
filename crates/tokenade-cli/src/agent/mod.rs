//! # WORK IN PROGRESS — PAUSED, NOT COMPILED, NOT SHIPPED
//!
//! Nothing in this directory is built by the default profile. It lives behind
//! `--features light-agent` (and `local-llm` for the on-device engine), the
//! released binary exposes no `agent` / `local-serve` command, and it links no
//! llama.cpp. It is kept in-tree because it is a lot of working code that may
//! come back — not because it is on its way in.
//!
//! **Why it is paused (2026-07):** no approach to local exploration agents has
//! earned its place yet. Delegation benched net-negative — it added latency and
//! turns without beating direct compacted exploration, with many trap cases —
//! and the local-model path is slow and weak on customer-grade hardware. The
//! direction is unproven, not merely unfinished.
//!
//! **If you switch it on, read this first:** `local::n_ctx` defaults to the
//! model's FULL trained context (40 960 on Qwen3-4B, ~130 000 on MiniCPM5). The
//! KV cache is ~144 KB/token on Qwen3-4B, so that default alone reserves **~6 GB
//! of RAM**, and a `local-serve` left running holds it (measured: 9.5 GB RSS,
//! 12 GB peak). Set `TOKENADE_LOCAL_CTX` before doing anything else.
//!
//! Do not wire this into a hook, a scaffold, or a default path without a bench
//! that beats the direct route. That bench does not exist today.
//!
//! ---
//!
//! `tokenade agent` — spawn a zero-cost light-agent to solve one scoped,
//! simple task (code exploration, output digestion, web lookup, info
//! retrieval) on free no-signup models, inside an orchestrator-set
//! sandbox, with NO ability to spawn further sub-agents.
//!
//! Built from two third-party references:
//!   - OmniRoute  → free-model client (fallback chain + retry): `providers`
//!   - opencode   → sandboxed agent loop + permission gating: `runner`/`sandbox`/`tools`
//!
//! The orchestrator (Claude) is told about this feature via the project
//! scaffold (see `project_scaffold::scaffold_block`); the delegate itself
//! is NOT told it can delegate — recursion is impossible by construction.

// Local-model path (llama.cpp engine + native function-calling loop). Kept in
// the tree but compiled ONLY with `--features local-llm`: benched too slow /
// too weak on customer-grade hardware — the free-tier provider chain is the
// production path.
#[cfg(feature = "local-llm")]
mod local;
mod pool;
mod providers;
mod runner;
mod sandbox;
// Scout v2 (experimental): bounded SLM pipelines with
// grammar-constrained decoding + verified grounding. Needs the local engine.
#[cfg(feature = "local-llm")]
mod explore;
#[cfg(feature = "local-llm")]
mod scout;
#[cfg(feature = "local-llm")]
mod toolcall;
mod tools;
mod trace;

#[cfg(feature = "local-llm")]
pub use scout::cmd_scout;

/// Stub when the binary was built without the local engine.
#[cfg(not(feature = "local-llm"))]
pub fn cmd_scout(_args: &[String]) -> i32 {
    eprintln!(
        "[tokenade scout] this binary was built without the local model engine — \
         rebuild with `--features local-llm`."
    );
    2
}

use sandbox::{Capability, Sandbox};
use std::path::{Path, PathBuf};
use tokenade_core::gain;

/// Startup housekeeping: prune agent audit traces older than 7 days. Safe for
/// gain accounting (gains live in the ledger, not these dirs). Throttled to one
/// scan per 12h.
pub fn gc_traces() {
    trace::gc_old_traces();
}

/// `tokenade local-serve` — run the local model inference server (loads a small
/// GGUF once, serves many agent clients concurrently over a socket). Requires
/// the `local-llm` build feature.
#[cfg(feature = "local-llm")]
pub fn cmd_local_serve(_args: &[String]) -> i32 {
    match local::serve() {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("[tokenade local-serve] {e}");
            1
        }
    }
}

/// Stub when the binary was built without the local engine.
#[cfg(not(feature = "local-llm"))]
pub fn cmd_local_serve(_args: &[String]) -> i32 {
    eprintln!(
        "[tokenade local-serve] this binary was built without the local model \
         engine — rebuild with `--features local-llm`."
    );
    1
}

const HELP: &str = "\
tokenade agent — delegate a simple task to a free light-agent (zero token cost)

USAGE:
  tokenade agent \"<task>\" [options]   # non-blocking: prints TRACE-DIR then PID
  tokenade agent --follow <dir>       # block until DONE, print the answer
  tokenade agent --collect <dir>      # fetch the result (waits up to 60s)
  tokenade agent --status <dir>       # instant one-line state (done/running/error)
  echo \"<task>\" | tokenade agent - [options]

Launch is always non-blocking: it returns immediately, printing the audit
trace directory on line 1 and the worker PID on line 2.
BEST PATTERN (agents with background execution, e.g. Claude Code): launch,
then run `tokenade agent --follow <dir>` AS A BACKGROUND task — you are
notified the moment it exits with the answer. No polling, no tool timeout.
Otherwise: `--collect <dir>` waits up to TOKENADE_AGENT_COLLECT_SECS (default
60) then exits 2 with a status line — do other work and re-collect; `--status`
answers instantly.

DEFAULT SANDBOX (jailed to cwd): read files · web search/fetch · run scripts ·
  create/edit/delete files IT created (provenance-tracked). It CANNOT touch
  files it didn't create, leave the jail, mutate files from raw shell, or spawn
  sub-agents.

OPTIONS:
  --dir <path>        add a directory to the jail (repeatable; default: cwd)
  --cap <list>        set an explicit capability set instead of the default:
                      shell,net,write,delete (comma-sep; read always implied)
  --read-only         narrow to read-only (no net/shell/write/delete)
  --model <sel>       free|auto (default) | <provider> | <provider>/<model>
  --steps <n>         max tool iterations (default 30)
  --follow <dir>      block until the run finishes, then print the answer
  --collect <dir>     print a launched run's result (waits up to 60s, exit 2 if still running)
  --status <dir>      one-line state, instant (exit 0 done / 2 running / 1 error)
  --json              emit a JSON result ({answer, agent_in, agent_out, saved})
  -h, --help          this help

MODELS:
  Tries free no-signup endpoints (opencode-zen, theoldllm, …) in a fallback
  chain. For a guaranteed path, point it at any OpenAI-compatible endpoint:
    TOKENADE_AGENT_BASE_URL   e.g. http://localhost:11434/v1 (local Ollama — free, no signup)
    TOKENADE_AGENT_MODEL      e.g. llama3.2  (default gpt-4o-mini)
    TOKENADE_AGENT_API_KEY    optional bearer, sent ONLY to that endpoint

EXAMPLES:
  tokenade agent \"where is the retry/backoff logic? give file:line\"
  tokenade agent \"summarize the failures\" --cap shell        # runs read-only cmds
  tokenade agent \"what's the latest tokio release?\" --cap net # web lookup
  tokenade agent \"list every pub fn in gain.rs\" --model opencode-zen
";

pub fn cmd_agent(args: &[String]) -> i32 {
    // Internal worker mode: the detached background process that actually runs
    // the agent and streams the transcript into the trace dir.
    if let Some(i) = args.iter().position(|a| a == "--_detached-run") {
        return match args.get(i + 1) {
            Some(dir) => run_job(std::path::Path::new(dir), false),
            None => 2,
        };
    }
    // Follow a launched run to COMPLETION (no time cap). Designed for the
    // caller's background-execution facility (e.g. Claude Code's Bash
    // `run_in_background`): the caller is notified when this exits — no
    // polling, no tool-timeout risk.
    if let Some(i) = args.iter().position(|a| a == "--follow") {
        let json = args.iter().any(|a| a == "--json");
        return match args.get(i + 1) {
            Some(dir) => follow(std::path::Path::new(dir), json),
            None => {
                eprintln!("[tokenade agent] --follow needs a trace-dir path");
                2
            }
        };
    }
    // Instant one-line status of a launched run (any agent can poll this
    // cheaply between other work).
    if let Some(i) = args.iter().position(|a| a == "--status") {
        return match args.get(i + 1) {
            Some(dir) => status(std::path::Path::new(dir)),
            None => {
                eprintln!("[tokenade agent] --status needs a trace-dir path");
                2
            }
        };
    }
    // Collect the result of a previously-launched run.
    if let Some(i) = args.iter().position(|a| a == "--collect") {
        let json = args.iter().any(|a| a == "--json");
        return match args.get(i + 1) {
            Some(dir) => collect(std::path::Path::new(dir), json),
            None => {
                eprintln!("[tokenade agent] --collect needs a trace-dir path");
                2
            }
        };
    }
    if crate::help_routing::asks_for_help(args) {
        print!("{HELP}");
        return 0;
    }
    // --- parse ---
    let mut task: Option<String> = None;
    let mut dirs: Vec<PathBuf> = Vec::new();
    // None → the capable default box (read+net+shell+write+delete, provenance
    // gated). `--cap` sets an explicit set; `--read-only` narrows to read.
    let mut caps: Option<Vec<Capability>> = None;
    // Sub-agents run on the free-provider chain by default (`auto` = fallback
    // across free no-signup models). `--model <x>` still overrides; `local`
    // needs a `--features local-llm` build.
    let mut model = String::from("auto");
    let mut steps: u32 = 30;
    let mut json = false;

    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--dir" | "-d" => {
                i += 1;
                if let Some(p) = args.get(i) {
                    dirs.push(PathBuf::from(p));
                }
            }
            "--cap" | "--caps" => {
                i += 1;
                let set = caps.get_or_insert_with(|| vec![Capability::Read]);
                if let Some(list) = args.get(i) {
                    for c in list.split(',') {
                        match Capability::parse(c) {
                            Some(cap) => set.push(cap),
                            None if c.trim().is_empty() => {}
                            None => {
                                eprintln!("[tokenade agent] unknown capability `{c}` (want: shell,net,write,delete)");
                                return 2;
                            }
                        }
                    }
                }
            }
            "--read-only" | "--readonly" | "--ro" => {
                caps = Some(vec![Capability::Read]);
            }
            "--model" | "-m" => {
                i += 1;
                if let Some(m) = args.get(i) {
                    model = m.clone();
                }
            }
            "--steps" | "-s" => {
                i += 1;
                steps = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(steps);
            }
            "--json" => json = true,
            // `--wait` retired: launches are always non-blocking (a blocking
            // multi-minute run gets killed by the caller's own tool timeout).
            // Swallow the flag so older prompts don't break.
            "--wait" => {}
            "-" => {
                use std::io::Read;
                let mut s = String::new();
                let _ = crate::bounded_input::read_stdin_lossy(&mut s);
                task = Some(s.trim().to_string());
            }
            other if !other.starts_with('-') && task.is_none() => {
                task = Some(other.to_string());
            }
            other => {
                eprintln!("[tokenade agent] unexpected arg `{other}` (see --help)");
                return 2;
            }
        }
        i += 1;
    }

    let task = match task {
        Some(t) if !t.trim().is_empty() => t,
        _ => {
            eprintln!("[tokenade agent] no task given. `tokenade agent --help`");
            return 2;
        }
    };

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let roots = if dirs.is_empty() {
        vec![cwd.clone()]
    } else {
        dirs
    };
    let roots_abs: Vec<String> = roots
        .iter()
        .map(|p| {
            p.canonicalize()
                .unwrap_or_else(|_| p.clone())
                .display()
                .to_string()
        })
        .collect();
    // Default = the capable-but-safe box; `--cap`/`--read-only` override.
    let caps = caps.unwrap_or_else(|| {
        vec![
            Capability::Read,
            Capability::Net,
            Capability::Shell,
            Capability::Write,
            Capability::Delete,
        ]
    });
    let cap_names: Vec<String> = caps.iter().map(|c| cap_name(*c).to_string()).collect();
    let exe = std::env::current_exe()
        .ok()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "tokenade".into());

    // Trace dir + a self-contained job spec, so the detached worker needs
    // nothing but this directory.
    let dir = match trace::new_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("[tokenade agent] cannot create trace dir: {e}");
            return 1;
        }
    };
    trace::write_meta(
        &dir,
        &serde_json::json!({
            "task": task, "roots": roots_abs, "caps": cap_names,
            "model": model, "steps": steps, "json": json,
            "exe": exe, "status": "starting", "created": tokenade_core::platform::now_unix_secs(),
        }),
    );

    // Always non-blocking: spawn a detached worker and return the handle.
    // (Inline `--wait` retired — a multi-minute blocking run gets killed by
    // the calling agent's own tool timeout; fire-then-collect never does.)

    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("agent")
        .arg("--_detached-run")
        .arg(&dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    tokenade_core::process_util::configure_no_window(&mut cmd);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0); // detach from the caller's group so it survives
    }
    match cmd.spawn() {
        Ok(child) => {
            trace::patch_meta(
                &dir,
                serde_json::json!({"pid": child.id(), "status": "running"}),
            );
            // FIRST: the trace dir, then the PID — for auditability.
            println!("{}", dir.display());
            println!("{}", child.id());
            eprintln!(
                "[tokenade agent] launched (non-blocking) — collect with: tokenade agent --collect {}",
                dir.display()
            );
            0
        }
        Err(e) => {
            eprintln!("[tokenade agent] failed to spawn worker: {e}");
            1
        }
    }
}

/// Reconstruct a job from its trace dir's meta.json, run the agent (streaming
/// the transcript into the dir), and write result.json. `inline` prints the
/// answer + banner (blocking `--wait` mode); the detached worker leaves stdout
/// silent.
fn run_job(dir: &Path, inline: bool) -> i32 {
    let meta = match trace::read_meta(dir) {
        Some(m) => m,
        None => {
            if inline {
                eprintln!("[tokenade agent] missing meta.json in {}", dir.display());
            }
            return 1;
        }
    };
    let task = meta["task"].as_str().unwrap_or("").to_string();
    let roots: Vec<PathBuf> = meta["roots"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default();
    let caps: Vec<Capability> = meta["caps"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .filter_map(Capability::parse)
                .collect()
        })
        .unwrap_or_else(|| vec![Capability::Read]);
    let model = meta["model"].as_str().unwrap_or("auto").to_string();
    let steps = meta["steps"].as_u64().unwrap_or(30) as u32;
    let json = meta["json"].as_bool().unwrap_or(false);
    let exe = meta["exe"].as_str().unwrap_or("tokenade").to_string();

    // Run inside the jail so relative paths resolve there.
    if let Some(root) = roots.first() {
        let _ = std::env::set_current_dir(root);
    }
    trace::patch_meta(
        dir,
        serde_json::json!({"status": "running", "pid": std::process::id()}),
    );
    trace::append_event(
        dir,
        serde_json::json!({"kind": "start", "task": task, "model": model}),
    );

    let sb = Sandbox { roots, caps };
    let scaffold = tokenade_core::project_scaffold::scaffold_block_plain();
    let spec = runner::AgentSpec {
        task: &task,
        sandbox: sb,
        model,
        max_steps: steps,
        exe,
        trace: Some(dir.to_path_buf()),
    };

    let outcome = match runner::run(&spec, &scaffold) {
        Ok(o) => o,
        Err(e) => {
            trace::patch_meta(dir, serde_json::json!({"status": "error", "error": e}));
            trace::write_result(dir, &serde_json::json!({"error": e}));
            if inline {
                eprintln!("[tokenade agent] {e}");
            }
            return 1;
        }
    };

    let tool_in = providers_est(&task) + BRIEF_OVERHEAD;
    let tool_out = providers_est(&outcome.answer);
    gain::record_light_agent(
        &task_kind(&task),
        &format!("{}/{}", outcome.provider, outcome.model),
        outcome.agent_in,
        outcome.agent_out,
        tool_in,
        tool_out,
    );
    let paid = tool_in + tool_out;
    let saved = (outcome.agent_in + outcome.agent_out).saturating_sub(paid);
    let result = serde_json::json!({
        "answer": outcome.answer, "provider": outcome.provider, "model": outcome.model,
        "steps": outcome.steps, "agent_in": outcome.agent_in, "agent_out": outcome.agent_out,
        "tool_in": tool_in, "tool_out": tool_out, "saved": saved,
    });
    trace::write_result(dir, &result);
    trace::patch_meta(dir, serde_json::json!({"status": "done"}));

    if inline {
        if json {
            println!("{result}");
        } else {
            println!("{}", outcome.answer);
            print_banner(&outcome, paid, saved);
        }
    }
    0
}

/// Read (optionally waiting for) a launched run's result.
/// Print a finished run's result (answer + accounting line). Shared by
/// `--collect` and `--follow`.
fn print_result(v: &serde_json::Value, json: bool) -> i32 {
    if let Some(err) = v["error"].as_str() {
        eprintln!("[tokenade agent] run failed: {err}");
        return 1;
    }
    if json {
        println!("{v}");
    } else {
        println!("{}", v["answer"].as_str().unwrap_or(""));
        eprintln!(
            "[tokenade agent] {}/{} · {} steps · saved {} tok",
            v["provider"].as_str().unwrap_or(""),
            v["model"].as_str().unwrap_or(""),
            v["steps"].as_u64().unwrap_or(0),
            v["saved"].as_u64().unwrap_or(0),
        );
    }
    0
}

/// True when the worker process is demonstrably gone. Conservative: unknown →
/// assume alive.
///
/// It asked `/proc/<pid>` under `#[cfg(unix)]`, and macOS is `cfg(unix)` with
/// no `/proc` — so on macOS this answered "demonstrably gone" for every LIVE
/// worker, and `follow` stopped following a run that was working. The doc line
/// above promised the conservative direction and the code delivered its
/// opposite. `process_is_alive` asks `kill(pid, 0)` instead, in one place.
fn worker_dead(meta: &Option<serde_json::Value>) -> bool {
    let Some(pid) = meta.as_ref().and_then(|m| m["pid"].as_u64()) else {
        return false;
    };
    let Ok(pid) = u32::try_from(pid) else {
        return false;
    };
    pid > 0 && !tokenade_core::process_util::process_is_alive(pid)
}

/// Follow a launched run until it FINISHES, then print the result. No time
/// cap — this is the mode to run inside the caller's background-execution
/// facility (Claude Code: Bash `run_in_background: true`), which notifies the
/// caller on exit. Guards against a dead worker so it can never hang forever.
fn follow(dir: &Path, json: bool) -> i32 {
    loop {
        if let Some(v) = std::fs::read_to_string(trace::result_path(dir))
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        {
            return print_result(&v, json);
        }
        let meta = trace::read_meta(dir);
        let st = meta
            .as_ref()
            .and_then(|m| m["status"].as_str())
            .unwrap_or("unknown");
        if st == "error" {
            eprintln!("[tokenade agent] run errored (see {})", dir.display());
            return 1;
        }
        if worker_dead(&meta) {
            // Tiny grace for the result file to land after process exit.
            std::thread::sleep(std::time::Duration::from_millis(700));
            if !trace::result_path(dir).exists() {
                eprintln!(
                    "[tokenade agent] worker died without a result (see {})",
                    dir.display()
                );
                return 1;
            }
            continue;
        }
        std::thread::sleep(std::time::Duration::from_millis(800));
    }
}

/// Instant one-line status of a launched run: done / running (+ last action)
/// / error. Exit code mirrors collect: 0 done, 2 running, 1 error.
fn status(dir: &Path) -> i32 {
    if let Some(v) = std::fs::read_to_string(trace::result_path(dir))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
    {
        if v["error"].as_str().is_some() {
            println!("error — collect for details");
            return 1;
        }
        println!(
            "done · {} steps — fetch with --collect {}",
            v["steps"].as_u64().unwrap_or(0),
            dir.display()
        );
        return 0;
    }
    let meta = trace::read_meta(dir);
    if meta.as_ref().and_then(|m| m["status"].as_str()) == Some("error") || worker_dead(&meta) {
        println!("error — worker gone without a result");
        return 1;
    }
    // Last transcript event = the most recent visible activity.
    let last = std::fs::read_to_string(trace::transcript_path(dir))
        .ok()
        .and_then(|s| {
            s.lines()
                .rev()
                .find_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        });
    let (step, act) = last
        .map(|e| {
            (
                e["step"].as_u64().unwrap_or(0),
                e["action"]
                    .as_str()
                    .or_else(|| e["kind"].as_str())
                    .unwrap_or("working")
                    .chars()
                    .take(60)
                    .collect::<String>(),
            )
        })
        .unwrap_or((0, "starting".into()));
    println!("running · step {step} · last: {act}");
    2
}

/// Fetch a launched run's result. Waits up to `TOKENADE_AGENT_COLLECT_SECS`
/// (default 60 — safely under any caller's own tool timeout) for the worker to
/// finish; if it is still running past that, prints a status line and exits 2
/// so the caller re-collects later instead of being killed mid-wait.
fn collect(dir: &Path, json: bool) -> i32 {
    let max_secs: u64 = std::env::var("TOKENADE_AGENT_COLLECT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);
    let started = std::time::Instant::now();
    loop {
        if let Some(v) = std::fs::read_to_string(trace::result_path(dir))
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        {
            return print_result(&v, json);
        }
        let meta = trace::read_meta(dir);
        let status = meta
            .as_ref()
            .and_then(|m| m["status"].as_str())
            .unwrap_or("unknown");
        if status == "error" {
            eprintln!("[tokenade agent] run errored (see {})", dir.display());
            return 1;
        }
        if started.elapsed().as_secs() >= max_secs {
            let pid = meta.as_ref().and_then(|m| m["pid"].as_u64()).unwrap_or(0);
            eprintln!(
                "[tokenade agent] still running (pid {pid}) after {max_secs}s — do other work, \
                 then re-run `tokenade agent --collect {}`",
                dir.display()
            );
            return 2;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

fn cap_name(c: Capability) -> &'static str {
    match c {
        Capability::Read => "read",
        Capability::Net => "net",
        Capability::Shell => "shell",
        Capability::Write => "write",
        Capability::Delete => "delete",
    }
}

fn print_banner(o: &runner::AgentOutcome, paid: u64, saved: u64) {
    if crate::dashboard_cli::user_lang_is_fr() {
        eprintln!(
            "[tokenade agent] {} · {} étapes · agent {}in/{}out gratuits · vous avez payé ~{} · net −{} tok",
            spec_model_label(o), o.steps, o.agent_in, o.agent_out, paid, saved,
        );
    } else {
        eprintln!(
            "[tokenade agent] {} · {} steps · agent {}in/{}out free · you paid ~{} · net −{} tok",
            spec_model_label(o),
            o.steps,
            o.agent_in,
            o.agent_out,
            paid,
            saved,
        );
    }
}

const BRIEF_OVERHEAD: u64 = 120; // fixed orchestration framing per delegation

fn providers_est(s: &str) -> u64 {
    (s.chars().count() / 4) as u64
}

fn spec_model_label(o: &runner::AgentOutcome) -> String {
    if o.provider.is_empty() {
        o.model.clone()
    } else {
        format!("{}/{}", o.provider, o.model)
    }
}

/// Bucket the task for the ledger `op` field (grouping in the dashboard).
fn task_kind(task: &str) -> String {
    let t = task.to_ascii_lowercase();
    let kind = if t.contains("http")
        || t.contains("web")
        || t.contains("latest")
        || t.contains("search the")
    {
        "web_lookup"
    } else if t.contains("summar")
        || t.contains("digest")
        || t.contains("failures")
        || t.contains("output")
    {
        "output_digestion"
    } else if t.contains("where")
        || t.contains("find")
        || t.contains("locate")
        || t.contains("which file")
    {
        "code_exploration"
    } else if t.contains("list") || t.contains("read") || t.contains("extract") {
        "info_retrieval"
    } else {
        "delegate"
    };
    kind.to_string()
}
