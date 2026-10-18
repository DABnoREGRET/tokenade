//! `tokenade llm-proxy` — the route that reaches rank 4.
//!
//! Wired on the operator's decision of 2026-08-16, after the module had been
//! written and tested with nothing reaching it. Two things follow from that,
//! and both are deliberate:
//!
//! * **`tokenade install` sets it up only where it adds something** the agent
//!   cannot have otherwise ([`features::offer`], e.g. Codex with untrusted
//!   hooks), health-checked and rolled back on failure. Everywhere else rank 4
//!   is opt-in per agent, by an explicit `llm-proxy install --agent <name>`:
//!   this rung can break an agent (see [`super::autostart`]).
//! * **The verbs are separate.** `llm-proxy` serves, `llm-proxy install` points
//!   an agent at it. Serving does not install and installing does not serve,
//!   because an install that silently started a background process would be the
//!   invisible-daemon problem this product already refuses elsewhere.
//!
//! The argument parsing is a pure function over `&[String]` with its own tests.
//! A CLI defect that only shows up when a user types the command is a defect
//! nobody on this side ever sees.

use super::{agent_matrix, autostart, config, features, install, server};
use std::path::PathBuf;
use tokenade_core::detect::Agent;
// `setup` and not `install`: `llm_proxy::install` already exists and carries
// the installation RECORDS. This one carries the VERBS.
mod args;
mod serve;
mod setup;
pub(crate) use args::*;
pub(crate) use serve::*;
pub(crate) use setup::*;

/// `tokenade llm-proxy …`
pub fn cmd_llm_proxy(rest: &[String]) -> i32 {
    match parse(rest) {
        Cmd::Help => {
            println!("{}", help());
            0
        }
        Cmd::Bad(msg) => {
            eprintln!("tokenade llm-proxy: {msg}\n\n{}", help());
            2
        }
        Cmd::Status => status(),
        Cmd::AutoInstall => {
            let present = tokenade_core::detect::detect_all_present(home().as_deref());
            // Silent by construction: nobody reads its output. What it does
            // shows in `tokenade llm-proxy status`, and what it refused to do
            // shows in the agent's untouched config.
            let _ = ensure_installed_for(&present);
            0
        }
        Cmd::Install {
            agent,
            listen,
            autostart,
            enable,
        } => do_install(agent, &listen, autostart, enable),
        Cmd::Uninstall { agent } => do_uninstall(agent),
        Cmd::Autostart {
            listen,
            upstream,
            remove,
            enable,
            agent,
        } => do_autostart(&listen, &upstream, remove, enable, agent.as_deref()),
        Cmd::EnsureRunning {
            listen,
            upstream,
            agent,
        } => do_ensure_running(&listen, &upstream, agent.as_deref()),
        Cmd::ServeInstalled { remote, compaction } => serve_installed(remote, compaction),
        Cmd::Serve {
            listen,
            upstream,
            agent,
            remote,
            compaction,
        } => {
            let cfg = config::Config {
                listen,
                upstream,
                deployment: if remote {
                    config::Deployment::Remote
                } else {
                    config::Deployment::Local
                },
                compaction,
                agent,
                // The preamble the operator set with `tokenade style`, resolved
                // once at startup.
                //
                // Only where the agent has no prompt-time hook of its own:
                // where one exists, rank 1 already says this at the right
                // moment and saying it twice is the double authority that
                // "one authority per payload" forbids. `Style::Off` resolves to an empty preamble, which
                // `prepend_system_preamble` treats as no rewrite at all, so an
                // operator who never set a style is unaffected.
                // No filter on which agents get this.
                //
                // It was gated on `hook_name.is_none()` for one build, and
                // that is the wrong question twice over: `hook_name` names a
                // TOOL hook, not a prompt-time one, so it excluded opencode
                // — which has no prompt event at all and is exactly who needs
                // this — while including nobody it should have. And gating by
                // agent is the mistake this whole session has been undoing.
                //
                // What prevents the double declaration is
                // `prepend_system_preamble` refusing to add text the request
                // already carries, wherever a rank-1 hook happened to put it.
                // Gated by the measured matrix since 1.2.0: an agent whose
                // rules file already carries the style (`LeanOutput` Full)
                // got it twice.
                system_preamble: features::preamble_for(agent),
                ..config::Config::default()
            };
            let proxy = server::Proxy::new(
                cfg,
                server::default_folder(),
                Box::new(server::LedgerAccounting),
            );
            match proxy.serve() {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("tokenade llm-proxy: {e}");
                    1
                }
            }
        }
    }
}

