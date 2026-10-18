//! Installing rank 4 in front of a REAL agent, and watching the request land.
//!
//! Every other test in this module proves the proxy behaves. These prove the
//! only thing those cannot: that after `install::apply` rewrote a config file,
//! the agent actually reads it, actually points at us, and actually sends. That
//! is the step where a measured key path turns out to be the wrong one, and no
//! amount of unit testing finds it.
//!
//! The shape of each test:
//!
//! 1. a fake provider on loopback, recording what it receives;
//! 2. the real [`server::Proxy`] in front of it, in this process;
//! 3. a throwaway home, an agent config in it, then `install::plan` + `apply`;
//! 4. the agent's own binary, spawned with that home, given one prompt;
//! 5. assert the FAKE PROVIDER saw a request — i.e. it travelled agent → proxy
//!    → provider.
//!
//! No network and no credit is spent: the proxy forwards to the fake provider,
//! so the agent's key is never used against anything real. A dummy key is put
//! in the CHILD's environment, which is not the thing the operator ruled out —
//! that was writing a base URL into a user's shell rc file. Nothing here writes
//! outside the throwaway home.
//!
//! `#[ignore]` because they need the agent installed on the machine. Run with:
//!
//! ```text
//! cargo test -p tokenade-cli --bin tokenade live_agent -- --ignored --nocapture
//! ```

use super::*;
use std::io::{BufReader, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokenade_core::detect::Agent;

/// How long we let an agent get its first request out.
///
/// Generous: a cold agent binary loads a runtime, reads config, and may probe
/// for updates. The test does not wait for the agent to FINISH — only for the
/// provider to have been reached.
const REACH_TIMEOUT: Duration = Duration::from_secs(60);

/// A provider that counts requests and answers something each wire accepts.
fn counting_provider(body: &'static str) -> (String, Arc<AtomicUsize>) {
    let l = TcpListener::bind("127.0.0.1:0").expect("bind provider");
    let addr = l.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let out = Arc::clone(&hits);
    std::thread::spawn(move || {
        for stream in l.incoming() {
            let Ok(mut stream) = stream else { continue };
            let hits = Arc::clone(&hits);
            std::thread::spawn(move || {
                let Ok(peer) = stream.try_clone() else { return };
                let mut r = BufReader::new(peer);
                while http::read_request(&mut r).is_ok() {
                    hits.fetch_add(1, Ordering::SeqCst);
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.flush();
                }
            });
        }
    });
    (format!("http://{addr}"), out)
}

/// The real proxy, in this process, in front of `upstream`.
fn proxy_in_front_of(upstream: &str, agent: Agent) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind proxy");
    let addr = listener.local_addr().unwrap().to_string();
    let proxy = Arc::new(server::Proxy::new(
        config::Config {
            listen: addr.clone(),
            upstream: upstream.to_string(),
            agent: Some(agent),
            ..config::Config::default()
        },
        server::default_folder(),
        Box::new(server::NoAccounting),
    ));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let p = Arc::clone(&proxy);
            std::thread::spawn(move || p.handle_for_test(stream));
        }
    });
    addr
}

