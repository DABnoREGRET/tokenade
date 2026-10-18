//! Noticing that the installed hooks are gone.
//!
//! A plugin overwriting `settings.json` removes every tokenade hook, and
//! hook-side code cannot report that — it no longer runs. The only remaining
//! trigger is the user invoking the CLI, which is what these do.

use super::*;

/// See call site in `main`. Cheap: two stat/read calls, once per day.
pub(super) fn warn_if_hooks_vanished(cmd: &str) {
    const USER_FACING: &[&str] = &[
        // PRIMARY names: `primary_of` resolves the alias before the
        // question, so `doctor`, `sem` and `run-script` are covered by
        // `healthcheck`, `ask` and `execute` without being listed.
        "ask",
        "dashboard",
        "discover",
        "execute",
        "gain",
        "healthcheck",
        "impact",
        "map",
        "query",
        "skeleton",
        "upgrade",
        "web",
    ];
    if !USER_FACING.contains(&crate::command_table::primary_of(cmd)) {
        return;
    }
    let Some(home) = tokenade_core::platform::home_dir() else {
        return;
    };
    if !home.join(".tokenade").join("installed-at").exists() {
        return; // never installed here — nothing to guard
    }
    let throttle = home.join(".tokenade").join("hooks-vanished-warned");
    if let Ok(md) = std::fs::metadata(&throttle) {
        if let Ok(age) = md.modified().and_then(|m| {
            std::time::SystemTime::now()
                .duration_since(m)
                .map_err(std::io::Error::other)
        }) {
            if age.as_secs() < 86_400 {
                return;
            }
        }
    }
    // Only guard agents we actually wired HERE: a user who installed
    // tokenade for another agent (or deliberately unwired Claude) must
    // not get a daily false alarm.
    if !home
        .join(".tokenade")
        .join("claude-hooks-installed")
        .exists()
    {
        return;
    }
    let settings = tokenade_core::platform::claude_config_dir_for(&home).join("settings.json");
    let Ok(txt) = std::fs::read_to_string(&settings) else {
        return; // no Claude Code here — other agents have their own configs
    };
    if txt.contains("tokenade") {
        return; // hooks (or at least references) still present
    }
    let _ = std::fs::write(&throttle, b"1");
    eprintln!(
        "\n⚠️  tokenade: your Claude Code settings.json no longer contains ANY \
tokenade hook — every session is running WITHOUT compaction (a plugin or \
manual edit likely overwrote the hooks). Run `tokenade install` to restore \
them, then `tokenade doctor` to verify.\n"
    );
    tokenade_core::debug_log::log(
        "cli",
        "hooks_vanished_warning",
        serde_json::json!({"cmd": cmd}),
    );
}

/// Heals the out-of-repo CLI lessons. Not opt-in: neither `tokenade install`,
/// nor the version gate, nor `TOKENADE_NO_AUTO_UPDATE`. Silent — hooks and
/// `wrap` can call it without polluting stdout. A file already up to date is
/// not rewritten.
pub(super) fn maybe_heal_global_lectures() {
    if current_exe_must_not_touch_user_config() {
        return;
    }
    let home = tokenade_core::platform::home_dir();
    let _ = tokenade_core::project_scaffold::ensure_global_rules(home.as_deref());
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let root = tokenade_core::platform::project_root_of(&cwd);
    let _ = tokenade_core::project_scaffold::heal_project_lectures(&root);
}

fn current_exe_must_not_touch_user_config() -> bool {
    let cur_exe = std::env::current_exe()
        .map(|e| e.display().to_string())
        .unwrap_or_default();
    tokenade_core::platform::is_build_dir_path(&cur_exe)
        || crate::install_cli::is_temp_new_exe_path(&cur_exe)
        || another_installed_binary_owns_the_hooks(
            &crate::install_cli::stable_hook_exe(),
            tokenade_core::platform::home_dir().as_deref(),
        )
}

