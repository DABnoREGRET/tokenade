//! End-to-end tests driving `tokenade install`.

use super::*;

#[test]
fn e2e_help_lists_known_subcommands() {
    let h = TempHome::new("help");
    let (stdout, _err, code) = run(&["help"], h.path());
    assert_eq!(code, 0);
    // Smoke check on a few canonical subcommands — full listing
    // changes regularly, but these have been stable since 0.1.
    for needle in ["dashboard", "install", "wrap"] {
        assert!(
            stdout.contains(needle),
            "help missing `{needle}` — got:\n{stdout}"
        );
    }
}

/// #552 — `tokenade uninstall` must strip the fenced tokenade
/// block from `~/.claude/CLAUDE.md`. Before the fix the block was
/// leaked forever, leaving a permanent ad in the user's global
/// agent config even after the binary was uninstalled.
#[test]
fn e2e_uninstall_strips_global_claude_md_block() {
    let h = TempHome::new("uninstall-strip");
    // Seed ~/.claude/CLAUDE.md with USER content surrounding the
    // tokenade block so we can verify the user's stuff survives.
    let claude_dir = h.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let claude_md = claude_dir.join("CLAUDE.md");
    let content = "\
# My personal rules

- Always run cargo fmt.
- Don't use unwrap() in production.

<!-- tokenade:start -->
## Tokenade is installed

The `tokenade` MCP server is available globally. Prefer these tools…

- `exec_compact <cmd>` instead of raw `Bash <cmd>` …
<!-- tokenade:end -->

# After-tokenade rules

Keep this section.
";
    std::fs::write(&claude_md, content).unwrap();

    let (out, _err, code) = run_copied(&["uninstall"], h.path());
    assert_eq!(code, 0, "uninstall must succeed");
    assert!(
        out.contains("stripped") && out.contains("CLAUDE.md"),
        "uninstall must announce the strip; got:\n{out}"
    );

    let after = std::fs::read_to_string(&claude_md).unwrap();
    assert!(
        !after.contains("<!-- tokenade:start -->"),
        "start fence must be gone: {after}"
    );
    assert!(
        !after.contains("<!-- tokenade:end -->"),
        "end fence must be gone: {after}"
    );
    assert!(
        !after.contains("Tokenade is installed"),
        "block body must be gone: {after}"
    );
    // User content on both sides survives.
    assert!(after.contains("Always run cargo fmt."));
    assert!(after.contains("After-tokenade rules"));
    assert!(after.contains("Keep this section."));

    // Backup was taken.
    let backup_exists = std::fs::read_dir(&claude_dir).unwrap().any(|e| {
        e.ok()
            .map(|e| {
                e.file_name()
                    .to_string_lossy()
                    .contains("CLAUDE.md.tokenade-backup-")
            })
            .unwrap_or(false)
    });
    assert!(
        backup_exists,
        "a CLAUDE.md.tokenade-backup-<ts> must be created"
    );

    // Re-running uninstall is a no-op (idempotent).
    let (out2, _, code2) = run_copied(&["uninstall"], h.path());
    assert_eq!(code2, 0);
    assert!(
        !out2.contains("stripped"),
        "second uninstall must not claim to strip again; got:\n{out2}"
    );
}