/// A throwaway home that cleans up after itself.
struct Home(std::path::PathBuf);
impl Home {
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn home(tag: &str) -> Home {
    let d = std::env::temp_dir().join(format!("tokenade-live-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("temp home");
    Home(d)
}

fn have(binary: &str) -> bool {
    std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {binary}"))
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Spawn an agent in its OWN process group, with a reaper that outlives us.
///
/// The question `every_process_launcher_either_guards_its_work_or_says_why_not`
/// asks is "who kills this when tokenade is SIGKILLed?", and for a test that
/// launches a real coding agent the answer must not be "nobody". A killed test
/// binary that leaves a `codex` running is a process the developer did not start
/// and will not find, holding a config we are about to delete out from under it.
///
/// `Drop` on the returned reaper is what covers the signals we can catch; the
/// reaper process covers the one we cannot.
fn spawn_guarded(
    mut cmd: std::process::Command,
) -> (
    std::process::Child,
    Option<tokenade_core::process_util::GroupReaper>,
) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let child = cmd.spawn().expect("spawn agent");
    let reaper = tokenade_core::process_util::guard_group_against_our_death(child.id());
    (child, reaper)
}

/// Wait until the provider has been reached, or give up.
///
/// Polls a counter rather than sleeping a fixed time: the verdict is "did it
/// arrive", never "did it arrive within exactly N seconds", so a slow machine
/// makes the test slower and not flakier.
fn wait_for_hit(hits: &AtomicUsize) -> bool {
    let start = Instant::now();
    while start.elapsed() < REACH_TIMEOUT {
        if hits.load(Ordering::SeqCst) > 0 {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

#[test]
#[ignore = "live network: needs the installed agent and a provider key"]
fn codex_reaches_the_provider_through_the_installed_proxy() {
    if !have("codex") {
        eprintln!("codex is not installed here — nothing measured, nothing claimed");
        return;
    }
    // A Responses-API shaped answer, enough that codex does not treat the reply
    // as a transport error before we have counted the request.
    let (upstream, hits) = counting_provider(
        r#"{"id":"resp_1","object":"response","status":"completed","output":[],"usage":{"input_tokens":1,"output_tokens":1}}"#,
    );
    let listen = proxy_in_front_of(&upstream, Agent::Codex);
    let proxy_url = format!("http://{listen}");

    let h = home("codex");
    let cfg = h.path().join(".codex").join("config.toml");
    std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
    std::fs::write(&cfg, "model = \"gpt-5.5\"\n").unwrap();

    let plan = install::plan(Agent::Codex, h.path(), &proxy_url).expect("planned");
    assert_eq!(
        plan.upstream, "https://api.openai.com/v1",
        "the vendor default is what the proxy must forward to"
    );
    install::apply(&plan).expect("applied");

    // `CODEX_HOME` rather than `HOME`: measured in the binary codex actually
    // runs, and it keeps the developer's own ~/.codex out of the test.
    let mut cmd = std::process::Command::new("codex");
    cmd.args(["exec", "--skip-git-repo-check", "say hi"])
        .env("CODEX_HOME", h.path().join(".codex"))
        .env("OPENAI_API_KEY", "sk-test-not-a-real-key")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let (mut child, _reaper) = spawn_guarded(cmd);

    let reached = wait_for_hit(&hits);
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        reached,
        "codex never reached the provider through the proxy — the config key we \
         write is not the one it reads"
    );
}

#[test]
#[ignore = "live network: needs the installed agent and a provider key"]
fn aider_reaches_the_provider_through_the_installed_proxy() {
    if !have("aider") {
        eprintln!("aider is not installed here — nothing measured, nothing claimed");
        return;
    }
    let (upstream, hits) = counting_provider(
        r#"{"id":"chatcmpl-1","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#,
    );
    let listen = proxy_in_front_of(&upstream, Agent::Aider);
    let proxy_url = format!("http://{listen}");

    let h = home("aider");
    std::fs::write(h.path().join(".aider.conf.yml"), "model: gpt-4o\n").unwrap();

    let plan = install::plan(Agent::Aider, h.path(), &proxy_url).expect("planned");
    install::apply(&plan).expect("applied");

    let work = h.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let mut child = std::process::Command::new("aider")
        .args([
            "--message",
            "say hi",
            "--no-git",
            "--yes",
            "--no-check-update",
        ])
        .current_dir(&work)
        .env("HOME", h.path())
        .env("OPENAI_API_KEY", "sk-test-not-a-real-key")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn aider");

    let reached = wait_for_hit(&hits);
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        reached,
        "aider never reached the provider through the proxy — `openai-api-base` \
         in ~/.aider.conf.yml is not the key it reads"
    );
}

#[test]
#[ignore = "live network: needs the installed agent and a provider key"]
fn opencode_reaches_the_provider_through_the_installed_proxy() {
    if !have("opencode") {
        eprintln!("opencode is not installed here — nothing measured, nothing claimed");
        return;
    }
    let (upstream, hits) = counting_provider(
        r#"{"id":"chatcmpl-1","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#,
    );
    let listen = proxy_in_front_of(&upstream, Agent::OpenCode);
    let h = home("opencode");
    let cfg = h
        .path()
        .join(".config")
        .join("opencode")
        .join("opencode.json");
    std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
    std::fs::write(&cfg, r#"{"model":"openai/gpt-4o"}"#).unwrap();

    let plan =
        install::plan(Agent::OpenCode, h.path(), &format!("http://{listen}")).expect("planned");
    install::apply(&plan).expect("applied");

    let mut cmd = std::process::Command::new("opencode");
    cmd.args(["run", "say hi"])
        .current_dir(h.path())
        .env("HOME", h.path())
        .env("XDG_CONFIG_HOME", h.path().join(".config"))
        .env("OPENAI_API_KEY", "sk-test-not-a-real-key")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let (mut child, _reaper) = spawn_guarded(cmd);
    let reached = wait_for_hit(&hits);
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        reached,
        "opencode never reached the provider — `provider.<id>.options.baseURL` in \
         opencode.json is not the key it reads"
    );
}

#[test]
#[ignore = "live network: needs the installed agent and a provider key"]
fn kilo_reaches_the_provider_through_the_installed_proxy() {
    if !have("kilo") {
        eprintln!("kilo is not installed here — nothing measured, nothing claimed");
        return;
    }
    let (upstream, hits) = counting_provider(
        r#"{"id":"chatcmpl-1","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#,
    );
    let listen = proxy_in_front_of(&upstream, Agent::KiloCode);
    let h = home("kilo");
    let cfg = h.path().join(".config").join("kilo").join("kilo.json");
    std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
    std::fs::write(&cfg, r#"{"model":"openai/gpt-4o"}"#).unwrap();

    let plan =
        install::plan(Agent::KiloCode, h.path(), &format!("http://{listen}")).expect("planned");
    install::apply(&plan).expect("applied");

    let mut cmd = std::process::Command::new("kilo");
    cmd.args(["run", "say hi"])
        .current_dir(h.path())
        .env("HOME", h.path())
        .env("XDG_CONFIG_HOME", h.path().join(".config"))
        .env("OPENAI_API_KEY", "sk-test-not-a-real-key")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let (mut child, _reaper) = spawn_guarded(cmd);
    let reached = wait_for_hit(&hits);
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        reached,
        "kilo never reached the provider — the opencode schema is not what it reads"
    );
}

#[test]
#[ignore = "live network: needs the installed agent and a provider key"]
fn claude_code_reaches_the_provider_through_the_installed_proxy() {
    // The agent the matrix says we must NOT fold for, installed anyway — for
    // the cost figures, which exist nowhere else. This proves the two
    // decisions are genuinely independent: the proxy is in the path AND the
    // payloads go through untouched.
    if !have("claude") {
        eprintln!("claude is not installed here — nothing measured, nothing claimed");
        return;
    }
    let (upstream, hits) = counting_provider(
        r#"{"id":"msg_1","type":"message","role":"assistant","content":[],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}"#,
    );
    let listen = proxy_in_front_of(&upstream, Agent::ClaudeCode);
    let h = home("claude");
    let cfg = h.path().join(".claude").join("settings.json");
    std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
    std::fs::write(
        &cfg,
        r#"{"env":{"CLAUDE_CODE_AUTO_COMPACT_WINDOW":"700000"}}"#,
    )
    .unwrap();

    let plan =
        install::plan(Agent::ClaudeCode, h.path(), &format!("http://{listen}")).expect("planned");
    assert_eq!(plan.upstream, "https://api.anthropic.com");
    install::apply(&plan).expect("applied");
    // The user's own env entry survives the install.
    let after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(after["env"]["CLAUDE_CODE_AUTO_COMPACT_WINDOW"], "700000");

    let mut cmd = std::process::Command::new("claude");
    cmd.args(["-p", "say hi"])
        .current_dir(h.path())
        .env("HOME", h.path())
        .env("ANTHROPIC_API_KEY", "sk-ant-test-not-a-real-key")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let (mut child, _reaper) = spawn_guarded(cmd);
    let reached = wait_for_hit(&hits);
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        reached,
        "claude code never reached the provider — `env.ANTHROPIC_BASE_URL` in \
         ~/.claude/settings.json is not a route it honours"
    );
}

#[test]
#[ignore = "live network: needs the installed agent and a provider key"]
fn qwen_reaches_the_provider_through_the_installed_proxy() {
    if !have("qwen") {
        eprintln!("qwen is not installed here — nothing measured, nothing claimed");
        return;
    }
    let (upstream, hits) = counting_provider(
        r#"{"id":"chatcmpl-1","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#,
    );
    let listen = proxy_in_front_of(&upstream, Agent::Qwen);
    let h = home("qwen");
    let cfg = h.path().join(".qwen").join("settings.json");
    std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
    // `selectedAuthType` is not decoration in this harness: qwen refuses to
    // run non-interactively without one ("No auth type is selected"), so a
    // config without it never reaches the network and the test would report
    // "the key is wrong" about a key it never used. Measured by running the
    // binary after the first attempt failed — the live test caught an
    // inference I had drawn from `settings.env ×40` alone.
    std::fs::write(&cfg, r#"{"selectedAuthType":"openai"}"#).unwrap();

    let plan = install::plan(Agent::Qwen, h.path(), &format!("http://{listen}")).expect("planned");
    install::apply(&plan).expect("applied");

    let mut cmd = std::process::Command::new("qwen");
    cmd.args(["-p", "say hi"])
        .current_dir(h.path())
        .env("HOME", h.path())
        .env("OPENAI_API_KEY", "sk-test-not-a-real-key")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let (mut child, _reaper) = spawn_guarded(cmd);
    let reached = wait_for_hit(&hits);
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        reached,
        "qwen never reached the provider — `env.OPENAI_BASE_URL` in ~/.qwen/settings.json \
         is not a route it honours"
    );
}

#[test]
#[ignore = "live network: needs the installed agent and a provider key"]
fn grok_reaches_the_provider_through_the_installed_proxy() {
    if !have("grok") {
        eprintln!("grok is not installed here — nothing measured, nothing claimed");
        return;
    }
    // Grok asks `{base_url}/models` before it will talk to anything, so a
    // provider that only answers chat completions is never reached at all and
    // the test would report "the key is wrong" about a key it did use. One
    // body that satisfies both shapes: `data` is the model list, `choices` is
    // the completion, and neither wire minds the other's field being present.
    let (upstream, hits) = counting_provider(
        r#"{"data":[{"id":"grok-4.5","object":"model","owned_by":"xai"}],"object":"list","id":"chatcmpl-1","choices":[{"index":0,"message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#,
    );
    let listen = proxy_in_front_of(&upstream, Agent::Grok);

    let h = home("grok");
    let cfg = h.path().join(".grok").join("config.toml");
    std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
    std::fs::write(&cfg, "[ui]\nyolo = true\n").unwrap();

    let plan = install::plan(Agent::Grok, h.path(), &format!("http://{listen}")).expect("planned");
    install::apply(&plan).expect("applied");

    let mut cmd = std::process::Command::new("grok");
    cmd.args(["-p", "say hi"])
        .current_dir(h.path())
        .env("HOME", h.path())
        .env("XAI_API_KEY", "xai-test-not-a-real-key")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let (mut child, _reaper) = spawn_guarded(cmd);
    let reached = wait_for_hit(&hits);
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        reached,
        "grok never reached the provider — `[endpoints] models_base_url` in ~/.grok/config.toml \
         is not a route it honours"
    );
}

#[test]
#[ignore = "live network: needs the installed agent and a provider key"]
fn cline_reaches_the_provider_through_the_installed_proxy() {
    if !have("cline") {
        eprintln!("cline is not installed here — nothing measured, nothing claimed");
        return;
    }
    let (upstream, hits) = counting_provider(
        r#"{"id":"chatcmpl-1","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#,
    );
    let listen = proxy_in_front_of(&upstream, Agent::Cline);

    let h = home("cline");
    // The agent's OWN writer produces the file, rather than a fixture I typed
    // out: a hand-written config that happens to match today's schema is a
    // test of my memory, and this is the one place the schema can drift under
    // us. `--data-dir` is cline's documented isolation flag, so nothing here
    // touches the developer's real `~/.cline`.
    let data = h.path().join(".cline");
    let seeded = std::process::Command::new("cline")
        .args([
            "auth",
            "--data-dir",
            data.to_str().unwrap(),
            "--provider",
            "openai-native",
            "--apikey",
            "sk-test-not-a-real-key",
            "--modelid",
            "gpt-5",
            "--baseurl",
            "https://api.openai.com/v1",
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run cline auth");
    assert!(
        seeded.status.success(),
        "cline auth could not seed the config: {}",
        String::from_utf8_lossy(&seeded.stderr)
    );

    let plan = install::plan(Agent::Cline, h.path(), &format!("http://{listen}")).expect("planned");
    assert_eq!(plan.upstream, "https://api.openai.com/v1");
    assert_eq!(plan.previous_selector.as_deref(), Some("openai-native"));
    install::apply(&plan).expect("applied");

    let work = h.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let mut cmd = std::process::Command::new("cline");
    cmd.args(["--data-dir", data.to_str().unwrap(), "say hi"])
        .current_dir(&work)
        .env("HOME", h.path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let (mut child, _reaper) = spawn_guarded(cmd);
    let reached = wait_for_hit(&hits);
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        reached,
        "cline never reached the provider — `providers.<lastUsedProvider>.settings.baseUrl` in \
         ~/.cline/settings/providers.json is not a route it honours"
    );
}

/// An agent with no MEASURED config site has no install path, and says so.
///
/// This test used to be "the fully covered agents have no install path", which
/// conflated two questions. Being covered by a higher rung decides whether the
/// proxy may FOLD; it does not decide whether the proxy is worth installing,
/// because the provider's own usage figures exist nowhere else in the product.
/// What actually gates an installer is whether somebody measured where that
/// agent keeps its base URL.
#[test]
fn an_agent_with_no_measured_config_site_has_no_install_path() {
    let h = home("refusals");
    // Droid and GeminiCli were both here, and both were wrong, and the way
    // they were wrong is the lesson. The note below said gemini "has an env
    // route and env is the one thing this module may not write", after
    // counting `baseUrl` in its bundle. Two things that counting missed:
    // gemini does not dial `baseUrl` at all, it dials `CODE_ASSIST_ENDPOINT`,
    // and it LOADS `~/.gemini/.env` itself — so the env route runs through a
    // file after all. Droid's `~/.factory/config.json` was called
    // "unmeasurable here"; writing one and running droid against a local
    // listener measured it in a minute.
    //
    // Counting strings in a bundle answers a different question from running
    // the agent. What is left in this list is what neither method has reached.
    //
    // Reasonix left this list on 2026-08-20, by the second method: `npm i -g
    // reasonix`, `reasonix doctor --json` for the path, and `reasonix -p`
    // against a local listener for the proof. T3Code stays, and for a reason
    // the list did not have a shape for until today — it is not unmeasured,
    // it is covered by claude-code's write, which
    // `a_measured_row_with_nowhere_to_write_is_still_not_a_capability` pins.
    // Cursor left on 2026-08-21, by the first path AND by a decision: its
    // route is indeed the environment variable, it was indeed measured by
    // running the agent, and that day the operator lifted the constraint
    // that forbade writing into a shell profile. What remains here is
    // t3-code, which is not unmeasured but covered by claude-code's write.
    {
        let a = Agent::T3Code;
        assert!(
            install::recipe(a).is_none(),
            "{} must have no install recipe until its config site is measured",
            a.as_str()
        );
        assert!(install::plan(a, h.path(), "http://127.0.0.1:1").is_err());
    }
}

/// Folding is decided by the matrix, INDEPENDENTLY of installability.
#[test]
fn folding_is_decided_by_the_wire_and_not_by_installability() {
    // This asserted until 2026-08-20 that opencode must be relayed untouched
    // because a plugin bridge sits above it. Measured, the bridge does not own
    // its whole history: opencode's own `mcp`, `sobriety` and `context` cells
    // read `Partial bin`, and the proxy folded the same history 56% on agents
    // this rule forbade it for.
    //
    // The pair the test needs is still a pair, it just holds different ends:
    // installability and permission remain independent, and what decides
    // permission is the WIRE.
    let opencode =
        crate::llm_proxy::agent_matrix::entry(Agent::OpenCode).role(Agent::OpenCode.coverage());
    assert!(install::recipe(Agent::OpenCode).is_some());
    assert!(
        opencode.may_fold(),
        "opencode speaks a wire we rewrite and its bridge does not own every payload"
    );

    // Cursor is the opposite end: installable, and NOT ours to fold, because
    // its body is a shape we do not parse. There the proxy relays and counts,
    // and its own description has to say so.
    let cursor =
        crate::llm_proxy::agent_matrix::entry(Agent::Cursor).role(Agent::Cursor.coverage());
    assert!(install::recipe(Agent::Cursor).is_some());
    assert!(!cursor.may_fold(), "a proprietary wire is never rewritten");
    assert!(cursor.describe().contains("cost figures"));
}