/// **The binary the hooks name is the one that keeps them.**
///
/// The two path tests above catch a build directory and the updater's temp
/// copy, and nothing else. The compat bench stages its binary OUT of the
/// build directory on purpose (`~/.cache/tokenade-compat-bin/…`) and runs the
/// proxy's supervisor with the operator's real HOME: that 1.1.22 copy saw an
/// installed 1.1.21, ran the post-upgrade refresh on the real
/// `~/.claude/settings.json`, and the operator was left with no hooks and a
/// status line pointing at a deleted binary (2026-10-03; the same family as
/// 2026-08-20, closed then on the unit's `HOME` only).
///
/// So: when the hooks already name a tokenade binary that EXISTS and is not
/// this one, this process is a visitor — a bench copy, a second checkout, a
/// stray download — and the installed binary does its own refresh the next
/// time it runs. A named binary that is gone is a broken install, and
/// repairing it stays allowed.
pub(crate) fn another_installed_binary_owns_the_hooks(me: &str, home: Option<&Path>) -> bool {
    let Some(path) = hook_settings_path(&root(), true, home) else {
        return false;
    };
    let Ok(content) = fs::read_to_string(path) else {
        return false;
    };
    hook_binaries_named_in(&content)
        .iter()
        .any(|named| named != me && Path::new(named).is_file())
}

/// Absolute paths to a `tokenade` binary that a settings file's hook and
/// status-line commands invoke.
pub(crate) fn hook_binaries_named_in(content: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tok in content.split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '\\')) {
        let name = tok.rsplit(['/', '\\']).next().unwrap_or("");
        let absolute = tok.starts_with('/') || tok.get(1..3) == Some(":\\");
        if absolute
            && (name == "tokenade" || name == "tokenade.exe")
            && !out.iter().any(|o| o == tok)
        {
            out.push(tok.to_string());
        }
    }
    out
}

