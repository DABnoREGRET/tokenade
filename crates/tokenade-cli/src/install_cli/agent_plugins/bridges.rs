//! **One agent, one plugin directory, one file to write there.**
//!
//! Hermes, pi, OpenCode, Kilo, Cline, OpenClaw, aider. What differs is not
//! the content — it is the same bridge — but the PLACE, and every agent has
//! its own idea of the place.
//!
//! Refreshing bridges already installed lives here with them: a bridge from
//! an earlier version left in place calls a binary whose arguments have
//! changed, and the user reads the failure as their agent breaking down.

use super::*;

/// Resolve Hermes' home dir for install (mirrors agent_hermes' reader):
/// `$HERMES_HOME` › `%LOCALAPPDATA%\hermes` (Windows) › `~/.hermes`.
pub(crate) fn hermes_home_dir(home: &Path) -> PathBuf {
    if let Some(h) = std::env::var_os("HERMES_HOME") {
        return PathBuf::from(h);
    }
    if cfg!(windows) {
        return std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData").join("Local"))
            .join("hermes");
    }
    home.join(".hermes")
}

/// Install the Hermes read_file/MCP compaction plugin: drop the stdlib bridge
/// into `~/.hermes/plugins/tokenade/` and add `tokenade` to `plugins.enabled`
/// in `config.yaml` (Hermes won't load a user plugin that isn't on that
/// allow-list). Idempotent. Returns Ok(true) when it wrote/changed anything.
pub(crate) fn install_hermes_plugin(home: &Path, dry_run: bool) -> Result<bool, String> {
    let hh = hermes_home_dir(home);
    let plugin_dir = hh.join("plugins").join("tokenade");
    let cfg = hh.join("config.yaml");
    if dry_run {
        return Ok(true);
    }
    fs::create_dir_all(&plugin_dir).map_err(|e| format!("mkdir {}: {e}", plugin_dir.display()))?;
    crate::write_text_atomic(
        &plugin_dir.join("__init__.py"),
        inject_tokenade_bin(HERMES_PLUGIN_INIT).as_ref(),
    )
    .map_err(|e| format!("write plugin: {e}"))?;
    crate::write_text_atomic(&plugin_dir.join("plugin.yaml"), HERMES_PLUGIN_YAML)
        .map_err(|e| format!("write manifest: {e}"))?;
    // Hermes' config.yaml carries the user's models, API keys and settings.
    // A file that EXISTS but can't be read as UTF-8 must NOT collapse to
    // "" — that made enable_…() emit a plugins-only document and the write
    // below ERASED the whole config (the exact class the scaffold/lsp
    // audits closed). Bail loudly instead; the plugin dir is already in
    // place so the user can enable it by hand.
    let existing = match fs::read_to_string(&cfg) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(format!(
                "{} exists but is unreadable ({e}) — left untouched to avoid \
                 clobbering your Hermes config; add `tokenade` to plugins.enabled \
                 manually",
                cfg.display()
            ));
        }
    };
    let (updated, changed) =
        tokenade_core::mcp_wrap::enable_plugin_in_hermes_config(&existing, "tokenade");
    if changed {
        if let Some(p) = cfg.parent() {
            let _ = fs::create_dir_all(p);
        }
        // Timestamped backup before touching an existing config (no restore
        // path reads it, but it's the user's safety net for a bad merge).
        if !existing.is_empty() {
            let _ = fs::write(cfg.with_extension("yaml.tokenade-backup"), &existing);
        }
        crate::write_text_atomic(&cfg, updated.as_ref())
            .map_err(|e| format!("write {}: {e}", cfg.display()))?;
    }
    Ok(true)
}