/// #556 — `TOKENADE_NO_HOOK=1` must opt out of EVERY hook installer,
/// including the LSP enforcement hook (which used to ignore it).
/// Run `tokenade install --with-lsp-enforcement-hook --yes --only
/// claude-code` with the env var set; assert NO PreToolUse hook
/// gets written to settings.json.
#[test]
fn e2e_tokenade_no_hook_opts_out_of_lsp_enforcement() {
    let h = TempHome::new("no-hook-env");
    // Claude Code marker so install detects an agent.
    let claude_dir = h.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(claude_dir.join("settings.json"), r#"{"keep_me":true}"#).unwrap();

    let bin = binary();
    let out = Command::new(&bin)
        .args([
            "install",
            "--with-lsp-enforcement-hook",
            "--yes",
            "--only",
            "claude-code",
        ])
        .env_remove("TOKENADE_DEBUG_LOG")
        .env("HOME", h.path())
        .current_dir(h.path())
        .env("USERPROFILE", h.path())
        .env("TOKENADE_HOOK_DISABLED", "1")
        .env("TOKENADE_NO_HOOK", "1")
        .output()
        .expect("spawn tokenade install");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code().unwrap_or(-1),
        0,
        "install failed: {stdout}"
    );
    let settings: serde_json::Value = std::fs::read_to_string(claude_dir.join("settings.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    // The user content must still be there.
    assert_eq!(settings["keep_me"], serde_json::json!(true));
    // No hooks of any matcher should have been added — TOKENADE_NO_HOOK
    // must opt out uniformly.
    let pre = &settings["hooks"]["PreToolUse"];
    let post = &settings["hooks"]["PostToolUse"];
    assert!(
        pre.is_null() || pre.as_array().map(|a| a.is_empty()).unwrap_or(true),
        "PreToolUse hook should not have been installed with TOKENADE_NO_HOOK=1, got: {pre}"
    );
    assert!(
        post.is_null() || post.as_array().map(|a| a.is_empty()).unwrap_or(true),
        "PostToolUse hook should not have been installed with TOKENADE_NO_HOOK=1, got: {post}"
    );
    // And the install output should mention the opt-out so the user knows.
    assert!(
        stdout.contains("TOKENADE_NO_HOOK"),
        "install must surface that TOKENADE_NO_HOOK skipped the hooks; got:\n{stdout}"
    );
}

/// #560 — when ~/.claude/CLAUDE.md already has a tokenade fence
/// pair but the content between them is stale (didn't match the
/// current GLOBAL_CLAUDE_MD_BLOCK), `tokenade install` must splice
/// the new block in place — not silently leave the stale version.
/// Before the fix, install short-circuited on the first start fence,
/// leaving users stuck with whatever block existed at first-install
/// time even when later tokenade versions refreshed the prose.
#[test]
fn e2e_install_refreshes_stale_global_claude_md_block() {
    let h = TempHome::new("global-refresh");
    let claude_dir = h.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(claude_dir.join("settings.json"), r#"{"keep_me":true}"#).unwrap();
    let claude_md = claude_dir.join("CLAUDE.md");
    // Pre-seed with a stale tokenade block (uses the real fences but a
    // body that no current tokenade release ships).
    let stale = "\
# My personal rules

- Always run cargo fmt.

<!-- tokenade:start -->
## OBSOLETE STALE BLOCK CONTENT FROM 2025

This block refers to retired tools `memory_set`, `wiki_get`, etc.
<!-- tokenade:end -->

# After

Keep me.
";
    std::fs::write(&claude_md, stale).unwrap();

    let (out, _err, code) = run(
        &[
            "install",
            "--with-claude-md",
            "--yes",
            "--only",
            "claude-code",
        ],
        h.path(),
    );
    assert_eq!(code, 0, "install failed: {out}");
    let after = std::fs::read_to_string(&claude_md).unwrap();
    // Stale body is gone.
    assert!(
        !after.contains("OBSOLETE STALE BLOCK CONTENT"),
        "stale block body must be replaced: {after}"
    );
    assert!(
        !after.contains("memory_set"),
        "retired tool refs must be gone: {after}"
    );
    // The short install fence is gone: the CLI lecture itself now lives in
    // this global file, spliced under the scaffold marker — a fence saying
    // « the rules are elsewhere » next to the rules was a second copy.
    assert!(
        !after.contains("Tokenade is installed"),
        "the short fence must not sit beside the lecture: {after}"
    );
    assert!(
        after.contains("tokenade map"),
        "Claude's global file must carry the CLI lecture: {after}"
    );
    // User content on both sides survives.
    assert!(after.contains("Always run cargo fmt."));
    assert!(after.contains("Keep me."));
    // Backup was taken (the refresh path always takes one before rewriting).
    let backup_exists = std::fs::read_dir(&claude_dir).unwrap().any(|e| {
        e.ok()
            .map(|e| {
                e.file_name()
                    .to_string_lossy()
                    .contains("CLAUDE.md.tokenade-backup-")
            })
            .unwrap_or(false)
    });
    assert!(
        backup_exists,
        "refresh must take a .tokenade-backup-<ts> copy first"
    );

    // Idempotent: a second install with the same content must NOT
    // refresh again — there's nothing stale anymore.
    let (out2, _, code2) = run(
        &[
            "install",
            "--with-claude-md",
            "--yes",
            "--only",
            "claude-code",
        ],
        h.path(),
    );
    assert_eq!(code2, 0);
    assert!(
        !out2.contains("refreshed tokenade block"),
        "second install must be a no-op for the global block; got:\n{out2}"
    );
}

#[test]
fn e2e_detect_competitors_returns_clean_on_empty_home() {
    let h = TempHome::new("detect-comp");
    let (_out, _err, code) = run(&["detect-competitors"], h.path());
    // Empty HOME → no competitors installed → graceful exit.
    assert_eq!(code, 0);
}

/// **Where a hook already folds, the proxy must step aside — and only there.**
///
/// Cursor carried both for one version: the `postToolUse` hook, which folds an
/// MCP result, AND the `mcp-proxy` wrapper around the same server. Both fold
/// the same bytes, which had been found earlier with Gemini. The step-back is
/// conditioned on a capability MEASURED on the installed bundle, so this test
/// forces it in both directions: it is the only way to hold both halves on a
/// machine that has only one.
///
/// The NEGATIVE direction is the more important of the two. Being wrong by
/// saying "the hook covers" does not weaken the fold: it switches it off.
#[test]
fn e2e_the_mcp_proxy_steps_back_exactly_where_the_cursor_hook_covers() {
    fn seed(tag: &str) -> TempHome {
        let h = TempHome::new(tag);
        let d = h.path().join(".cursor");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("mcp.json"),
            r#"{"mcpServers":{"demo":{"command":"node","args":["server.mjs"]}}}"#,
        )
        .unwrap();
        h
    }
    fn demo_command(h: &TempHome) -> String {
        let raw = std::fs::read_to_string(h.path().join(".cursor").join("mcp.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        v["mcpServers"]["demo"]["command"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    // Capable: the hook carries the fold, so the config must come back exactly
    // as the user wrote it.
    let covered = seed("cursor-mcp-covered");
    run_with_env(
        &["install", "--only", "cursor"],
        covered.path(),
        &[("TOKENADE_CAP_CURSOR_MCP_POST", "1")],
    );
    assert_eq!(
        demo_command(&covered),
        "node",
        "the hook folds this result; wrapping it as well folds the same bytes twice"
    );

    // Not capable: nothing else would fold it, so the wrapper stays.
    let bare = seed("cursor-mcp-bare");
    run_with_env(
        &["install", "--only", "cursor"],
        bare.path(),
        &[("TOKENADE_CAP_CURSOR_MCP_POST", "0")],
    );
    assert!(
        demo_command(&bare).contains("tokenade"),
        "without the hook capability the proxy is the only fold left — it must stay, got {:?}",
        demo_command(&bare)
    );
}

/// End-to-end invariant for `install --dry-run`: across the full
/// agent-detection, MCP-config-write, Claude-Code-hook, scaffold,
/// and wrap-mcps pipeline, NOTHING may land on disk inside the
/// per-test HOME.
///
/// Captures regressions for the entire dry-run sweep
/// (iterations 416 / 431 / 432 / 433 / 434 / 435).
///
/// Strategy: seed HOME with a Claude Code marker so the installer
/// detects an agent and exercises the hook installer; seed an
/// existing codex config with a third-party MCP entry to exercise
/// the wrap_one_config path (the exact pattern that 435 caught);
/// snapshot every file under HOME; run install --dry-run --yes;
/// re-snapshot; assert the snapshots match.
#[test]
fn e2e_install_dry_run_does_not_modify_home() {
    use std::collections::BTreeMap;

    let h = TempHome::new("install-dry");
    // (1) Claude Code marker.
    let claude_dir = h.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(claude_dir.join("settings.json"), r#"{"keep_me":true}"#).unwrap();
    // (2) Codex config with a third-party MCP entry (unwrapped).
    let codex_dir = h.path().join(".codex");
    std::fs::create_dir_all(&codex_dir).unwrap();
    let codex_cfg = codex_dir.join("config.json");
    let codex_content = r#"{"mcpServers":{"supertokensaver":{"command":"sts","args":["mcp"]}}}"#;
    std::fs::write(&codex_cfg, codex_content).unwrap();

    // Snapshot helper: walks HOME and records (rel_path -> (size, content_hash)).
    fn snapshot(root: &Path) -> BTreeMap<String, (u64, u64)> {
        let mut out = BTreeMap::new();
        fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, (u64, u64)>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let p = entry.path();
                if let Ok(meta) = p.metadata() {
                    let rel = p.strip_prefix(root).unwrap_or(&p).display().to_string();
                    if meta.is_dir() {
                        walk(root, &p, out);
                    } else {
                        let bytes = std::fs::read(&p).unwrap_or_default();
                        // Cheap content hash via DJB2.
                        let mut h: u64 = 5381;
                        for b in &bytes {
                            h = h.wrapping_mul(33).wrapping_add(*b as u64);
                        }
                        out.insert(rel, (meta.len(), h));
                    }
                }
            }
        }
        walk(root, root, &mut out);
        out
    }

    let before = snapshot(h.path());

    // (4) Run install --dry-run with --yes (no prompts) + --no-cleanup
    // (we don't want competitor scans wandering off into the real
    // system); --with-claude-md to exercise the scaffold gate.
    let (_out, _err, code) = run(
        &[
            "install",
            "--dry-run",
            "--yes",
            "--no-cleanup",
            "--with-claude-md",
            "--with-lsp-enforcement-hook",
        ],
        h.path(),
    );
    assert_eq!(code, 0, "install --dry-run should exit 0");

    let after = snapshot(h.path());

    // (5) Snapshots must match exactly. Any divergence — new file,
    // mutated size, content hash change — is a dry-run leak.
    assert_eq!(
        before, after,
        "install --dry-run mutated HOME — diff: \nbefore: {:?}\nafter: {:?}",
        before, after
    );
    // Sanity: the third-party MCP entry is still unwrapped (otherwise
    // the hash check above would already fail, but make the regression
    // crystal clear).
    let codex_after = std::fs::read_to_string(&codex_cfg).unwrap();
    assert_eq!(
        codex_after, codex_content,
        "third-party MCP entry was rewritten under --dry-run"
    );
}

/// Zero-MCP migration: a user upgrading from a pre-pivot tokenade had a
/// `tokenade` MCP self-entry spawning the now-removed `tokenade mcp`
/// command. `tokenade install` must PURGE that stale entry on every
/// scope/agent so Claude Code stops trying to spawn a binary that exits
/// "unknown command". Third-party MCP servers must survive untouched.
///
/// Covers both the user-scope store (`~/.claude.json`) and a
/// project-scope config (`<root>/.mcp.json`). This is the end-to-end
/// proof of the MCP→non-MCP migration that the per-format unit tests in
/// `mcp_wrap.rs` only exercise in isolation.
#[test]
fn e2e_install_purges_stale_tokenade_mcp_entry() {
    let h = TempHome::new("mcp-migrate");
    let home = h.path();

    // A pre-pivot config: tokenade self-entry (dead) + a third-party
    // server (github) that MUST be preserved.
    let pre_pivot = r#"{
  "mcpServers": {
    "tokenade": { "command": "tokenade", "args": ["mcp"] },
    "github": { "command": "gh-mcp-server", "args": ["--stdio"] }
  }
}"#;
    let user_store = home.join(".claude.json");
    let project_cfg = home.join(".mcp.json");
    std::fs::write(&user_store, pre_pivot).unwrap();
    std::fs::write(&project_cfg, pre_pivot).unwrap();

    // Run the real installer, confined to the temp HOME *and* cwd so it
    // never touches the developer's workspace. Skip the competitor
    // cleanup and hook wiring — we're validating the MCP purge only.
    let bin = binary();
    let out = Command::new(&bin)
        .args(["install", "--no-hook", "--no-cleanup", "--no-wrap-mcps"])
        .current_dir(home)
        .env_remove("TOKENADE_DEBUG_LOG")
        .env("HOME", home)
        .env("USERPROFILE", home)
        .output()
        .expect("spawn tokenade install");
    assert_eq!(
        out.status.code().unwrap_or(-1),
        0,
        "install failed; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    for cfg in [&user_store, &project_cfg] {
        let after: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(cfg).unwrap())
            .unwrap_or_else(|e| panic!("{} no longer valid JSON: {e}", cfg.display()));
        let servers = after
            .get("mcpServers")
            .and_then(|v| v.as_object())
            .unwrap_or_else(|| panic!("{} lost its mcpServers map", cfg.display()));
        assert!(
            !servers.contains_key("tokenade"),
            "stale `tokenade` MCP entry survived migration in {}: {after}",
            cfg.display()
        );
        assert!(
            servers.contains_key("github"),
            "third-party server was wrongly removed from {}: {after}",
            cfg.display()
        );
    }
}

#[test]
fn a_format_name_answers_the_same_way_however_it_is_typed() {
    let home = TempHome::new("format-name-routes");
    for args in [
        vec!["csv"],
        vec!["csv", "--help"],
        vec!["yaml", "--help"],
        vec!["help", "csv"],
    ] {
        let (_out, err, code) = run(&args, home.path());
        assert_ne!(code, 0, "a format is not a command: {args:?}");
        assert!(
            err.contains("is a FORMAT, not a command"),
            "{args:?} answered: {err}"
        );
    }
    // A real typo must still get its suggestion, not a format lecture.
    let (_out, err, _code) = run(&["insall"], home.path());
    assert!(
        err.contains("install"),
        "a typo still gets corrected: {err}"
    );
}

#[test]
fn a_format_name_points_at_the_command_that_handles_it() {
    let home = TempHome::new("format-not-command");
    for name in ["csv", "logs", "yaml", "ndjson", "openapi"] {
        let (_out, err, code) = run(&[name, "/dev/null"], home.path());
        assert_eq!(code, 2, "{name} should still be a usage error");
        assert!(
            err.contains("tokenade read"),
            "{name} must point at the command that handles it, got: {err}"
        );
        assert!(
            !err.contains("Did you mean"),
            "{name} is not a typo, got: {err}"
        );
    }
    // A real typo still gets its suggestion.
    let (_out, err, _) = run(&["insall"], home.path());
    assert!(
        err.contains("Did you mean 'install'"),
        "typo suggestion lost: {err}"
    );
}

/// The `--with-hook` note must name the agent being INSTALLED FOR.
///
/// It printed `detect::detect(root)` — whichever agent is running the install
/// — so `tokenade install --only <agent>` from inside Claude Code announced
/// "the claude-code agent here has no compatible PostToolUse hook". Claude
/// Code is the one agent for which that is false, and also the likeliest thing
/// to be running an install, so the sentence was usually wrong twice.
///
/// The test panics if the note does NOT fire, rather than returning early. An
/// early return here would make it pass while asserting nothing — the failure
/// mode that four other tests written the same day actually hit.
#[cfg(unix)]
#[test]
fn e2e_the_with_hook_note_names_the_target_agent_not_the_ambient_one() {
    let h = TempHome::new("with-hook-note");
    std::fs::create_dir_all(h.path().join(".hermes")).unwrap();

    let (out, err, _code) = run(
        &["install", "--dry-run", "--only", "hermes", "--with-hook"],
        h.path(),
    );
    let all = format!("{out}{err}");
    // Located on `--with-hook` + `note:` rather than on a phrase from the
    // sentence. It was reworded once (the old text ended with "(<agent> has no
    // compatible PostToolUse hook)", a claim about the host that is false for
    // Copilot and Codex) and this locator went looking for words that no longer
    // existed — so the guard failed for a reason that had nothing to do with
    // the property it defends. The property is unchanged; the anchor was too
    // literal.
    let note = all
        .lines()
        .find(|l| l.contains("note:") && l.contains("--with-hook"))
        .unwrap_or_else(|| {
            panic!("the --with-hook note did not fire, so this test would assert nothing. Output:\n{all}")
        });
    assert!(
        note.contains("hermes"),
        "the note must name the install target, got:\n{note}"
    );
    assert!(
        !note.contains("claude-code"),
        "the note named the ambient agent instead of the target, and claimed \
         Claude Code has no PostToolUse hook:\n{note}"
    );
    // And it must not deny the target a hook tokenade registers for it.
    assert!(
        !note.contains("no compatible"),
        "the note denies the host a hook; tokenade registers PostToolUse for \
         Copilot and Codex, and PreToolUse for Copilot:\n{note}"
    );
}

/// An install run from a build directory must not wire hooks to that directory.
///
/// `stable_hook_exe` exists for this and its doc records the live incident:
/// "the dry-run dist binary self-healed the user's config to its own path",
/// after which a rebuild or `cargo clean` kills every hook and the statusline.
/// It was wired into the shim and the Cursor path and NOT into `cmd_install`,
/// so eight of the nine hook agents kept the bug it was written to fix.
///
/// This test is naturally in the failing configuration: the e2e harness runs
/// `CARGO_BIN_EXE_tokenade`, which lives under `target/`. Before the fix it
/// wrote that path into `~/.claude/settings.json`.
#[cfg(unix)]
#[test]
fn e2e_install_does_not_wire_hooks_to_a_build_directory() {
    // `stable_hook_exe` can only escape a build path if an installed tokenade
    // exists on PATH. Where none does (a pure dev box, some CI images) falling
    // back to the build path is the documented, correct behaviour — and this
    // test has nothing to say.
    let installed = std::process::Command::new("sh")
        .arg("-c")
        .arg("command -v tokenade")
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false);
    if !installed {
        eprintln!("SKIP: no installed `tokenade` on PATH — the build-dir fallback is correct here");
        return;
    }

    // `run_in`, not `run`: `run` leaves the child's cwd inherited from cargo —
    // i.e. the CRATE DIRECTORY — and `install` scaffolds its cwd. Three tests
    // here were therefore writing `CLAUDE.md` + `.tokenade/scaffold.flag` into
    // the working repository on every suite run, one of them a git-TRACKED
    // file. The temp HOME made it look contained; it never was, because
    // HOME and cwd are two different things.
    let h = TempHome::new("hook-exe-path");
    std::fs::create_dir_all(h.path().join(".claude")).unwrap();
    let (_out, _err, _code) = run_in(
        &["install", "--yes", "--only", "claude-code"],
        h.path(),
        h.path(),
    );

    let settings = std::fs::read_to_string(h.path().join(".claude").join("settings.json"))
        .unwrap_or_else(|e| panic!("install wrote no Claude Code settings: {e}"));
    assert!(
        settings.contains("tokenade"),
        "install wrote settings with no tokenade hook at all:\n{settings}"
    );
    assert!(
        !settings.contains("/target/") && !settings.contains("/dist/"),
        "the hook command points into a build directory — one `cargo clean` and \
         every hook dies:\n{settings}"
    );
}

/// Every subcommand the installed hook configs name must exist in the binary.
///
/// The configs are written by tokenade and read by the agent, so a renamed or
/// typo'd subcommand is invisible from both ends: `tokenade <unknown>` prints
/// its error and **exits 0** (the product's fail-open posture for hooks), so
/// the agent sees a hook that succeeded and did nothing — for the life of the
/// install.
///
/// Checked by execution, not against a hardcoded list: the list is exactly
/// what `install` just wrote.
#[cfg(unix)]
#[test]
fn e2e_every_hook_subcommand_in_the_written_configs_exists() {
    let mut checked = 0usize;
    for (agent, marker) in [
        ("claude-code", ".claude"),
        ("codex", ".codex"),
        ("droid", ".factory"),
        ("grok", ".grok"),
        ("cursor", ".cursor"),
    ] {
        let h = TempHome::new(&format!("hook-subs-{agent}"));
        std::fs::create_dir_all(h.path().join(marker)).unwrap();
        let (_o, _e, _c) = run_in(&["install", "--yes", "--only", agent], h.path(), h.path());

        let mut subs: Vec<String> = Vec::new();
        collect_tokenade_subcommands(h.path(), &mut subs);
        subs.sort();
        subs.dedup();
        for sub in &subs {
            let (out, err, code) = run_with_stdin(&[sub.as_str()], h.path(), "{}");
            assert_eq!(
                code, 0,
                "`tokenade {sub}` (wired for {agent}) exited {code}"
            );
            let all = format!("{out}{err}");
            assert!(
                !all.contains("unknown command"),
                "{agent}'s config names `tokenade {sub}`, which the binary does not have:\n{all}"
            );
            checked += 1;
        }
    }
    assert!(
        checked >= 10,
        "only {checked} hook subcommands were found across six agents — the \
         extraction stopped working, so this test is asserting nothing"
    );
}

/// Pull `<…/tokenade> <subcommand>` out of every config file under `home`.
///
/// Deliberately textual: the commands live inside `sh -c '…'` wrappers with
/// escaped quotes, and unwrapping those properly needs a shell parser. Taking
/// the next token and stripping quote/escape characters is enough to name the
/// subcommand, and anything it mis-reads gets caught by the run below rather
/// than silently skipped.
#[cfg(unix)] // only cfg(unix) tests use it (D5 cross-check)
fn collect_tokenade_subcommands(home: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(home) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name()
                .is_some_and(|n| n == ".tokenade" || n == ".npm")
            {
                continue;
            }
            collect_tokenade_subcommands(&p, out);
        } else if p.extension().is_some_and(|x| x == "json" || x == "toml") {
            let Ok(text) = std::fs::read_to_string(&p) else {
                continue;
            };
            for (i, _) in text.match_indices("tokenade") {
                let rest = &text[i + "tokenade".len()..];
                let token: String = rest
                    .trim_start_matches(['\'', '"', '\\', ' '])
                    .chars()
                    .take_while(|c| c.is_ascii_lowercase() || *c == '-')
                    .collect();
                // Real hook subcommands are these two families; anything else
                // in these files is prose, a key, or a path fragment.
                if token.starts_with("hookwrap") || token.starts_with("gemini-") {
                    out.push(token);
                }
            }
        }
    }
}

