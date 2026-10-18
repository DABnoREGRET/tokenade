//! Installing the proxy into a real config file, and getting back out of it.
//!
//! Every test writes an actual file in a temp directory and reads the result
//! back with the same parser the agent would use. Asserting on a string the
//! writer returned would pass while producing a config the agent rejects, and
//! the failure mode of that mistake is an agent that will not start.

use super::*;
use tokenade_core::detect::Agent;

/// A throwaway home directory.
///
/// Path-based rather than `HOME`-based on purpose: every function in this
/// module takes the home as an argument, so no test here needs the process-wide
/// env lock. The name carries the pid AND a per-call counter — a pid alone is
/// shared by every test in one binary, which is how this repo previously got a
/// fixed temp path wiped out from under a concurrent run.
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

fn home() -> Home {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let d = std::env::temp_dir().join(format!(
        "tokenade-llm-install-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&d).expect("temp home");
    Home(d)
}

fn write(path: &std::path::Path, body: &str) {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

fn read(path: &std::path::Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

/// What the installer actually writes: an origin with NO path. The agent
/// appends its own route, and the proxy appends that to the provider's base.
const PROXY: &str = "http://127.0.0.1:8787";

// ---------------------------------------------------------------------------
// Codex — TOML, a selector plus a provider table.
// ---------------------------------------------------------------------------

/// A config shaped like the real one on this machine: a model, and hook trust
/// state the user approved. Both must survive untouched.
const CODEX_REAL: &str = r#"model = "gpt-5.5"
model_reasoning_effort = "medium"
hooks = { state = { "hooks.json:post_tool_use:0:0" = { enabled = true, trusted_hash = "sha256:919423a7" } } }

[projects."/home/u/work"]
trust_level = "trusted"
"#;

#[test]
fn codex_with_no_provider_configured_takes_the_vendor_default_as_upstream() {
    // The COMMON case, and the one the rung is most useful for: a user on the
    // vendor's own endpoint who never customised anything. Refusing to install
    // without an explicit base URL would limit rank 4 to people who had already
    // configured a provider by hand.
    let h = home();
    let p = Site::CodexToml.path(h.path());
    write(&p, CODEX_REAL);

    let plan = plan(Agent::Codex, h.path(), PROXY).expect("planned");
    assert_eq!(plan.upstream, "https://api.openai.com/v1");
    assert_eq!(plan.previous_selector, None);
}

#[test]
fn codex_with_a_custom_provider_hands_that_url_to_the_proxy() {
    // The read-then-replace requirement: the value we are about to overwrite IS
    // the proxy's upstream. Get this wrong and the proxy forwards to the wrong
    // provider, with the user's key.
    let h = home();
    let p = Site::CodexToml.path(h.path());
    write(
        &p,
        r#"model_provider = "azure"

[model_providers.azure]
name = "Azure"
base_url = "https://contoso.openai.azure.com/openai/v1"
wire_api = "responses"
"#,
    );

    let plan = plan(Agent::Codex, h.path(), PROXY).expect("planned");
    assert_eq!(plan.upstream, "https://contoso.openai.azure.com/openai/v1");
    assert_eq!(plan.previous_selector.as_deref(), Some("azure"));
}

#[test]
fn installing_points_codex_at_us_and_leaves_everything_else_alone() {
    let h = home();
    let p = Site::CodexToml.path(h.path());
    write(&p, CODEX_REAL);

    let plan = plan(Agent::Codex, h.path(), PROXY).unwrap();
    apply(&plan).expect("applied");

    let doc: toml_edit::DocumentMut = read(&p).parse().expect("still valid TOML");
    assert_eq!(doc["model_provider"].as_str(), Some(PROVIDER_ID));
    assert_eq!(
        doc["model_providers"][PROVIDER_ID]["base_url"].as_str(),
        Some(PROXY)
    );
    assert_eq!(
        doc["model_providers"][PROVIDER_ID]["wire_api"].as_str(),
        Some("responses"),
        "codex's only route is the Responses API"
    );

    // The user's own keys survive. The hook trust state is the one that matters:
    // a rewrite that dropped `trusted_hash` would silently disable the hooks
    // this very user approved — the rung ABOVE us — while we claimed to be
    // adding coverage.
    assert_eq!(doc["model"].as_str(), Some("gpt-5.5"));
    assert_eq!(doc["model_reasoning_effort"].as_str(), Some("medium"));
    assert!(
        read(&p).contains("trusted_hash"),
        "the approved hook state must survive an install"
    );
    assert!(read(&p).contains("trust_level"));

    // And nothing we added is noise. An explicit empty `[model_providers]`
    // header above our entry is a line the user has to read and reason about,
    // in their own file, for no reason.
    assert!(
        !read(&p).contains("[model_providers]\n"),
        "an empty parent table header was written:\n{}",
        read(&p)
    );
}

#[test]
fn uninstalling_codex_puts_the_previous_selector_back() {
    let h = home();
    let p = Site::CodexToml.path(h.path());
    write(
        &p,
        r#"model_provider = "azure"

[model_providers.azure]
base_url = "https://contoso.openai.azure.com/openai/v1"
"#,
    );

    let record = apply(&plan(Agent::Codex, h.path(), PROXY).unwrap()).unwrap();
    revert(&record).expect("reverted");

    let doc: toml_edit::DocumentMut = read(&p).parse().unwrap();
    assert_eq!(doc["model_provider"].as_str(), Some("azure"));
    assert!(
        doc["model_providers"].get(PROVIDER_ID).is_none(),
        "our provider entry must be gone"
    );
    assert_eq!(
        doc["model_providers"]["azure"]["base_url"].as_str(),
        Some("https://contoso.openai.azure.com/openai/v1"),
        "the user's own provider is untouched"
    );
}

#[test]
fn uninstalling_removes_a_selector_we_invented() {
    // There was no `model_provider` before us. Leaving one behind — even
    // pointing somewhere valid — is a change we made and did not undo, and the
    // user would be on a provider they never chose.
    let h = home();
    let p = Site::CodexToml.path(h.path());
    write(&p, CODEX_REAL);

    let record = apply(&plan(Agent::Codex, h.path(), PROXY).unwrap()).unwrap();
    revert(&record).unwrap();

    let doc: toml_edit::DocumentMut = read(&p).parse().unwrap();
    assert!(doc.get("model_provider").is_none());
    assert!(
        doc.get("model_providers").is_none(),
        "a table we created and then emptied is ours to remove"
    );
    assert_eq!(doc["model"].as_str(), Some("gpt-5.5"));
}

// ---------------------------------------------------------------------------
// The failure this module exists to prevent.
// ---------------------------------------------------------------------------

#[test]
fn installing_twice_refuses_rather_than_making_the_proxy_its_own_upstream() {
    // The nastiest bug available here. A second install reads back OUR address
    // as "what the agent was talking to", so the proxy forwards to itself and
    // every request becomes an infinite regress. It would look like a hang, not
    // like a config error.
    let h = home();
    let p = Site::CodexToml.path(h.path());
    write(&p, CODEX_REAL);

    let first = plan(Agent::Codex, h.path(), PROXY).unwrap();
    install(h.path(), &first).unwrap();

    let second = plan(Agent::Codex, h.path(), PROXY);
    assert_eq!(second, Err(InstallError::WouldLoop(PROXY.to_string())));
}

#[test]
fn a_proxy_moved_to_another_port_is_still_recognised_as_us() {
    // The same loop, one hop longer: install on 8787, move the proxy to 9999,
    // install again. An exact-match check would happily capture the old proxy
    // as the upstream.
    let h = home();
    let p = Site::CodexToml.path(h.path());
    write(&p, CODEX_REAL);
    // `install`, not `apply`: only `install` writes the records file, and the
    // records file is what remembers which ports are ours.
    install(h.path(), &plan(Agent::Codex, h.path(), PROXY).unwrap()).unwrap();

    let again = plan(Agent::Codex, h.path(), "http://127.0.0.1:9999");
    assert!(matches!(again, Err(InstallError::WouldLoop(_))));

    // Recognition is by PORT, and the records file is what remembers it — so
    // the same port spelled three ways is still us.
    for spelling in [
        "http://localhost:8787/v1",
        "http://[::1]:8787/v1",
        "http://127.0.0.1:8787",
    ] {
        assert!(
            loop_by_port(spelling, h.path(), "http://127.0.0.1:9999"),
            "{spelling}"
        );
    }
    // And a real provider is not mistaken for us.
    for real in [
        "https://api.openai.com/v1",
        "https://contoso.openai.azure.com/openai/v1",
        "https://api.anthropic.com",
    ] {
        assert!(!loop_by_port(real, h.path(), PROXY), "{real}");
    }
}

#[test]
fn a_local_model_server_is_chained_in_front_of_and_not_refused() {
    // The regression this is here for: `points_at_this_machine` answered on the
    // HOST alone, so every loopback upstream was called a loop. That refused the
    // install for every local-model user — Ollama 11434, LM Studio 1234, vLLM
    // 8000, llama.cpp — telling them "the agent already points at this proxy"
    // about a server that is not this proxy. Nothing is installed on any of
    // these ports in this HOME, and none of them answers our health route.
    let h = home();
    for local in [
        "http://127.0.0.1:11434/v1",
        "http://localhost:1234/v1",
        "http://127.0.0.1:8000/v1",
        "http://[::1]:8080",
    ] {
        assert!(
            !loop_by_port(local, h.path(), PROXY),
            "a local model server must be chained, not refused: {local}"
        );
    }

    // End to end: an agent configured against Ollama plans cleanly, and the
    // plan keeps Ollama as the upstream rather than losing it.
    let p = Site::AiderYaml.path(h.path());
    write(&p, "openai-api-base: http://127.0.0.1:11434/v1\n");
    let plan = plan(Agent::Aider, h.path(), PROXY).expect("a local upstream must not refuse");
    assert_eq!(plan.upstream, "http://127.0.0.1:11434/v1");
    assert!(plan.had_explicit_upstream);
}

#[test]
fn an_unrecorded_endpoint_is_asked_and_only_our_answer_counts() {
    // The probe half, against listeners this test starts itself — never against
    // whatever happens to be running on the machine.
    fn serve(body: &'static str) -> (u16, std::thread::JoinHandle<()>) {
        use std::io::{Read as _, Write as _};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            if let Ok((mut c, _)) = l.accept() {
                let mut junk = [0u8; 1024];
                let _ = c.read(&mut junk);
                let _ = c.write_all(body.as_bytes());
            }
        });
        (port, h)
    }

    // Our proxy: 200 plus the health body's own shape.
    let (ours, h1) = serve(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n         {\"compaction\":true,\"deployment\":\"local\",\"ok\":true}",
    );
    // A local model server: the route means nothing to it.
    let (theirs, h2) = serve("HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\nnot found");

    let home = home();
    assert!(
        would_loop(&format!("http://127.0.0.1:{ours}/v1"), home.path(), PROXY),
        "a live proxy nobody recorded must still be caught"
    );
    assert!(
        !would_loop(&format!("http://127.0.0.1:{theirs}/v1"), home.path(), PROXY),
        "a local server that is not us must be chained, not refused"
    );
    // A port with nothing on it at all: connect fails, and that is not a loop.
    let dead = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let p = l.local_addr().unwrap().port();
        drop(l);
        p
    };
    assert!(!would_loop(
        &format!("http://127.0.0.1:{dead}"),
        home.path(),
        PROXY
    ));

    let _ = h1.join();
    let _ = h2.join();
}

#[test]
fn host_and_port_are_read_the_way_a_url_spells_them() {
    // `[::1]:8787` has colons of its own, and a scheme with no port is not
    // "port unknown" — it is the scheme's default. Both were places the old
    // rsplit-on-colon could have gone wrong once a port started mattering.
    assert_eq!(
        host_port("http://[::1]:8787/v1"),
        Some(("::1".into(), 8787))
    );
    assert_eq!(host_port("http://[::1]/v1"), Some(("::1".into(), 80)));
    assert_eq!(
        host_port("https://api.openai.com/v1"),
        Some(("api.openai.com".into(), 443))
    );
    assert_eq!(
        host_port("http://LocalHost:1234"),
        Some(("localhost".into(), 1234))
    );
    assert_eq!(
        host_port("127.0.0.1:8787"),
        Some(("127.0.0.1".into(), 8787))
    );
}

// ---------------------------------------------------------------------------
// Aider — a flat YAML config.
// ---------------------------------------------------------------------------

#[test]
fn aider_gains_the_key_when_it_had_none_and_keeps_its_other_settings() {
    let h = home();
    let p = Site::AiderYaml.path(h.path());
    write(&p, "model: gpt-4o\nauto-commits: false\n");

    let plan = plan(Agent::Aider, h.path(), PROXY).expect("planned");
    assert_eq!(plan.upstream, "https://api.openai.com/v1");
    apply(&plan).unwrap();

    let out = read(&p);
    assert!(out.contains(&format!("openai-api-base: {PROXY}")), "{out}");
    assert!(out.contains("model: gpt-4o"));
    assert!(out.contains("auto-commits: false"));
}

#[test]
fn aider_with_an_existing_base_hands_it_over_and_gets_it_back() {
    let h = home();
    let p = Site::AiderYaml.path(h.path());
    write(
        &p,
        "model: gpt-4o\nopenai-api-base: https://openrouter.ai/api/v1\n",
    );

    let plan = plan(Agent::Aider, h.path(), PROXY).unwrap();
    assert_eq!(plan.upstream, "https://openrouter.ai/api/v1");

    let record = apply(&plan).unwrap();
    assert!(read(&p).contains(&format!("openai-api-base: {PROXY}")));

    revert(&record).unwrap();
    let out = read(&p);
    assert!(
        out.contains("openai-api-base: https://openrouter.ai/api/v1"),
        "{out}"
    );
    assert!(out.contains("model: gpt-4o"));
    assert_eq!(
        out.matches("openai-api-base").count(),
        1,
        "no duplicate key"
    );
}

#[test]
fn a_commented_out_key_is_not_mistaken_for_a_setting() {
    // `# openai-api-base: …` is a user's note to themselves. Reading it as the
    // live value would hand the proxy an upstream the agent never used.
    let h = home();
    let p = Site::AiderYaml.path(h.path());
    write(
        &p,
        "# openai-api-base: https://never-used.example\nmodel: x\n",
    );

    let plan = plan(Agent::Aider, h.path(), PROXY).unwrap();
    assert_eq!(plan.upstream, "https://api.openai.com/v1");
    apply(&plan).unwrap();
    let out = read(&p);
    assert!(out.contains("# openai-api-base: https://never-used.example"));
    assert!(out.contains(&format!("openai-api-base: {PROXY}")));
}

// ---------------------------------------------------------------------------
// The record — the link between `install` and a LATER `uninstall`.
// ---------------------------------------------------------------------------

#[test]
fn an_install_survives_the_process_that_made_it() {
    // The gap this closes: `apply` returned a Record and nobody wrote it down,
    // so a second run of the binary had no way to learn what the agent used to
    // point at. Uninstall could only guess, and guessing here means moving a
    // user off a provider they configured.
    let h = home();
    let p = Site::CodexToml.path(h.path());
    write(
        &p,
        r#"model_provider = "azure"

[model_providers.azure]
base_url = "https://contoso.openai.azure.com/openai/v1"
"#,
    );

    install(h.path(), &plan(Agent::Codex, h.path(), PROXY).unwrap()).expect("installed");

    // A LATER process: nothing in memory, only the file on disk.
    let recorded = load_records(h.path()).expect("records readable");
    assert_eq!(recorded.len(), 1);
    assert_eq!(
        recorded[0].upstream,
        "https://contoso.openai.azure.com/openai/v1"
    );

    let undone = uninstall(h.path(), Agent::Codex)
        .expect("uninstalled")
        .expect("there was something to undo");
    assert_eq!(
        undone.upstream,
        "https://contoso.openai.azure.com/openai/v1"
    );

    let doc: toml_edit::DocumentMut = read(&p).parse().unwrap();
    assert_eq!(doc["model_provider"].as_str(), Some("azure"));
    assert!(load_records(h.path()).unwrap().is_empty());
}

#[test]
fn uninstalling_something_never_installed_is_a_no_op() {
    let h = home();
    assert_eq!(uninstall(h.path(), Agent::Codex), Ok(None));
}

#[test]
fn a_failed_install_leaves_no_record_behind() {
    // The record goes down FIRST so a crash cannot lose it. The cost of that
    // order is a record for an install that did not happen, which would make a
    // later uninstall rewrite a config we never touched — so the failure path
    // takes it back out.
    let h = home();
    let p = Site::CodexToml.path(h.path());
    write(&p, CODEX_REAL);
    let plan = plan(Agent::Codex, h.path(), PROXY).unwrap();
    // Make `apply` fail by removing the file between plan and install.
    std::fs::remove_file(&p).unwrap();

    assert!(install(h.path(), &plan).is_err());
    assert!(
        load_records(h.path()).unwrap().is_empty(),
        "a record for an install that failed is worse than none"
    );
}

#[test]
fn a_torn_record_file_is_an_error_and_not_an_empty_install_list() {
    // The precise mistake this project has already shipped: a zero-length state
    // file read as "never used". Here that would tell a user nothing is
    // installed while their agent points at a proxy that is gone.
    let h = home();
    let rp = records_path(h.path());
    write(&rp, "");
    assert_eq!(
        load_records(h.path()),
        Err(RecordsError::Unreadable(rp.clone()))
    );

    write(&rp, "{ this is not json");
    assert!(matches!(
        load_records(h.path()),
        Err(RecordsError::Unreadable(_))
    ));

    // And uninstall refuses rather than inventing a previous configuration.
    let e = uninstall(h.path(), Agent::Codex);
    assert!(matches!(e, Err(InstallError::Io(_))), "{e:?}");
    let msg = format!("{}", e.unwrap_err());
    assert!(msg.contains(".tokenade-backup-"), "{msg}");
}

#[test]
fn a_second_install_of_the_same_agent_replaces_its_record() {
    // Two entries for one agent would make uninstall order-dependent, and the
    // loser would be a stale upstream nobody can act on.
    let h = home();
    let p = Site::AiderYaml.path(h.path());
    write(&p, "openai-api-base: https://first.example/v1\n");
    install(h.path(), &plan(Agent::Aider, h.path(), PROXY).unwrap()).unwrap();
    uninstall(h.path(), Agent::Aider).unwrap();

    write(&p, "openai-api-base: https://second.example/v1\n");
    install(h.path(), &plan(Agent::Aider, h.path(), PROXY).unwrap()).unwrap();

    let recs = load_records(h.path()).unwrap();
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0].upstream, "https://second.example/v1");
}