/// Generic cross-agent plugin/extension installer shared by the Pi / opencode /
/// Kilo / OpenClaw drop-a-file-into-a-dir flows. Creates `dir`, writes each
/// asset (applying `inject_tokenade_bin` when the asset's flag is set), then runs
/// the optional `post_install` step (used by OpenClaw to register the package).
/// Honours `dry_run` with an early `Ok(true)` before touching disk. Returns
/// `Ok(true)` once the files are written, mirroring the wrappers' contract.
///
/// `assets` is `(filename, content, apply_inject_shim_bin)`; the per-write error
/// strings come from `err_label` (e.g. `"extension"`, `"plugin"`, `"index"`)
/// keyed by filename so each wrapper keeps its exact original message. The
/// `post_install` return is intentionally ignored here (the OpenClaw register
/// step is best-effort and reports itself), matching the inline originals.
pub fn install_plugin_files(
    dir: PathBuf,
    dry_run: bool,
    assets: &[(&str, &str, bool)],
    agent_label: &str,
    err_label: impl Fn(&str) -> &'static str,
    post_install: impl FnOnce(&Path) -> bool,
) -> Result<bool, String> {
    if dry_run {
        return Ok(true);
    }
    fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    for (name, content, inject) in assets {
        let body = if *inject {
            // Both substitutions travel together: an asset that needs its
            // binary path injected is one tokenade generated, and the agent
            // label is the other thing only the installer knows.
            inject_tokenade_bin(content).replace("@@TOKENADE_AGENT@@", agent_label)
        } else {
            (*content).to_string()
        };
        // Atomic, like the hermes installer two functions up. `fs::write`
        // truncates first, so a crash, a full disk or a killed upgrade between
        // truncate and write leaves a HALF-WRITTEN bridge on disk — and a
        // half-written `tokenade.ts` is a parse error the agent reports as a
        // broken plugin, not as "tokenade needs reinstalling". Same class as
        // the torn state files (`paths::write_atomic`, 7 sites).
        crate::write_text_atomic(&dir.join(name), &body)
            .map_err(|e| format!("write {}: {e}", err_label(name)))?;
    }
    // The registration result is the ONLY thing that says whether the bridge is
    // live, and it used to be thrown away here (`let _ =`) — then thrown away
    // again by the caller's `Ok(_)`. So `install` printed a green
    // "✓ openclaw — … registered via `openclaw plugins install`" two lines
    // under its own note saying registration had NOT run. A summary that
    // contradicts the diagnostic printed above it is worse than either alone.
    //
    // Bridges with no registration step pass `|_| true`, so `false` here can
    // only mean a real failure.
    Ok(post_install(&dir))
}

/// Install the Pi read_file/tool-result compaction extension: drop the bridge
/// into `~/.pi/agent/extensions/tokenade.ts`. Pi auto-discovers extensions
/// there — no manifest, no enable-list, no config edit. Idempotent (overwrites
/// with the current bridge). Returns Ok(true) when written.
pub(crate) fn install_pi_extension(home: &Path, dry_run: bool) -> Result<bool, String> {
    let ext_dir = home.join(".pi").join("agent").join("extensions");
    install_plugin_files(
        ext_dir,
        dry_run,
        &[("tokenade.ts", PI_EXTENSION_TS, true)],
        "pi",
        |_| "extension",
        |_| true,
    )
}

