//! **One agent, one function** — the six that are not Claude Code.
//!
//! Codex, droid, antigravity, copilot, grok, reasonix. They expose the SAME
//! lifecycle events and the same stdin/stdout contract, so `cmd_hookwrap*` is
//! reused VERBATIM: only the configuration file and the group's schema change.
//!
//! Hence the temptation of a single parameterised function — refused. What
//! differs is not a parameter, it is a SHAPE: Codex adds a regex `matcher`,
//! antigravity names its hooks, copilot has its own document. A function that
//! unified them would pass these differences off as detail, and that is
//! exactly where an overly broad matcher slips in.

use super::*;

/// Wire tokenade's passive hooks into Codex's `~/.codex/hooks.json`.
///
/// Recent Codex CLI versions expose the SAME lifecycle hooks as Claude Code —
/// SessionStart / PreToolUse / PostToolUse / UserPromptSubmit / PreCompact —
/// with the same stdin JSON (`hook_event_name`, `tool_name`, `tool_input`,
/// `tool_response`, `cwd`, …) AND the same stdout contract
/// (`hookSpecificOutput.additionalContext`; PreToolUse `permissionDecision:
/// allow` + `updatedInput.command`). So `cmd_hookwrap*` is reused VERBATIM —
/// only the config file and the group schema (Codex adds a regex `matcher`)
/// differ from the Claude `settings.json` path. Idempotent via the `_tag`
/// marker; merges with any user-authored hooks already in the file.
///
/// Codex's tool surface is narrower than Claude's (no first-class `Read`/
/// `WebFetch`/`Task` tools — it reads via Bash and edits via `apply_patch`),
/// so the matchers target `Bash`; the per-call handlers no-op on anything
/// else, exactly as on Claude.
pub(crate) fn install_codex_passive_hooks(
    exe: &str,
    debug_log_path: Option<&str>,
    home: Option<&Path>,
    dry_run: bool,
) -> HookOutcome {
    let Some(h) = home else {
        return HookOutcome::Failed("no home dir resolved for codex hooks".into());
    };
    // (event, matcher, subcommand, tag) — mirrors the Claude set.
    let specs = [
        (
            "SessionStart",
            "startup|resume|compact|clear",
            "hookwrap-sessionstart",
            "tokenade-sessionstart",
        ),
        (
            "UserPromptSubmit",
            "*",
            "hookwrap-userprompt",
            "tokenade-userprompt",
        ),
        ("PreToolUse", "Bash", "hookwrap-pretool", "tokenade-pretool"),
        ("PostToolUse", "Bash", "hookwrap", "tokenade-posttool"),
        (
            "PreCompact",
            "*",
            "hookwrap-precompact",
            "tokenade-precompact",
        ),
        // Codex knows `SubagentStart`, and tokenade already had the
        // handler: it is registered on Copilot and on Grok. Declared on
        // Codex's OWN shipped schema, not on inference — the binary carries
        // `subagent-start.command.input`/`.output` in full, and
        // `SubagentStartHookSpecificOutputWire` accepts `additionalContext`.
        // Without this line a Codex subagent starts knowing nothing about
        // tokenade, which is the one moment the brief exists to cover.
        (
            "SubagentStart",
            "*",
            "hookwrap-subagentstart",
            "tokenade-subagentstart",
        ),
    ];
    install_lifecycle_hooks_json(
        exe,
        debug_log_path,
        h.join(".codex").join("hooks.json"),
        &specs,
        dry_run,
    )
}

/// Wire tokenade's hooks into Factory Droid CLI's `~/.factory/hooks.json`.
/// Droid's hook system is Claude-Code-compatible (verified against
/// factory.ai hooks-guide + hooks-reference, 2026-07): same events, same
/// snake_case stdin payload (`hook_event_name`, `tool_name`, `tool_input`),
/// same `hookSpecificOutput.updatedInput` rewrite contract on PreToolUse.
/// One delta: the shell tool is named `Execute` (not `Bash`) — the matcher
/// AND the pretool/posttool handlers both account for it.
pub(crate) fn install_droid_hooks(
    exe: &str,
    debug_log_path: Option<&str>,
    home: Option<&Path>,
    dry_run: bool,
) -> HookOutcome {
    let Some(h) = home else {
        return HookOutcome::Failed("no home dir resolved for droid hooks".into());
    };
    let specs = [
        (
            "SessionStart",
            "startup|resume|compact|clear",
            "hookwrap-sessionstart",
            "tokenade-sessionstart",
        ),
        (
            "UserPromptSubmit",
            "*",
            "hookwrap-userprompt",
            "tokenade-userprompt",
        ),
        (
            "PreToolUse",
            "Execute",
            "hookwrap-pretool",
            "tokenade-pretool",
        ),
        ("PostToolUse", "Execute", "hookwrap", "tokenade-posttool"),
        (
            "PreCompact",
            "*",
            "hookwrap-precompact",
            "tokenade-precompact",
        ),
    ];
    install_lifecycle_hooks_json(
        exe,
        debug_log_path,
        h.join(".factory").join("hooks.json"),
        &specs,
        dry_run,
    )
}