#[test]
fn two_agents_are_remembered_independently() {
    let h = home();
    write(&Site::CodexToml.path(h.path()), CODEX_REAL);
    write(&Site::AiderYaml.path(h.path()), "model: gpt-4o\n");
    install(h.path(), &plan(Agent::Codex, h.path(), PROXY).unwrap()).unwrap();
    install(h.path(), &plan(Agent::Aider, h.path(), PROXY).unwrap()).unwrap();
    assert_eq!(load_records(h.path()).unwrap().len(), 2);

    uninstall(h.path(), Agent::Codex).unwrap();
    let left = load_records(h.path()).unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].agent, Agent::Aider.as_str());
}

#[test]
fn a_proxy_address_carrying_a_path_is_refused() {
    // The silent misroute this turns into a refusal: the proxy composes
    // `upstream + target`, so a `/v1` here is prepended to every request and
    // then joined to an upstream that already ends in `/v1`. A real provider
    // answers 404 on every single turn; a fake one answers 200 and the mistake
    // ships.
    let h = home();
    write(&Site::CodexToml.path(h.path()), CODEX_REAL);
    for bad in [
        "http://127.0.0.1:8787/v1",
        "http://127.0.0.1:8787/openai/v1",
        "http://127.0.0.1:8787/?x=1",
    ] {
        assert_eq!(
            plan(Agent::Codex, h.path(), bad),
            Err(InstallError::ListenUrlHasPath(bad.to_string())),
            "{bad}"
        );
    }
    // A bare origin, with or without the empty trailing slash, is fine.
    for good in ["http://127.0.0.1:8787", "http://127.0.0.1:8787/"] {
        assert!(plan(Agent::Codex, h.path(), good).is_ok(), "{good}");
    }
}