/// aider has no hook, no bridge, no MCP — and yet it has a LAUNCHER.
///
/// `--load FILE` "load and execute /commands from a file on launch", and
/// `/run <cmd>` executes a shell command. Verified at runtime on 2026-08-22 in
/// a throwaway HOME, without an API key: aider prints `Executing: /run …`
/// then the command's output, before any model call. It was the only missing
/// link for the "binary access" cell on this agent, whose rules file already
/// NAMED the commands without anything guaranteeing they would be run.
///
/// What we launch is `tokenade map`, and the choice is not neutral: it is
/// exactly what the installed rules file asks the model to do first.
/// Launching something USELESS to turn a cell green would be buying the colour
/// at the price of a side effect, which this project refuses elsewhere
/// (`--lint-cmd` would replace the user's linter: damage, not a channel).
///
/// **We never overwrite an existing `load:`.** If the user already has one,
/// it points at THEIR commands, and replacing it would make their startup
/// disappear. In that case we touch nothing and the agent stays on its rules
/// file.
pub(crate) fn install_aider_startup(home: &Path, dry_run: bool) -> Result<bool, String> {
    let conf = home.join(".aider.conf.yml");
    // Not `unwrap_or_default()`: the tail of this function does
    // `body = existing` + our `load:` lines and writes it back, so a conf that
    // exists but cannot be decoded would come out truncated to our two lines —
    // and the empty read also makes `load_present` false, which is what would
    // let the write happen on a file we are supposed to leave alone.
    let existing = super::super::install::json_util::read_text_or_bail(&conf)?;
    if existing
        .lines()
        .any(|l| l.trim_start().starts_with("load:") && !l.trim_start().starts_with("load: #"))
    {
        return Ok(false);
    }
    if dry_run {
        return Ok(true);
    }
    let startup = home.join(".tokenade").join("aider-startup.txt");
    fs::create_dir_all(startup.parent().unwrap_or(home))
        .map_err(|e| format!("mkdir ~/.tokenade: {e}"))?;
    let exe = crate::install_cli::stable_hook_exe();
    crate::write_text_atomic(
        &startup,
        &format!(
            "# Written by tokenade. `tokenade uninstall` removes it.\n\
             /run {exe} map\n"
        ),
    )
    .map_err(|e| format!("write aider startup file: {e}"))?;
    tokenade_core::paths::backup_once(&conf);
    let mut body = existing;
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    body.push_str(&format!(
        "# tokenade: run the repo map through tokenade at startup. Remove this\n\
         # line (and the file it names) to undo, or run `tokenade uninstall`.\n\
         load: {}\n",
        startup.display()
    ));
    crate::write_text_atomic(&conf, &body).map_err(|e| format!("write .aider.conf.yml: {e}"))?;
    Ok(true)
}

/// XDG config root for an install targeting `home`.
///
/// `XDG_CONFIG_HOME` is honoured ONLY when `home` IS the process home — i.e.
/// when we are installing for the current user, which is the case the variable
/// exists to describe. When a caller names a different home (a test, a
/// `--home` install, a fixture), that argument is authoritative: a
/// process-global must not silently redirect a write the caller explicitly
/// located.
///
/// Before this, both plugin dirs read the variable unconditionally, so passing
/// a throwaway `home` did NOT isolate them — the write landed wherever the
/// ambient environment pointed, which in a parallel test run is whatever
/// another thread had just set. That is the "tests write into the real HOME"
/// class, and it is also why `declared_channel_matches_what_the_installer_writes`
/// could not be made reliable.
pub(crate) fn xdg_config_root(home: &Path) -> PathBuf {
    tokenade_core::platform::xdg_config_root(home)
}

/// opencode's plugin auto-discovery dir: `$XDG_CONFIG_HOME/opencode/plugin`
/// (else `~/.config/opencode/plugin`) — same XDG path on every OS.
pub(crate) fn opencode_plugin_dir(home: &Path) -> PathBuf {
    xdg_config_root(home).join("opencode").join("plugin")
}

/// Install the opencode tool-result compaction plugin: drop the bridge into
/// `~/.config/opencode/plugin/tokenade.ts`. opencode globs that dir on startup
/// — no config edit, no enable-list. Idempotent.
pub(crate) fn install_opencode_plugin(home: &Path, dry_run: bool) -> Result<bool, String> {
    install_plugin_files(
        opencode_plugin_dir(home),
        dry_run,
        &[("tokenade.ts", OPENCODE_PLUGIN_TS, true)],
        "opencode",
        |_| "plugin",
        |_| true,
    )
}

/// Kilo Code's plugin auto-discovery dir (`~/.config/kilo/plugins/`). Kilo
/// mirrors OpenCode's plugin system, so it loads the same tool-result
/// compaction bridge.
pub(crate) fn kilo_plugin_dir(home: &Path) -> PathBuf {
    xdg_config_root(home).join("kilo").join("plugins")
}

/// Install the Kilo Code tool-result compaction plugin — the same bridge
/// tokenade drops for OpenCode, into Kilo's auto-discovery dir. Idempotent.
pub(crate) fn install_kilo_plugin(home: &Path, dry_run: bool) -> Result<bool, String> {
    install_plugin_files(
        kilo_plugin_dir(home),
        dry_run,
        &[("tokenade.ts", OPENCODE_PLUGIN_TS, true)],
        "kilo-code",
        |_| "plugin",
        |_| true,
    )
}