/// See callers for rationale. Reads the version-mismatch
/// gate, runs `refresh_global_claude_md_block_in_place` on the user's
/// `~/.claude/CLAUDE.md` if fences are present, marks done. Never
/// prompts. Errors are absorbed (best-effort).
pub(super) fn maybe_run_post_upgrade_refresh() {
    // PROTOCOL-QUIET commands: their stdout is consumed by a machine —
    // eval'd by the shell proxy (shell-init), parsed as hook JSON
    // (hookwrap*/gemini-hook), rendered as a status bar line, spoken as
    // JSON-RPC (mcp-proxy) or delivered as command output (wrap/replay).
    // The refresh's "✓ refreshed …" line inside `eval "$(tokenade
    // shell-init)"` caused a live shell syntax error (stdout-pollution
    // class from the retrospective). Defer to the next interactive
    // command — the version marker keeps it pending, nothing is lost.
    const PROTOCOL_QUIET: &[&str] = &[
        // PRIMARY names, for the same reason as `USER_FACING` — and here it
        // bit: `mcp-wrap`, the canonical name, was missing (only its
        // deprecated alias `mcp-proxy` was listed), so the "✓ refreshed …"
        // line could end up inside JSON-RPC.
        "gemini-after-tool",
        "gemini-afterhook",
        "gemini-hook",
        "hookwrap",
        "hookwrap-copilot",
        "hookwrap-postread",
        "hookwrap-postwebfetch",
        "hookwrap-precompact",
        "hookwrap-pretool",
        "hookwrap-sessionstart",
        "hookwrap-subagentstop",
        "hookwrap-userprompt",
        "hookwrap-websearch-post",
        "mcp-wrap",
        "pre-edit-snapshot",
        "real-savings-refresh",
        "replay",
        "reporter-run",
        "shell-init",
        "shim-exec",
        "statusline",
        "wrap",
    ];
    if std::env::args()
        .nth(1)
        .map(|c| PROTOCOL_QUIET.contains(&crate::command_table::primary_of(&c)))
        .unwrap_or(true)
    {
        return;
    }
    // A BUILD-DIR binary (dist/, target/) must never silently claim the
    // user's config: during a release dry-run the dist binary healed
    // statusLine/hooks to its own path, then the build wiped dist/ —
    // dead statusline, dead hooks (live incident). Explicit
    // `tokenade install` from such a binary remains allowed (dev intent).
    // Also skip when we're the auto-updater's freshly-downloaded temp
    // binary (`.tokenade.new.<pid>`): the updater probes it with `<tmp>
    // --version` BEFORE the atomic swap, and running the refresh here
    // would (a) rewrite the user's hooks to this transient path — which
    // is unlinked seconds later by the rename, bricking every hook — and
    // (b) consume the version-mismatch marker, so the REAL binary would
    // then skip its own legitimate refresh. After the rename, the swap
    // launches the live path (`--version --no-auto-update`) so THIS
    // gate runs on the installed inode — explicit `tokenade upgrade`
    // and the daily `upgrade --auto-background` check.
    if current_exe_must_not_touch_user_config() {
        return;
    }
    // One-shot recalc of the local stats ledgers under the corrected
    // accounting rules (delivery caps / web-binary credits / bounded
    // compounding). Own marker (`~/.tokenade/recalc-v1.done`) — deliberately
    // OUTSIDE the version-refresh gate below, so it still runs when the
    // version marker was already consumed. Fail-open + cheap (one stat()).
    tokenade_core::recalc::run_if_needed();
    // Stale `tokenade` MCP self-entry purge — deliberately OUTSIDE the
    // version-refresh gate below (same reasoning as recalc). Claude is
    // hooks-only since the zero-MCP pivot, so a `tokenade` entry in any MCP
    // config (~/.claude.json top-level + per-project, .mcp.json, Gemini/
    // OpenCode/Hermes) is always stale and wastes tool-definition surface.
    // The gated copy in `post_upgrade_refresh_installed_surfaces` only ran on
    // the one upgrade where the version marker flipped — a user whose marker
    // was already current when this purge shipped kept the dead entry forever
    // (field report). Running it here makes it self-heal on every interactive
    // invocation. Idempotent + cheap (no-op when absent), fail-open. Nothing
    // re-adds the entry: the MCP self-entry writers are test-only dead code
    // post-pivot, so there is no purge/install thrash.
    let _ = tokenade_core::mcp_wrap::purge_tokenade_mcp_entries(
        &root(),
        tokenade_core::platform::home_dir().as_deref(),
        false,
    );
    // No-hook fallback dismantler (PATH shim + legacy alias proxy) —
    // deliberately OUTSIDE the version-refresh gate below, for exactly the
    // reason recalc and the MCP purge above are: the copy that used to live in
    // `post_upgrade_refresh_installed_surfaces` only ran on the ONE upgrade
    // where the version marker flipped. A machine whose marker was already
    // consumed for the current version (updater temp-binary races it, or the
    // refresh ran before the strip shipped) kept a `PATH`-hijacking shim
    // FOREVER — field report 2026-07: a stale `~/.tokenade/shim/node` broke a
    // Claude Code plugin hook and no code path was left that would repair it.
    // Now every interactive invocation heals it. Both passes no-op per file
    // when there is nothing to strip and no ≤0.9.4 emptied guard to repair, so
    // the cost on a clean machine is reading a handful of small rc files.
    {
        let home = tokenade_core::platform::home_dir();
        let _ = uninstall_shell_proxy(home.as_deref(), false);
        let _ = uninstall_path_shim(home.as_deref(), false);
    }
    let current = env!("CARGO_PKG_VERSION");
    if !tokenade_core::updater::needs_post_upgrade_refresh(current) {
        return;
    }
    // Silent refresh — only the in-place splice path, no prompts, no
    // fresh install. If fences are absent (user never installed or
    // uninstalled) the helper returns None and we mark done anyway:
    // we don't want to retry every invocation forever.
    // The CLI lesson is NOT here: `maybe_heal_global_lectures` runs on
    // every invocation, wrap/hooks included, without this gate.
    let home = tokenade_core::platform::home_dir();
    let _ = refresh_global_claude_md_block_in_place(home.as_deref(), false);
    post_upgrade_refresh_installed_surfaces(home.as_deref());
    tokenade_core::updater::mark_post_upgrade_refresh_done(current);
}