/// `mcp-proxy install` must not wire MCP servers to a build directory either.
///
/// An earlier fix covered `cmd_install`; `mcp_wrap::wrapper_command_string` is in another
/// crate and had its own naive `env::current_exe()`. Its doc even reasoned
/// that it should "match the install asymmetry: same absolute path everywhere"
/// — which was true, and the path `install` used was itself the bug. Measured
/// 2026-08-08: a build binary rewrote a real MCP entry to
/// `…/target/release/tokenade`, so one `cargo clean` and the agent's MCP
/// servers stop launching.
#[cfg(unix)]
#[test]
fn e2e_mcp_proxy_install_does_not_wire_a_build_directory() {
    let installed = std::process::Command::new("sh")
        .arg("-c")
        .arg("command -v tokenade")
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false);
    if !installed {
        eprintln!("SKIP: no installed `tokenade` on PATH — the build-dir fallback is correct here");
        return;
    }

    let h = TempHome::new("mcp-exe-path");
    let cfg = h.path().join(".claude.json");
    std::fs::write(
        &cfg,
        r#"{"mcpServers":{"probe":{"command":"python3","args":["/tmp/probe.py"]}}}"#,
    )
    .unwrap();
    let (_out, _err, _code) = run(&["mcp-proxy", "install"], h.path());

    let written = std::fs::read_to_string(&cfg).unwrap();
    assert!(
        written.contains("mcp-proxy"),
        "the server was not wrapped at all:\n{written}"
    );
    let v: serde_json::Value = serde_json::from_str(&written).expect("config stays valid JSON");
    let command = v["mcpServers"]["probe"]["command"]
        .as_str()
        .unwrap_or_default();
    assert!(
        !command.contains("/target/") && !command.contains("/dist/"),
        "the wrapped launch command points into a build directory — one \
         `cargo clean` and the agent's MCP servers stop launching: {command}"
    );
}

