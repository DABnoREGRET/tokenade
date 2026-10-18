//! Being there at boot, on the three platforms.
//!
//! This is not a convenience. Rank 4 is the only rung whose installation can
//! BREAK the agent: a hook that is absent does nothing, a plugin that is absent
//! does nothing, but a base URL pointing at a proxy that is not running fails
//! every single turn. The moment `install` rewrites a config, something has to
//! guarantee the listener exists — after a reboot, after a crash, after a
//! logout.
//!
//! One supervisor per platform, each the native one, none of them a daemon we
//! wrote ourselves:
//!
//! | OS | mechanism | restarts on crash | survives reboot |
//! |---|---|---|---|
//! | Linux | systemd **user** unit | `Restart=always` | needs `loginctl enable-linger` |
//! | macOS | launchd `LaunchAgent` | `KeepAlive` | `RunAtLoad` at login |
//! | Windows | Scheduled Task | task restart options | `ONLOGON` trigger |
//!
//! **The lingering line on Linux is the one that is easy to get wrong.** A
//! systemd *user* manager normally starts at first login and stops at last
//! logout, so a unit that is `enabled` still will not be running on a machine
//! nobody has logged into. `enable-linger` is what makes "survives reboot" true
//! rather than "survives reboot if you log in first", and this module emits the
//! command rather than assuming it.
//!
//! Everything here GENERATES text and names commands. Nothing runs a privileged
//! step behind the user's back — the unit file is written, the enabling command
//! is printed, and the operator runs it. Installing a background service that
//! outlives the shell is exactly the class of action that has to be visible.

use std::path::{Path, PathBuf};

/// The identifier every platform uses for the service, in its own idiom.
pub const SERVICE_NAME: &str = "tokenade-llm-proxy";

mod names;
pub use names::*;

/// launchd wants a reverse-DNS label; Windows a path-like task name.
pub const LAUNCHD_LABEL: &str = "com.tokenade.llm-proxy";

/// Which launchd domain this Mac can actually load a service into.
///
/// Not a preference — a fact about the machine. `gui/<uid>` only EXISTS while
/// somebody is logged in at the screen; on a Mac reached over ssh, with
/// nobody at the console, `launchctl bootstrap gui/501 …` answers
/// `Bootstrap failed: 125: Domain does not support specified action` and
/// there is no flag that changes it. Measured 2026-08-19 on the build Mac:
/// `stat -f %Su /dev/console` = `root`, `who` empty — and a LaunchDaemon in
/// the `system` domain, carrying `UserName`, loaded and came back `state =
/// running` on the same machine in the same minute.
///
/// So "macOS cannot autostart headlessly" was never true; the AGENT domain
/// cannot. Every build machine, CI runner and headless Mac falls in the
/// second case, which is most of the ones that want a proxy running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacScope {
    /// Somebody is at the screen: a per-user LaunchAgent, no root anywhere.
    Gui,
    /// Nobody is: a LaunchDaemon in the system domain, which needs `sudo`.
    System,
}

/// Read the console user, and say which domain is available.
///
/// `TOKENADE_MAC_SCOPE=gui|system` overrides it, so a test can exercise both
/// branches on a machine that only has one.
pub fn mac_scope() -> MacScope {
    if let Ok(v) = std::env::var("TOKENADE_MAC_SCOPE") {
        return match v.trim().to_ascii_lowercase().as_str() {
            "gui" => MacScope::Gui,
            _ => MacScope::System,
        };
    }
    let out = tokenade_core::process_util::internal_command("stat")
        .args(["-f", "%Su", "/dev/console"])
        .output();
    match out {
        Ok(o) => {
            let who = String::from_utf8_lossy(&o.stdout).trim().to_string();
            // `root` is what the console reports when the login window is up
            // and nobody has logged in. An empty answer is the same fact.
            if who.is_empty() || who == "root" {
                MacScope::System
            } else {
                MacScope::Gui
            }
        }
        Err(_) => MacScope::System,
    }
}

/// Where the system-domain plist has to end up, and where we stage it first.
///
/// `/Library/LaunchDaemons` is root-owned. This module writes NOTHING as root
/// — every privileged act stays in the list it prints, which is the whole
/// design — so the file is written into the user's own tokenade directory and
/// the printed list copies it into place.
pub const LAUNCHD_SYSTEM_DIR: &str = "/Library/LaunchDaemons";
pub const WINDOWS_TASK: &str = "tokenade\\llm-proxy";

/// Which supervisor we are generating for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Linux,
    MacOs,
    Windows,
}

impl Platform {
    /// The platform this binary is running on.
    pub fn current() -> Option<Self> {
        match std::env::consts::OS {
            "linux" => Some(Self::Linux),
            "macos" => Some(Self::MacOs),
            "windows" => Some(Self::Windows),
            // A platform with no supervisor we have measured. Better to say so
            // than to write a unit file nothing will read.
            _ => None,
        }
    }
}

/// Everything the generated service needs to know.
#[derive(Debug, Clone)]
pub struct Service {
    /// Absolute path to the tokenade binary. Absolute on purpose: a supervisor
    /// starts with a minimal environment and frequently no useful `PATH`, so a
    /// bare name is the classic "works in my shell, not at boot" failure.
    pub exe: PathBuf,
    /// `host:port` the proxy binds.
    pub listen: String,
    /// The provider the proxy forwards to — captured by `install::plan` from
    /// the agent's own config before it was overwritten.
    pub upstream: String,
    /// The agent this port serves (`--agent`): the matrix and the ledger need it.
    pub agent: Option<String>,
}

impl Service {
    /// The argv the supervisor will run.
    ///
    /// Kept as a list rather than a string so each platform quotes it its own
    /// way, and so a path with a space cannot silently split into two arguments
    /// — which on Windows is not hypothetical, the default install lives under
    /// `C:\Users\<name>\AppData`.
    pub fn argv(&self) -> Vec<String> {
        vec![
            self.exe.display().to_string(),
            "llm-proxy".into(),
            "--listen".into(),
            self.listen.clone(),
            "--upstream".into(),
            self.upstream.clone(),
        ]
        .into_iter()
        .chain(self.agent_args())
        .collect()
    }

    fn agent_args(&self) -> Vec<String> {
        self.agent
            .iter()
            .flat_map(|a| ["--agent".to_string(), a.clone()])
            .collect()
    }