/// The silent post-upgrade refresh used to
/// cover ONLY the global CLAUDE.md block; hook commands, MCP entries
/// and shell-proxy rc fragments written by `tokenade install` kept
/// drifting after an auto-update until the user happened to re-run the
/// installer by hand. Re-run the SAME drift-detecting upserts here, but
/// strictly presence-gated so a background refresh never installs a
/// surface the user didn't opt into:
///
///   - Claude Code hooks: only when the GLOBAL ~/.claude/settings.json
///     already carries a tokenade hook (`hookwrap` marker). The standard
///     suite is upserted as a unit (it's installed as a unit — a hook
///     added in a newer version, e.g. the Read-delivery hook or the
///     PreToolUse Bash-rewrite hook, counts as drift of the suite, not a
///     new opt-in). Project-local settings are never touched (repo-owned;
///     silently dirtying checkouts is worse than the drift).
///   - MCP configs: for each agent whose user-level config exists AND
///     already references tokenade, the entry is re-upserted in
///     canonical shape (other keys preserved by the writers).
///   - Shell proxy: rc fragments + ~/.tokenade/shellproxy.* are
///     re-synced only when shellproxy.sh proves a prior proxy install.
///
/// Debug-log installs (`tokenade install` enables ~/.tokenade/debug.log
/// BY DEFAULT, so this is the majority case): the existing debug-log
/// path is reconstructed from the installed entries and passed back to
/// the installers, so the refreshed commands keep the user's debug
/// setup. Only when reconstruction fails (exotic quoting we don't
/// recognise) is the file left strictly alone — never half-refreshed.
///
/// Everything is best-effort and silent: outcomes and IO errors are
/// swallowed; the next manual `tokenade install` reports interactively.
pub(super) fn post_upgrade_refresh_installed_surfaces(home: Option<&Path>) {
    if is_hook_opted_out() {
        return;
    }
    let root = root();
    // `stable_hook_exe` resolves transient updater-temp / build-dir paths
    // to the stable install destination so a background refresh never
    // wires hooks to a path that's about to vanish.
    let exe = tokenade_core::profile::timed(
        "ensure_hooks:stable_hook_exe",
        crate::install_cli::stable_hook_exe,
    );

    // (a) Claude Code hooks — global settings only.
    if let Some(settings_path) = hook_settings_path(&root, true, home) {
        if let Ok(content) = fs::read_to_string(&settings_path) {
            // Outer Option = "is the install state refreshable?", inner
            // Option = the debug-log path to preserve (None = no debug).
            let debug_log: Option<Option<String>> = if content.contains("TOKENADE_DEBUG_LOG") {
                extract_debug_log_from_settings(&content).map(Some)
            } else {
                Some(None)
            };
            if let Some(dbg) = debug_log {
                let dbg = dbg.as_deref();
                if content.contains("hookwrap") {
                    let _ = install_claude_code_hook(&root, &exe, dbg, true, home, false);
                    let _ = install_claude_code_postread_hook(&root, &exe, dbg, true, home, false);
                    let _ =
                        install_claude_code_postwebfetch_hook(&root, &exe, dbg, true, home, false);
                    let _ = install_claude_code_index_hook(&root, &exe, dbg, true, home, false);
                    let _ = install_claude_code_pre_edit_hook(&root, &exe, dbg, true, home, false);
                    let _ = install_claude_code_passive_hooks(&root, &exe, dbg, true, home, false);
                    // PreToolUse Bash-rewrite hook is now part of the
                    // default suite (installed unless --no-hook), so it
                    // refreshes with the rest — no separate opt-in gate.
                    let _ = install_claude_code_lsp_enforcement_hook(
                        &root, &exe, dbg, true, home, false,
                    );
                }
            }
        }
    }

    // (b) MCP self-entry purge moved OUT of this version-gated path into
    // `maybe_run_post_upgrade_refresh` (runs on every interactive
    // invocation, not only the one upgrade where the version marker flips)
    // so a pre-pivot `tokenade` entry no longer lingers forever. See there.

    // (c) No-hook fallback — REMOVED as of 0.9.5 (PO decision). The PATH shim
    // and the legacy alias proxy both sat in the exec path of every command the
    // user ran, and their blast radius dwarfed their value (they only ever
    // served no-hook agents): a shim whose baked tokenade path went stale left
    // `git`/`npm`/`docker` dead at exit 127 with no fallback; the alias block
    // could login-loop a display manager; both were the fork-bomb surface. So
    // upgrade now RIPS BOTH OUT and never re-installs either.
    //
    // The dismantling pass MOVED OUT of this function to the ungated section
    // of `maybe_run_post_upgrade_refresh` — see the comment there. Keeping it
    // here would have made it fire only on the single invocation where the
    // version marker flips, which is precisely how a stale shim survived on a
    // user's machine indefinitely.
    // (d) Plugin bridges — the surface this function forgot. Until now
    // `PLUGIN_INSTALLS` had exactly one caller, the interactive `tokenade
    // install`, so an upgrade refreshed Claude Code's hooks and Gemini's and
    // left every plugin agent (opencode, pi, openclaw, cline, kilo, hermes)
    // running the bridge from the day it was installed. Same proof-of-prior-
    // install rule as (a): only bridges whose dropped file is already on disk
    // are rewritten, and the registration steps are not re-run.
    if let Some(h) = home {
        crate::install_cli::refresh_installed_plugin_bridges(h);
        refresh_other_hook_agents(&exe, h, home);
    }
}