/// Install the Cline tool-result folding plugin. Cline discovers plugins via a
/// package.json `cline.plugins` entry; `cline plugin install <dir>` performs
/// that registration. Write the package to `~/.tokenade/cline-plugin/` and run
/// the registration best-effort — non-fatal if `cline` isn't on PATH (the
/// files are on disk with a manual-finish note). Idempotent. The plugin folds
/// large tool_result/file blocks in the provider-bound message list via
/// `api.registerMessageBuilder`, deterministically (cache-safe), errors kept
/// verbatim. E2E-verified against a real `@cline/core` load + build().
pub(crate) fn install_cline_plugin(home: &Path, dry_run: bool) -> Result<bool, String> {
    let dir = home.join(".tokenade").join("cline-plugin");
    install_plugin_files(
        dir,
        dry_run,
        &[
            ("tokenade.ts", CLINE_PLUGIN_TS, true),
            ("package.json", CLINE_PLUGIN_PKG, false),
        ],
        "cline",
        |name| {
            if name == "tokenade.ts" {
                "plugin"
            } else {
                "pkg"
            }
        },
        |dir| {
            let Some(mut cline) = agent_cli("cline", home) else {
                eprintln!(
                    "  note: wrote Cline plugin to {} but `cline` is not on PATH here — \
                     finish with: cline plugin install {}",
                    dir.display(),
                    dir.display()
                );
                return false;
            };
            let registered = cline
                .args(["plugin", "install"])
                .arg(dir)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !registered {
                eprintln!(
                    "  note: wrote Cline plugin to {} but `cline plugin install` \
                     didn't run here — finish with: cline plugin install {}",
                    dir.display(),
                    dir.display()
                );
            }
            registered
        },
    )
}

/// **Confine a registration to the home it is registering into.**
///
/// `install_cline_plugin` spawned `cline plugin install <dir>` with the
/// ambient environment, so the CHILD resolved its state directory from the
/// process's own `$HOME` while tokenade was installing into a different one.
/// Measured 2026-09-11 on this machine: running the single test
/// `install_cline_plugin_writes_folding_plugin`, which installs into a throwaway
/// temp home, added **one** directory to the real `~/.cline/plugins/_installed/local`
/// — which by then held **3 688 of them, 129 MB**, every one a copy of the same
/// tokenade plugin, accumulating at roughly 200 a day since 2026-08-10. Cline
/// registers a fresh hashed copy per call rather than replacing, so nothing
/// upstream bounds it, and it loads all of them.
///
/// The answer was already in this file, one function down: the OpenClaw
/// registration pins `HOME` and `OPENCLAW_STATE_DIR` on the child, with a
/// comment about a path that came back pointing at `/tmp/tok-ocl-inst-<pid>/…`.
/// The same fix was needed for every spawned registration and lived at ONE call
/// site — again: count the callers of a general mechanism. It moves here,
/// where no future registration can forget it.
///
/// `USERPROFILE` too: it is what Windows resolves a home from, and the two must
/// not disagree on a machine that reads both.
pub(crate) fn pin_home(c: &mut std::process::Command, home: &Path) {
    c.env("HOME", home);
    c.env("USERPROFILE", home);
}