// ---------------------------------------------------------------------------
// opencode and kilo — one JSON schema, two files.
// ---------------------------------------------------------------------------

#[test]
fn opencode_writes_under_the_provider_its_model_names() {
    // The base URL lives UNDER a provider id. Writing to the wrong one leaves
    // the agent talking to its vendor with a `tokenade` entry sitting unused
    // beside it, and nothing looks wrong from either side.
    let h = home();
    let p = Site::OpenCodeJson {
        path: OpenCodeFlavour::OpenCode,
    }
    .path(h.path());
    write(
        &p,
        r#"{"$schema":"https://opencode.ai/config.json","model":"anthropic/claude-sonnet-4","mcp":{}}"#,
    );

    let plan = plan(Agent::OpenCode, h.path(), PROXY).expect("planned");
    assert_eq!(plan.previous_selector.as_deref(), Some("anthropic"));
    assert!(!plan.had_explicit_upstream);
    apply(&plan).unwrap();

    let v: serde_json::Value = serde_json::from_str(&read(&p)).expect("still valid JSON");
    assert_eq!(v["provider"]["anthropic"]["options"]["baseURL"], PROXY);
    // Everything else the user had is still there.
    assert_eq!(v["model"], "anthropic/claude-sonnet-4");
    assert_eq!(v["$schema"], "https://opencode.ai/config.json");
    assert!(v["mcp"].is_object());
}