/// Wire tokenade's hooks into GitHub Copilot CLI via a dedicated hook file
/// `~/.copilot/hooks/tokenade.json`. Copilot CLI merges `hooks/*.json`
/// (docs.github.com hooks-configuration); PascalCase event names opt into
/// Claude-compatible payloads, matcher semantics AND the
/// `hookSpecificOutput.updatedInput` rewrite (honored since v1.0.24) — so
/// the identical `hookwrap*` subcommands are reused. Older Copilot versions
/// ignore unknown event spellings, so the file is inert there. Being a
/// tokenade-owned file, idempotence = overwrite and uninstall = delete.
/// Antigravity CLI (`agy`) lifecycle hooks — `~/.gemini/config/hooks.json`.
///
/// This agent was filed "appel manuel" in the matrix, which was a statement
/// about our install and not about the host: `agy` has a native hook system
/// and nothing here was wired into it.
///
/// Every fact below is MEASURED, and two of them contradict what a family
/// resemblance would have produced:
///
/// * **The file is a map of NAMED bundles at the root**, not the Claude
///   `{ "hooks": { "<Event>": [...] } }` envelope. From `agy`'s own reference
///   text: "The `hooks.json` file is a JSON object where each top-level key is
///   a **hook name**, mapping to its event configuration." So our entry is one
///   named key, which is also what makes uninstall surgical.
/// * **Tool-scoped events are GROUPED and the others are FLAT.** `PreToolUse`
///   and `PostToolUse` take `{matcher, hooks:[…]}`; `PreInvocation`,
///   `PostInvocation` and `Stop` take handler objects directly. Same file,
///   two shapes.
/// * The matcher is a regex over the tool name, and the tool names are "derived
///   by lowercasing the step type and removing the `CORTEX_STEP_TYPE_` prefix"
///   — so the shell tool is `run_command`, not `Bash`.
///
/// Only `PreToolUse` is written, and that is a measurement too: `PostToolUse`
/// "expects an empty JSON object `{}`" — it can observe a result and it cannot
/// substitute one. The rewrite seam is `PreToolUse`'s `overwrite`, "merged into
/// the tool call's arguments before it runs … The modified tool call is what
/// actually executes and is recorded". Writing a PostToolUse entry here would
/// cost a synchronous round trip per tool call and fold nothing, on a host
/// whose own note says "Hooks run synchronously and block the agent loop".
///
/// **The path was probed, not read off a third party.** `agy` loads a
/// `hooks.json` from BOTH `~/.gemini/hooks.json` and `~/.gemini/config/`
/// (planted one in each of four candidate roots, 2026-08-25, and read
/// `hooks_manager.go: loaded N named hooks from N hooks.json file(s)` back out
/// of its log). `~/.gemini/` itself is Gemini CLI's root — its `settings.json`
/// and `GEMINI.md` live there and tokenade already writes into them — so the
/// entry goes in `config/`, the directory that belongs to this CLI alone. A
/// green cell on another agent's file is the trap this bench has already been
/// caught by.
///
/// The directory must already exist: it is the proof `agy` has run.
pub(crate) fn install_antigravity_cli_hooks(
    exe: &str,
    debug_log_path: Option<&str>,
    home: Option<&Path>,
    dry_run: bool,
) -> HookOutcome {
    let Some(h) = home else {
        return HookOutcome::Failed("no home dir resolved for antigravity-cli hooks".into());
    };
    let dir = h.join(".gemini").join("config");
    let path = dir.join("hooks.json");
    // The directory is CREATED, and that is the guard
    // `declared_channel_matches_what_the_installer_writes` talking. It was
    // gated on `dir.is_dir()` first — "agy has never run here" — and the guard
    // failed at once: `Integration` declares a shell hook for this agent, so an
    // installer that can decline to write one makes the declaration and the
    // write disagree, which is the exact drift that guard exists to catch.
    //
    // Creating it is right on the merits too. This installer runs only for a
    // DETECTED Antigravity (seed `~/.gemini/antigravity/`), so the product is
    // on the machine; `~/.gemini/config/` is its own config root, beside the
    // `mcp_config.json` tokenade already writes there. An install that lands
    // before the CLI's first launch is then already in place — the opposite of
    // scattering a config for a product nobody has.
    if dry_run {
        return HookOutcome::Installed(path);
    }
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return HookOutcome::Failed(format!("mkdir {}: {e}", dir.display()));
    }
    let entry = serde_json::json!({
        "PreToolUse": [{
            // Regex over the tool name. Narrow on purpose: `PreToolUse` is
            // synchronous and blocks the loop, and the only step we can improve
            // is the shell one.
            "matcher": "run_command",
            "hooks": [{
                "type": "command",
                "command": build_hook_command(exe, "hookwrap-pretool", debug_log_path),
                "timeout": 60
            }]
        }]
    });
    // A file of OTHER people's named bundles is theirs: merge one key, never
    // replace the document.
    let mut root = match crate::install::json_util::read_json_object_or_bail(&path) {
        Ok(v) => v,
        Err(e) => return HookOutcome::Failed(e),
    };
    let Some(obj) = root.as_object_mut() else {
        return HookOutcome::Failed(format!("{} is not a JSON object", path.display()));
    };
    let existed = obj.get(ANTIGRAVITY_HOOK_NAME) == Some(&entry);
    if existed {
        return HookOutcome::AlreadyPresent;
    }
    let refreshed = obj.contains_key(ANTIGRAVITY_HOOK_NAME);
    obj.insert(ANTIGRAVITY_HOOK_NAME.to_string(), entry);
    if let Err(e) = crate::write_json_atomic(
        &path,
        &serde_json::to_string_pretty(&root).unwrap_or_default(),
    ) {
        return HookOutcome::Failed(format!("write {}: {e}", path.display()));
    }
    if refreshed {
        HookOutcome::Refreshed(path)
    } else {
        HookOutcome::Installed(path)
    }
}