fn status() -> i32 {
    let Some(home) = home() else {
        eprintln!("tokenade llm-proxy: no home directory");
        return 1;
    };
    match install::load_records(&home) {
        Ok(rs) if rs.is_empty() => {
            println!("no agent is pointed at the tokenade llm proxy");
            println!("\n{}", agent_matrix::render());
            0
        }
        Ok(rs) => {
            for r in &rs {
                println!(
                    "{:<12} {} on {} → {}",
                    r.agent,
                    r.path.display(),
                    if r.listen.is_empty() {
                        DEFAULT_LISTEN
                    } else {
                        &r.listen
                    },
                    shown(&r.upstream)
                );
            }
            // What the ten boxes say for these agents, from the binary rather
            // than from the web page. If the two ever disagree, one of them is
            // lying to a user, and this is the copy they can check.
            for r in &rs {
                let Some(a) = agent_matrix::by_name(&r.agent) else {
                    continue;
                };
                println!("\n{}", r.agent);
                println!("  ({})", features::Support::legend());
                for (f, before, after) in features::support_table(a) {
                    let arrow = if after > before {
                        format!("{} → {}", before.mark(), after.mark())
                    } else {
                        after.mark().to_string()
                    };
                    println!("  {:<22} {arrow}", f.label());
                }
            }
            0
        }
        // The distinction that matters: unreadable is NOT "nothing installed".
        Err(install::RecordsError::Unreadable(p)) => {
            eprintln!(
                "tokenade llm-proxy: {} could not be read, so what is installed is UNKNOWN — \
                 the agents' own configs and their .tokenade-backup-* files are the record now",
                p.display()
            );
            1
        }
    }
}