#[test]
fn opencode_hands_over_an_existing_base_url_and_gets_it_back() {
    let h = home();
    let p = Site::OpenCodeJson {
        path: OpenCodeFlavour::OpenCode,
    }
    .path(h.path());
    write(
        &p,
        r#"{"model":"openai/gpt-4o","provider":{"openai":{"options":{"baseURL":"https://openrouter.ai/api/v1","apiKey":"kept"}}}}"#,
    );

    let plan = plan(Agent::OpenCode, h.path(), PROXY).unwrap();
    assert_eq!(plan.upstream, "https://openrouter.ai/api/v1");
    assert!(plan.had_explicit_upstream);
    let record = install(h.path(), &plan).unwrap();
    let v: serde_json::Value = serde_json::from_str(&read(&p)).unwrap();
    assert_eq!(v["provider"]["openai"]["options"]["baseURL"], PROXY);

    revert(&record).unwrap();
    let v: serde_json::Value = serde_json::from_str(&read(&p)).unwrap();
    assert_eq!(
        v["provider"]["openai"]["options"]["baseURL"],
        "https://openrouter.ai/api/v1"
    );
    assert_eq!(
        v["provider"]["openai"]["options"]["apiKey"], "kept",
        "a sibling key we never touched must survive"
    );
}

#[test]
fn uninstalling_removes_a_base_url_the_user_never_had() {
    // The provenance flag earns its place here. The agent was on its vendor
    // default, so putting `https://api.openai.com/v1` back would write OUR
    // assumption into the user's file as if they had chosen it — an install
    // that quietly becomes a configuration change.
    let h = home();
    let p = Site::OpenCodeJson {
        path: OpenCodeFlavour::OpenCode,
    }
    .path(h.path());
    write(&p, r#"{"model":"openai/gpt-4o"}"#);

    let record = install(h.path(), &plan(Agent::OpenCode, h.path(), PROXY).unwrap()).unwrap();
    assert!(!record.had_explicit_upstream);
    revert(&record).unwrap();

    let v: serde_json::Value = serde_json::from_str(&read(&p)).unwrap();
    assert!(
        v.get("provider").is_none(),
        "the scaffolding we created is ours to remove: {v}"
    );
    assert_eq!(v["model"], "openai/gpt-4o");
}

#[test]
fn kilo_is_the_same_schema_in_a_different_file() {
    let h = home();
    let p = Site::OpenCodeJson {
        path: OpenCodeFlavour::Kilo,
    }
    .path(h.path());
    assert!(p.ends_with(".config/kilo/kilo.json"));
    write(
        &p,
        r#"{"$schema":"https://opencode.ai/config.json","mcp":{}}"#,
    );

    let plan = plan(Agent::KiloCode, h.path(), PROXY).unwrap();
    // No `model` key at all: the schema's own default provider.
    assert_eq!(plan.previous_selector.as_deref(), Some("openai"));
    apply(&plan).unwrap();
    let v: serde_json::Value = serde_json::from_str(&read(&p)).unwrap();
    assert_eq!(v["provider"]["openai"]["options"]["baseURL"], PROXY);
}

/// Copilot's VS Code half is a DIFFERENT file and a DIFFERENT key from
/// Copilot's CLI half, and the uninstall has to leave the user's own
/// settings untouched.
///
/// Pinned because the two halves are one product with one name: a recipe
/// that reached for `~/.copilot/settings.json` here would write a file the
/// extension never reads and report success.
#[test]
fn copilot_in_vscode_is_a_different_file_from_copilot_the_cli() {
    let h = home();
    let p = Site::VsCodeCopilotSettings.path(h.path());
    assert!(p.ends_with(".config/Code/User/settings.json"), "{p:?}");
    assert_ne!(
        p,
        Site::SettingsEnvJson {
            flavour: SettingsFlavour::Copilot
        }
        .path(h.path())
    );
    write(&p, r#"{"editor.fontSize":13}"#);

    let record = apply(&plan(Agent::CopilotVsCode, h.path(), PROXY).unwrap()).unwrap();
    let v: serde_json::Value = serde_json::from_str(&read(&p)).unwrap();
    assert_eq!(v["github.copilot.internal.capiUrl"], PROXY);
    // The user's own setting is still there, untouched.
    assert_eq!(v["editor.fontSize"], 13);

    revert(&record).expect("reverted");
    let v: serde_json::Value = serde_json::from_str(&read(&p)).unwrap();
    assert!(
        v.get("github.copilot.internal.capiUrl").is_none(),
        "the key was ours and the uninstall left it behind: {v}"
    );
    assert_eq!(v["editor.fontSize"], 13);
}

#[test]
fn a_third_format_is_reverted_by_its_own_writer() {
    // The bug this pins: `revert` used to branch on `path.ends_with(
    // "config.toml")` and treat everything else as aider's flat YAML. True
    // with two recipes, silently destructive with three — an opencode JSON
    // file would have been line-rewritten by a YAML writer.
    let h = home();
    let p = Site::OpenCodeJson {
        path: OpenCodeFlavour::OpenCode,
    }
    .path(h.path());
    write(&p, r#"{"model":"openai/gpt-4o"}"#);
    let record = install(h.path(), &plan(Agent::OpenCode, h.path(), PROXY).unwrap()).unwrap();
    revert(&record).unwrap();
    serde_json::from_str::<serde_json::Value>(&read(&p))
        .expect("a JSON config must still be JSON after a revert");
}

// ---------------------------------------------------------------------------
// grok and cline.
// ---------------------------------------------------------------------------

/// grok with no `[endpoints]` at all: the vendor default becomes the upstream,
/// the install adds the table, and the revert takes the whole table back out.
///
/// The removal is the half worth pinning. Leaving `[endpoints]` behind, empty,
/// is not cosmetic here: grok's own documentation says an `[endpoints]` block
/// pointing anywhere other than xAI changes how it treats fleet policy, so an
/// empty leftover is a config the user did not write and cannot explain.
#[test]
fn grok_gains_an_endpoints_table_and_loses_it_again() {
    let h = home();
    let p = Site::GrokToml.path(h.path());
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, "[ui]\nyolo = true\n").unwrap();

    let plan = plan(Agent::Grok, h.path(), PROXY).expect("planned");
    assert_eq!(plan.upstream, "https://api.x.ai/v1");
    assert!(!plan.had_explicit_upstream);
    let record = apply(&plan).expect("applied");

    let doc: toml_edit::DocumentMut = read(&p).parse().unwrap();
    assert_eq!(doc["endpoints"]["models_base_url"].as_str(), Some(PROXY));
    assert_eq!(
        doc["ui"]["yolo"].as_bool(),
        Some(true),
        "the user's own settings must survive the install"
    );

    revert(&record).unwrap();
    let doc: toml_edit::DocumentMut = read(&p).parse().unwrap();
    assert!(
        doc.get("endpoints").is_none(),
        "a table we created must not outlive the install: {}",
        read(&p)
    );
    assert_eq!(doc["ui"]["yolo"].as_bool(), Some(true));
}

/// grok that already had a base URL gets THAT one back, not the vendor default.
#[test]
fn grok_with_its_own_endpoint_gets_that_one_back() {
    let h = home();
    let p = Site::GrokToml.path(h.path());
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(
        &p,
        "[endpoints]\nmodels_base_url = \"https://llm.corp.example/v1\"\n",
    )
    .unwrap();

    let plan = plan(Agent::Grok, h.path(), PROXY).expect("planned");
    assert_eq!(plan.upstream, "https://llm.corp.example/v1");
    assert!(plan.had_explicit_upstream);
    let record = apply(&plan).expect("applied");
    revert(&record).unwrap();

    let doc: toml_edit::DocumentMut = read(&p).parse().unwrap();
    assert_eq!(
        doc["endpoints"]["models_base_url"].as_str(),
        Some("https://llm.corp.example/v1")
    );
}

/// cline is written under the provider it is actually using, and no other.
///
/// The second provider in the fixture is the point: an install that wrote
/// `providers.<first>` or iterated the map would leave the agent talking to its
/// vendor while a redirected entry sat unused next to it, and nothing about the
/// file would look wrong.
#[test]
fn cline_is_written_under_the_provider_it_last_used() {
    let h = home();
    let p = Site::ClineProvidersJson.path(h.path());
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(
        &p,
        r#"{"lastUsedProvider":"openai-native","providers":{
             "anthropic":{"settings":{"baseUrl":"https://api.anthropic.com","model":"a"}},
             "openai-native":{"settings":{"apiKey":"sk-x","baseUrl":"https://api.openai.com/v1","model":"gpt-5"},"tokenSource":"manual"}}}"#,
    )
    .unwrap();

    let plan = plan(Agent::Cline, h.path(), PROXY).expect("planned");
    assert_eq!(plan.upstream, "https://api.openai.com/v1");
    assert_eq!(plan.previous_selector.as_deref(), Some("openai-native"));
    let record = apply(&plan).expect("applied");

    let v: serde_json::Value = serde_json::from_str(&read(&p)).unwrap();
    assert_eq!(
        v["providers"]["openai-native"]["settings"]["baseUrl"],
        serde_json::json!(PROXY)
    );
    assert_eq!(
        v["providers"]["anthropic"]["settings"]["baseUrl"],
        serde_json::json!("https://api.anthropic.com"),
        "the provider it is NOT using must be left alone"
    );
    assert_eq!(
        v["providers"]["openai-native"]["settings"]["apiKey"],
        serde_json::json!("sk-x"),
        "the install must not lose the credential next to the key it writes"
    );

    revert(&record).unwrap();
    let v: serde_json::Value = serde_json::from_str(&read(&p)).unwrap();
    assert_eq!(
        v["providers"]["openai-native"]["settings"]["baseUrl"],
        serde_json::json!("https://api.openai.com/v1")
    );
}