/// Gemini CLI (retired since) and Qwen installed the SAME hook commands, so
/// the hook process could not tell them apart — unless the installer says
/// which.
///
/// Both `cmd_gemini_hook` and `cmd_gemini_afterhook` hardcoded
/// `TOKENADE_AGENT=gemini-cli`, and the third and busiest entry point,
/// `cmd_gemini_after_tool`, set nothing at all. Measured 2026-08-08: its rows
/// landed on whatever `detect::from_env` fell back to — the parent process
/// chain, so `claude-code` in the probe. Two agents' savings, attributed to a
/// third.
#[cfg(unix)]
#[test]
fn e2e_the_gemini_family_hooks_name_the_agent_they_serve() {
    for (agent, dir, slug) in [("qwen-code", ".qwen", "qwen-code")] {
        let h = TempHome::new(&format!("gemini-attr-{agent}"));
        std::fs::create_dir_all(h.path().join(dir)).unwrap();
        std::fs::write(h.path().join(dir).join("settings.json"), "{}\n").unwrap();
        let (_o, _e, _c) = run_in(&["install", "--yes", "--only", agent], h.path(), h.path());

        let written = std::fs::read_to_string(h.path().join(dir).join("settings.json")).unwrap();
        // **No `AfterTool` in Qwen.** It replaces a tool's OUTPUT; the options
        // Qwen's shipped docs grant its `PostToolUse` are `decision`, `reason`
        // and `additionalContext` — no substitution.
        assert!(
            !written.contains("gemini-after-tool"),
            "{agent}: nothing receives this output, do not install it:\n{written}"
        );
        let expected: &[&str] = &["gemini-hook"];
        for hook in expected {
            let at = written
                .find(*hook)
                .unwrap_or_else(|| panic!("{agent}: `{hook}` missing:\n{written}"));
            let tail = &written[at..];
            let end = tail.find('"').unwrap_or(tail.len());
            assert!(
                tail[..end].contains(&format!("--agent {slug}")),
                "{agent}: `{hook}` does not name the agent it serves, so its savings \
                 land on whatever the process tree looks like: {}",
                &tail[..end]
            );
        }
    }
}