fn help() -> String {
    format!(
        "tokenade llm-proxy — rank 4 of the compaction ladder\n\
         \n\
         Sits between an agent and its provider: on the way out it folds tool results\n\
         already in the history, sends an identical tool output only once and scrubs\n\
         credentials; on the way back it reads the provider's own usage.\n\
         It does NOT refund the turn a raw result first arrived on; it removes its\n\
         cost from every later turn. A hook catches it before the first send, which\n\
         is why rank 1 stays above this one.\n\
         \n\
         `tokenade install` sets it up by itself only where it adds something the\n\
         agent's other channels cannot (e.g. Codex with untrusted hooks), and checks\n\
         it at each update. Everywhere else it is opt-in, per agent.\n\
         \n\
         SERVE\n  \
         tokenade llm-proxy [--upstream <url>] [--listen {DEFAULT_LISTEN}] [--agent <name>]\n\
         \x20                    [--remote] [--no-compaction]\n\
         Without --upstream, serves every agent recorded by `install`, each on its port.\n\
         \n\
         POINT AN AGENT AT IT\n  \
         tokenade llm-proxy install --agent <name> [--listen <addr>] [--autostart [--enable]]\n  \
         tokenade llm-proxy uninstall --agent <name>\n  \
         tokenade llm-proxy status\n\
         `install` gives each agent its own port (8787, 8788, …).\n\
         \n\
         --remote is required to bind anything but loopback: this process holds your\n\
         API key, and a non-loopback bind is reachable from off this machine.\n\
         --autostart writes the platform's own service definition and prints the\n\
         commands that enable it; --enable also runs them.\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Cmd {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    /// The serving question is asked on the START path and on no other.
    ///
    /// A source guard rather than a behavioural one because the defect this
    /// closes was a MISSING CALL, and the only test that can fail on a missing
    /// call is one that reads the call site. Two ways to get it wrong, and
    /// both are worse than the silence they replace: not asking (the original
    /// defect, where `enable --now` reported success over a dead unit), and
    /// asking on removal — where the unit is supposed to be gone and `is-active`
    /// answering "inactive" is the goal, so an unguarded call would fail every
    /// successful uninstall.
    #[test]
    fn the_serving_check_runs_on_the_start_path_only() {
        // The FAMILY: `do_autostart` and `confirm_it_serves` moved down into
        // `cli/setup.rs`, and a CALL-SITE guard reading only the trunk would
        // conclude "the call disappeared" when it has moved.
        let root = tokenade_core::source_guard::crates_dir()
            .join("tokenade-cli")
            .join("src")
            .join("llm_proxy");
        let src = tokenade_core::source_guard::module_family(&root, "cli");
        let src = src.as_str();
        let body = src
            // The family contains THIS file, so the seam written verbatim
            // would find itself. Split in two, it only designates the real
            // declaration.
            .split_once(concat!("fn ", "apply_supervisor("))
            .expect("apply_supervisor is still the enable path")
            .1;
        let body = body.split_once("\n}\n").expect("function has an end").0;
        assert!(
            body.contains("confirm_it_serves(platform, listen)"),
            "the enable path stopped asking whether the unit actually serves"
        );
        let call = body
            .split_once("confirm_it_serves(platform, listen)")
            .unwrap()
            .0;
        let guard_line = call.rsplit_once("if ").expect("the call is guarded").1;
        assert!(
            guard_line.starts_with("!removing"),
            "the serving question must be asked only when starting, not when \
             removing: found guard `{guard_line}`"
        );
    }

    /// Undoing an autostart must not require the URL it was installed with.
    ///
    /// Removal deletes a file and prints the disable commands; it never reads
    /// the upstream. Requiring one made the user recall a URL to be allowed to
    /// undo.
    #[test]
    fn removing_an_autostart_needs_nothing_but_the_word_remove() {
        assert_eq!(
            p(&["autostart", "--remove"]),
            Cmd::Autostart {
                listen: DEFAULT_LISTEN.into(),
                upstream: String::new(),
                remove: true,
                enable: false,
                agent: None,
            }
        );
        // Installing one still needs it: a unit with no provider starts and
        // answers nothing.
        assert!(matches!(p(&["autostart"]), Cmd::Bad(_)));
    }

    /// No `--upstream` means "serve what is installed", and still never guesses.
    ///
    /// This used to assert a REFUSAL, which was right while one process served
    /// one agent. The property it was protecting is narrower than the refusal
    /// was: never invent a provider. Serving the recorded set does not invent
    /// one — every upstream there was read out of an agent's own config — and
    /// an empty set is still refused, by `serve_all`, in words that name the
    /// command which fills it.
    #[test]
    fn an_upstream_less_serve_takes_the_installed_set_and_still_guesses_nothing() {
        assert_eq!(
            p(&[]),
            Cmd::ServeInstalled {
                remote: false,
                compaction: true,
            }
        );
        let refusal = server::serve_all(Vec::new()).unwrap_err();
        assert!(refusal.contains("install --agent"), "{refusal}");
        assert!(refusal.contains("--upstream"), "{refusal}");
    }

    #[test]
    fn a_serve_takes_the_loopback_default_and_the_flags_given() {
        assert_eq!(
            p(&["--upstream", "https://api.openai.com/v1"]),
            Cmd::Serve {
                listen: DEFAULT_LISTEN.into(),
                upstream: "https://api.openai.com/v1".into(),
                agent: None,
                remote: false,
                compaction: true,
            }
        );
        assert_eq!(
            p(&[
                "serve",
                "--listen",
                "0.0.0.0:9000",
                "--upstream",
                "https://x/v1",
                "--agent",
                "codex",
                "--remote",
                "--no-compaction",
            ]),
            Cmd::Serve {
                listen: "0.0.0.0:9000".into(),
                upstream: "https://x/v1".into(),
                agent: Some(Agent::Codex),
                remote: true,
                compaction: false,
            }
        );
    }

    #[test]
    fn install_and_uninstall_need_an_agent_by_name() {
        assert_eq!(
            p(&["install", "--agent", "codex"]),
            Cmd::Install {
                agent: Agent::Codex,
                listen: DEFAULT_LISTEN.into(),
                autostart: false,
                enable: false,
            }
        );
        assert_eq!(
            p(&["install", "--agent", "aider", "--autostart"]),
            Cmd::Install {
                agent: Agent::Aider,
                listen: DEFAULT_LISTEN.into(),
                autostart: true,
                enable: false,
            }
        );
        assert_eq!(
            p(&["uninstall", "--agent", "codex"]),
            Cmd::Uninstall {
                agent: Agent::Codex
            }
        );
        assert!(matches!(p(&["install"]), Cmd::Bad(_)));
        assert!(matches!(p(&["uninstall"]), Cmd::Bad(_)));
    }

    /// What the product spawns at ITSELF, the product must be able to parse.
    ///
    /// `auto-install` had an arm in the `match sub` below and was missing from
    /// the whitelist that decides what a subcommand IS, so it fell to
    /// `_ => "serve"`, stayed in the iterator, and came back out of the option
    /// loop as `unknown option auto-install`. `Cmd::AutoInstall` was therefore
    /// unreachable — and its only caller,
    /// `hook_health::spawn_proxy_auto_install`, spawns it DETACHED and
    /// discards the exit code, after stamping a 24h marker. The daily proxy
    /// install had never run once, on any machine, while the marker said it
    /// had.
    ///
    /// The argv is read out of the spawning source rather than retyped, so a
    /// rename on either side fails here instead of going quiet again.
    #[test]
    fn every_argv_the_product_spawns_at_itself_parses() {
        const SPAWNER: &str = include_str!("../hook_health.rs");
        let flat: String = SPAWNER.split_whitespace().collect::<Vec<_>>().join(" ");
        let needle = "&[\"llm-proxy\", \"";
        let at = flat
            .find(needle)
            .expect("hook_health no longer spawns `llm-proxy` — has the caller moved?");
        let rest = &flat[at + needle.len()..];
        let sub = &rest[..rest.find('"').expect("unterminated argv literal")];
        assert!(
            !sub.is_empty(),
            "the spawned subcommand is empty: {sub:?} — the reader is wrong, not the code"
        );
        let got = p(&[sub]);
        assert!(
            !matches!(got, Cmd::Bad(_)),
            "the product spawns `llm-proxy {sub}` and refuses it: {got:?}"
        );
        // And not the `serve` fallback either: an unknown word lands there
        // silently, which is how this went unnoticed. `serve` with no upstream
        // is `Bad`, but a future default could make it Ok and hide the gap
        // again, so the shape is named.
        assert!(
            !matches!(got, Cmd::Serve { .. }),
            "`llm-proxy {sub}` fell through to the serve fallback instead of \
             being recognised as a subcommand"
        );
        assert_eq!(got, Cmd::AutoInstall, "argv read from the spawner: {sub:?}");
    }

    #[test]
    fn an_unknown_agent_or_option_is_named_rather_than_ignored() {
        // Silently ignoring an unknown flag is how a user ends up believing
        // they installed with `--autostart` when they typed `--auto-start`.
        let Cmd::Bad(a) = p(&["install", "--agent", "not-an-agent"]) else {
            panic!("unknown agent must be refused")
        };
        assert!(a.contains("not-an-agent"), "{a}");
        let Cmd::Bad(b) = p(&["--upstream", "https://x", "--auto-start"]) else {
            panic!("unknown option must be refused")
        };
        assert!(b.contains("--auto-start"), "{b}");
        let Cmd::Bad(c) = p(&["--upstream"]) else {
            panic!("a flag with no value must be refused")
        };
        assert!(c.contains("--upstream"), "{c}");
    }

    #[test]
    fn claude_desktop_is_refused_as_rank3_not_unknown() {
        // Matrix `Ko proxy` used to print `unknown agent` — as if the
        // product did not exist. mcp-wrap already discovers it; rank 4
        // has no door. The refusal must name the live channel.
        let Cmd::Bad(a) = p(&["install", "--agent", "claude-desktop"]) else {
            panic!("claude-desktop must be refused, not installed")
        };
        assert!(a.contains("mcp-wrap"), "must name rank 3, got {a}");
        assert!(
            !a.contains("unknown agent"),
            "must not look like a missing product: {a}"
        );
        let Cmd::Bad(b) = p(&["install", "--agent", "not-an-agent"]) else {
            panic!("a truly unknown name still uses unknown agent")
        };
        assert!(b.contains("unknown agent"), "{b}");
    }

    #[test]
    fn claude_cowork_and_codex_app_are_refused_as_class_not_unknown() {
        // Same class as claude-desktop: known products, no rank-4
        // door. Matrix `Ko proxy` used to print `unknown agent`.
        for (name, must) in [("claude-cowork", "Anthropic"), ("codex-app", "compiled")] {
            let Cmd::Bad(a) = p(&["install", "--agent", name]) else {
                panic!("{name} must be refused, not installed")
            };
            assert!(
                a.contains(must),
                "{name} must name the measured reason, got {a}"
            );
            assert!(
                !a.contains("unknown agent"),
                "{name} must not look like a missing product: {a}"
            );
        }
        // Codex CLI is a different product and stays installable by name.
        assert_eq!(
            p(&["install", "--agent", "codex"]),
            Cmd::Install {
                agent: Agent::Codex,
                listen: DEFAULT_LISTEN.into(),
                autostart: false,
                enable: false,
            }
        );
    }

    #[test]
    fn help_is_reachable_from_every_position() {
        for args in [
            vec!["--help"],
            vec!["-h"],
            vec!["help"],
            vec!["install", "--help"],
            vec!["--upstream", "https://x", "--help"],
        ] {
            assert_eq!(p(&args), Cmd::Help, "{args:?}");
        }
    }

    #[test]
    fn the_written_base_url_never_carries_a_path() {
        // The constructor that cannot build the misroute `install::plan`
        // refuses: a path here is prepended to every request and then joined to
        // an upstream that already has one.
        let url = base_url_for(DEFAULT_LISTEN);
        assert_eq!(url, "http://127.0.0.1:8787");
        assert!(!install::url_has_path(&url));
    }

    #[test]
    fn autostart_is_its_own_verb_and_needs_the_upstream() {
        // The unit has to carry the provider: a service that starts and has
        // nowhere to forward answers nothing, and it does that at boot, where
        // nobody is watching.
        assert_eq!(
            p(&["autostart", "--upstream", "https://api.openai.com/v1"]),
            Cmd::Autostart {
                listen: DEFAULT_LISTEN.into(),
                upstream: "https://api.openai.com/v1".into(),
                remove: false,
                enable: false,
                agent: None,
            }
        );
        assert_eq!(
            p(&["autostart", "--upstream", "https://x/v1", "--remove"]),
            Cmd::Autostart {
                listen: DEFAULT_LISTEN.into(),
                upstream: "https://x/v1".into(),
                remove: true,
                enable: false,
                agent: None,
            }
        );
        assert!(matches!(p(&["autostart"]), Cmd::Bad(_)));
        assert!(matches!(
            p(&["autostart", "--upstream", "https://x", "--autostart"]),
            Cmd::Bad(_)
        ));
    }

    #[test]
    fn a_provider_url_that_carries_a_key_is_never_printed_whole() {
        // Gemini takes its key as a query parameter, so the upstream captured
        // from an agent's config is not reliably secret-free — and `install`
        // and `status` both print it back. An ordinary URL stays copyable.
        let secret = "https://generativelanguage.googleapis.com/v1beta?key=AIzaSyA1B2C3D4E5F6G7H8I9J0K1L2M3N4O5P6Q";
        let out = shown(secret);
        assert!(
            !out.contains("AIzaSyA1B2C3D4E5F6G7H8I9J0K1L2M3N4O5P6Q"),
            "{out}"
        );
        for plain in [
            "https://api.openai.com/v1",
            "https://api.anthropic.com",
            "http://127.0.0.1:8787",
        ] {
            assert_eq!(shown(plain), plain, "an ordinary URL must stay copyable");
        }
    }

    #[test]
    fn the_help_text_states_the_limit_rather_than_only_the_feature() {
        // The honest limit belongs in the product doc. The help is the
        // doc most users read, so it carries it.
        let h = help();
        assert!(h.contains("does NOT refund the turn"));
        assert!(h.contains("rank 1 stays above"));
        // And the two things that can bite: a non-loopback bind, and a config
        // pointing at a proxy nobody started.
        assert!(h.contains("--remote is required"));
        // Enabling the service stays explicit: printed, or run on --enable.
        assert!(h.contains("prints the\ncommands that enable it; --enable also runs them"));
    }
}

#[cfg(test)]
mod ensure_running_tests {
    use std::io::{Read, Write};

    /// A port occupied by someone ELSE is not our proxy, and the supervisor
    /// must start its own. A `connect()` said the opposite, and the "proxy
    /// survives a kill" column went from 24/24 `Ok proxy` to 24/24 `Ko proxy`
    /// on Windows for that reason — the scheduled task found the port taken
    /// by a bench proxy and started nothing.
    #[test]
    fn a_stranger_on_the_port_is_not_our_proxy() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = l.local_addr().expect("addr").to_string();
        let t = std::thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                let mut junk = [0u8; 512];
                let _ = s.read(&mut junk);
                let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi");
            }
        });
        assert!(
            super::our_proxy_health(&addr).is_none(),
            "a plain HTTP server answered for our proxy"
        );
        let _ = t.join();
    }

    /// And the POSITIVE, otherwise the probe could always answer "no" and the
    /// two tests above would pass: the supervisor would restart an already
    /// live proxy on every tick. The body is the one `write_health` writes —
    /// the two cannot diverge without this test saying so.
    #[test]
    fn our_own_health_answer_is_recognised() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = l.local_addr().expect("addr").to_string();
        let t = std::thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                let mut junk = [0u8; 512];
                let _ = s.read(&mut junk);
                let body = serde_json::json!({
                    "ok": true,
                    "version": env!("CARGO_PKG_VERSION"),
                    "deployment": "local",
                    "compaction": true,
                    "web_capabilities": Vec::<String>::new(),
                })
                .to_string();
                let _ = s.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    )
                    .as_bytes(),
                );
            }
        });
        assert!(
            super::our_proxy_health(&addr).is_some(),
            "our own health body was not recognised"
        );
        let _ = t.join();
    }

    /// And a port where nobody listens either, obviously.
    #[test]
    fn a_closed_port_is_not_our_proxy() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = l.local_addr().expect("addr").to_string();
        drop(l);
        assert!(super::our_proxy_health(&addr).is_none());
    }
}