    /// What a POLLING supervisor should run.
    ///
    /// systemd and launchd watch a process: the service IS the proxy, and the
    /// moment it dies they start it again. Windows Task Scheduler fires a
    /// repeating trigger instead, and with `IgnoreNew` it skips a firing while
    /// it still believes the previous instance is alive — which, with the proxy
    /// itself as the action, it does for minutes after the process is gone.
    ///
    /// Measured on the Windows guest 2026-08-20, proxy killed at T: back at
    /// **T+220 s**, nearly four firings ignored. Under a full 24-agent pass the
    /// same wait blew the probe's 240 s budget on every row, so the column read
    /// `Ko proxy` 24 times for a supervisor that does work, slowly and
    /// unpredictably. The run before it, same binary, read `Ok proxy` 24 times.
    /// A cell that flips with machine load is not measuring the feature.
    ///
    /// A short action ends in about a second, so no firing is ever skipped and
    /// a dead proxy comes back at the next minute — bounded, and by a number
    /// the trigger declares.
    pub fn polling_argv(&self) -> Vec<String> {
        vec![
            self.exe.display().to_string(),
            "llm-proxy".into(),
            "ensure-running".into(),
            "--listen".into(),
            self.listen.clone(),
            "--upstream".into(),
            self.upstream.clone(),
        ]
        .into_iter()
        .chain(self.agent_args())
        .collect()
    }
}

/// Where the unit/plist/task definition belongs, under `home`.
/// ALL the units this product may have installed, whatever the port.
///
/// There is one per proxy since each agent got its own port; an uninstall
/// that knows only one leaves N-1 behind, and they restart at the next session
/// start.
///
/// The historical name — without a port — is part of it: it is the one older
/// installs wrote, and exactly the one that must be removed.
pub fn unit_paths_all(platform: Platform, home: &Path) -> Vec<PathBuf> {
    let one = unit_path(platform, home, "");
    let Some(dir) = one.parent() else {
        return vec![one];
    };
    let Some(stem) = one.file_name().map(|n| n.to_string_lossy().to_string()) else {
        return vec![one];
    };
    // `tokenade-llm-proxy.service` → prefix `tokenade-llm-proxy`, suffix
    // `.service`. Whatever sits in between is a port.
    let (prefix, ext) = match stem.rsplit_once('.') {
        Some((p, e)) => (p.to_string(), format!(".{e}")),
        None => (stem.clone(), String::new()),
    };
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            n.starts_with(&prefix) && n.ends_with(&ext)
        })
        .collect();
    out.sort();
    if out.is_empty() {
        out.push(one);
    }
    out
}

pub fn unit_path(platform: Platform, home: &Path, listen: &str) -> PathBuf {
    match platform {
        Platform::Linux => home
            .join(".config")
            .join("systemd")
            .join("user")
            .join(format!("{}.service", service_name(listen))),
        // Staged in the user's own directory when the target is the system
        // domain: see `LAUNCHD_SYSTEM_DIR`.
        Platform::MacOs if mac_scope() == MacScope::System => home
            .join(".tokenade")
            .join(format!("{}.plist", launchd_label(listen))),
        Platform::MacOs => home
            .join("Library")
            .join("LaunchAgents")
            .join(format!("{}.plist", launchd_label(listen))),
        // Windows Task Scheduler keeps its own store; the XML is a file we hand
        // to `schtasks /create /xml`, not a location the OS watches.
        Platform::Windows => home
            .join("AppData")
            .join("Local")
            .join("tokenade")
            .join(windows_task_file(listen)),
    }
}

/// The service definition text.
pub fn unit_text(platform: Platform, svc: &Service) -> String {
    match platform {
        Platform::Linux => systemd_unit(svc),
        Platform::MacOs => launchd_plist(svc),
        Platform::Windows => windows_task_xml(svc),
    }
}

/// The commands that make the definition live, in order.
///
/// Returned rather than run: enabling a background service that outlives the
/// shell is a visible act, and a product that does it silently is one the user
/// cannot audit.
pub fn enable_commands(platform: Platform, listen: &str) -> Vec<String> {
    let svc = service_name(listen);
    let label = launchd_label(listen);
    match platform {
        Platform::Linux => vec![
            "systemctl --user daemon-reload".into(),
            format!("systemctl --user enable --now {svc}.service"),
            // Without this the user manager stops at logout and the proxy is
            // gone until somebody logs in again — "enabled" would be true and
            // "running after a reboot" would be false.
            "loginctl enable-linger $USER".into(),
        ],
        // `launchctl bootstrap` is the one enable verb on the roster that is
        // NOT idempotent, and it says so in a way nobody can act on. The other
        // two replace whatever is there — `systemctl --user enable --now`
        // re-enables a unit that is already enabled, `schtasks /create /f`
        // overwrites the task — while bootstrapping a label launchd already
        // knows answers `Bootstrap failed: 5: Input/output error`, which names
        // neither the label nor the reason.
        //
        // Measured 2026-08-19 on the Apple Silicon Mac: a pass that had
        // already enabled the daemon left it registered, and the next pass's
        // enable failed with exactly that line. The bench read the failure
        // correctly and reported the machine as unable to supervise, on a
        // machine that was supervising the proxy at that very moment.
        //
        // So the list clears the registration first. `|| true` because the
        // usual case is that there is nothing to clear, and a bootout of an
        // unknown label is an error rather than a no-op. It is a shell
        // construct, so `sudo`'s argv is unchanged and a NOPASSWD rule scoped
        // to the bootout still matches it.
        Platform::MacOs if mac_scope() == MacScope::System => vec![
            format!("sudo cp ~/.tokenade/{label}.plist {LAUNCHD_SYSTEM_DIR}/"),
            format!("sudo chown root:wheel {LAUNCHD_SYSTEM_DIR}/{label}.plist"),
            format!("sudo launchctl bootout system/{label} 2>/dev/null || true"),
            format!("sudo launchctl bootstrap system {LAUNCHD_SYSTEM_DIR}/{label}.plist"),
        ],
        Platform::MacOs => vec![
            format!("launchctl bootout gui/$(id -u)/{label} 2>/dev/null || true"),
            format!("launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/{label}.plist"),
        ],
        Platform::Windows => vec![format!(
            "schtasks /create /tn \"{}\" /xml \"%LOCALAPPDATA%\\tokenade\\{}\" /f",
            windows_task(listen),
            windows_task_file(listen)
        )],
    }
}

/// The commands that take it back off.
///
/// As long as the install list, and that symmetry is the point: an autostart
/// entry that survives the uninstall is a process the user cannot find and did
/// not ask for.
pub fn disable_commands(platform: Platform, listen: &str) -> Vec<String> {
    let svc = service_name(listen);
    let label = launchd_label(listen);
    match platform {
        Platform::Linux => vec![
            format!("systemctl --user disable --now {svc}.service"),
            "systemctl --user daemon-reload".into(),
            // The one the enable list turns ON and this list did not turn
            // back off. `loginctl enable-linger` keeps the user's systemd
            // manager alive after logout, FOREVER, and it is not scoped to
            // this service: nothing about removing the proxy removes it, and
            // nothing in the product ever mentions it again.
            //
            // Measured 2026-08-18 on the machine this was written on: after
            // running the printed enable list and then the printed disable
            // list, `/var/lib/systemd/linger/<user>` was still there, created
            // that minute. That is exactly the leftover this module's own
            // comment says must not exist — "an autostart entry that survives
            // the uninstall is a process the user cannot find and did not ask
            // for".
            //
            // Printed rather than run, like the rest of the list, and last so
            // the service is already gone when it fires.
            "loginctl disable-linger $USER".into(),
        ],
        Platform::MacOs if mac_scope() == MacScope::System => vec![
            format!("sudo launchctl bootout system/{label}"),
            format!("sudo rm -f {LAUNCHD_SYSTEM_DIR}/{label}.plist"),
        ],
        Platform::MacOs => vec![format!("launchctl bootout gui/$(id -u)/{label}")],
        Platform::Windows => vec![format!(
            "schtasks /delete /tn \"{}\" /f",
            windows_task(listen)
        )],
    }
}