/// `shell-init` refuses an argument it does not know, like every sibling verb.
///
/// It accepted anything and printed the default fragment: measured 2026-09-03,
/// `--json`, `--nope`, `-x` and a bare `garbage` each returned 0 and the same
/// 4,916 bytes, while `detect`, `read-mode`, `measure`, `receipt`,
/// `semantic-status`, `incompatibilities`, `mcp-stats` and `hooks` all refuse
/// with exit 2.
///
/// The silence costs something here, which is why this verb is worth the test:
/// the ignored argument decides WHICH SHELL the fragment is written for.
/// `shell-init fish | source` printed POSIX syntax into fish, and `-all` (one
/// dash) printed the short list where `--all` prints six times as much.
#[test]
fn e2e_shell_init_refuses_what_it_does_not_understand() {
    let h = TempHome::new("shell-init-args");

    // A sibling that already refuses — the norm this verb was outside of.
    let (_o, sib_err, sib_code) = run(&["read-mode", "--definitely-not-a-flag"], h.path());
    assert_eq!(sib_code, 2, "the sibling's verdict is the norm: {sib_err}");

    for junk in [
        "--json", "--nope", "-x",
        // One dash where two were meant: the shape that silently halved the
        // command list.
        "-all",
        // The shell named without its flag: the shape that piped POSIX syntax
        // into fish.
        "fish",
    ] {
        let (out, err, code) = run(&["shell-init", junk], h.path());
        assert_eq!(
            code,
            2,
            "`shell-init {junk}` was accepted; it printed {} bytes",
            out.len()
        );
        assert!(
            err.contains(junk) && err.contains("shell-init"),
            "the refusal must name the argument and the command: {err}"
        );
        assert!(
            out.trim().is_empty(),
            "a refused call must print no fragment: {:.200}",
            out
        );
    }

    // And what IS documented still works — otherwise the fix is "refuse
    // everything".
    let (plain, _e, c0) = run(&["shell-init"], h.path());
    assert_eq!(c0, 0);
    assert!(!plain.trim().is_empty(), "the default fragment must print");
    let (all, _e, c1) = run(&["shell-init", "--all"], h.path());
    assert_eq!(c1, 0);
    assert!(
        all.len() > plain.len(),
        "`--all` must still widen the list: {} vs {}",
        all.len(),
        plain.len()
    );
    let (fish, _e, c2) = run(&["shell-init", "--shell", "fish"], h.path());
    assert_eq!(c2, 0);
    assert!(
        fish.contains("function __tokenade_proxy") && fish.contains("isatty stdin"),
        "`--shell fish` must still select the fish fragment: {:.200}",
        fish
    );
    assert_ne!(
        fish, plain,
        "`--shell fish` must differ from the default (POSIX) fragment"
    );

    // A flag that eats the next flag as its value is the same silence one
    // level down.
    let (_o, err, c3) = run(&["shell-init", "--shell", "--all"], h.path());
    assert_eq!(c3, 2, "`--shell --all` must not name the shell '--all'");
    assert!(err.contains("expects a value"), "{err}");
}