// ---------------------------------------------------------------------------
// Refusals.
// ---------------------------------------------------------------------------

#[test]
fn an_agent_with_no_measured_config_site_is_refused_by_name() {
    // Fourteen agents are reachable by environment variable and none of them
    // count: the constraint is config files, and a `baseUrl` string somewhere
    // in a bundle is not a measured key path. The refusal says so rather than
    // guessing a file.
    let h = home();
    // Droid and Gemini CLI left this list on 2026-08-19: both turned out to
    // have a FILE, found by running the agent against a local listener rather
    // than by reading a bundle. Cursor stays, and is now the measured case —
    // `cli-config.json` carries permissions, model and display, and no
    // endpoint key at all.
    // Cursor left this list on 2026-08-21: the operator lifted the "config
    // file only" constraint, and the shell profile IS a file — backed up,
    // compared, restored byte for byte like the others.
    for a in [Agent::T3Code, Agent::Antigravity] {
        let e = plan(a, h.path(), PROXY);
        assert_eq!(e, Err(InstallError::NoRecipeMeasured(a.as_str())));
    }
}

/// A site we CAN write does not imply an upstream we can name.
///
/// cline's file is the only one that multiplexes providers, so "no base URL
/// written" means "that provider's own default", which this module has not
/// measured for an arbitrary provider id. The refusal separates the two
/// unknowns instead of quietly defaulting to OpenAI's base and pointing the
/// proxy at a vendor the user never chose.
#[test]
fn a_measured_site_with_an_unknown_upstream_is_its_own_refusal() {
    let h = home();
    let p = Site::ClineProvidersJson.path(h.path());
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(
        &p,
        r#"{"lastUsedProvider":"anthropic","providers":{"anthropic":{"settings":{"model":"x"}}}}"#,
    )
    .unwrap();
    assert_eq!(
        plan(Agent::Cline, h.path(), PROXY),
        Err(InstallError::NoUpstreamMeasured {
            agent: "cline",
            provider: "anthropic".to_string(),
        })
    );
}

