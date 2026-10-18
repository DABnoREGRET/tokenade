//! **What the command line SAYS** — and nothing of what it does.
//!
//! `parse` returns a [`Cmd`] and touches neither the disk nor the network.
//! That is what makes the verbs testable: half the questions one asks about
//! this command ("does it uninstall?", "which port if none is given?") are
//! answered on a value, without starting a server.
//!
//! `undoes_an_install` lives here for the same reason: whether an invocation
//! UNDOES something is a property of the request, not of its execution, and
//! it is the question a confirmation must ask before acting.

use super::*;

/// What the user asked for.
#[derive(Debug, PartialEq, Eq)]
pub enum Cmd {
    /// Install the proxy wherever it brings something, without saying anything.
    AutoInstall,
    Serve {
        listen: String,
        upstream: String,
        agent: Option<Agent>,
        remote: bool,
        compaction: bool,
    },
    Install {
        agent: Agent,
        listen: String,
        autostart: bool,
        enable: bool,
    },
    Uninstall {
        agent: Agent,
    },
    /// Start the proxy if it is not already answering, then EXIT.
    ///
    /// The action a supervisor that POLLS should run, as opposed to one that
    /// watches a process. Windows Task Scheduler is the polling kind: it fires
    /// a repeating trigger every minute and, with `IgnoreNew`, skips a firing
    /// while it still believes the previous instance is alive. Measured on the
    /// Windows guest 2026-08-20 with the proxy ITSELF as the action: killed at
    /// T, back at T+220 s — nearly four firings ignored, and 24 of 24 rows read
    /// `Ko proxy` under a full bench because the probe's 240 s budget is not
    /// four minutes of slack.
    ///
    /// With a short action the instance is always over in about a second, so a
    /// firing is never skipped and a dead proxy comes back at the next minute.
    EnsureRunning {
        listen: String,
        upstream: String,
        agent: Option<String>,
    },
    /// Write the platform's service definition and print how to enable it.
    ///
    /// Separate from `install` because the two answer different questions —
    /// "point this agent at the proxy" and "keep the proxy running" — and
    /// because the second is the one an operator needs to be able to verify on
    /// its own, on a machine where no agent is installed at all.
    Autostart {
        listen: String,
        upstream: String,
        remove: bool,
        enable: bool,
        /// The agent this port serves, booked on every row it writes.
        agent: Option<String>,
    },
    /// Serve every agent the records name, from one process.
    ServeInstalled {
        remote: bool,
        compaction: bool,
    },
    Status,
    Help,
    /// Something we could not parse. Carries the message to print.
    Bad(String),
}

/// The address the proxy binds when nobody says.
///
/// Loopback, and a port well outside anything a dev server picks. There is no
/// discovery and no fallback port: a proxy that quietly moved would leave every
/// installed agent pointing at the old one, which is a broken agent rather than
/// a degraded one.
pub const DEFAULT_LISTEN: &str = "127.0.0.1:8787";