/// True when `content` already carries a tokenade hook — not a first install.
///
/// Claude-shaped writers bake `hookwrap`; Gemini/Qwen bake `gemini-hook`
/// / `gemini-lifecycle`. Gating on `hookwrap` alone left Qwen's
/// `~/.qwen/settings.json` unhealed after a `cargo clean`.
pub(crate) fn config_proves_a_tokenade_hook(content: &str) -> bool {
    content.contains("hookwrap")
        || content.contains("gemini-hook")
        || content.contains("gemini-lifecycle")
        || content.contains("gemini-after-tool")
        || content.contains("gemini-afterhook")
}

/// Re-assert the hook suite of every agent that is NOT Claude Code or Gemini.
///
/// (a) above refreshes Claude Code; the unconditional self-heal adds Gemini.
/// Codex, Droid, Copilot, Grok, Reasonix, Qwen and Cursor had installers with a
/// single caller — the interactive `tokenade install` — so an upgrade never
/// touched them. What that costs is not hypothetical: the hook command carries
/// an ABSOLUTE path to the binary, so a move (a new npm prefix, a reinstall)
/// leaves those seven agents invoking a path that no longer exists. Claude Code
/// gets repaired on the next upgrade; they did not, and a dead hook is silent.
///
/// Same marker gate as (a) — the config must already contain a tokenade
/// hook command (`hookwrap` on Claude-shaped agents, `gemini-hook` /
/// `gemini-lifecycle` on the Gemini/Qwen family), which proves a prior
/// install. An upgrade therefore repairs, and never installs.
pub(crate) fn refresh_other_hook_agents(exe: &str, h: &Path, home: Option<&Path>) {
    for (cfg, install) in crate::install_cli::hook_agent_configs(h) {
        // Claude Code and Gemini carry `None`: their own refresh paths above
        // already re-assert them, in a shape this table cannot express.
        let Some(install) = install else { continue };
        let Ok(content) = fs::read_to_string(&cfg) else {
            continue;
        };
        if !config_proves_a_tokenade_hook(&content) {
            continue;
        }
        // Same two-level answer as (a): outer = "is this refreshable?", inner =
        // the debug-log path to preserve. A config that mentions
        // TOKENADE_DEBUG_LOG in a shape we cannot parse is left alone rather
        // than refreshed WITHOUT the user's debug setup.
        let dbg: Option<Option<String>> = if content.contains("TOKENADE_DEBUG_LOG") {
            extract_debug_log_from_settings(&content).map(Some)
        } else {
            Some(None)
        };
        if let Some(dbg) = dbg {
            let _ = install(exe, dbg.as_deref(), home, false);
        }
    }
}