/// Install the OpenClaw compaction plugin: write the package to
/// `~/.tokenade/openclaw-plugin/` then register it with `openclaw plugins
/// install <dir> --force` (local dir, no publish, no trust prompt;
/// `enabledByDefault` in the manifest means no separate enable step). The
/// files are always written; the register step is best-effort and reported
/// so the user can re-run it if the `openclaw` binary isn't reachable here.
/// A `Command` for an agent's own CLI, resolved the way the OS would.
///
/// `Command::new("openclaw")` does NOT find `openclaw.cmd` on Windows: the
/// Rust spawn appends `.exe` and nothing else, while every npm-installed agent
/// CLI on Windows IS a `.cmd` shim. Measured 2026-08-20 on the Windows VM —
/// `where openclaw` answers `…\AppData\Roaming\npm\openclaw.cmd`, the agent
/// is installed and working, and the bridge registration failed with "could
/// not be run" on every install. The compat matrix read that as "no in-agent
/// channel for openclaw on Windows", which is a statement about this function
/// and not about the agent.
///
/// Returns `None` when the CLI is genuinely not on PATH, so the caller can
/// still tell "absent" from "present and refused" — two different notes.
pub(crate) fn agent_cli(name: &str, home: &Path) -> Option<std::process::Command> {
    let path_var = std::env::var_os("PATH")?;
    // The extensions Windows would try, in the order it tries them. `.ps1` is
    // deliberately absent: running one needs a PowerShell host and an
    // execution policy, which is a different failure the caller cannot fix.
    // Windows has NO extensionless slot, and leaving one in is worse than
    // useless: npm drops a `openclaw` shell script (for Git Bash) NEXT TO
    // `openclaw.cmd`, so an empty extension tried first finds the script and
    // the loader answers `%1 is not a valid Win32 application (os error 193)`
    // — measured on the VM 2026-08-20, with the working `.cmd` sitting in the
    // same directory.
    let exts: &[&str] = if cfg!(windows) {
        &[".exe", ".cmd", ".bat", ".com"]
    } else {
        &[""]
    };
    for dir in std::env::split_paths(&path_var) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        for ext in exts {
            let cand = dir.join(format!("{name}{ext}"));
            if !cand.is_file() {
                continue;
            }
            let is_script = matches!(
                cand.extension()
                    .and_then(|e| e.to_str())
                    .map(str::to_ascii_lowercase)
                    .as_deref(),
                Some("cmd") | Some("bat")
            );
            if is_script {
                // A batch file is not an image the loader can execute; the
                // command interpreter runs it. `/C` so it exits when done.
                let mut c = tokenade_core::process_util::internal_command("cmd");
                c.arg("/C").arg(cand);
                // This builder hands the command back for a CAPTURED run, so
                // the console it would otherwise pop is a bare flash on the
                // user's desktop with nothing in it. Set here rather than at
                // the call site: the caller sees a ready-to-run `Command` and
                // has no way to know a `cmd /C` is hiding inside it.
                tokenade_core::process_util::configure_no_window(&mut c);
                pin_home(&mut c, home);
                return Some(c);
            }
            let mut c = tokenade_core::process_util::internal_command(cand);
            tokenade_core::process_util::configure_no_window(&mut c);
            pin_home(&mut c, home);
            return Some(c);
        }
    }
    None
}