/// The command that answers "does the supervisor know about this unit?".
///
/// Exists because ONE of the three enable paths lies: `schtasks /create`
/// exits 0 on a failure the bench caught 2026-08-18, so a run that trusts the
/// exit code reports an autostart nobody registered. A separate query is the
/// only honest confirmation, and having one for all three keeps the check the
/// same shape everywhere.
pub fn verify_command(platform: Platform, listen: &str) -> String {
    let svc = service_name(listen);
    let label = launchd_label(listen);
    match platform {
        Platform::Linux => format!("systemctl --user is-enabled {svc}.service"),
        Platform::MacOs if mac_scope() == MacScope::System => {
            format!("sudo launchctl print system/{label}")
        }
        Platform::MacOs => format!("launchctl print gui/$(id -u)/{label}"),
        Platform::Windows => format!("schtasks /query /tn \"{}\"", windows_task(listen)),
    }
}

/// The command that answers "is the unit actually SERVING?".
///
/// `verify_command` asks whether the supervisor KNOWS the unit, and that is a
/// different question. Measured 2026-08-31 with 127.0.0.1:8787 held by another
/// process: `enable --now` exits 0, `is-enabled` answers `enabled`, and the
/// service is `failed` / `Result=start-limit-hit` five seconds later. The
/// enable path printed `confirmed by ...: the supervisor knows this unit` over
/// a proxy that was never going to answer a request. The same end-state was
/// already measured on 2026-08-19 from a different cause — a binary under
/// `/tmp` versus `PrivateTmp` — and that fix removed the cause it had found
/// and left the blind confirmation, so the next cause said the same nothing.
///
/// `None` where the answer is not verified rather than guessed: launchd's
/// `print` and `schtasks /query` both carry a state word, but which word and
/// in which locale is not something this machine can check, and a grep that is
/// wrong cries wolf on every install of that platform. Registration stays
/// confirmed there, exactly as before; only Linux gained the second question.
pub fn running_command(platform: Platform, listen: &str) -> Option<String> {
    let svc = service_name(listen);
    let _ = &svc;
    match platform {
        Platform::Linux => Some(format!("systemctl --user is-active {svc}.service")),
        Platform::MacOs | Platform::Windows => None,
    }
}

/// What the supervisor says about a unit that is registered and not serving,
/// in the supervisor's own words. `start-limit-hit` is the word that tells a
/// user their proxy will never come back on its own; a shrug is not.
pub fn failure_reason_command(platform: Platform, listen: &str) -> Option<String> {
    let svc = service_name(listen);
    let _ = &svc;
    match platform {
        Platform::Linux => Some(format!(
            "systemctl --user show {svc}.service -p SubState -p Result --value"
        )),
        Platform::MacOs | Platform::Windows => None,
    }
}

/// Run a supervisor query for its OUTPUT rather than for its exit status.
///
/// `run_supervisor_command` keeps stdout only on failure, and the two queries
/// above answer on stdout with a zero exit: `is-active` prints `failed`,
/// `show -p Result` prints `start-limit-hit`. Both are the answer, not an
/// error, so the output is what is wanted whichever way the exit code went.
pub fn supervisor_says(cmd: &str) -> Option<String> {
    let out = if cfg!(target_os = "windows") {
        windows_shell(cmd).output()
    } else {
        tokenade_core::process_util::internal_command("sh")
            .args(["-c", cmd])
            .output()
    };
    let o = out.ok()?;
    let said = String::from_utf8_lossy(&o.stdout);
    let said: Vec<&str> = said.split_whitespace().collect();
    if said.is_empty() {
        return None;
    }
    Some(said.join(" "))
}

/// Run one supervisor command through the platform's own shell.
///
/// A shell rather than a direct spawn because the lists carry `$USER` and
/// `$(id -u)` and `%LOCALAPPDATA%`, which are the supervisor's own idiom and
/// not ours to expand.
pub fn run_supervisor_command(cmd: &str) -> Result<(), String> {
    // **tokenade never elevates its privileges.** On a client workstation,
    // `sudo` waits for a password on the user's terminal in the middle of an
    // install or an update (maintainer rule, 2026-10-06). These commands — the
    // `system` domain of a Mac with no on-screen session — are handed to the
    // user, never run.
    if needs_admin(cmd) {
        return Err(format!(
            "{cmd}: needs administrator rights — run it yourself"
        ));
    }
    let out = if cfg!(target_os = "windows") {
        windows_shell(cmd).output()
    } else {
        tokenade_core::process_util::internal_command("sh")
            .args(["-c", cmd])
            .output()
    };
    match out {
        Err(e) => Err(format!("{cmd}: {e}")),
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => {
            let said = String::from_utf8_lossy(&o.stderr);
            let said = if said.trim().is_empty() {
                String::from_utf8_lossy(&o.stdout).trim().to_string()
            } else {
                said.trim().to_string()
            };
            Err(format!("{cmd}: {said}"))
        }
    }
}

/// A command this product must print, never run.
pub fn needs_admin(cmd: &str) -> bool {
    cmd.trim_start().starts_with("sudo ")
}

/// `cmd /c <line>` with the line handed over VERBATIM.
///
/// Rust escapes each argument, `cmd` then strips the outer pair, and a line
/// carrying its own quotes arrives mangled. Measured 2026-08-19 on a Windows
/// VM: `schtasks /create /tn "tokenade\llm-proxy" /xml "%LOCALAPPDATA%\...xml"
/// /f` came back with "The filename, directory name, or volume label syntax is
/// incorrect" (French-locale Windows) for a file the product had just written
/// one line earlier — so
/// `--enable` reported a failure that was entirely its own quoting. The bench
/// hit exactly this on the other side of the wire and fixed it the same way
/// (Node's `windowsVerbatimArguments` in its probe harness).
#[cfg(target_os = "windows")]
fn windows_shell(cmd: &str) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    let mut c = tokenade_core::process_util::internal_command("cmd");
    c.raw_arg("/c ");
    c.raw_arg(cmd);
    // Callers capture with `output()`; `cmd.exe` is a console subsystem
    // program and would paint a window when tokenade has no console.
    tokenade_core::process_util::configure_no_window(&mut c);
    c
}