/// Self-heal the agent hook suite. Unlike
/// [`post_upgrade_refresh_installed_surfaces`] — which only refreshes when a
/// tokenade hook STILL proves a prior install (the `hookwrap` marker) — this
/// re-asserts the FULL suite UNCONDITIONALLY whenever the machine ever
/// installed tokenade (the install sentinel exists). So a `settings.json` that
/// another tool or an agent upgrade fully overwrote gets rebuilt rather than
/// left broken — the failure mode where ZERO tokenade hooks survived and the
/// marker-gated refresh therefore did nothing.
///
/// Channels re-asserted, per agent detected present:
///   - Claude Code: the six PostToolUse / PreToolUse / SessionStart /
///     UserPromptSubmit / PreCompact hooks (global settings).
///   - Gemini CLI: its BeforeTool hook.
///   - The universal shell proxy (rc files) — the RESILIENT channel, rarely
///     touched by plugin systems, and the one that calls back here via
///     `tokenade wrap`, so it can rebuild Claude Code hooks even when EVERY
///     `settings.json` hook was wiped.
///
/// `respect_throttle` true → skip unless the throttle window elapsed (hot
/// paths: `tokenade wrap`, PostToolUse hookwrap). SessionStart passes false
/// (rare; idempotent — `AlreadyPresent` writes nothing). Returns the count of
/// drift repairs (Installed + Refreshed). Best-effort, silent, never panics.
/// Launch the proxy auto-install, detached, at most once a day.
///
/// Its own marker rather than the hooks' one: the two have neither the same
/// cost nor the same useful frequency. A broken hook is repaired every six
/// hours because it is cheap; the proxy is installed once and has no reason
/// to be retried that often — and every attempt starts a process.
fn spawn_proxy_auto_install(home: &Path) {
    const TTL_SECS: u64 = 24 * 3600;
    let marker = home.join(".tokenade").join("llm-proxy-autocheck");
    if let Ok(md) = fs::metadata(&marker) {
        if let Ok(age) = md
            .modified()
            .and_then(|t| t.elapsed().map_err(std::io::Error::other))
        {
            if age.as_secs() < TTL_SECS {
                return;
            }
        }
    }
    if let Some(dir) = marker.parent() {
        let _ = fs::create_dir_all(dir);
    }
    // Mark BEFORE launching: if the subprocess dies, we don't want to
    // relaunch it on the next command.
    let _ = tokenade_core::paths::write_atomic(&marker, "1");
    let exe = crate::install_cli::stable_hook_exe();
    let _ = tokenade_core::process_util::spawn_outliving_its_job(
        std::path::Path::new(&exe),
        &["llm-proxy", "auto-install"],
    );
}