/// `uninstall` must not leave behind a `CLAUDE.md` that only ever held its
/// own block.
///
/// `install` CREATES `~/.claude/CLAUDE.md` when the machine has none. The
/// strip removed the fenced block and wrote the remainder back — which, on
/// such a machine, is an empty file. MEASURED on a sandbox home with no
/// CLAUDE.md: install → uninstall left a **0-byte `~/.claude/CLAUDE.md`**
/// that had never existed, from the one command whose whole job is to leave
/// no trace. A file the user wrote himself is untouched by this: anything but
/// whitespace survives the strip and takes the normal rewrite, and the second
/// half of this test holds that line.
#[test]
fn e2e_uninstall_leaves_no_empty_claude_md_behind() {
    // 1. A machine with NO CLAUDE.md.
    let h = TempHome::new("uninstall-no-litter");
    let claude_dir = h.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(claude_dir.join("settings.json"), "{\"hooks\":{}}\n").unwrap();
    let md = claude_dir.join("CLAUDE.md");
    assert!(!md.exists(), "fixture must start without a CLAUDE.md");

    let (_o, err, code) = run_copied(&["install"], h.path());
    assert_eq!(code, 0, "install failed:\n{err}");
    // Anti-vacuity: without this, the assertion below passes on a run where
    // install never wrote the file and there was nothing to clean up.
    assert!(
        md.exists(),
        "install did not create CLAUDE.md, so this test proves nothing"
    );

    // The dry run must PREDICT the removal, and must not perform it. It has
    // its own branch, so a mutation deleting it lived through this test until
    // these lines existed.
    let (dry, err, code) = run_copied(&["uninstall", "--dry-run"], h.path());
    assert_eq!(code, 0, "uninstall --dry-run failed:\n{err}");
    let dry_line = dry
        .lines()
        .find(|l| l.contains("CLAUDE.md"))
        .unwrap_or_else(|| panic!("dry run said nothing about CLAUDE.md:\n{dry}"));
    assert!(
        dry_line.contains("remove"),
        "the dry run must say the file WOULD BE REMOVED: {dry_line:?}"
    );
    assert!(
        md.exists(),
        "`--dry-run` removed the file it was only supposed to predict:\n{dry}"
    );

    let (out, err, code) = run_copied(&["uninstall"], h.path());
    assert_eq!(code, 0, "uninstall failed:\n{err}");
    assert!(
        !md.exists(),
        "uninstall left an empty CLAUDE.md behind:\n{out}"
    );
    // The word matters: the file is gone, and reporting a "strip" would
    // describe a file that is still there. Assert on the CLAUDE.md LINE, not
    // on the whole output — `uninstall` says "removed" about several other
    // things, so a blob-wide `contains("removed")` stayed true even when this
    // line was mutated back to "stripped".
    let md_line = out
        .lines()
        .find(|l| l.contains("CLAUDE.md"))
        .unwrap_or_else(|| panic!("uninstall said nothing about CLAUDE.md:\n{out}"));
    assert!(
        md_line.contains("removed") && !md_line.contains("stripped"),
        "the CLAUDE.md line must say the file was REMOVED, not stripped: {md_line:?}"
    );

    // 2. A machine where the user wrote his own CLAUDE.md: byte-identical.
    let h2 = TempHome::new("uninstall-keeps-user-md");
    let dir2 = h2.path().join(".claude");
    std::fs::create_dir_all(&dir2).unwrap();
    std::fs::write(dir2.join("settings.json"), "{\"hooks\":{}}\n").unwrap();
    let md2 = dir2.join("CLAUDE.md");
    let mine = "# My rules\n\nAlways run tests before committing.\n";
    std::fs::write(&md2, mine).unwrap();

    let (_o, err, code) = run_copied(&["install"], h2.path());
    assert_eq!(code, 0, "install failed:\n{err}");
    let (_o, err, code) = run_copied(&["uninstall"], h2.path());
    assert_eq!(code, 0, "uninstall failed:\n{err}");
    assert!(
        md2.exists(),
        "a CLAUDE.md the user wrote must NOT be removed"
    );
    assert_eq!(
        std::fs::read_to_string(&md2).unwrap(),
        mine,
        "the user's own CLAUDE.md must come back byte-identical"
    );
}