/// Never called off Windows; present so the call site stays one expression.
#[cfg(not(target_os = "windows"))]
fn windows_shell(cmd: &str) -> std::process::Command {
    let mut c = tokenade_core::process_util::internal_command("cmd");
    c.args(["/c", cmd]);
    c
}

/// `PrivateTmp=true` gives the service its own empty `/tmp`, which is real
/// hardening and a self-inflicted wound when the BINARY lives there.
///
/// Measured 2026-08-19 on Linux: with the executable at
/// `/tmp/tokenade-compat-bin/tokenade`, systemd wrote the unit, enabled it,
/// answered `is-enabled: enabled` — and every start died `status=203/EXEC`
/// until `Result=start-limit-hit`. The unit is present, enabled and permanently
/// dead, and nothing in the enable path says so. A binary under `/tmp` is
/// unusual, but "enabled and can never run" is the worst answer a supervisor
/// can give, so the hardening yields to the executable and the unit says why.
fn private_tmp_would_hide(exec_path: &str) -> bool {
    let p = exec_path.trim_start_matches('"');
    p.starts_with("/tmp/") || p.starts_with("/var/tmp/")
}

/// `Environment=HOME=…` / launchd's `EnvironmentVariables`, for the SAME
/// reason `Service::exe` is absolute: a supervisor starts with a minimal
/// environment, and `HOME` is part of it. `systemd --user` sets `HOME` from
/// `/etc/passwd`, which is not necessarily the home the install targeted — a
/// bench, a test account, a container user. Measured 2026-08-20: the compat
/// bench enabled the unit for a temporary home and the service came up on the
/// OPERATOR's home, where the proxy's own start-up self-heal then repointed
/// all eleven Claude Code hooks at the bench's staged binary under
/// `~/.cache/`. The service ran fine; it simply ran somewhere else.
fn service_home() -> Option<String> {
    tokenade_core::platform::home_dir().map(|h| h.display().to_string())
}

fn systemd_unit(svc: &Service) -> String {
    let exec = svc.argv().join(" ");
    let home_env = match service_home() {
        Some(h) => format!("Environment=HOME={h}\n"),
        None => String::new(),
    };
    let private_tmp =
        if private_tmp_would_hide(svc.argv().first().map(|s| s.as_str()).unwrap_or("")) {
            "# PrivateTmp is OFF here, on purpose: this binary lives under /tmp, and a\n\
         # private /tmp would hide it from its own service (status=203/EXEC, then\n\
         # start-limit-hit). Move the binary out of /tmp to get the sandbox back.\n"
                .to_string()
        } else {
            "# The proxy holds an API key in memory and writes no request bodies;\n\
         # these keep a compromised process from reaching the rest of the home\n\
         # directory anyway.\n\
         PrivateTmp=true\n"
                .to_string()
        };
    format!(
        "[Unit]\n\
         Description=tokenade LLM proxy (ladder rank 4)\n\
         Documentation=https://github.com/pi-infected/tokenade\n\
         # The proxy is useless before the network is up and harmless after it\n\
         # goes down, so this is a want and not a requirement.\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         # `Restart=always` with systemd's default start limit is not always: a\n\
         # proxy that cannot bind dies five times in five seconds, systemd calls\n\
         # start-limit-hit, and the unit stays enabled and permanently dead even\n\
         # once the port frees. Measured 2026-08-31 with 127.0.0.1:8787 held by\n\
         # another process: NRestarts=5, Result=start-limit-hit, still failed\n\
         # twenty-five seconds after the holder exited. No limit means a\n\
         # transient conflict resolves itself, which is what `always` promised.\n\
         StartLimitIntervalSec=0\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={exec}\n\
         {home_env}\
         # An agent that gets one connection refused shows the user an API\n\
         # error, so coming back fast matters more than backing off politely.\n\
         Restart=always\n\
         RestartSec=1\n\
         {private_tmp}\
         NoNewPrivileges=true\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n"
    )
}

fn launchd_plist(svc: &Service) -> String {
    let label = launchd_label(&svc.listen);
    // A system-domain daemon runs as ROOT unless it says otherwise, and this
    // process holds an API key. `UserName` puts it back under the person who
    // installed it — the same account a LaunchAgent would have run as.
    let home_env = match service_home() {
        Some(h) => format!(
            "\x20 <key>EnvironmentVariables</key>\n\x20 <dict>\n\x20   <key>HOME</key>\n\x20   <string>{}</string>\n\x20 </dict>\n",
            xml_escape(&h)
        ),
        None => String::new(),
    };
    let user_key = match mac_scope() {
        MacScope::System => {
            let who = std::env::var("USER").unwrap_or_default();
            if who.is_empty() {
                String::new()
            } else {
                format!(
                    "\x20 <key>UserName</key>\n\x20 <string>{}</string>\n",
                    xml_escape(&who)
                )
            }
        }
        MacScope::Gui => String::new(),
    };
    let args = svc
        .argv()
        .iter()
        .map(|a| format!("    <string>{}</string>", xml_escape(a)))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \x20 <key>Label</key>\n\
         \x20 <string>{label}</string>\n\
         {user_key}\
         \x20 <key>ProgramArguments</key>\n\
         \x20 <array>\n{args}\n\
         \x20 </array>\n\
         {home_env}\
         \x20 <key>RunAtLoad</key>\n\
         \x20 <true/>\n\
         \x20 <key>KeepAlive</key>\n\
         \x20 <true/>\n\
         \x20 <key>ProcessType</key>\n\
         \x20 <string>Background</string>\n\
         </dict>\n\
         </plist>\n"
    )
}

/// The Windows definition, and the one place where the obvious setting is the
/// wrong one.
///
/// `RestartOnFailure` reads like systemd's `Restart=always` and is not the same
/// thing: it covers a task that fails to START. Measured on a real Windows 10
/// guest 2026-08-17 — the proxy was killed with `taskkill /F` and, with
/// `RestartOnFailure` declared at `PT1M` × 999, nothing came back in five
/// minutes and the task sat at "Ready". Every other rung of this ladder has a
/// supervisor that reacts to a DEAD process; the Windows one, as first written,
/// reacted to nothing.
///
/// What does work, measured on the same guest: a `TimeTrigger` repeating every
/// minute from a start boundary in the past, paired with the
/// `MultipleInstancesPolicy` of `IgnoreNew` that was already declared. While
/// the proxy runs, each firing is a no-op; once it dies, the next one starts it
/// — back in ~50 s after a kill, and health answered again.
///
/// It also removes the dependence on a logon EVENT, which is a second thing the
/// first version got wrong by omission. The repeat covers the case where the
/// machine is up and nobody has logged in yet; the logon trigger keeps the fast
/// path at sign-in.
///
/// And that was only half the story, found again on 2026-08-21: a repeating
/// `TimeTrigger` does not rescue an `InteractiveToken` principal either. The
/// LOGON TYPE gates every trigger, not just the logon one — on a machine with
/// no console session the last result stayed `267011` (SCHED_S_TASK_HAS_NOT_RUN)
/// with `Next Run Time` a minute away, forever. `S4U` is what makes the
/// repeat mean something; see the comment on the XML below.
/// The account the task runs as, as launchd's `UserName` is on the other OS.
///
/// Measured 2026-08-19 on a Windows VM, and the two tasks differed by this
/// one element: a twin built with `schtasks /create /tr "<same exe> --version"`
/// ran and returned `0`, while the XML task returned `2`
/// (ERROR_FILE_NOT_FOUND) for an executable that exists, is readable and
/// executable (`icacls`: `BUILTIN\Users:(I)(RX)`), on a machine with a
/// live interactive console session. `InteractiveToken` with no `UserId` is a
/// principal the scheduler cannot resolve, and the error it reports for that
/// is about the FILE — which is why this cost three runs to find.
fn windows_principal_user() -> String {
    let who = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_default();
    if who.is_empty() {
        String::new()
    } else {
        format!("\x20     <UserId>{}</UserId>\n", xml_escape(&who))
    }
}