pub(super) fn ensure_hooks(home: Option<&Path>, respect_throttle: bool) -> usize {
    // BEFORE every gate below, on purpose. A leftover `~/.tokenade/shim` sits
    // at the FRONT of `PATH` and hijacks `node`, `git`, `npm`… for every child
    // process — it breaks the machine whether or not the user opted out of
    // hooks, whether or not an install sentinel survives, and it must not wait
    // for a 6 h throttle window. Guarded by a single `exists()` so the hot path
    // (PostToolUse hookwrap, `tokenade wrap`) pays one stat, and the full sweep
    // runs only on a machine that actually still carries the shim.
    if let Some(h) = home {
        let _p = tokenade_core::profile::phase("ensure_hooks:shim_probe");
        if crate::install::shim_dir_present(h) {
            let _ = uninstall_path_shim(Some(h), false);
            let _ = uninstall_shell_proxy(Some(h), false);
        }
    }
    if is_hook_opted_out() {
        return 0;
    }
    let Some(home) = home else {
        return 0;
    };
    // Opt-in gate: only machines that installed tokenade. Never resurrect on a
    // machine that never installed or that uninstalled (uninstall clears it).
    if !tokenade_core::install_state::installed_ever(home) {
        return 0;
    }
    const TTL_SECS: u64 = 6 * 3600;
    if respect_throttle && !tokenade_core::install_state::hooks_check_due(home, TTL_SECS) {
        return 0;
    }
    // After the throttle: this reads the settings file, and the hot path
    // (every PostToolUse hookwrap, every `wrap`) must keep paying one stat.
    if current_exe_must_not_touch_user_config() {
        return 0;
    }
    // Stamp first so concurrent hot-path callers don't pile up on the work.
    tokenade_core::install_state::mark_hooks_checked(home);

    // The local proxy, for machines that updated without ever re-running
    // `install`. That is the NORMAL case: `upgrade` swaps the binary and
    // nothing else, so without this entry point the feature only reaches
    // people who reinstall by hand.
    //
    // DETACHED, and that is the whole point of the subprocess: installing the
    // proxy starts a supervisor then waits up to 8 s for it to answer, and
    // this path is `tokenade wrap`'s, called on every command. Making the
    // agent wait eight seconds to install something it did not ask for would
    // be worse than not installing it.
    spawn_proxy_auto_install(home);

    let root = root();
    // Never wire the user's hooks to a transient build path (dist/, target/).
    // A dry-run dist binary running this heal previously re-pointed every
    // hook + the statusline to its own path, which then got wiped.
    let exe = crate::install_cli::stable_hook_exe();
    let hp = Some(home);
    let count =
        |o: &HookOutcome| matches!(o, HookOutcome::Installed(_) | HookOutcome::Refreshed(_));
    let mut repaired = 0usize;
    let present =
        tokenade_core::profile::timed("ensure_hooks:detect_all", || detect::detect_all_present(hp));

    // Claude Code hooks — global settings. Reconstruct the debug-log path from
    // the live settings if present; else fall back to the install default so a
    // fully-wiped settings.json is rebuilt WITH debug logging intact.
    // What the user chose at install. This repair used to re-assert the
    // DEFAULT install every session: global hooks after `--no-hook` or
    // `--per-project`, every detected agent after `--only`, the debug log
    // after `--no-debug-log`.
    let chose = crate::install_cli::saved_install_choices(home);
    let hooks_chosen = !chose.no_hook && !chose.per_project;

    let _p_claude = tokenade_core::profile::phase("ensure_hooks:claude_code");
    if hooks_chosen
        && chose.covers(detect::Agent::ClaudeCode)
        && present.contains(&detect::Agent::ClaudeCode)
    {
        let settings =
            hook_settings_path(&root, true, hp).and_then(|p| fs::read_to_string(&p).ok());
        // Hooks present without a debug-log path = installed without one:
        // keep it off. The default path is only for a settings file that lost
        // our hooks altogether.
        let hooks_without_log = settings
            .as_deref()
            .is_some_and(|c| c.contains("hookwrap") && !c.contains("TOKENADE_DEBUG_LOG"));
        let dbg_owned: Option<String> = if chose.no_debug_log || hooks_without_log {
            None
        } else {
            settings
                .as_deref()
                .filter(|c| c.contains("TOKENADE_DEBUG_LOG"))
                .and_then(extract_debug_log_from_settings)
                .or_else(|| {
                    home.join(".tokenade")
                        .join("debug.log")
                        .to_str()
                        .map(|s| s.to_string())
                })
        };
        let dbg = dbg_owned.as_deref();
        for o in [
            install_claude_code_hook(&root, &exe, dbg, true, hp, false),
            install_claude_code_postread_hook(&root, &exe, dbg, true, hp, false),
            install_claude_code_postwebfetch_hook(&root, &exe, dbg, true, hp, false),
            install_claude_code_index_hook(&root, &exe, dbg, true, hp, false),
            install_claude_code_pre_edit_hook(&root, &exe, dbg, true, hp, false),
            install_claude_code_passive_hooks(&root, &exe, dbg, true, hp, false),
            install_claude_code_lsp_enforcement_hook(&root, &exe, dbg, true, hp, false),
        ] {
            if count(&o) {
                repaired += 1;
            }
        }
    }

    // Qwen Code (Gemini CLI fork) with its own `~/.qwen/settings.json`;
    // leaving it off this walk left live hooks pointing at a wiped
    // `target/debug/tokenade` (playbook 1-in-4 example).
    drop(_p_claude);
    {
        let _p = tokenade_core::profile::phase("ensure_hooks:gemini");
        if hooks_chosen
            && chose.covers(detect::Agent::Qwen)
            && present.contains(&detect::Agent::Qwen)
            && count(&install_qwen_cli_hook(&exe, hp, false))
        {
            repaired += 1;
        }
    }

    // No-hook fallback — REMOVED as of 0.9.5 (PO decision); see the note in
    // `post_upgrade_refresh_installed_surfaces`. Self-heal now means RIPPING
    // OUT any shim/alias an older release left, never re-asserting one.
    // Ungated on purpose: a user whose rc file an older strip already emptied
    // carries a syntax error with no tokenade trace left to detect, so keying
    // this off "is a shim present?" would never reach them. Both passes no-op
    // per file when there is nothing to strip and no guard to repair.
    {
        let _p = tokenade_core::profile::phase("ensure_hooks:strip_shell_proxy");
        let _ = uninstall_shell_proxy(hp, false);
    }
    {
        let _p = tokenade_core::profile::phase("ensure_hooks:strip_path_shim");
        let _ = uninstall_path_shim(hp, false);
    }

    // Copilot CLI / Cursor CLI ticker — same writer as `tokenade install`.
    // Without this, an upgrade or `ensure_hooks` heals Claude/Gemini/Qwen
    // hooks and leaves `~/.cursor/cli-config.json` without `statusLine`
    // (live 2026-09-20: Cursor CLI present, no ticker).
    {
        let _p = tokenade_core::profile::phase("ensure_hooks:statusline_hosts");
        if chose.only.is_none() {
            let _ = crate::install::wire_statusline_hosts(hp, &exe, false);
        }
    }

    if repaired > 0 {
        tokenade_core::debug_log::log(
            "ensure-hooks",
            "self_healed",
            serde_json::json!({
                "repaired": repaired,
                "trigger": if respect_throttle { "hot" } else { "sessionstart" },
            }),
        );
    }
    repaired
}