/// `tokenade fold-payload` — a provider's payload on stdin, the same payload
/// folded on stdout.
///
/// It is rank 4 made available WITHOUT a proxy, for hosts that expose the
/// outgoing request themselves. pi documents `before_provider_request` as
/// "returning any other value replaces the payload for the actual request":
/// a bridge that posts the payload here and returns the result does in-agent
/// what the proxy does on the wire, with no port, no service and no second
/// process.
///
/// The "context optimisation" column lives there. A bridge that only folds
/// the current turn's tool result lets the HISTORY grow; here the whole
/// conversation goes through again, on every request.
///
/// Never claim a saving twice: `authority::may_fold` looks at the CONTENT, so
/// a block the result seam already folded carries its marker and is refused
/// here. The two seams can coexist on the same host without counting the same
/// payload twice.
///
/// Output unchanged when there is nothing to do (unknown wire, nothing to
/// fold), so a caller can assign without comparing.
pub(crate) fn cmd_fold_payload(_args: &[String]) -> i32 {
    use std::io::{Read as _, Write as _};
    let mut raw = Vec::new();
    if std::io::stdin().read_to_end(&mut raw).is_err() {
        return 1;
    }
    let out = super::compact_outgoing(
        &raw,
        |s: &str, h: Option<&str>| (super::server::default_folder())(s, h),
        None,
    );
    let bytes = match out {
        Some((body, r, _w)) => {
            book_payload_folds(&r);
            body
        }
        None => raw,
    };
    let mut so = std::io::stdout();
    let _ = so.write_all(&bytes);
    let _ = so.flush();
    0
}