pub(crate) fn install_openclaw_plugin(home: &Path, dry_run: bool) -> Result<bool, String> {
    let dir = home.join(".tokenade").join("openclaw-plugin");
    install_plugin_files(
        dir,
        dry_run,
        &[
            ("index.js", OPENCLAW_PLUGIN_INDEX, true),
            ("package.json", OPENCLAW_PLUGIN_PKG, false),
            ("openclaw.plugin.json", OPENCLAW_PLUGIN_MANIFEST, false),
        ],
        "openclaw",
        |name| match name {
            "index.js" => "index",
            "package.json" => "pkg",
            _ => "manifest",
        },
        |dir| {
            // Best-effort registration. Failure (e.g. `openclaw` not on PATH in
            // this shell) is non-fatal: the package is on disk for a manual re-run.
            // `--link` is the option openclaw actually has (`-l/--link`);
            // `--force` never existed and produced `error: unknown option`
            // on every install since the bridge was written.
            //
            // HOME and OPENCLAW_STATE_DIR are pinned to the home we were ASKED
            // to install into, and that is a correctness fix rather than test
            // hygiene. Without it the child inherits the ambient home and
            // registers the plugin wherever the caller happens to live — so an
            // install into `/somewhere/else` silently rewrote the developer's
            // own `~/.openclaw/openclaw.json`.
            //
            // Measured 2026-08-17: `install_openclaw_plugin_writes_package`
            // runs with a temp home, and the real `~/.openclaw/openclaw.json`
            // came back pointing at `/tmp/tok-ocl-inst-<pid>/…`. openclaw then
            // REFUSES TO START — "plugin path not found" — the moment that
            // directory is gone, which for a temp dir is the next reboot.
            let Some(mut openclaw) = agent_cli("openclaw", home) else {
                eprintln!(
                    "  note: OpenClaw plugin written to {} but `openclaw` is not on PATH here. \
                     Once it is, register it with:\n  openclaw plugins install {} --link",
                    dir.display(),
                    dir.display()
                );
                return false;
            };
            let outcome = openclaw
                .args(["plugins", "install"])
                .arg(dir)
                .arg("--link")
                // `HOME`/`USERPROFILE` are pinned by `agent_cli` for every
                // spawned registration; this one needs its state dir named too.
                .env("OPENCLAW_STATE_DIR", home.join(".openclaw"))
                .output();
            // What openclaw SAID, not what we guessed it meant.
            //
            // This used to collapse every failure into "openclaw not on PATH in
            // this shell?" and print a recipe. Measured 2026-08-18 on macOS with
            // openclaw 2026.5.7 on PATH, the command ran fine and answered:
            //
            //   Plugin "tokenade" installation blocked: dangerous code patterns
            //   detected: Shell command execution detected (child_process)
            //
            // The bridge calls the tokenade binary — that IS what a bridge does
            // — and openclaw's plugin scanner refuses it. The note sent the
            // reader to check their PATH, which was correct, and irrelevant.
            //
            // openclaw also exits 0 while refusing, so `status.success()` is not
            // the discriminator either: we look for the refusal in its output.
            let registered = match &outcome {
                Ok(o) => {
                    let said = format!(
                        "{}{}",
                        String::from_utf8_lossy(&o.stdout),
                        String::from_utf8_lossy(&o.stderr)
                    );
                    o.status.success()
                        && !said.contains("installation blocked")
                        && !said.to_lowercase().contains("error")
                }
                Err(_) => false,
            };
            if !registered {
                match &outcome {
                    Err(e) => eprintln!(
                        "  note: OpenClaw plugin written to {} but `openclaw` could not be run \
                         ({e}). Once it is on PATH, register it with:\n  \
                         openclaw plugins install {} --link",
                        dir.display(),
                        dir.display()
                    ),
                    Ok(o) => {
                        let said = format!(
                            "{}{}",
                            String::from_utf8_lossy(&o.stdout),
                            String::from_utf8_lossy(&o.stderr)
                        );
                        let first = said
                            .lines()
                            .find(|l| {
                                let l = l.to_lowercase();
                                l.contains("blocked") || l.contains("error") || l.contains("warn")
                            })
                            .unwrap_or_else(|| {
                                said.lines().find(|l| !l.trim().is_empty()).unwrap_or("")
                            });
                        eprintln!(
                            "  note: OpenClaw plugin written to {} — openclaw REFUSED to register it:\n  \
                             {}",
                            dir.display(),
                            first.trim()
                        );
                        if said.contains("dangerous code patterns") {
                            eprintln!(
                                "  This openclaw REFUSES the bridge at install time: its plugin scanner \
                                 flags `child_process`, which is how a bridge reaches the tokenade \
                                 binary. No flag turns that off, and working around a security scanner \
                                 is not something this installer will do.\n  \
                                 It is fixed UPSTREAM: openclaw stopped blocking installs on that rule \
                                 in 2026.6.10 (measured on the published tarballs 2026-08-21 — blocked \
                                 in 2026.4.8, 2026.5.7 and 2026.6.1, accepted from 2026.6.10 on, and \
                                 verified end to end on 2026.7.1-2 where the plugin registers and \
                                 `openclaw plugins list` shows it enabled). Upgrade openclaw \
                                 (`npm install -g openclaw@latest`) and run this install again.\n  \
                                 Until then tool results stay uncompacted for openclaw; command output \
                                 is still folded by the universal shell proxy."
                            );
                        }
                    }
                }
            }
            registered
        },
    )
}