/// `uninstall` must leave no tokenade CODE running in a shell rc file.
///
/// The per-line matcher removes lines carrying a tokenade-unique marker, and
/// the v2 shell fragment puts its only such line — the `alias` — INSIDE
/// `for __tc in <~150 command names>; do … done`. So uninstall took the alias
/// and left the loop: a hundred and fifty iterations executing a comment, in
/// the user's own `.bashrc`. Beside it two guards survived that carry no
/// tokenade word at all, one of them `shopt -s expand_aliases` — a change to
/// how the user's shell expands aliases, still in force after the product was
/// removed.
///
/// The fixture is generated by `shell-init` rather than pasted, so it cannot
/// drift from what the installer actually writes.
#[test]
fn e2e_uninstall_leaves_no_shell_proxy_code_behind() {
    let h = TempHome::new("uninstall-shell-proxy");
    let claude = h.path().join(".claude");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::write(claude.join("settings.json"), "{\"hooks\":{}}\n").unwrap();

    let (fragment, err, code) = run(&["shell-init", "--shell", "bash"], h.path());
    assert_eq!(code, 0, "shell-init failed:\n{err}");
    // Anti-vacuity: the shapes this test is about must really be in there.
    for needle in [
        "for __tc in",
        "__tokenade_proxy",
        "expand_aliases",
        "unset __tc",
    ] {
        assert!(
            fragment.contains(needle),
            "the generated fragment no longer contains {needle:?}; this test \
             would pass without proving anything:\n{fragment}"
        );
    }

    let rc = h.path().join(".bashrc");
    let head = "# my bashrc\nexport EDITOR=vim\n\n";
    let tail = "\n# after tokenade\nalias ll=\"ls -la\"\n";
    std::fs::write(&rc, format!("{head}{fragment}{tail}")).unwrap();

    let (_o, err, code) = run_copied(&["uninstall"], h.path());
    assert_eq!(code, 0, "uninstall failed:\n{err}");
    let after = std::fs::read_to_string(&rc).unwrap();

    // 1. Nothing tokenade EXECUTES. Comments may remain (the strip preserves
    //    prose so it can never eat a user's), but no code may.
    let code_lines: Vec<&str> = after
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    for banned in [
        "__tokenade_proxy",
        "for __tc in",
        "unset __tc",
        "expand_aliases",
        "FUNCNEST",
    ] {
        assert!(
            !code_lines.iter().any(|l| l.contains(banned)),
            "uninstall left executable tokenade code ({banned}):\n{code_lines:#?}"
        );
    }

    // 2. The file must still be VALID. Removing a loop header without its
    //    closer would orphan a `done` — a broken rc file is worse than litter,
    //    and is the outcome the strip exists to prevent.
    let opens = code_lines.iter().filter(|l| l.ends_with("do")).count();
    let closes = code_lines.iter().filter(|l| **l == "done").count();
    assert_eq!(opens, closes, "orphaned loop keyword:\n{code_lines:#?}");
    let ifs = code_lines
        .iter()
        .filter(|l| l.starts_with("if ") && l.ends_with("then"))
        .count();
    let fis = code_lines.iter().filter(|l| **l == "fi").count();
    assert_eq!(ifs, fis, "orphaned if/fi:\n{code_lines:#?}");

    // 3. The user's own lines survive, both sides of the block.
    assert!(
        after.contains("export EDITOR=vim") && after.contains("alias ll=\"ls -la\""),
        "the user's own rc lines must survive:\n{after}"
    );
}