/// Escape the five XML metacharacters.
///
/// Needed on two of the three platforms and easy to skip: a Windows home under
/// `C:\Users\O'Brien` or a listen address that somebody typed with an `&` would
/// otherwise produce a document the OS refuses, at boot, with no message the
/// user will ever see.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Write the definition next to where its supervisor expects it.
/// Like [`write_unit`], but says whether the content CHANGED.
///
/// **"enable --now" does not restart an already active unit.** A unit whose
/// `ExecStart` was broken therefore keeps its old process — or none — and the
/// supervisor answers "active" on the strength of what it believes is running.
/// Observed plainly on a real machine after 1.1.14: `ExecStart=… tokenade
/// (deleted) llm-proxy --listen 127.0.0.1:8788`, a process from the previous
/// version still alive on 8787, and every update rewrote the unit without ever
/// restarting it.
///
/// Rewriting the file repairs nothing as long as nobody reads it again: it is
/// the boolean returned here that triggers the `restart`.
pub fn write_unit_reporting(
    platform: Platform,
    home: &Path,
    svc: &Service,
) -> std::io::Result<(PathBuf, bool)> {
    let path = unit_path(platform, home, &svc.listen);
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let text = unit_text(platform, svc);
    let changed = std::fs::read_to_string(&path)
        .map(|old| old != text)
        .unwrap_or(true);
    tokenade_core::paths::write_atomic(&path, &text)?;
    Ok((path, changed))
}

/// What to run when the unit has just CHANGED: `enable --now` leaves an
/// already active process in place, whatever command line it carries.
/// Restarting is the only way to make a corrected `ExecStart` take effect —
/// and it is what repairs, on update, a machine whose unit named a deleted
/// binary.
pub fn restart_commands(platform: Platform, listen: &str) -> Vec<String> {
    let svc = service_name(listen);
    let _ = &svc;
    match platform {
        Platform::Linux => vec![format!(
            "systemctl --user restart {svc}.service"
        )],
        Platform::MacOs if mac_scope() == MacScope::System => vec![
            format!("sudo launchctl kickstart -k system/{}", launchd_label(listen)),
        ],
        Platform::MacOs => vec![
            format!("launchctl kickstart -k gui/$(id -u)/{}", launchd_label(listen)),
        ],
        // The task launches `ensure-running`, which DETACHES the proxy: `/end`
        // only stops the launcher. Measured 2026-10-06 on a Windows VM: after
        // `/end` + `/run`, the 11:01 proxy was still serving, and the
        // relaunched launcher found it alive. So we stop the tokenade listening
        // on THIS port — and only it.
        Platform::Windows => vec![
            format!("schtasks /end /tn \"{}\"", windows_task(listen)),
            format!(
                "powershell -NoProfile -Command \"Get-NetTCPConnection -LocalPort {} -State Listen -ErrorAction SilentlyContinue | ForEach-Object {{ Get-Process -Id $_.OwningProcess -ErrorAction SilentlyContinue }} | Where-Object Name -eq 'tokenade' | Stop-Process -Force\"",
                listen.rsplit_once(':').map_or(listen, |(_, p)| p)
            ),
            format!("schtasks /run /tn \"{}\"", windows_task(listen)),
        ],
    }
}

mod windows_task;
use windows_task::windows_task_xml;

#[cfg(test)]
mod tests {
    use super::*;