#[test]
fn a_missing_config_is_a_named_refusal_and_not_a_new_file() {
    // Creating `~/.codex/config.toml` from nothing would mean inventing the
    // rest of that agent's configuration, and the user would find settings they
    // never wrote.
    let h = home();
    let e = plan(Agent::Codex, h.path(), PROXY);
    assert_eq!(
        e,
        Err(InstallError::NoConfig(Site::CodexToml.path(h.path())))
    );
    assert!(!Site::CodexToml.path(h.path()).exists());
}

#[test]
fn an_unparseable_config_is_left_exactly_as_it_was() {
    let h = home();
    let p = Site::CodexToml.path(h.path());
    let broken = "model = \"gpt-5.5\"\n[[[ this is not toml\n";
    write(&p, broken);

    assert_eq!(
        plan(Agent::Codex, h.path(), PROXY),
        Err(InstallError::Unparseable(p.clone()))
    );
    assert_eq!(
        read(&p),
        broken,
        "a file we could not read is not rewritten"
    );
}

#[test]
fn the_environment_route_writes_a_marked_block_and_nothing_else() {
    // The constraint changed shape on 2026-08-21, it did not disappear. The
    // operator allowed the environment variable; what remains forbidden is
    // the way the aliases and the PATH shim went about it — mutating the
    // process environment, or laying down a free-form line we could no longer
    // find to remove it.
    let src = &tokenade_core::source_guard::module_family(
        &tokenade_core::source_guard::crates_dir()
            .join("tokenade-cli")
            .join("src")
            .join("llm_proxy"),
        "install",
    );
    for forbidden in [
        // Mutating the current process environment cannot be undone nor
        // backed up; that is exactly what the shim did.
        "std::env::set_var",
        "setenv",
        // The Windows verb that writes to the registry: not a file, so neither
        // backup nor byte-for-byte restore, which is what "reversible install"
        // measures. Searched as a COMMAND LITERAL — the bare name appears in
        // the prose of `install.rs` explaining why we do not use it, and a
        // guard that fires on its own documentation is a guard someone
        // weakens the next time it fires.
        "\"setx\"",
    ] {
        assert!(
            !src.contains(forbidden),
            "install.rs must not touch {forbidden}"
        );
    }
    // And what is written is written BETWEEN MARKERS, otherwise uninstall
    // would have to find our line by resemblance in a file the person edits
    // themselves.
    assert!(src.contains(">>> tokenade llm proxy >>>"));
    assert!(src.contains("<<< tokenade llm proxy <<<"));
}