/// `uninstall` must take the llm-proxy autostart unit with it.
///
/// `autostart.rs` states the rule itself — "an autostart entry that survives
/// the uninstall is a process the user cannot find and did not ask for" — and
/// `uninstall` was not honouring it: the unit stayed, and nothing said so. It
/// carries `Restart=always` with `RestartSec=1`, so once the binary is gone
/// systemd retries the dead `ExecStart` every second until it hits the start
/// limit. Same shape as the `#611` MCP self-entries: after the binary is
/// uninstalled, something on the machine keeps trying to spawn it.
///
/// Linux-only because the unit's location is platform-specific (systemd user
/// dir here, LaunchAgents on macOS, Task Scheduler on Windows) and an e2e test
/// drives the binary from outside, so it cannot ask `autostart::unit_path`.
#[cfg(target_os = "linux")]
#[test]
fn e2e_uninstall_removes_the_autostart_unit() {
    let unit_rel = ".config/systemd/user/tokenade-llm-proxy.service";
    let seed = |h: &std::path::Path| {
        let claude = h.join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::write(claude.join("settings.json"), "{\"hooks\":{}}\n").unwrap();
        let unit = h.join(unit_rel);
        std::fs::create_dir_all(unit.parent().unwrap()).unwrap();
        std::fs::write(
            &unit,
            "[Unit]\nDescription=tokenade LLM proxy\n\n[Service]\n\
             ExecStart=/nonexistent/tokenade llm-proxy\nRestart=always\nRestartSec=1\n\n\
             [Install]\nWantedBy=default.target\n",
        )
        .unwrap();
        unit
    };

    // 1. The dry run PREDICTS the removal and performs none of it.
    let hd = TempHome::new("uninstall-unit-dry");
    let unit_d = seed(hd.path());
    let (dry, err, code) = run_copied(&["uninstall", "--dry-run"], hd.path());
    assert_eq!(code, 0, "uninstall --dry-run failed:\n{err}");
    assert!(
        dry.contains("autostart unit"),
        "the dry run must name the unit it would remove:\n{dry}"
    );
    assert!(
        unit_d.exists(),
        "`--dry-run` removed the unit it was only supposed to predict"
    );

    // 2. The real run removes it, and tells the user to stop the service —
    //    removing a unit out from under a running service leaves a process
    //    nothing can name, so the supervisor is told first.
    let h = TempHome::new("uninstall-unit");
    let unit = seed(h.path());
    let (out, err, code) = run_copied(&["uninstall"], h.path());
    assert_eq!(code, 0, "uninstall failed:\n{err}");
    assert!(!unit.exists(), "uninstall left the autostart unit:\n{out}");
    assert!(
        out.contains("systemctl --user disable"),
        "uninstall must print the command that stops the service before the \
         unit goes:\n{out}"
    );

    // 3. Anti-vacuity in the other direction: a home with no unit must not
    //    grow a line about one. Without this, a step that printed
    //    unconditionally would pass assertion 2 and lie on every other run.
    let hq = TempHome::new("uninstall-unit-absent");
    let claude = hq.path().join(".claude");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::write(claude.join("settings.json"), "{\"hooks\":{}}\n").unwrap();
    let (quiet, err, code) = run_copied(&["uninstall"], hq.path());
    assert_eq!(code, 0, "uninstall failed:\n{err}");
    assert!(
        !quiet.contains("autostart unit"),
        "uninstall mentioned an autostart unit that was never there:\n{quiet}"
    );
}

/// A disposable HOME nested under another home — what a test gets when
/// `TMPDIR` lives under the account home — must not let `uninstall` climb out
/// and strip the outer `.claude/settings.json`, the user's global config. It
/// did, twice, on the operator's real settings (2026-10-03).
#[test]
fn e2e_uninstall_never_climbs_out_of_its_home_to_a_global_settings_file() {
    let outer = TempHome::new("uninstall-outer");
    let outer_settings = outer.path().join(".claude").join("settings.json");
    std::fs::create_dir_all(outer_settings.parent().unwrap()).unwrap();
    let hooked = r#"{"hooks":{"PostToolUse":[{"hooks":[{"type":"command","command":"/x/tokenade hookwrap"}]}]}}"#;
    std::fs::write(&outer_settings, hooked).unwrap();
    let inner = outer.path().join("cache").join("tmp").join("home");
    std::fs::create_dir_all(&inner).unwrap();

    let (_out, _err, code) = run_copied(&["uninstall"], &inner);
    assert_eq!(code, 0, "uninstall must succeed");
    assert_eq!(
        std::fs::read_to_string(&outer_settings).unwrap(),
        hooked,
        "uninstall climbed out of its HOME and rewrote another home's settings"
    );
}
