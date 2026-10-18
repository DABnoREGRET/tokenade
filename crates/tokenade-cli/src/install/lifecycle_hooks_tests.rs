// The tests of `lifecycle_hooks`.
//
// Spliced in by `include!` from `lifecycle_hooks.rs`: the same module, so the same
// private access and the same `super::` paths, without one visibility line
// changed. This is the shape this repo already uses for 33 other modules.
//
// A `//!` is impossible here: a file spliced in by `include!` is not a
// module, and an INNER doc comment has nothing to qualify.

#[cfg(test)]
mod websearch_post_install {
    use super::*;

    /// Std-only tempdir (tokenade-cli has no tempfile dev-dep): unique
    /// per test invocation, left for the OS tmp cleaner.
    fn tmpdir(tag: &str) -> std::path::PathBuf {
        // The clock alone does not make this unique: `SystemTime` granularity
        // is coarse on some platforms (~15 ms on Windows), so parallel tests
        // can read the same nanos, and `create_dir_all` adopts an existing
        // directory silently. A process-wide counter makes the name unique by
        // construction; `create_dir` fails loudly on any residual collision.
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!(
            "tokenade-test-{tag}-{}-{}-{seq}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir(&p).expect("test tempdir name must be unique");
        p
    }

    fn settings_json(home: &Path) -> serde_json::Value {
        let p = home.join(".claude").join("settings.json");
        serde_json::from_str(&fs::read_to_string(p).unwrap()).unwrap()
    }

    fn websearch_groups(settings: &serde_json::Value) -> Vec<serde_json::Value> {
        settings
            .pointer("/hooks/PostToolUse")
            .and_then(|a| a.as_array())
            .map(|a| {
                a.iter()
                    .filter(|g| {
                        group_matches_tokenade_hook(
                            g,
                            "tokenade-websearch-post",
                            "hookwrap-websearch-post",
                        )
                        .0
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// **The two folders live side by side, each under its own label.**
    /// The install body has been shared since the MCP hook exists, so the
    /// risk is no longer divergence but COLLISION: a shared label would make
    /// the second overwrite the first, and the measurement would be a single
    /// hook where the report announces two.
    #[test]
    fn the_mcp_hook_coexists_with_the_websearch_hook() {
        let dir = tmpdir("mcp-install");
        let home = dir.as_path();
        let root = home.join("proj");
        fs::create_dir_all(&root).unwrap();
        let ws = install_claude_code_websearch_post_hook(
            &root,
            "/usr/bin/tokenade",
            None,
            true,
            Some(home),
            false,
        );
        assert!(matches!(ws, HookOutcome::Installed(_)), "{ws:?}");
        let mcp = install_claude_code_posttool_hook(
            &root,
            "/usr/bin/tokenade",
            None,
            true,
            Some(home),
            false,
            "mcp__.*",
            "hookwrap-mcp-post",
            "tokenade-mcp-post",
        );
        assert!(matches!(mcp, HookOutcome::Installed(_)), "{mcp:?}");

        let s = settings_json(home);
        assert_eq!(
            websearch_groups(&s).len(),
            1,
            "the WebSearch hook was overwritten"
        );
        let mcp_groups: Vec<serde_json::Value> = s
            .pointer("/hooks/PostToolUse")
            .and_then(|a| a.as_array())
            .map(|a| {
                a.iter()
                    .filter(|g| {
                        group_matches_tokenade_hook(g, "tokenade-mcp-post", "hookwrap-mcp-post").0
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(mcp_groups.len(), 1, "MCP hook missing or duplicated");
        // The `matcher` is a regular expression: MCP tools are named
        // `mcp__<server>__<tool>`, and a literal matcher would catch none of
        // them.
        assert_eq!(mcp_groups[0]["matcher"], "mcp__.*");

        // Removing one leaves the other in place.
        let rm = remove_claude_code_posttool_hook(
            &root,
            true,
            Some(home),
            false,
            "hookwrap-mcp-post",
            "tokenade-mcp-post",
        );
        assert!(matches!(rm, HookOutcome::Refreshed(_)), "{rm:?}");
        assert_eq!(websearch_groups(&settings_json(home)).len(), 1);
    }

    #[test]
    fn install_writes_entry_and_reinstall_is_idempotent() {
        let dir = tmpdir("ws-install");
        let home = dir.as_path();
        let root = home.join("proj");
        fs::create_dir_all(&root).unwrap();
        let o = install_claude_code_websearch_post_hook(
            &root,
            "/usr/bin/tokenade",
            None,
            true,
            Some(home),
            false,
        );
        assert!(matches!(o, HookOutcome::Installed(_)), "{o:?}");
        let s = settings_json(home);
        let groups = websearch_groups(&s);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0]["matcher"], "WebSearch");
        let cmd = groups[0]["hooks"][0]["command"].as_str().unwrap();
        assert!(cmd.contains("hookwrap-websearch-post"), "{cmd}");
        if cfg!(not(windows)) {
            assert!(cmd.ends_with("; exit 0'"), "fail-open tail: {cmd}");
        }
        // Re-install: no duplicate, AlreadyPresent.
        let o2 = install_claude_code_websearch_post_hook(
            &root,
            "/usr/bin/tokenade",
            None,
            true,
            Some(home),
            false,
        );
        assert!(matches!(o2, HookOutcome::AlreadyPresent), "{o2:?}");
        assert_eq!(websearch_groups(&settings_json(home)).len(), 1);
        // Drift (exe moved) heals in place — still exactly one entry.
        let o3 = install_claude_code_websearch_post_hook(
            &root,
            "/opt/new/tokenade",
            None,
            true,
            Some(home),
            false,
        );
        assert!(matches!(o3, HookOutcome::Refreshed(_)), "{o3:?}");
        let groups = websearch_groups(&settings_json(home));
        assert_eq!(groups.len(), 1);
        assert!(groups[0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("/opt/new/tokenade"));
    }

    #[test]
    fn sync_respects_forced_capability_and_removes_on_loss() {
        // Env-forced probe both ways (the documented TEST-ONLY override),
        // serialized against every other TOKENADE_CAP_* mutator via the
        // shared capability lock.
        let _g = super::super::capability::cap_env_lock();
        let dir = tmpdir("ws-sync");
        let home = dir.as_path();
        let root = home.join("proj");
        fs::create_dir_all(&root).unwrap();
        std::env::set_var("TOKENADE_CAP_UPDATED_TOOL_OUTPUT", "1");
        let (cap, o) = sync_claude_code_websearch_post_hook(
            &root,
            "/usr/bin/tokenade",
            None,
            true,
            Some(home),
            false,
        );
        assert!(cap && matches!(o, HookOutcome::Installed(_)), "{o:?}");
        assert_eq!(websearch_groups(&settings_json(home)).len(), 1);
        // Capability disappeared → the same sync REMOVES the entry.
        std::env::set_var("TOKENADE_CAP_UPDATED_TOOL_OUTPUT", "0");
        let (cap, o) = sync_claude_code_websearch_post_hook(
            &root,
            "/usr/bin/tokenade",
            None,
            true,
            Some(home),
            false,
        );
        std::env::remove_var("TOKENADE_CAP_UPDATED_TOOL_OUTPUT");
        assert!(!cap && matches!(o, HookOutcome::Refreshed(_)), "{o:?}");
        assert_eq!(websearch_groups(&settings_json(home)).len(), 0);
        // Other PostToolUse groups (none here) untouched: array still exists.
        assert!(settings_json(home).pointer("/hooks/PostToolUse").is_some());
    }

    #[test]
    fn forced_off_never_installs() {
        let _g = super::super::capability::cap_env_lock();
        let dir = tmpdir("ws-sync");
        let home = dir.as_path();
        let root = home.join("proj");
        fs::create_dir_all(&root).unwrap();
        std::env::set_var("TOKENADE_CAP_UPDATED_TOOL_OUTPUT", "0");
        let (cap, o) = sync_claude_code_websearch_post_hook(
            &root,
            "/usr/bin/tokenade",
            None,
            true,
            Some(home),
            false,
        );
        std::env::remove_var("TOKENADE_CAP_UPDATED_TOOL_OUTPUT");
        assert!(!cap && matches!(o, HookOutcome::AlreadyPresent), "{o:?}");
        assert!(
            !home.join(".claude").join("settings.json").exists()
                || websearch_groups(&settings_json(home)).is_empty()
        );
    }

    #[test]
    fn uninstall_strips_websearch_post_entry() {
        let dir = tmpdir("ws-uninstall");
        let home = dir.as_path();
        let root = home.join("proj");
        fs::create_dir_all(&root).unwrap();
        install_claude_code_websearch_post_hook(
            &root,
            "/usr/bin/tokenade",
            None,
            true,
            Some(home),
            false,
        );
        let settings_path = home.join(".claude").join("settings.json");
        let out =
            super::super::uninstall::strip_tokenade_hooks_from_settings_at(&settings_path, false);
        assert!(
            matches!(out, StripHooksOutcome::Stripped { .. }),
            "uninstall must remove the websearch-post entry"
        );
        assert_eq!(websearch_groups(&settings_json(home)).len(), 0);
    }
}

#[cfg(test)]
mod matcher_coverage {
    /// Every tool name ONE handler dispatches on, extracted from its own
    /// function body.
    ///
    /// Function-scoped on purpose. `hooks/mod.rs` holds three handlers behind
    /// three different matchers, so a file-wide scan asserts one hook's branches
    /// against another hook's matcher — it passes today only because the sets
    /// happen to overlap, and it would send a new `postread` branch to the
    /// `pretool` matcher for approval.
    ///
    /// Both dispatch shapes are read: `tool == "X"` and a `match tool` arm
    /// (`"X" => …`). Missing the match arms would have left `Grep` invisible in
    /// `cmd_hookwrap_postread`, whose only `==` comparison is `Read`.
    fn routed_tools(src: &str, func: &str) -> Vec<String> {
        let needle = format!("fn {func}(");
        let start = src
            .find(&needle)
            .unwrap_or_else(|| panic!("handler `{func}` not found — did it move or get renamed?"));
        // The body ends at the next top-level `fn` (every handler here is a
        // free function at column 0, declared `pub(crate) fn` or `fn`).
        let rest = &src[start + needle.len()..];
        let end = ["\npub(crate) fn ", "\npub fn ", "\nfn "]
            .iter()
            .filter_map(|m| rest.find(m))
            .min()
            .unwrap_or(rest.len());
        let body = &rest[..end];

        let mut out: Vec<String> = Vec::new();
        let mut push = |name: &str| {
            if !name.is_empty()
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !out.iter().any(|s| s == name)
            {
                out.push(name.to_string());
            }
        };
        for cap in body.split("tool == \"").skip(1) {
            if let Some(name) = cap.split('"').next() {
                push(name);
            }
        }
        // `tool_is(tool, "Read")` — the case-tolerant form. Grok lowercases
        // every tool name, and literal equality dropped eight of the nine
        // routed tools on that host (`is_shell_tool_name` had already been
        // fixed for the ninth). Teaching this extractor the second spelling is
        // not optional: it went blind the moment the dispatch changed, which
        // is what the emptiness assertion below is for, and it caught it.
        for cap in body.split("tool_is(tool, \"").skip(1) {
            if let Some(name) = cap.split('"').next() {
                push(name);
            }
        }
        // `match tool { "Read" => … }` — only the arms of a match ON `tool`.
        if let Some(m) = body.find("match tool {") {
            let arms = &body[m..];
            let arms = &arms[..arms.find("\n    };").unwrap_or(arms.len())];
            for cap in arms.split("        \"").skip(1) {
                if let Some((name, tail)) = cap.split_once('"') {
                    if tail.trim_start().starts_with("=>") {
                        push(name);
                    }
                }
            }
        }
        out.sort();
        out
    }

    /// Assert one hook's handler and one hook's installed matcher agree, in
    /// BOTH directions, and that the matcher literal in this test still exists
    /// in the shipping source so the test cannot rot silently.
    ///
    /// The reverse direction matters as much as the forward one: a matcher
    /// naming a tool no branch handles spawns a hook process per call to do
    /// nothing, and it reads like coverage that isn't there.
    /// `claude_hooks.rs` has BRANCHES (`claude_hooks/entries.rs`,
    /// `claude_hooks/autocompact.rs`): reading the trunk alone would hide
    /// an entry moved down into a child, and the contract would be checked
    /// next to the hook it claims to hold.
    fn claude_hooks_family() -> String {
        install_family("claude_hooks")
    }

    /// `lifecycle_hooks.rs` has BRANCHES too (`peers.rs`,
    /// `claude_code.rs`): the canonical matcher this test pins is written
    /// in one of them.
    fn lifecycle_hooks_family() -> String {
        install_family("lifecycle_hooks")
    }

    /// The hook router is no longer only in `hooks/mod.rs`:
    /// `cmd_hookwrap_postread` moved down into `hooks/postread.rs`.
    fn hooks_family() -> String {
        let src = tokenade_core::source_guard::crates_dir()
            .join("tokenade-cli")
            .join("src");
        tokenade_core::source_guard::module_family(&src, "hooks")
    }

    fn install_family(name: &str) -> String {
        let src = tokenade_core::source_guard::crates_dir()
            .join("tokenade-cli")
            .join("src");
        tokenade_core::source_guard::module_family(&src.join("install"), name)
    }

    fn assert_hook_contract(
        src: &str,
        func: &str,
        matcher: &str,
        install_src: &str,
        shell_aliases: &[&str],
    ) {
        let routed = routed_tools(src, func);
        assert!(
            !routed.is_empty(),
            "extraction found no routed tool in `{func}` — the dispatch shape \
             changed and this guard went blind, which is the failure it exists \
             to prevent"
        );
        for t in &routed {
            assert!(
                matcher.split('|').any(|m| m == t),
                "`{func}` handles tool `{t}` but its installed matcher \
                 `{matcher}` does not route it — the handler would be DEAD on \
                 real installs (D2 class). Extend the matcher AND its \
                 heal-in-place path."
            );
        }
        for m in matcher.split('|') {
            let handled = routed.iter().any(|t| t == m)
                || shell_aliases.contains(&m)
                // MultiEdit/Write/Edit-style hooks act on the EVENT, not on a
                // tool-name branch; they are declared by passing them as
                // aliases, so an unexplained extra stays a failure.
                ;
            assert!(
                handled,
                "matcher `{matcher}` routes `{m}` to `{func}`, which has no \
                 branch for it — either the branch was removed (narrow the \
                 matcher) or it never existed (a hook process per call, doing \
                 nothing)."
            );
        }
        assert!(
            install_src.contains(&format!("let canonical_matcher = \"{matcher}\"")),
            "matcher literal `{matcher}` drifted — update BOTH the constant \
             and this test"
        );
    }

    /// Class-6 structural guard (D2 shipped dead-on-arrival in 0.7.5: the
    /// Write steer existed in code but the installed matcher predated it).
    /// Every tool name the pretool ROUTER dispatches on must appear in the
    /// canonical matcher, or the handler is unreachable on real installs.
    /// Editing the router without extending the matcher breaks this test.
    #[test]
    fn canonical_matcher_covers_every_routed_tool() {
        assert_hook_contract(
            hooks_family().as_str(),
            "cmd_hookwrap_pretool",
            "Bash|Read|Grep|WebFetch|WebSearch|Task|Agent|PowerShell|Write|Edit",
            lifecycle_hooks_family().as_str(),
            // The shell tool is matched through `is_shell_tool_name`, not by a
            // literal, because its name differs per agent (Bash / bash /
            // Execute).
            &["Bash"],
        );
    }

    /// Same contract for the OTHER two multi-tool hooks. The 2026-06 pivot
    /// narrowed the PostToolUse matcher to `Bash`, silently disabling the
    /// read-hint while its handler stayed live — the same class omni shipped
    /// for months (their PostToolUse was registered for `Bash` only, and the
    /// telemetry built to notice sat downstream of the exclusion). Install
    /// tests pin the matcher STRING; these pin the relationship, so adding a
    /// branch without widening the matcher fails too.
    #[test]
    fn postread_matcher_covers_every_tool_its_handler_dispatches_on() {
        assert_hook_contract(
            hooks_family().as_str(),
            "cmd_hookwrap_postread",
            "Read|Grep|Glob",
            claude_hooks_family().as_str(),
            &[],
        );
    }

    #[test]
    fn posttool_matcher_covers_every_tool_its_handler_dispatches_on() {
        // `cmd_hookwrap` moved down into `hookwrap/dispatch.rs`: we read the
        // family, otherwise the contract would be checked on a file that no
        // longer holds the hook body.
        let hookwrap = tokenade_core::source_guard::module_family(
            &tokenade_core::source_guard::crates_dir()
                .join("tokenade-cli")
                .join("src"),
            "hookwrap",
        );
        assert_hook_contract(
            hookwrap.as_str(),
            "cmd_hookwrap",
            "Bash|Read",
            claude_hooks_family().as_str(),
            &["Bash"],
        );
    }
}