    /// **A rewritten unit must be RESTARTED, otherwise the repair never
    /// arrives.**
    ///
    /// Real case after 1.1.14: `ExecStart=… tokenade (deleted) llm-proxy
    /// --listen 127.0.0.1:8788`, a process from the previous version still
    /// alive on 8787, and `enable --now` leaving what already runs in place.
    /// The update rewrote the file and had nobody read it again.
    #[test]
    fn rewriting_the_unit_is_reported_so_the_caller_can_restart() {
        // The unit text carries `Environment=HOME=…` read from the process
        // environment, and `service_home_tests` changes HOME under ENV_LOCK:
        // two renderings compared here must not straddle that (red in full
        // runs 2026-10-02 and 2026-10-03, green alone).
        let _env = tokenade_core::platform::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let home = std::env::temp_dir().join(format!("tkn-unit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).expect("tmp");

        let first = svc();
        let (path, changed) =
            write_unit_reporting(Platform::Linux, &home, &first).expect("written");
        assert!(changed, "a unit that did not exist is a change");

        let (_, again) = write_unit_reporting(Platform::Linux, &home, &first).expect("written");
        assert!(!again, "the same content is not a change");

        // The repairing case: the previous `ExecStart` named a deleted binary.
        let broken = format!(
            "ExecStart=/home/u/.local/bin/tokenade (deleted) llm-proxy --listen {} --upstream {}\n",
            first.listen, first.upstream
        );
        std::fs::write(&path, broken).expect("written");
        let (_, repaired) = write_unit_reporting(Platform::Linux, &home, &first).expect("written");
        assert!(
            repaired,
            "a broken ExecStart that gets replaced is a change"
        );
        let text = std::fs::read_to_string(&path).expect("relu");
        assert!(
            !text.contains("(deleted)"),
            "the unit still carries a deleted binary: {text}"
        );

        // And the restart exists for all three supervisors, on THE service of
        // that port.
        //
        // The update repairs by restarting: a daemon launched before the
        // binary was replaced still answers `ok` while serving the old code,
        // on all three systems (Linux unlinks the inode, Windows refuses to
        // replace a live `.exe` and the updater renames the old one, macOS
        // keeps the image mapped). If the restart command named the service
        // WITHOUT its port — the failure fixed in 1.1.16 — the repair would
        // target the neighbour, or nothing.
        for p in [Platform::Linux, Platform::MacOs, Platform::Windows] {
            let cmds = restart_commands(p, "127.0.0.1:8789");
            assert!(!cmds.is_empty(), "no restart command for {p:?}");
            let svc = match p {
                Platform::Linux => service_name("127.0.0.1:8789"),
                Platform::MacOs => launchd_label("127.0.0.1:8789"),
                Platform::Windows => windows_task("127.0.0.1:8789"),
            };
            assert!(svc.ends_with("-8789"), "{svc}");
            for c in &cmds {
                // Or the process listening on THIS port (Windows: the task
                // detaches the proxy, `/end` does not stop it).
                assert!(
                    c.contains(&svc) || c.contains("-LocalPort 8789 "),
                    "{p:?} does not target {svc}: {c}"
                );
            }
        }
        let _ = std::fs::remove_dir_all(&home);
    }

    /// No `sudo` command is run: it is handed back as is.
    #[test]
    fn a_sudo_command_is_never_run() {
        let e = run_supervisor_command("sudo launchctl bootout system/x").unwrap_err();
        assert!(e.contains("administrator"), "{e}");
        assert!(run_supervisor_command("true").is_ok() || cfg!(windows));
    }

    /// Two agents, two ports: two distinct units on all THREE systems, and
    /// what the list enables is the file we write.
    #[test]
    fn two_ports_get_two_units_on_every_platform() {
        let h = Path::new("/h");
        for p in [Platform::Linux, Platform::MacOs, Platform::Windows] {
            let (a, b) = (
                unit_path(p, h, "127.0.0.1:8787"),
                unit_path(p, h, "127.0.0.1:8788"),
            );
            assert_ne!(a, b, "{p:?}");
            for (listen, path) in [("127.0.0.1:8787", &a), ("127.0.0.1:8788", &b)] {
                let file = path.file_name().unwrap().to_string_lossy().to_string();
                let other = if listen.ends_with("8787") {
                    "8788"
                } else {
                    "8787"
                };
                let on = enable_commands(p, listen).join("\n");
                assert!(
                    on.contains(&file) || p == Platform::Linux,
                    "{p:?} enables something other than {file}: {on}"
                );
                assert!(!on.contains(other), "{p:?} touches the other port: {on}");
                assert!(
                    !disable_commands(p, listen).join("\n").contains(other),
                    "{p:?}"
                );
            }
        }
        // Uninstall finds them all: `com.tokenade.llm-proxy.plist` is cut at
        // the LAST dot, otherwise the prefix is `com`.
        let home = std::env::temp_dir().join(format!("tk-units-{}", std::process::id()));
        for p in [Platform::MacOs, Platform::Windows] {
            let dir = unit_path(p, &home, "").parent().unwrap().to_path_buf();
            std::fs::create_dir_all(&dir).unwrap();
            for l in ["", "127.0.0.1:8788"] {
                std::fs::write(unit_path(p, &home, l), "").unwrap();
            }
            std::fs::write(dir.join("com.apple.other.plist"), "").unwrap();
            assert_eq!(unit_paths_all(p, &home).len(), 2, "{p:?}");
        }
        let _ = std::fs::remove_dir_all(&home);
        assert!(launchd_plist(&Service {
            listen: "127.0.0.1:8788".into(),
            ..svc()
        })
        .contains("<string>com.tokenade.llm-proxy-8788</string>"));
    }

    fn svc() -> Service {
        Service {
            exe: PathBuf::from("/home/u/.local/bin/tokenade"),
            listen: "127.0.0.1:8787".into(),
            upstream: "https://api.openai.com/v1".into(),
            agent: None,
        }
    }

    /// A private `/tmp` and a binary IN `/tmp` cannot both be honoured, and the
    /// silent version of that conflict is a unit that is enabled and can never
    /// run. Measured 2026-08-19 with the executable at
    /// `/tmp/tokenade-compat-bin/tokenade`: `is-enabled` said `enabled`, every
    /// start died `status=203/EXEC`, and systemd stopped trying at
    /// `start-limit-hit`. Both directions are asserted, because dropping the
    /// sandbox everywhere would be the other way to make this test pass.
    #[test]
    fn a_binary_under_tmp_does_not_get_a_private_tmp_that_hides_it() {
        let normal = unit_text(Platform::Linux, &svc());
        assert!(
            normal.contains("PrivateTmp=true"),
            "the ordinary install lost its sandbox:\n{normal}"
        );
        let in_tmp = Service {
            exe: PathBuf::from("/tmp/tokenade-compat-bin/tokenade"),
            ..svc()
        };
        let t = unit_text(Platform::Linux, &in_tmp);
        assert!(
            !t.contains("PrivateTmp=true"),
            "a binary under /tmp got a private /tmp, which hides it from its own \
             service (status=203/EXEC):\n{t}"
        );
        assert!(
            t.contains("PrivateTmp is OFF here"),
            "the unit dropped the sandbox without saying why:\n{t}"
        );
        assert!(
            t.contains("NoNewPrivileges=true"),
            "the OTHER hardening line went with it:\n{t}"
        );
    }

    #[test]
    fn every_platform_carries_the_binary_by_absolute_path() {
        // A supervisor starts with a minimal environment and often no useful
        // PATH. A bare `tokenade` is the classic unit that works when you test
        // it by hand and fails at boot.
        for p in [Platform::Linux, Platform::MacOs, Platform::Windows] {
            let t = unit_text(p, &svc());
            assert!(
                t.contains("/home/u/.local/bin/tokenade"),
                "{p:?} lost the absolute path"
            );
            assert!(
                t.contains("127.0.0.1:8787"),
                "{p:?} lost the listen address"
            );
            assert!(
                t.contains("https://api.openai.com/v1"),
                "{p:?} lost the upstream — the proxy would have nowhere to forward"
            );
        }
    }

    #[test]
    fn each_platform_restarts_the_proxy_rather_than_leaving_it_dead() {
        // The property that makes rank 4 safe to install at all: the agent's
        // config now points here, so a proxy that dies and stays dead fails
        // every turn until somebody notices.
        assert!(unit_text(Platform::Linux, &svc()).contains("Restart=always"));
        assert!(unit_text(Platform::MacOs, &svc()).contains("<key>KeepAlive</key>"));
        // This line asserted `<RestartOnFailure>` and was GREEN while the
        // Windows unit restarted nothing. The setting covers a task that fails
        // to start, not a process that started and died — measured on a real
        // guest, where a killed proxy never came back. A test that pins a
        // SPELLING passes on a directive that does not do what its name says;
        // what makes this one worth keeping is that the string it now pins was
        // watched working.
        let w = unit_text(Platform::Windows, &svc());
        assert!(w.contains("<Repetition>") && w.contains("<Interval>PT1M</Interval>"));
        // The ACTION must be short, not the proxy itself. `IgnoreNew` only
        // holds if the instance terminates: with the proxy as the action, the
        // scheduler skips firings while it still believes the instance alive —
        // 220 s measured on the VM before this change.
        assert!(
            w.contains("ensure-running"),
            "the Windows action must be the short starter, not the proxy: {w}"
        );
        // The LOGON TYPE gates ALL triggers, not only the logon one. Measured
        // 2026-08-21 on the VM: with `InteractiveToken` and nobody at the
        // console, `schtasks /query /v` returns "Last Result: 267011" (never
        // run) while "Next Run Time" is within the minute. The same XML with
        // only that field changed returns "Last Result: 0" and the proxy runs.
        assert!(
            w.contains("<LogonType>S4U</LogonType>"),
            "the Windows task must not need an interactive session: {w}"
        );
        assert!(
            w.contains("<MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>"),
            "without IgnoreNew the minute repeat starts a SECOND proxy on a port \
             the first one holds"
        );
    }

    #[test]
    fn each_platform_starts_without_a_human_present() {
        assert!(unit_text(Platform::Linux, &svc()).contains("WantedBy=default.target"));
        assert!(unit_text(Platform::MacOs, &svc()).contains("<key>RunAtLoad</key>"));
        assert!(unit_text(Platform::Windows, &svc()).contains("<LogonTrigger>"));
    }

    /// `Restart=always` must not be capped into `Restart=five-times`.
    ///
    /// systemd's default start limit is 5 starts per 10 s, and a proxy that
    /// cannot bind burns them in five seconds. Measured 2026-08-31 with
    /// 127.0.0.1:8787 held by another process: NRestarts=5,
    /// Result=start-limit-hit, and the unit was STILL failed twenty-five
    /// seconds after the holder exited — enabled and permanently dead, over a
    /// conflict that had gone away by itself. With no limit the same run
    /// reached NRestarts=7, kept retrying, and came back `active` four seconds
    /// after the port freed, with nothing asked of the user.
    #[test]
    fn restart_always_is_not_capped_by_a_start_limit() {
        let u = unit_text(Platform::Linux, &svc());
        assert!(u.contains("Restart=always"), "{u}");
        assert!(
            u.contains("StartLimitIntervalSec=0"),
            "Restart=always without StartLimitIntervalSec=0 gives up after five \
             tries and never comes back: {u}"
        );
    }

    /// "The supervisor knows this unit" and "the unit is serving" are two
    /// questions, and the enable path used to ask only the first.
    ///
    /// Measured 2026-08-31: `enable --now` exits 0 and `is-enabled` answers
    /// `enabled` over a service that is `failed`. The same end-state had
    /// already been measured on 2026-08-19 from an unrelated cause (a binary
    /// under `/tmp` versus `PrivateTmp`), and that fix removed the cause and
    /// left the blind confirmation — so the next cause said the same nothing.
    /// This asserts the two commands stay two commands.
    #[test]
    fn registration_and_serving_are_two_different_questions() {
        let known = verify_command(Platform::Linux, "127.0.0.1:8787");
        let serving =
            running_command(Platform::Linux, "127.0.0.1:8787").expect("Linux answers this one");
        assert!(known.contains("is-enabled"), "{known}");
        assert!(serving.contains("is-active"), "{serving}");
        assert_ne!(known, serving, "one command cannot answer both questions");
        for c in [&known, &serving] {
            assert!(c.contains(SERVICE_NAME), "{c} does not name {SERVICE_NAME}");
        }
    }

    /// The second question is asked where its answer is VERIFIED, and nowhere
    /// else.
    ///
    /// launchd's `print` and `schtasks /query` both carry a state word, and
    /// which word — in which locale — is not something this machine can check.
    /// A grep that is wrong would report "registered and NOT serving" over
    /// every healthy install on that platform, which is worse than the silence
    /// it replaces. `None` says "not asked" rather than guessing, and the two
    /// commands travel as a pair: a reason with no question to explain is
    /// dead code, and a question with no reason is a shrug.
    #[test]
    fn the_serving_question_is_asked_only_where_its_answer_is_verified() {
        for p in [Platform::Linux, Platform::MacOs, Platform::Windows] {
            assert_eq!(
                running_command(p, "127.0.0.1:8787").is_some(),
                failure_reason_command(p, "127.0.0.1:8787").is_some(),
                "{p:?} asks one half of the pair"
            );
        }
        assert!(running_command(Platform::Linux, "127.0.0.1:8787").is_some());
        assert!(running_command(Platform::MacOs, "127.0.0.1:8787").is_none());
        assert!(running_command(Platform::Windows, "127.0.0.1:8787").is_none());
    }

    #[test]
    fn linux_is_told_to_enable_lingering() {
        // The line that turns "enabled" into "actually running after a
        // reboot". A systemd USER manager stops at last logout, so without
        // lingering the unit is enabled and absent — the worst combination,
        // because `systemctl --user is-enabled` says yes.
        let cmds = enable_commands(Platform::Linux, "127.0.0.1:8787");
        assert!(cmds.iter().any(|c| c.contains("enable-linger")), "{cmds:?}");
        assert!(cmds.iter().any(|c| c.contains("daemon-reload")));
    }

    /// Everything the enable list TURNS ON, the disable list must turn off.
    ///
    /// Written after leaving `/var/lib/systemd/linger/<user>` behind on a real
    /// machine: `loginctl enable-linger` was in the first list and in no other,
    /// so the product asked the user to enable a permanent setting, out of reach
    /// of its own uninstall. The test compares VERBS rather than strings, so
    /// that a future `enable-x` without `disable-x` fails here.
    ///
    /// This comment spent a while attached to the NEXT test, which is about
    /// something else: the doc described the enable/disable symmetry and the
    /// test below it compared unit names. A comment placed on the wrong test
    /// never fails.
    #[test]
    fn every_platform_can_be_taken_back_off() {
        // Symmetry, asserted. An autostart entry that survives the uninstall is
        // a process the user cannot find and did not ask for.
        for p in [Platform::Linux, Platform::MacOs, Platform::Windows] {
            let off = disable_commands(p, "127.0.0.1:8787");
            assert!(!off.is_empty(), "{p:?} has no way back");
        }
        assert!(disable_commands(Platform::Linux, "127.0.0.1:8787")[0].contains("disable --now"));
        assert!(disable_commands(Platform::MacOs, "127.0.0.1:8787")[0].contains("bootout"));
        assert!(disable_commands(Platform::Windows, "127.0.0.1:8787")[0].contains("/delete"));
    }

    #[test]
    fn the_verify_command_names_the_unit_the_enable_list_creates() {
        // `--enable` believes the supervisor's own answer rather than an exit
        // code, and the answer is only worth anything if it is about the SAME
        // unit. A verify command pointed at a different name would confirm
        // every install, including the ones that did nothing — which is the
        // failure `schtasks /create` exits 0 on.
        for p in [Platform::Linux, Platform::MacOs, Platform::Windows] {
            let name = match p {
                Platform::Linux => SERVICE_NAME,
                Platform::MacOs => LAUNCHD_LABEL,
                Platform::Windows => WINDOWS_TASK,
            };
            let check = verify_command(p, "127.0.0.1:8787");
            assert!(check.contains(name), "{check} does not name {name}");
            assert!(
                enable_commands(p, "127.0.0.1:8787")
                    .iter()
                    .any(|c| c.contains(name)),
                "the enable list for {p:?} does not name {name}"
            );
        }
    }

    /// Running the printed enable list TWICE must not fail.
    ///
    /// Every platform's list is something an operator pastes, and the second
    /// paste is the common case: re-running the installer, moving the binary,
    /// re-enabling after an upgrade. Two of the three verbs replace what is
    /// there on their own; `launchctl bootstrap` does not, so its list has to
    /// clear the registration itself. The test names the verb rather than the
    /// platform so a fourth platform cannot be added without answering this.
    #[test]
    fn every_enable_list_can_be_run_twice() {
        for p in [Platform::Linux, Platform::MacOs, Platform::Windows] {
            let on = enable_commands(p, "127.0.0.1:8787").join(" ; ");
            let replaces = on.contains("enable --now")      // systemctl: re-enable is a no-op
                || on.contains("/create")                   // schtasks: paired with /f below
                || on.contains("bootout"); // launchctl: cleared before bootstrap
            assert!(
                replaces,
                "{p:?}: nothing in `{on}` replaces or clears an existing registration, \
                 so running the list a second time fails"
            );
            if on.contains("schtasks") {
                assert!(
                    on.contains("/f"),
                    "{p:?}: `schtasks /create` without `/f` refuses an existing task"
                );
            }
        }
    }

    #[test]
    fn every_enable_verb_has_a_disable_counterpart() {
        for p in [Platform::Linux, Platform::MacOs, Platform::Windows] {
            let on = enable_commands(p, "127.0.0.1:8787").join(" ");
            let off = disable_commands(p, "127.0.0.1:8787").join(" ");
            for (turns_on, turns_off) in [
                ("enable-linger", "disable-linger"),
                ("enable --now", "disable --now"),
                ("bootstrap", "bootout"),
                ("/create", "/delete"),
            ] {
                if on.contains(turns_on) {
                    assert!(
                        off.contains(turns_off),
                        "{p:?}: enable runs `{turns_on}` and disable never runs `{turns_off}` — \
                         the uninstall leaves it on"
                    );
                }
            }
        }
    }

    #[test]
    fn xml_metacharacters_in_a_path_do_not_produce_an_invalid_document() {
        // `C:\Users\O'Brien` is a real home directory, and an unescaped
        // apostrophe here fails at BOOT, silently, on a machine we will never
        // see. Both XML platforms are checked.
        let s = Service {
            exe: PathBuf::from("C:\\Users\\O'Brien & Sons\\tokenade.exe"),
            listen: "127.0.0.1:8787".into(),
            upstream: "https://api.example.com/v1?a=1&b=2".into(),
            agent: None,
        };
        for p in [Platform::MacOs, Platform::Windows] {
            let t = unit_text(p, &s);
            assert!(t.contains("&apos;"), "{p:?} left a raw apostrophe");
            assert!(t.contains("&amp;"), "{p:?} left a raw ampersand");
            // The give-away of a botched escape: an `&` that is not the start
            // of an entity.
            for (i, _) in t.match_indices('&') {
                let tail = &t[i..];
                assert!(
                    ["&amp;", "&lt;", "&gt;", "&quot;", "&apos;"]
                        .iter()
                        .any(|e| tail.starts_with(e)),
                    "{p:?} has a bare & at {i}"
                );
            }
        }
    }

    #[test]
    fn an_argument_containing_a_space_survives_windows_quoting() {
        // Windows splits `Arguments` on spaces. The default install path lives
        // under `C:\Users\<name>\AppData`, so this is not hypothetical.
        let s = Service {
            upstream: "https://example.com/a b/v1".into(),
            ..svc()
        };
        let t = unit_text(Platform::Windows, &s);
        assert!(t.contains("&quot;https://example.com/a b/v1&quot;"), "{t}");
    }

    #[test]
    fn the_unit_lands_where_its_supervisor_looks() {
        let h = Path::new("/home/u");
        // **The PORT is in the name.** Each agent has its own proxy — their
        // upstreams differ — so one supervisor per port; the single unit of
        // before was rewritten by each agent in turn, and the last one won.
        assert!(unit_path(Platform::Linux, h, "127.0.0.1:8787")
            .ends_with(".config/systemd/user/tokenade-llm-proxy-8787.service"));
        assert!(unit_path(Platform::Linux, h, "127.0.0.1:8789")
            .ends_with(".config/systemd/user/tokenade-llm-proxy-8789.service"));
        assert!(unit_path(Platform::MacOs, h, "127.0.0.1:8787")
            .ends_with("Library/LaunchAgents/com.tokenade.llm-proxy-8787.plist"));
        assert!(
            unit_path(Platform::Windows, h, "127.0.0.1:8787").ends_with("llm-proxy-task-8787.xml")
        );
        // And the HISTORICAL name stays reachable: it is the one older
        // installs wrote, and the one the migration removes.
        assert!(unit_path(Platform::Linux, h, "")
            .ends_with(".config/systemd/user/tokenade-llm-proxy.service"));
    }

    #[test]
    fn writing_the_unit_creates_its_directory_and_the_text_matches() {
        // The unit text carries `Environment=HOME=…` read from the process
        // environment, and `service_home_tests` changes HOME under ENV_LOCK:
        // two renderings compared here must not straddle that (red in full
        // runs 2026-10-02 and 2026-10-03, green alone).
        let _env = tokenade_core::platform::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!("tokenade-autostart-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (path, _) = write_unit_reporting(Platform::Linux, &dir, &svc()).expect("wrote unit");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            unit_text(Platform::Linux, &svc())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod service_home_tests {
    use super::*;

    /// A supervisor starts with a minimal environment, and `HOME` is part of
    /// it. `systemd --user` reads it from `/etc/passwd` and launchd from the
    /// session, neither of which is necessarily the home the install targeted.
    /// The unit says it, so the service runs where the install ran.
    #[test]
    fn every_unit_names_the_home_it_was_installed_for() {
        let dir = std::env::temp_dir().join(format!("tokenade-svc-home-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let _g = tokenade_core::platform::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let previous = std::env::var_os("HOME");
        std::env::set_var("HOME", &dir);
        let svc = Service {
            exe: std::path::PathBuf::from("/usr/local/bin/tokenade"),
            listen: "127.0.0.1:8787".into(),
            upstream: "https://api.anthropic.com".into(),
            agent: None,
        };
        let unit = systemd_unit(&svc);
        let plist = launchd_plist(&svc);
        match previous {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        let home = dir.display().to_string();
        assert!(
            unit.contains(&format!("Environment=HOME={home}")),
            "the systemd unit does not pin HOME:\n{unit}"
        );
        assert!(
            plist.contains("<key>HOME</key>") && plist.contains(&home),
            "the launchd plist does not pin HOME:\n{plist}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