/// P2c — recover the debug-log path embedded in an installed Claude
/// Code hook so a silent refresh can rebuild the canonical command
/// WITH it (rather than downgrading the user to a no-debug install).
/// Walks the parsed settings for a tokenade-tagged hook command and
/// inverts `build_hook_command`'s two shapes. Returns None when no
/// command parses — the caller must then leave the file alone.
pub(super) fn extract_debug_log_from_settings(content: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(content).ok()?;
    // New Windows location (since the cmd /C wrapper was dropped): the
    // debug-log path lives in the settings-level `env` block, not inline
    // in a hook command. Check it first so upgrades recover the path.
    if let Some(p) = v
        .get("env")
        .and_then(|e| e.get("TOKENADE_DEBUG_LOG"))
        .and_then(|p| p.as_str())
    {
        return Some(p.to_string());
    }
    for groups in v.get("hooks")?.as_object()?.values() {
        let Some(arr) = groups.as_array() else {
            continue;
        };
        for g in arr {
            let Some(hs) = g.get("hooks").and_then(|h| h.as_array()) else {
                continue;
            };
            for h in hs {
                let tagged = h
                    .get("_tag")
                    .and_then(|t| t.as_str())
                    .map(|t| t.starts_with("tokenade"))
                    .unwrap_or(false);
                let Some(cmd) = h.get("command").and_then(|c| c.as_str()) else {
                    continue;
                };
                if tagged && cmd.contains("TOKENADE_DEBUG_LOG=") {
                    return extract_debug_log_from_hook_command(cmd);
                }
            }
        }
    }
    None
}

/// Inverse of `build_hook_command` for the debug-log component.
/// POSIX shape (nested posix_quote):
///   `sh -c 'TOKENADE_DEBUG_LOG='\''<path>'\'' exec '\''<exe>'\'' <sub>'`
/// Windows: the path is NO LONGER inline in the hook command (the
/// `cmd /C "set …&&"` wrapper was dropped — it polluted the hook's JSON
/// stdout). It lives in the settings-level `env` block instead, which
/// `extract_debug_log_from_settings` reads directly; this inline parser
/// only ever matches the POSIX `sh -c` form now.
/// Paths with apostrophes (POSIX) don't round-trip through the simple
/// scan below → None, caller preserves the file untouched.
pub(super) fn extract_debug_log_from_hook_command(cmd: &str) -> Option<String> {
    const KEY: &str = "TOKENADE_DEBUG_LOG=";
    let rest = &cmd[cmd.find(KEY)? + KEY.len()..];
    if let Some(r) = rest.strip_prefix("'\\''") {
        let end = r.find('\'')?;
        // The path must be terminated by the matching nested
        // close-quote (`'\'' ` before `exec`); anything else means the
        // path itself contains quoting we'd corrupt.
        if !r[end..].starts_with("'\\'' ") {
            return None;
        }
        let p = &r[..end];
        (!p.is_empty()).then(|| p.to_string())
    } else {
        let end = rest.find("&&")?;
        let p = rest[..end].trim();
        (!p.is_empty()).then(|| p.to_string())
    }
}