/// What `fold-payload` folded, booked like the proxy books it: a block NEW in
/// this request (after the last assistant message) is a measured saving, with
/// its recovery offer; the history re-sent folded is a re-read spared, which
/// this path cannot price (no usage, no context size), so it books nothing for
/// it. One process per request remembers nothing — the position is the rule.
fn book_payload_folds(r: &super::wire::Rewrite) {
    let tok = |b: usize| b / 4;
    let (mut before, mut after, mut offered) = (0usize, 0usize, 0usize);
    for &(_, b, a, fresh, pointer) in &r.blocks {
        if fresh {
            before += tok(b);
            after += tok(a);
            if pointer {
                offered += tok(b).saturating_sub(tok(a));
            }
        }
    }
    if before > after {
        tokenade_core::gain::record(
            "fold-payload",
            "fold-payload:fold",
            "tool-result",
            tokenade_core::Savings::new(before, after),
        );
    }
    if offered > 0 {
        tokenade_core::gain::record_recovery("offered", offered);
    }
}

#[cfg(test)]
mod fold_payload_tests {
    /// Rank 4 without a proxy must fold what the proxy folds. Same Anthropic
    /// body, same `default_folder`, but without transport: if this one yields
    /// nothing, the command returns the input unchanged and the "context" cell
    /// stays `Partial` silently.
    #[test]
    fn an_anthropic_history_with_a_big_tool_result_shrinks() {
        // The SAME shape as the bench fixture: an `ls -l` listing. A first
        // version of this test invented lines that `auto::compact` classifies
        // as `Unknown` — the test failed on the fixture, not on the code, and
        // would have sent someone looking for a defect where there was none.
        let big: String = (0..4000)
            .map(|i| {
                format!(
                    "-rw-r--r--  1 user  staff  {}  Aug 18 10:{:02}  build/artifacts/module_{i}.o\n",
                    1000 + i,
                    i % 60
                )
            })
            .collect();
        let body = serde_json::json!({
            "model": "claude-x",
            "max_tokens": 100,
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "go"}]},
                {"role": "assistant", "content": [
                    {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "ls"}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "t1", "content": big}
                ]}
            ]
        });
        let raw = serde_json::to_vec(&body).unwrap();
        let folder = super::super::server::default_folder();
        let out =
            super::super::compact_outgoing(&raw, |s: &str, h: Option<&str>| folder(s, h), None);
        let (bytes, r, w) = out.expect("a 300 KB tool result must fold");
        assert_eq!(w, super::super::wire::Wire::Anthropic);
        assert!(r.folded > 0, "nothing folded: {r:?}");
        assert!(
            bytes.len() * 4 < raw.len(),
            "{} → {} is not a fold",
            raw.len(),
            bytes.len()
        );
    }

    /// The command hint comes from the payload and REACHES the folder.
    /// Measured 2026-08-21: without it, pi's "context" cell stayed under the
    /// threshold even though its seam worked. The assertion was about SIZE
    /// ("folds better with the hint"); it stopped separating when content
    /// detection learned `ls -l` without `total`, then flipped on `git log`
    /// (2 529 B without hint versus 3 962 with, 2026-10-03): what is guaranteed
    /// here is the transmission. That the named compactor folds better,
    /// `gemini_after_fold_uses_the_command_hint` measures it.
    #[test]
    fn the_command_hint_in_the_payload_reaches_the_folder() {
        let big: String = (0..3000)
            .map(|i| {
                format!(
                    "{:08x} feat(mod{}): change number {i} touching the {} path\n",
                    (i as u64).wrapping_mul(2_654_435_761) as u32 | 0x1000_0000,
                    i % 17,
                    i % 5
                )
            })
            .collect();
        let result = |with_hint: bool| {
            let mut messages = vec![serde_json::json!({
                "role": "user", "content": [{"type": "text", "text": "go"}]
            })];
            if with_hint {
                messages.push(serde_json::json!({
                    "role": "assistant",
                    "content": [{
                        "type": "tool_use", "id": "t1", "name": "Bash",
                        "input": {"command": "git log --oneline"}
                    }]
                }));
            }
            messages.push(serde_json::json!({
                "role": "user",
                "content": [{"type": "tool_result", "tool_use_id": "t1", "content": big}]
            }));
            let body = serde_json::json!({
                "model": "claude-x", "max_tokens": 100, "system": "s", "messages": messages
            });
            let raw = serde_json::to_vec(&body).unwrap();
            let mut hints: Vec<Option<String>> = Vec::new();
            super::super::compact_outgoing(
                &raw,
                |_: &str, h: Option<&str>| {
                    hints.push(h.map(str::to_string));
                    None
                },
                None,
            );
            hints
        };
        assert_eq!(result(true), vec![Some("git log --oneline".to_string())]);
        assert_eq!(result(false), vec![None]);
    }
}