/// Our one named bundle in Antigravity CLI's `hooks.json`. Uninstall removes
/// this key and leaves every other bundle alone.
pub(crate) const ANTIGRAVITY_HOOK_NAME: &str = "tokenade";

pub(crate) fn install_copilot_hooks(
    exe: &str,
    debug_log_path: Option<&str>,
    home: Option<&Path>,
    dry_run: bool,
) -> HookOutcome {
    let Some(h) = home else {
        return HookOutcome::Failed("no home dir resolved for copilot hooks".into());
    };
    let dir = h.join(".copilot").join("hooks");
    let path = dir.join("tokenade.json");
    // NATIVE Copilot config (docs.github.com hooks-reference): a command
    // `postToolUse` hook whose `bash` script folds the tool result and returns
    // `modifiedResult` — the one non-Claude seam that REPLACES what the model
    // sees. Matcher is `.*` (ALL tools): live capture showed Copilot tests the
    // matcher against the lowercase RUNTIME tool name (`bash`/`view`/`grep`),
    // NOT the documented Claude name — enumerating names is fragile and misses
    // `view`. The handler folds only large, foldable results (errors/small
    // pass through untouched), so matching everything is safe and catches any
    // big result regardless of which tool produced it (bash, MCP, web, …).
    // `postToolUse` fires only on SUCCESS, so no error result is ever touched.
    // `bash` (not `command`) is the command-hook field in Copilot's schema.
    //
    // `SessionStart` and `UserPromptSubmit` are written in the CLAUDE spelling
    // on purpose, and it is not a guess: Copilot's own config loader walks a
    // PascalCase→native map (`{SessionStart:"sessionStart",
    // UserPromptSubmit:"userPromptSubmitted", …}`), moves the entry under the
    // native key and stamps `_vsCodeCompat` itself. So the compat payload and
    // the `hookSpecificOutput` response shape come with the spelling, and the
    // same `hookwrap-sessionstart` / `hookwrap-userprompt` handlers the Claude
    // family uses are reachable here unchanged.
    //
    // Read off the shipped `app.js` (`@github/copilot`), whose native event
    // set is `sessionStart, sessionEnd, userPromptSubmitted, preToolUse,
    // preMcpToolCall, postToolUse, postToolUseFailure, …, preCompact,
    // permissionRequest, notification`. tokenade was registering ONE of them,
    // and the compat bench reported that as "no prompt-time hook exists for
    // this agent" — a statement about our install, taken for one about the
    // host.
    //
    // No `matcher` on the two non-tool events: Copilot tests it against the
    // runtime tool name, which these events do not carry.
    let doc = serde_json::json!({
        "version": 1,
        "hooks": {
            "postToolUse": [{
                "type": "command",
                "matcher": ".*",
                "bash": build_hook_command(exe, "hookwrap-copilot", debug_log_path),
                "_tag": "tokenade-copilot-posttool"
            }],
            // The command REWRITE, which Copilot users did not have.
            // `install_cli.rs` said "Claude Code only — other agents don't
            // expose a comparable Pre hook today", and that stopped being true
            // for this host: every link is read off the shipped `app.js`.
            //
            //  1. `PreToolUse:"preToolUse"` is in the PascalCase compat map, so
            //     the entry is moved under the native key and `_vsCodeCompat`
            //     is stamped by Copilot itself — the same mechanism the two
            //     lifecycle events above already rely on.
            //  2. Stamped, the payload comes from `S8s(t,e)`, which spreads
            //     `X$(t,e)` and adds `tool_name: e.toolName,
            //     tool_input: Xgr(e.toolArgs)` — the snake_case shape
            //     `hookwrap-pretool` already reads.
            //  3. The response is read as
            //     `c?.updatedInput ?? l?.modifiedArgs` where
            //     `c = _vsCodeCompat ? l?.hookSpecificOutput : undefined`, and
            //     `permissionDecision` beside it. That is the exact
            //     allow+`updatedInput` channel this binary speaks to Claude.
            //
            // Matcher `bash`, not `.*`: Copilot tests it against the lowercase
            // RUNTIME tool name (the live capture recorded above), and `bash`
            // is the only one this handler has a measured contract for here.
            // `.*` would route `view`, `grep` and every MCP call into branches
            // never exercised on this host — a wider change than the one that
            // was measured.
            "PreToolUse": [{
                "type": "command",
                "matcher": "bash",
                "bash": build_hook_command(exe, "hookwrap-pretool", debug_log_path),
                "_tag": "tokenade-copilot-pretool"
            }],
            "UserPromptSubmit": [{
                "type": "command",
                "bash": build_hook_command(exe, "hookwrap-userprompt", debug_log_path),
                "_tag": "tokenade-copilot-userprompt"
            }],
            "SessionStart": [{
                "type": "command",
                "bash": build_hook_command(exe, "hookwrap-sessionstart", debug_log_path),
                "_tag": "tokenade-copilot-sessionstart"
            }],
            // The compaction write-barrier, which Copilot users did not
            // have. `hookwrap-precompact` protects the recovery pointers the
            // summary is about to drop, voids pending replays, prunes what the
            // summariser is about to read, and writes the per-session manifest
            // the agent consults afterwards. tokenade installs it on the Claude
            // family and registered it nowhere else, on a host whose own event
            // set has always carried the seam.
            //
            // Read off the shipped `app.js` (`@github/copilot`), not the docs:
            // `preCompact` is in the native set, `PreCompact:"preCompact"` is in
            // the PascalCase compat map, and the payload is
            // `{sessionId, timestamp, cwd, transcriptPath, trigger,
            // customInstructions}` — the same spelling the two lifecycle hooks
            // above already receive and read. The handler tolerates a missing
            // transcript by design ("some agent dialects omit it"), so the
            // session-scoped half runs either way.
            "PreCompact": [{
                "type": "command",
                "bash": build_hook_command(exe, "hookwrap-precompact", debug_log_path),
                "_tag": "tokenade-copilot-precompact"
            }],
            // Copilot spawns SUBAGENTS (`search_code_subagent` and the
            // session-based ones), and until this entry existed they inherited
            // the folding and none of the instructions. `onSubagentStart`'s
            // `additionalContext` is joined into the child's prompt at all
            // three call sites in the shipped `app.js`. No `matcher`: the
            // event carries an agent name, not a tool name.
            "subagentStart": [{
                "type": "command",
                "bash": build_hook_command(exe, "hookwrap-subagentstart", debug_log_path),
                "_tag": "tokenade-copilot-subagentstart"
            }]
        }
    });
    let rendered = serde_json::to_string_pretty(&doc).unwrap_or_default() + "\n";
    if path.is_file() && fs::read_to_string(&path).is_ok_and(|c| c == rendered) {
        return HookOutcome::AlreadyPresent;
    }
    if dry_run {
        return HookOutcome::Installed(path);
    }
    if let Err(e) = fs::create_dir_all(&dir) {
        return HookOutcome::Failed(format!("create {}: {e}", dir.display()));
    }
    match fs::write(&path, rendered) {
        Ok(()) => HookOutcome::Installed(path),
        Err(e) => HookOutcome::Failed(format!("write {}: {e}", path.display())),
    }
}