/// Parse `rest` (everything after `llm-proxy`).
pub fn parse(rest: &[String]) -> Cmd {
    let mut it = rest.iter().map(String::as_str).peekable();
    let sub = match it.peek().copied() {
        Some("install")
        | Some("uninstall")
        | Some("status")
        | Some("autostart")
        // `auto-install` was MISSING here while the match below carried an arm
        // for it. A word absent from this list is not rejected — it falls to
        // `_ => "serve"`, stays in the iterator, and the option loop refuses it
        // as `unknown option auto-install`. So `Cmd::AutoInstall` was
        // unreachable, and the only thing that ever asks for it is
        // `hook_health::spawn_proxy_auto_install`, which spawns it detached and
        // discards the exit code — after stamping its 24h marker. The daily
        // proxy install has therefore never run once, on any machine: the
        // marker said "checked today", the subprocess had died with rc=2.
        // Found 2026-09-02 on a box whose agent config had pointed at a dead
        // port since 08-25.
        | Some("auto-install")
        | Some("ensure-running") => it.next().unwrap(),
        Some("--help") | Some("-h") | Some("help") => return Cmd::Help,
        Some("serve") => {
            it.next();
            "serve"
        }
        _ => "serve",
    };

    let mut listen: Option<String> = None;
    let mut upstream: Option<String> = None;
    let mut agent: Option<String> = None;
    let mut remote = false;
    let mut compaction = true;
    let mut autostart = false;
    // `--autostart` WRITES the definition and prints what makes it live, on
    // purpose: a background service that outlives the shell is a visible act.
    // `--enable` is the user saying it out loud, and it exists because
    // "printed but not run" is a real gap and not only a caution — the unit
    // sits there declaring `Restart=always` and nothing restarts anything.
    let mut enable = false;
    let mut remove = false;

    while let Some(a) = it.next() {
        let mut take = |name: &str| -> Result<String, String> {
            it.next()
                .map(str::to_string)
                .ok_or_else(|| format!("{name} needs a value"))
        };
        let r = match a {
            "--listen" => take("--listen").map(|v| listen = Some(v)),
            "--upstream" => take("--upstream").map(|v| upstream = Some(v)),
            "--agent" => take("--agent").map(|v| agent = Some(v)),
            "--remote" => {
                remote = true;
                Ok(())
            }
            "--no-compaction" => {
                compaction = false;
                Ok(())
            }
            "--enable" => {
                enable = true;
                Ok(())
            }
            "--autostart" => {
                autostart = true;
                Ok(())
            }
            "--remove" => {
                remove = true;
                Ok(())
            }
            "--help" | "-h" => return Cmd::Help,
            other => Err(format!("unknown option {other}")),
        };
        if let Err(e) = r {
            return Cmd::Bad(e);
        }
    }

    // Resolved through the product's own name table rather than a private list,
    // so `--agent claude-code` means the same thing here as everywhere else.
    let agent = match agent {
        Some(name) => match agent_matrix::by_name(&name) {
            Some(a) => Some(a),
            None => {
                if let Some(why) = agent_matrix::rank4_refusal(&name) {
                    return Cmd::Bad(why.to_string());
                }
                return Cmd::Bad(format!("unknown agent {name:?}"));
            }
        },
        None => None,
    };

    match sub {
        "status" => Cmd::Status,
        "ensure-running" => match upstream {
            Some(u) => Cmd::EnsureRunning {
                listen: listen.unwrap_or_else(|| DEFAULT_LISTEN.to_string()),
                upstream: u,
                agent: agent.map(|a| a.as_str().to_string()),
            },
            None => Cmd::Bad(
                "ensure-running needs --upstream <url>: it may have to START the proxy, \
                 and a proxy with no upstream answers nothing"
                    .into(),
            ),
        },
        "autostart" => {
            if autostart {
                // `--autostart` on the `autostart` subcommand is a user saying
                // the same thing twice, not a second meaning. `--remove` is the
                // opposite verb and is spelled that way.
                return Cmd::Bad("autostart does not take --autostart".into());
            }
            match upstream {
                Some(u) => Cmd::Autostart {
                    listen: listen.unwrap_or_else(|| DEFAULT_LISTEN.to_string()),
                    upstream: u,
                    remove,
                    enable,
                    agent: agent.map(|a| a.as_str().to_string()),
                },
                // Removal deletes a file and prints the disable commands. It
                // reads NOTHING out of the upstream, so demanding one made the
                // user recall the URL they installed with before they were
                // allowed to undo it — and on a machine whose licence had
                // lapsed, `Cmd::Bad` also meant the gate exemption below could
                // never fire. One requirement, two ways to strand somebody.
                None if remove => Cmd::Autostart {
                    listen: listen.unwrap_or_else(|| DEFAULT_LISTEN.to_string()),
                    upstream: String::new(),
                    remove,
                    enable,
                    agent: agent.map(|a| a.as_str().to_string()),
                },
                None => Cmd::Bad(
                    "autostart needs --upstream <url>: the unit has to carry the provider \
                     the proxy forwards to, or the service starts and answers nothing"
                        .into(),
                ),
            }
        }
        "install" => match agent {
            Some(a) => Cmd::Install {
                agent: a,
                listen: listen.unwrap_or_else(|| DEFAULT_LISTEN.to_string()),
                autostart,
                enable,
            },
            None => Cmd::Bad("install needs --agent <name>".into()),
        },
        // HIDDEN subcommand: the automatic installation, launched detached by
        // the startup repair. It does not appear in the help because a human
        // has no reason to type it — `install` already does everything, while
        // talking.
        "auto-install" => Cmd::AutoInstall,
        "uninstall" => match agent {
            Some(a) => Cmd::Uninstall { agent: a },
            None => Cmd::Bad("uninstall needs --agent <name>".into()),
        },
        _ => match upstream {
            Some(u) => Cmd::Serve {
                listen: listen.unwrap_or_else(|| DEFAULT_LISTEN.to_string()),
                upstream: u,
                agent,
                remote,
                compaction,
            },
            // No upstream given is not a mistake any more: it means "serve
            // whatever is installed". The records already hold each agent's
            // provider and port, so nothing is guessed — an empty set is still
            // refused, by `serve_all`, and it says which command fills it.
            None => Cmd::ServeInstalled { remote, compaction },
        },
    }
}

/// A provider URL made safe to print.
///
/// The classification guard asked whether this command can hand back a
/// credential, and the answer was worse than expected: some providers take
/// their key as a QUERY PARAMETER (Gemini's `?key=…`), so the "upstream" we
/// captured out of an agent's config — and print back in `install` and
/// `status` — is not reliably secret-free. Run through the product's own
/// scrubber, which leaves an ordinary URL copyable and masks a key-bearing one.
pub(crate) fn shown(url: &str) -> String {
    tokenade_core::redact::redact(url)
}

/// The address written into an agent's config: an ORIGIN, never a path.
///
/// The proxy appends whatever route the agent asked for to the provider's base
/// URL, so a path here would be sent twice — `…/v1/v1/responses`.
/// [`install::plan`] refuses one; this is the constructor that cannot build one.
pub(crate) fn base_url_for(listen: &str) -> String {
    format!("http://{listen}")
}