#[test]
fn only_the_measured_environment_agent_reaches_a_shell_profile() {
    // A single row of the fleet is concerned. If a second one ever arrives, it
    // must be a choice, not a side effect of this gate.
    let env_sites: Vec<&str> = Agent::all()
        .iter()
        .filter(|a| matches!(recipe(**a), Some(Site::ShellProfile { .. })))
        .map(|a| a.as_str())
        .collect();
    assert_eq!(env_sites, vec!["cursor"], "unexpected shell-profile agents");
}

#[test]
fn the_old_rc_ban_is_gone_from_this_file_only_where_it_had_to_be() {
    let src = &tokenade_core::source_guard::module_family(
        &tokenade_core::source_guard::crates_dir()
            .join("tokenade-cli")
            .join("src")
            .join("llm_proxy"),
        "install",
    );
    // Shell files are now allowed — but ONLY those `shell_profile_path` names,
    // and via `$SHELL`. A hard-coded path elsewhere in this file would be a
    // second list describing the same fact, the class that has already cost
    // four fixes here.
    let occurrences = src.matches(".zshrc").count();
    assert_eq!(
        occurrences, 1,
        "`.zshrc` must appear only in shell_profile_path, not in a second list"
    );
}

// ---------------------------------------------------------------------------
// A port per agent — what lets ONE process serve them all.
// ---------------------------------------------------------------------------

/// Each agent gets its own port, and re-installing keeps the one it had.
///
/// Sharing a port between two agents produces a config that looks right and
/// sends every request to the wrong provider, because the port is the only
/// thing telling the single proxy process which agent a connection belongs to.
/// Re-installing must be idempotent for the same reason in reverse: moving an
/// agent's port would leave its config naming one nobody listens on.
#[test]
fn every_agent_gets_its_own_port_and_keeps_it_across_reinstalls() {
    let h = home();
    let first = listen_for(h.path(), Agent::Codex, "127.0.0.1:8787", None);
    assert_eq!(
        first, "127.0.0.1:8787",
        "the first agent takes the base port"
    );

    // Record it the way an install would, then ask for the next agent.
    let mut recs = vec![Record {
        agent: Agent::Codex.as_str().into(),
        path: "/tmp/x".into(),
        upstream: "https://api.openai.com/v1".into(),
        previous_selector: None,
        had_explicit_upstream: false,
        created_file: false,
        listen: first.clone(),
    }];
    save_records(h.path(), &recs).unwrap();

    let second = listen_for(h.path(), Agent::Aider, "127.0.0.1:8787", None);
    assert_ne!(
        second, first,
        "a second agent must not share the first's port"
    );
    assert_eq!(second, "127.0.0.1:8788");

    // Codex again: the SAME port, not a third one.
    assert_eq!(
        listen_for(h.path(), Agent::Codex, "127.0.0.1:8787", None),
        first
    );

    // And an explicit request always wins.
    assert_eq!(
        listen_for(
            h.path(),
            Agent::Codex,
            "127.0.0.1:8787",
            Some("127.0.0.1:9999")
        ),
        "127.0.0.1:9999"
    );

    recs.push(Record {
        agent: Agent::Aider.as_str().into(),
        path: "/tmp/y".into(),
        upstream: "https://api.openai.com/v1".into(),
        previous_selector: None,
        had_explicit_upstream: false,
        created_file: false,
        listen: second,
    });
    save_records(h.path(), &recs).unwrap();
    assert_eq!(
        listen_for(h.path(), Agent::Qwen, "127.0.0.1:8787", None),
        "127.0.0.1:8789",
        "a third agent takes the next free port, not a taken one"
    );
}

/// The record keeps the authority, so `serve` can bind what `install` wrote.
#[test]
fn an_install_records_the_port_it_pointed_the_agent_at() {
    let h = home();
    let p = Site::CodexToml.path(h.path());
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, "model = \"gpt-5.5\"\n").unwrap();

    let plan = plan(Agent::Codex, h.path(), "http://127.0.0.1:8791").expect("planned");
    let rec = install(h.path(), &plan).expect("installed");
    assert_eq!(rec.listen, "127.0.0.1:8791");
    assert_eq!(load_records(h.path()).unwrap()[0].listen, "127.0.0.1:8791");
}

// ---------------------------------------------------------------------------
// retired agents.
// ---------------------------------------------------------------------------

/// Reasonix left the product: no more recipe, so no more install. But the
/// record a previous version left behind is still undone — otherwise
/// `uninstall` would leave the agent pointed at a proxy that no longer runs.
#[test]
fn a_retired_agent_record_still_reverts() {
    let h = home();
    let p = Site::ReasonixToml.path(h.path());
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    let before = "[[providers]]\nname = \"only\"\nbase_url = \"https://api.deepseek.com\"\n";
    std::fs::write(&p, before.replace("https://api.deepseek.com", PROXY)).unwrap();
    assert!(legacy_site("reasonix").is_some());
    let record = Record {
        agent: "reasonix".into(),
        path: p.clone(),
        upstream: "https://api.deepseek.com".into(),
        previous_selector: Some("only".into()),
        had_explicit_upstream: true,
        created_file: false,
        listen: String::new(),
    };
    revert(&record).unwrap();
    assert_eq!(
        read(&p),
        before,
        "the revert must give the file back as it was"
    );
}