/// Install tokenade's lifecycle hooks for xAI Grok Build CLI.
///
/// **This wrote to a file Grok never reads, and had done for a long time.**
/// The target was `~/.grok/user-settings.json`; Grok discovers hooks from
/// `~/.grok/hooks/*.json` (plus `~/.claude/settings.json` and
/// `~/.cursor/hooks.json` for vendor compatibility, and its own TOML layers).
/// Measured 2026-09-11 on grok 1.0.0 against a throwaway home: the identical
/// document at the old path gave `grok inspect` → **Hooks (0)**, and at
/// `~/.grok/hooks/tokenade.json` → **Hooks (6)**. Nothing about the JSON was
/// wrong; the address was. Same shape as Copilot's
/// `~/.copilot/hooks/tokenade.json`, which is why that agent worked.
///
/// The SET shrank at the same time, and that is the harder half. Read off the
/// shipped `~/.grok/docs/user-guide/10-hooks.md` and confirmed against the
/// binary's own strings:
///
/// * "For events like `SessionStart` or `PostToolUse`, stdout is ignored."
/// * `PreToolUse` output is `{"decision":"allow"}` / `{"decision":"deny",
///   "reason":…}` — nothing else.
/// * `Stop`/`SubagentStop` alone accept `hookSpecificOutput.additionalContext`.
/// * The 163 MB binary contains **zero** occurrences of `updatedToolOutput`,
///   `updated_tool_output`, `updatedInput` or `modifiedResult`.
///
/// So on grok 1.0.0 no hook can replace a tool's output, rewrite a command, or
/// inject context outside the stop gate. A `PostToolUse` entry there would
/// spawn a process per tool call, compact an output nobody receives, and book
/// the saving — exactly the shape of a promise that is not true on the path
/// that emits it. `PreToolUse`, `UserPromptSubmit` and `SubagentStart` are the
/// same story with different vocabulary.
///
/// What is left is what grok can honour: the two events we run for their SIDE
/// EFFECTS. `SessionStart` tags this session's gain rows (the shell proxy does
/// work here, so those rows are real) and runs the retention pass;
/// `PreCompact` raises the write barrier that flushes recovery pointers before
/// the summariser drops them. Neither prints anything grok needs to read.
///
/// Two smaller corrections ride along. The matcher is a **regular expression**
/// tested against the REAL tool name, so the old `"bash"` matched neither
/// `Bash` nor grok's own `run_terminal_command`; and `"*"` is not a regex at
/// all, where the documented way to match everything is to leave it empty.
pub(crate) fn install_grok_hooks(
    exe: &str,
    debug_log_path: Option<&str>,
    home: Option<&Path>,
    dry_run: bool,
) -> HookOutcome {
    let Some(h) = home else {
        return HookOutcome::Failed("no home dir resolved for grok hooks".into());
    };
    // Back-compat: every install before that fix laid these entries in
    // `user-settings.json`, where they are inert. Leaving them there would
    // keep a dead copy of our own configuration in the user's settings file
    // for ever, and `uninstall` would be the only thing that ever noticed.
    if !dry_run {
        let _ = crate::install::strip_tokenade_entries_from_json_hooks(
            &h.join(".grok").join("user-settings.json"),
            false,
        );
    }
    let specs = [
        (
            "SessionStart",
            "startup|resume|compact|clear",
            "hookwrap-sessionstart",
            "tokenade-sessionstart",
        ),
        // No matcher: `PreCompact`'s matcher tests the compaction TRIGGER
        // (`manual` / `auto`), and we want both. Empty is the documented way
        // to say so — `"*"` is a regex parse error dressed as a wildcard.
        (
            "PreCompact",
            "",
            "hookwrap-precompact",
            "tokenade-precompact",
        ),
    ];
    install_lifecycle_hooks_json(
        exe,
        debug_log_path,
        h.join(".grok").join("hooks").join("tokenade.json"),
        &specs,
        dry_run,
    )
}

/// `<Reasonix home>/settings.json` — Reasonix has left the product; this path
/// now only serves `uninstall`, which removes our entries from an older
/// install. Reasonix home is `~/.reasonix` on
/// Linux/macOS and `%APPDATA%\reasonix` (→ `AppData/Roaming/reasonix` relative
/// to the resolved home) on Windows (docs/CONFIG_PATHS.md).
pub(crate) fn reasonix_settings_path(home: &Path) -> PathBuf {
    if cfg!(windows) {
        home.join("AppData")
            .join("Roaming")
            .join("reasonix")
            .join("settings.json")
    } else {
        home.join(".reasonix").join("settings.json")
    }
}