/// A Qwen whose selected model carries its own `baseUrl` (the OpenRouter
/// setup on this machine): Qwen reads that BEFORE `OPENAI_BASE_URL`, so the
/// proxy must sit there, with that URL as upstream — and go back exactly.
#[test]
fn qwen_is_routed_through_the_selected_models_own_base_url() {
    let h = home();
    let path = h.path().join(".qwen").join("settings.json");
    let before = r#"{"model":{"name":"qwen/qwen3-coder-next"},"modelProviders":{"openai":[{"id":"qwen/qwen3-coder-next","baseUrl":"https://openrouter.ai/api/v1","envKey":"OPENROUTER_API_KEY"},{"id":"other","baseUrl":"https://x.example/v1"}]},"security":{"auth":{"selectedType":"openai"}}}"#;
    write(&path, before);
    assert!(
        bypassed(Agent::Qwen, h.path(), PROXY),
        "an env-only install would be bypassed"
    );
    let p = plan(Agent::Qwen, h.path(), PROXY).unwrap();
    assert_eq!(p.upstream, "https://openrouter.ai/api/v1");
    install(h.path(), &p).unwrap();
    let after: serde_json::Value = serde_json::from_str(&read(&path)).unwrap();
    assert_eq!(
        after
            .pointer("/modelProviders/openai/0/baseUrl")
            .and_then(|v| v.as_str()),
        Some(PROXY)
    );
    assert_eq!(
        after
            .pointer("/modelProviders/openai/1/baseUrl")
            .and_then(|v| v.as_str()),
        Some("https://x.example/v1"),
        "other models untouched"
    );
    assert!(after.get("env").is_none(), "no env key written");
    assert!(!bypassed(Agent::Qwen, h.path(), PROXY));
    uninstall(h.path(), Agent::Qwen).unwrap();
    let back: serde_json::Value = serde_json::from_str(&read(&path)).unwrap();
    assert_eq!(
        back,
        serde_json::from_str::<serde_json::Value>(before).unwrap()
    );
}

/// This machine's Qwen: the LEGACY `{ protocol, models }` provider shape
/// (which Qwen skips) and `OPENAI_BASE_URL` in `~/.qwen/.env` (which Qwen
/// loads before `settings.env`). The proxy goes in `.env`; settings are left
/// alone; uninstall puts `.env` back.
#[test]
fn qwen_with_a_dotenv_base_url_is_routed_through_dotenv() {
    let h = home();
    let settings = h.path().join(".qwen").join("settings.json");
    let legacy = r#"{"model":{"name":"m"},"modelProviders":{"openai":{"protocol":"openai","models":[{"id":"m","baseUrl":"https://openrouter.ai/api/v1"}]}}}"#;
    write(&settings, legacy);
    let dotenv = h.path().join(".qwen").join(".env");
    let env_before =
        "OPENROUTER_API_KEY=k\nOPENAI_BASE_URL=https://openrouter.ai/api/v1\nOPENAI_MODEL=m\n";
    write(&dotenv, env_before);
    assert!(bypassed(Agent::Qwen, h.path(), PROXY));
    let p = plan(Agent::Qwen, h.path(), PROXY).unwrap();
    assert_eq!(p.upstream, "https://openrouter.ai/api/v1");
    install(h.path(), &p).unwrap();
    assert!(
        read(&dotenv).contains(&format!("OPENAI_BASE_URL={PROXY}")),
        "{}",
        read(&dotenv)
    );
    assert_eq!(read(&settings), legacy, "settings untouched");
    assert!(!bypassed(Agent::Qwen, h.path(), PROXY));
    uninstall(h.path(), Agent::Qwen).unwrap();
    assert_eq!(read(&dotenv).trim_end(), env_before.trim_end());
}

/// A unit that does not name its agent leaves the proxy unable to ask the
/// matrix what it may do, and its rows attributed to nobody.
#[test]
fn the_unit_names_its_agent() {
    use crate::llm_proxy::autostart::Service;
    let base = Service {
        exe: "/x/tokenade".into(),
        listen: "127.0.0.1:8787".into(),
        upstream: "https://api.x.ai/v1".into(),
        agent: None,
    };
    let s = Service {
        agent: Some("grok".into()),
        ..base.clone()
    };
    assert!(
        s.argv().windows(2).any(|w| w == ["--agent", "grok"]),
        "{:?}",
        s.argv()
    );
    assert!(s
        .polling_argv()
        .windows(2)
        .any(|w| w == ["--agent", "grok"]));
    assert!(!base.argv().iter().any(|a| a == "--agent"));
}

/// Codex signed in with a ChatGPT account — the desktop app's default — has no
/// API key: a provider naming `env_key = "OPENAI_API_KEY"` left it with no
/// credential. It must ask Codex for its own account auth and go to the
/// ChatGPT backend; an API-key user keeps the key route.
#[test]
fn codex_with_a_chatgpt_account_keeps_its_account_through_the_proxy() {
    let h = home();
    let cfg = h.path().join(".codex").join("config.toml");
    write(&cfg, "model = \"gpt-5.5\"\n");
    write(
        &h.path().join(".codex").join("auth.json"),
        r#"{"auth_mode":"chatgpt","OPENAI_API_KEY":null,"tokens":{"id_token":"x"}}"#,
    );
    let p = plan(Agent::Codex, h.path(), PROXY).unwrap();
    assert_eq!(p.upstream, "https://chatgpt.com/backend-api/codex");
    install(h.path(), &p).unwrap();
    let t = read(&cfg);
    assert!(t.contains("requires_openai_auth = true"), "{t}");
    assert!(!t.contains("env_key"), "{t}");

    let h = home();
    let cfg = h.path().join(".codex").join("config.toml");
    write(&cfg, "model = \"gpt-5.5\"\n");
    write(
        &h.path().join(".codex").join("auth.json"),
        r#"{"OPENAI_API_KEY":"sk-test"}"#,
    );
    let p = plan(Agent::Codex, h.path(), PROXY).unwrap();
    assert_eq!(p.upstream, "https://api.openai.com/v1");
    install(h.path(), &p).unwrap();
    assert!(read(&cfg).contains("env_key = \"OPENAI_API_KEY\""));
}
