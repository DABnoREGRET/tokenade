// The tests of `crate::mcp_wrap`, moved out of the production file.
//
// Mounted by `include!`, not by `mod`: the tokens are spliced INTO
// `mcp_wrap`, so every `super::` keeps the meaning it had. Split by
// `srcsweep::production_lines`.

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tmp_config(label: &str, content: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "tokenade-mcp-wrap-{label}-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&p, content).unwrap();
        p
    }

    fn fake_env(home: &Path, cwd: &Path, os: OsKind) -> McpDiscoveryEnv {
        McpDiscoveryEnv {
            home: home.to_path_buf(),
            cwd: cwd.to_path_buf(),
            os,
        }
    }

    /// Who `mcp-proxy install` should reach, and who it must leave alone.
    ///
    /// Reach: an agent that declares a wrappable MCP config and has NO
    /// tokenade plugin. OpenCode, KiloCode, Gemini and Qwen all qualified
    /// and none were listed, so their MCP servers were never wrapped.
    ///
    /// Leave alone: an agent whose plugin already folds every tool result
    /// inside the agent. Wrapping its servers too would put two
    /// authorities on the same bytes — an
    /// extra process per MCP call to fold something already folded.
    #[test]
    fn discovery_finds_claude_desktop_on_every_os() {
        // Claude Desktop is a different product from Claude Code, it has no
        // shell and no hook, and this file is the only channel it can have.
        // It was missing from the site list: measured 2026-08-18 on a macOS VM
        // running the real app (0.14.10), a third-party server planted in its
        // config made `mcp-wrap install` answer "no MCP servers to wrap were
        // found across known agent configs".
        for (os, rel) in [
            (OsKind::Linux, ".config/Claude/claude_desktop_config.json"),
            (
                OsKind::Macos,
                "Library/Application Support/Claude/claude_desktop_config.json",
            ),
            (
                OsKind::Windows,
                "AppData/Roaming/Claude/claude_desktop_config.json",
            ),
        ] {
            let home = std::path::PathBuf::from("/fake/home");
            let env = McpDiscoveryEnv {
                home: home.clone(),
                cwd: std::path::PathBuf::from("/fake/cwd"),
                os,
            };
            let want = rel.split('/').fold(home.clone(), |acc, seg| acc.join(seg));
            assert!(
                discover_mcp_config_sites(&env)
                    .iter()
                    .any(|s| s.path == want),
                "{os:?}: no discovery site at {}",
                want.display()
            );
        }
    }

    /// The Microsoft Store build of Claude Desktop reads its config from its
    /// package's redirected AppData, not `%APPDATA%\Claude` (measured on the
    /// Windows VM, Claude 2.19675.0.0): that file is discovered too.
    #[test]
    fn discovery_finds_the_store_build_of_claude_desktop() {
        let home = std::env::temp_dir().join(format!("tk-msix-claude-{}", std::process::id()));
        let cfg =
            home.join("AppData/Local/Packages/Claude_pzs8sxrjxfjjc/LocalCache/Roaming/Claude");
        std::fs::create_dir_all(&cfg).unwrap();
        let env = McpDiscoveryEnv {
            home: home.clone(),
            cwd: std::path::PathBuf::from("/fake/cwd"),
            os: OsKind::Windows,
        };
        let want = cfg.join("claude_desktop_config.json");
        let found = discover_mcp_config_sites(&env)
            .iter()
            .any(|s| s.path == want && s.agent == "claude-desktop");
        let _ = std::fs::remove_dir_all(&home);
        assert!(found, "no discovery site at {}", want.display());
    }

    /// Devin Desktop keeps its MCP servers in Codeium's home-anchored file on
    /// EVERY OS, not in a per-OS config root.
    ///
    /// Read off the app itself 2026-08-18, because guessing from the vendor
    /// gave the wrong answer twice. Its `product.json` says
    /// `win32AppUserModelId = "Exafunction.Windsurf"` — a Windsurf fork, not a
    /// plain VSCode one — and its bundle builds the path as
    /// `joinPath(homedir, ".codeium", windsurfDirName, "mcp_config.json")`
    /// with `windsurfDirName = "windsurf"` on the stable channel, plus a
    /// user-visible string reading `Devin configurations
    /// (~/.codeium/{0}/mcp_config.json)`. Confirmed on disk on a machine with
    /// Devin installed and no Windsurf: `~/.codeium/windsurf/` is there, with
    /// the `user_settings.pb` that the same code block names next to
    /// `mcp_config.json`.
    ///
    #[test]
    fn devin_uses_the_codeium_home_file_on_every_os() {
        for os in [OsKind::Linux, OsKind::Macos, OsKind::Windows] {
            let home = std::path::PathBuf::from("/fake/home");
            let env = McpDiscoveryEnv {
                home: home.clone(),
                cwd: std::path::PathBuf::from("/fake/cwd"),
                os,
            };
            let want = home
                .join(".codeium")
                .join("windsurf")
                .join("mcp_config.json");
            let sites = discover_mcp_config_sites(&env);
            assert!(
                sites
                    .iter()
                    .any(|s| s.agent == "devin-desktop" && s.path == want),
                "{os:?}: devin has no site at {}",
                want.display()
            );
            assert_eq!(
                sites.iter().filter(|s| s.path == want).count(),
                1,
                "{os:?}: the Codeium file is listed once"
            );
        }
    }

    #[test]
    fn discovery_reaches_pluginless_mcp_agents_and_spares_the_rest() {
        use crate::detect::{Agent, McpConfigFormat};
        let home = Path::new("/fake/home");
        let cwd = Path::new("/proj");
        // One site predates the rule: Cline runs a plugin that folds tool
        // results, so wrapping its MCP servers too is redundant — left in
        // place because removing a shipped wrap path needs its own
        // measurement, not a drive-by. Named so the exception stays visible.
        // Cursor was on this list until its "plugin" turned out to be a
        // preToolUse HOOK; with no plugin, its wrap site is legitimate.
        const PRE_EXISTING: &[&str] = &["cline"];
        for os in [OsKind::Linux, OsKind::Macos, OsKind::Windows] {
            let sites = discover_mcp_config_sites(&fake_env(home, cwd, os));
            let norm = |p: &std::path::Path| p.display().to_string().replace('\\', "/");
            for agent in Agent::all() {
                if !agent.integration().mcp {
                    continue;
                }
                let wrappable = matches!(
                    agent.mcp_config_format(),
                    McpConfigFormat::JsonMcpServers
                        | McpConfigFormat::JsonOpenCodeMcp
                        | McpConfigFormat::JsonGeminiSettings
                        | McpConfigFormat::JsonOpenClawMcp
                        | McpConfigFormat::TomlMcpServers
                );
                if !wrappable {
                    continue; // Hermes' YAML has no wrap path (and needs none).
                }
                let Some(want) = agent.mcp_config_path_on(os, cwd, Some(home)) else {
                    continue;
                };
                let listed = sites.iter().any(|s| norm(&s.path) == norm(&want));
                if agent.has_plugin() {
                    assert!(
                        !listed || PRE_EXISTING.contains(&agent.as_str()),
                        "{agent:?} has a plugin AND a wrap site — two authorities on the same bytes"
                    );
                } else {
                    assert!(
                        listed,
                        "{agent:?} declares {} on {os:?} but no discovery site points there",
                        norm(&want)
                    );
                }
            }
        }
    }

    #[test]
    fn discovery_covers_agents_across_three_os() {
        let home = Path::new("/fake/home");
        let cwd = Path::new("/proj");
        // Claude global + Cursor project are OS-independent (home/cwd based).
        for os in [OsKind::Linux, OsKind::Macos, OsKind::Windows] {
            let sites = discover_mcp_config_sites(&fake_env(home, cwd, os));
            let by_agent = |a: &str| -> Vec<String> {
                sites
                    .iter()
                    .filter(|s| s.agent == a)
                    .map(|s| s.path.display().to_string())
                    .collect()
            };
            // Normalize separators: `Path::display()` uses the HOST's separator
            // (backslashes on a Windows host), while these expected fragments
            // are written with forward slashes. The discovery logic under test
            // is separator-agnostic, so match on the normalized form.
            let norm = |p: &String| p.replace('\\', "/");
            assert!(
                by_agent("claude")
                    .iter()
                    .any(|p| norm(p).ends_with("/fake/home/.claude.json")),
                "claude global config missing on {os:?}"
            );
            assert!(
                by_agent("cursor")
                    .iter()
                    .any(|p| norm(p).contains("/proj/.cursor/")),
                "cursor project config missing on {os:?}"
            );
            // At least two agents (claude + one more) always present.
            let agents: std::collections::HashSet<_> =
                sites.iter().map(|s| s.agent.clone()).collect();
            assert!(agents.len() >= 2, "expected >=2 agents on {os:?}");
        }
        // OS-specific: the VS Code User directory resolves per OS. This used
        // to be asserted through Cline's `globalStorage` path, which sits under
        // the same root — but Cline runs a tokenade PLUGIN and was removed from
        // MCP discovery so it does not get two compactors on one payload
        // (`an_agent_with_a_plugin_is_not_also_mcp_wrapped`). The property
        // under test was never Cline; it was the root.
        let linux = discover_mcp_config_sites(&fake_env(home, cwd, OsKind::Linux));
        assert!(linux.iter().any(|s| s.agent == "vscode"
            && s.path
                .display()
                .to_string()
                .replace('\\', "/")
                .contains("/.config/Code/User/")));
        let mac = discover_mcp_config_sites(&fake_env(home, cwd, OsKind::Macos));
        assert!(mac.iter().any(|s| s.agent == "vscode"
            && s.path
                .display()
                .to_string()
                .replace('\\', "/")
                .contains("/Library/Application Support/Code/User/")));
        let win = discover_mcp_config_sites(&fake_env(home, cwd, OsKind::Windows));
        assert!(win.iter().any(|s| s.agent == "vscode"
            && s.path
                .display()
                .to_string()
                .replace('\\', "/")
                .contains("/AppData/Roaming/Code/User/")));
        // And Cline must NOT be discoverable on any of the three.
        for (os, sites) in [("linux", &linux), ("macos", &mac), ("windows", &win)] {
            assert!(
                !sites.iter().any(|s| s.agent == "cline"),
                "cline is MCP-wrapped on {os} and also runs a plugin bridge"
            );
        }
    }

    #[test]
    fn wrap_all_then_unwrap_multi_server_with_backup() {
        let p = tmp_config(
            "multi",
            r#"{"mcpServers":{
                "github":{"command":"npx","args":["mcp-server-github"]},
                "fs":{"command":"/usr/bin/mcp-fs","args":["--root","/tmp"]},
                "tokenade":{"command":"tokenade","args":["mcp"]}
            }}"#,
        );
        // Wrap ALL (only=None). tokenade self-entry is skipped.
        let r = process_named_config(&p, None, true);
        assert!(r.error.is_none(), "wrap error: {:?}", r.error);
        let mut wrapped = r.wrapped.clone();
        wrapped.sort();
        assert_eq!(wrapped, vec!["fs".to_string(), "github".to_string()]);
        assert_eq!(r.skipped_self, vec!["tokenade".to_string()]);
        // A timestamped backup was written and still holds the ORIGINAL bytes.
        let bak = r.backup.clone().expect("backup path recorded");
        assert!(std::path::Path::new(&bak).exists(), "backup file missing");
        let bak_json: Value =
            serde_json::from_str(&std::fs::read_to_string(&bak).unwrap()).unwrap();
        assert_eq!(bak_json["mcpServers"]["github"]["command"], json!("npx"));

        // On-disk config now has both third-party entries wrapped.
        let after: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(
            after["mcpServers"]["github"][ORIGINAL_KEY]["command"],
            json!("npx")
        );
        assert!(after["mcpServers"]["fs"].get(ORIGINAL_KEY).is_some());
        // tokenade self-entry untouched (no wrapper snapshot).
        assert!(after["mcpServers"]["tokenade"].get(ORIGINAL_KEY).is_none());

        // Idempotent: second wrap-all wraps nothing (both already wrapped).
        let r2 = process_named_config(&p, None, true);
        assert!(r2.wrapped.is_empty(), "second wrap must be a no-op");
        assert_eq!(r2.skipped_already_wrapped.len(), 2);
        assert!(r2.backup.is_none(), "no backup when nothing changed");

        // Unwrap ALL restores the exact original commands/args.
        let u = process_named_config(&p, None, false);
        let mut un = u.unwrapped.clone();
        un.sort();
        assert_eq!(un, vec!["fs".to_string(), "github".to_string()]);
        let restored: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(restored["mcpServers"]["github"]["command"], json!("npx"));
        assert_eq!(
            restored["mcpServers"]["github"]["args"],
            json!(["mcp-server-github"])
        );
        assert_eq!(
            restored["mcpServers"]["fs"]["command"],
            json!("/usr/bin/mcp-fs")
        );
        assert!(restored["mcpServers"]["github"].get(ORIGINAL_KEY).is_none());
        // Idempotent unwrap.
        let u2 = process_named_config(&p, None, false);
        assert!(u2.unwrapped.is_empty());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn wrap_single_named_server_leaves_others_alone() {
        let p = tmp_config(
            "single",
            r#"{"mcpServers":{
                "github":{"command":"npx","args":["mcp-server-github"]},
                "fs":{"command":"mcp-fs","args":[]}
            }}"#,
        );
        let r = process_named_config(&p, Some("github"), true);
        assert_eq!(r.wrapped, vec!["github".to_string()]);
        let after: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert!(after["mcpServers"]["github"].get(ORIGINAL_KEY).is_some());
        // fs was not the target — untouched.
        assert!(after["mcpServers"]["fs"].get(ORIGINAL_KEY).is_none());
        assert_eq!(after["mcpServers"]["fs"]["command"], json!("mcp-fs"));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn corrupt_config_left_untouched() {
        let p = tmp_config("corrupt", "{ this is not json");
        let before = std::fs::read_to_string(&p).unwrap();
        let r = process_named_config(&p, None, true);
        assert!(r.error.is_some(), "parse error must be surfaced");
        assert!(r.wrapped.is_empty());
        assert!(r.backup.is_none());
        // File bytes are exactly as they were — never corrupted further.
        assert_eq!(std::fs::read_to_string(&p).unwrap(), before);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn wraps_object_shape_entry() {
        let p = tmp_config(
            "obj",
            r#"{"mcpServers":{"github":{"command":"npx","args":["mcp-server-github"]}}}"#,
        );
        let r = wrap_one_config(&p, &["tokenade"]);
        assert_eq!(r.wrapped, vec!["github".to_string()]);
        let after: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        let entry = &after["mcpServers"]["github"];
        // The command is the ABSOLUTE path of the running tokenade
        // binary. In tests current_exe() resolves to the
        // test harness (e.g. `/tmp/target/release/deps/tokenade_core-<hash>`),
        // so we just assert it's a non-empty string and verify the
        // args structure carries the original cmd at the expected slot.
        let cmd_str = entry["command"].as_str().expect("command is a string");
        assert!(!cmd_str.is_empty(), "wrapper command must be non-empty");
        assert_eq!(
            entry["args"],
            json!(["mcp-proxy", "--", "npx", "mcp-server-github"])
        );
        assert_eq!(entry[ORIGINAL_KEY]["command"], json!("npx"));
        assert_eq!(entry[ORIGINAL_KEY]["args"], json!(["mcp-server-github"]));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn wraps_array_shape_entry_codex_style() {
        let p = tmp_config(
            "codex",
            r#"{"mcp":{"github":{"command":["npx","mcp-server-github","--flag"]}}}"#,
        );
        let r = wrap_one_config(&p, &["tokenade"]);
        assert_eq!(r.wrapped, vec!["github".to_string()]);
        let after: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        let entry = &after["mcp"]["github"];
        // First argv element is the wrapper command (absolute
        // path); remaining elements are stable. In tests
        // current_exe() resolves to the test binary, so only verify
        // non-empty + structural assertion on the tail.
        let cmd_arr = entry["command"].as_array().expect("command is array");
        let wrapper = cmd_arr[0].as_str().expect("wrapper is string");
        assert!(!wrapper.is_empty(), "wrapper must be non-empty");
        assert_eq!(
            cmd_arr[1..],
            json!(["mcp-proxy", "--", "npx", "mcp-server-github", "--flag"])
                .as_array()
                .unwrap()[..]
        );
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn skips_self_entry() {
        let p = tmp_config(
            "self",
            r#"{"mcpServers":{"tokenade":{"command":"tokenade","args":["mcp"]}}}"#,
        );
        let r = wrap_one_config(&p, &["tokenade"]);
        assert!(r.wrapped.is_empty());
        assert_eq!(r.skipped_self, vec!["tokenade".to_string()]);
        let _ = std::fs::remove_file(&p);
    }

    /// A server the USER routed through tokenade by hand is already wrapped.
    ///
    /// The shipped CLAUDE.md block tells users to write
    /// `tokenade mcp-proxy -- <cmd>` in their MCP config. Detection keyed only
    /// off `ORIGINAL_KEY`, which such an entry does not carry, so
    /// `mcp-wrap install` — whose own help promises it is "idempotent" —
    /// wrapped it AGAIN: measured 2026-09-03,
    /// `tokenade mcp-proxy -- tokenade mcp-proxy -- npx -y @…/server-github`.
    /// Every tool result folded twice, one extra process per call, and
    /// `mcp-stats` reported the server as unwrapped while asking the user to
    /// wrap it.
    ///
    /// All FOUR paths, because the decision is spelled at four sites: JSON and
    /// TOML, each with an all-servers and a single-server variant. Three were
    /// found by reading and the fourth by measuring — `mcp-stats` still said
    /// "0 wrapped" after the first three were fixed.
    #[test]
    fn a_hand_routed_server_is_not_wrapped_a_second_time() {
        let hand = json!({"command": "tokenade",
                          "args": ["mcp-proxy", "--", "npx", "-y", "@x/server-github"]});
        let absolute = json!({"command": "/usr/local/bin/tokenade",
                              "args": ["mcp-proxy", "--", "node", "srv.js"]});
        let plain = json!({"command": "npx", "args": ["-y", "@x/server-slack"]});
        // tokenade's OWN server under a name other than "tokenade": the
        // self-skip beside each site asks about the NAME, so this one reached
        // `wrap_entry` and we would have wrapped ourselves.
        let ours = json!({"command": "/usr/local/bin/tokenade", "args": ["mcp"]});
        let cfg = serde_json::to_string(&json!({
            "mcpServers": {"hand": hand, "abspath": absolute, "plain": plain, "renamed": ours}
        }))
        .unwrap();

        for (label, run) in [
            (
                "all servers",
                Box::new(|p: &std::path::Path| wrap_one_config(p, &["tokenade"]))
                    as Box<dyn Fn(&std::path::Path) -> WrapReport>,
            ),
            (
                "one server",
                Box::new(|p: &std::path::Path| process_named_config(p, Some("hand"), true)),
            ),
        ] {
            let p = tmp_config(&format!("hand-routed-{}", label.replace(' ', "-")), &cfg);
            let before: Value =
                serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
            let r = run(&p);
            let after: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
            for name in ["hand", "abspath", "renamed"] {
                assert_eq!(
                    before["mcpServers"][name], after["mcpServers"][name],
                    "{label}: {name} was rewritten though it already routes through tokenade"
                );
                assert!(
                    r.skipped_already_wrapped.contains(&name.to_string())
                        || !r.wrapped.contains(&name.to_string()),
                    "{label}: {name} counted as a fresh wrap: {r:?}"
                );
            }
            // The control: an unrouted server in the same file IS wrapped, so
            // the test cannot pass by the whole pass doing nothing.
            if label == "all servers" {
                assert_eq!(r.wrapped, vec!["plain".to_string()], "{label}: {r:?}");
                // And the two tokenade-shaped entries land in DIFFERENT
                // buckets: the hand-routed pair is a third-party server we
                // already route (so `mcp-stats` counts it wrapped), our own
                // server is not a third-party server at all.
                assert!(
                    r.skipped_already_wrapped.contains(&"hand".to_string())
                        && r.skipped_already_wrapped.contains(&"abspath".to_string()),
                    "hand-routed entries must count as wrapped: {r:?}"
                );
                assert!(
                    r.skipped_self.contains(&"renamed".to_string())
                        && !r.skipped_already_wrapped.contains(&"renamed".to_string()),
                    "tokenade's own server is not a wrapped third-party server: {r:?}"
                );
            }
            let _ = std::fs::remove_file(&p);
        }

        // The OTHER TOML dialect: `[[plugins]]` entries carrying a `name`
        // field, handled by a second transformer. The `[mcp_servers.x]`
        // fixture below never reaches it, and a mutation there survived.
        for only in [None, Some("hand")] {
            let dir = std::env::temp_dir().join(format!(
                // `{:?}` of an `Option<&str>` writes QUOTES, and a quote is
                // not a legal character in a Windows path: this line answered
                // os error 123 (InvalidFilename) on the release build's
                // Windows leg while reading fine on Unix.
                "tokenade-mcp-hand-plugins-{}-{}",
                std::process::id(),
                only.unwrap_or("all")
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let cfg = dir.join("config.toml");
            let text = r#"[[plugins]]
name = "hand"
command = "tokenade"
args = ["mcp-proxy", "--", "npx", "gh-mcp"]

[[plugins]]
name = "plain"
command = "npx"
args = ["gh-mcp"]

[[plugins]]
name = "renamed"
command = "/usr/local/bin/tokenade"
args = ["mcp"]
"#;
            std::fs::write(&cfg, text).unwrap();
            let r = toml_config_pass(&cfg, only, true, false, WrapReport::default())
                .expect("a .toml path belongs to the toml pass");
            let after = std::fs::read_to_string(&cfg).unwrap();
            assert_eq!(
                after.matches("mcp-proxy").count(),
                if only.is_none() { 2 } else { 1 },
                "plugins {only:?}: nested wrapper, our own server wrapped, or the \
                 control did not wrap:\n{after}"
            );
            assert!(
                !r.wrapped.contains(&"hand".to_string()),
                "plugins {only:?}: {r:?}"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }

        // …and the same on the TOML side, both variants.
        for only in [None, Some("hand")] {
            let dir = std::env::temp_dir().join(format!(
                // `{:?}` of an `Option<&str>` writes QUOTES, and a quote is
                // not a legal character in a Windows path: this line answered
                // os error 123 (InvalidFilename) on the release build's
                // Windows leg while reading fine on Unix.
                "tokenade-mcp-hand-toml-{}-{}",
                std::process::id(),
                only.unwrap_or("all")
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let cfg = dir.join("config.toml");
            let text = r#"[mcp_servers.hand]
command = "tokenade"
args = ["mcp-proxy", "--", "npx", "gh-mcp"]

[mcp_servers.plain]
command = "npx"
args = ["gh-mcp"]

[mcp_servers.renamed]
command = "/usr/local/bin/tokenade"
args = ["mcp"]
"#;
            std::fs::write(&cfg, text).unwrap();
            let r = toml_config_pass(&cfg, only, true, false, WrapReport::default())
                .expect("a .toml path belongs to the toml pass");
            let after = std::fs::read_to_string(&cfg).unwrap();
            // Counting is quoting-free and says exactly the thing: the hand
            // entry carries ONE `mcp-proxy`, a double wrap carries two.
            assert_eq!(
                after.matches("mcp-proxy").count(),
                if only.is_none() { 2 } else { 1 },
                "TOML {only:?}: nested wrapper, our own server wrapped, or the \
                 control did not wrap:\n{after}"
            );
            assert!(
                !r.wrapped.contains(&"hand".to_string()),
                "TOML {only:?}: {r:?}"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn idempotent_when_already_wrapped() {
        let p = tmp_config(
            "idem",
            r#"{"mcpServers":{"github":{"command":"npx","args":["mcp-server-github"]}}}"#,
        );
        let _ = wrap_one_config(&p, &["tokenade"]);
        let r2 = wrap_one_config(&p, &["tokenade"]);
        assert!(r2.wrapped.is_empty(), "re-wrap should be a no-op");
        assert_eq!(r2.skipped_already_wrapped, vec!["github".to_string()]);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn refresh_heals_stale_bare_wrapper_object_shape() {
        // Regression: an older wrap stored `command:
        // "tokenade"` (bare). Running install today should detect
        // and rewrite to the current absolute path WITHOUT requiring
        // unwrap-mcps + install. `original` payload stays intact so
        // a later unwrap restores the user's command.
        let p = tmp_config(
            "refresh-obj",
            r#"{"mcpServers":{"github":{"command":"tokenade","args":["mcp-proxy","--","npx","mcp-server-github"],"tokenadeOriginal":{"command":"npx","args":["mcp-server-github"]}}}}"#,
        );
        let r = wrap_one_config(&p, &["tokenade"]);
        assert!(r.wrapped.is_empty(), "should refresh, not wrap fresh");
        assert!(
            r.refreshed_wrappers.contains(&"github".to_string()),
            "github entry should be refreshed: {:?}",
            r
        );
        let after: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        let cmd = after["mcpServers"]["github"]["command"].as_str().unwrap();
        assert_ne!(cmd, "tokenade", "command should now be the absolute path");
        assert!(
            std::path::Path::new(cmd).is_absolute(),
            "expected abs path or test-runner path: {cmd}"
        );
        // original payload preserved verbatim.
        let orig = &after["mcpServers"]["github"]["tokenadeOriginal"];
        assert_eq!(orig["command"], json!("npx"));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn refresh_heals_stale_bare_wrapper_array_shape() {
        // Codex shape uses `command: ["tokenade", "mcp-proxy", "--", ...]`.
        let p = tmp_config(
            "refresh-arr",
            r#"{"mcp":{"github":{"command":["tokenade","mcp-proxy","--","npx","mcp-server-github"],"tokenadeOriginal":{"command":["npx","mcp-server-github"]}}}}"#,
        );
        let r = wrap_one_config(&p, &["tokenade"]);
        assert!(
            r.refreshed_wrappers.contains(&"github".to_string()),
            "array-shape github entry should be refreshed: {:?}",
            r
        );
        let after: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        let first = after["mcp"]["github"]["command"][0].as_str().unwrap();
        assert_ne!(
            first, "tokenade",
            "first array element should be absolute path"
        );
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn refresh_noop_when_already_absolute_path() {
        // First wrap (fresh) writes the absolute path already.
        // Second pass should NOT report a refresh (no work to do).
        let p = tmp_config(
            "refresh-noop",
            r#"{"mcpServers":{"github":{"command":"npx","args":["mcp-server-github"]}}}"#,
        );
        let _ = wrap_one_config(&p, &["tokenade"]);
        let r2 = wrap_one_config(&p, &["tokenade"]);
        assert!(
            r2.refreshed_wrappers.is_empty(),
            "no refresh on a fresh absolute-path wrap: {:?}",
            r2
        );
        assert_eq!(r2.skipped_already_wrapped, vec!["github".to_string()]);
        let _ = std::fs::remove_file(&p);
    }

    /// The defect is not in the first run, it is in what happens the SECOND
    /// time. A user upgrades an MCP server the only way there is — by editing
    /// its version in the config — and the entry is wrapped at the time, so the
    /// edit lands in the tail after `--`. Unwrapping used to throw that away
    /// and restore the snapshot taken at wrap time: the server silently went
    /// back to the old version, with nothing in the output saying so.
    ///
    /// Measured 2026-09-01 on a real `~/.claude.json` shape: `env` and
    /// `timeout` came back intact (they are not ours to rewrite) and `args`
    /// did not.
    #[test]
    fn an_edit_made_while_wrapped_survives_the_unwrap() {
        let mut root: Value = serde_json::from_str(
            r#"{"mcpServers":{"github":{
                 "command":"tokenade",
                 "args":["mcp-proxy","--","npx","-y","@scope/server@2.0.0","--read-only"],
                 "env":{"TOK":"secret"},
                 "timeout":300,
                 "tokenadeOriginal":{"command":"npx","args":["-y","@scope/server@1.0.0"]}}}}"#,
        )
        .unwrap();
        let mut report = WrapReport::default();
        unwrap_in_root_selecting(&mut root, &|_| true, &mut report);
        let g = &root["mcpServers"]["github"];
        assert_eq!(g["command"], json!("npx"), "the tail names the real binary");
        assert_eq!(
            g["args"],
            json!(["-y", "@scope/server@2.0.0", "--read-only"]),
            "the user's version bump and flag are what the server launches today"
        );
        assert_eq!(g["env"]["TOK"], json!("secret"), "env is not ours to touch");
        assert_eq!(g["timeout"], json!(300), "nor is timeout");
        assert!(
            g.get(ORIGINAL_KEY).is_none(),
            "the snapshot is consumed by the unwrap"
        );
    }

    /// A `--` inside the server's OWN arguments must survive: the separator we
    /// inserted is the first one, and everything after it — including a second
    /// `--` — belongs to the user.
    #[test]
    fn only_the_first_separator_is_ours() {
        let args = vec![
            json!("mcp-proxy"),
            json!("--"),
            json!("uvx"),
            json!("some-server"),
            json!("--"),
            json!("--inner-flag"),
        ];
        let (bin, rest) = original_from_live_tail(&args).expect("a tail is present");
        assert_eq!(bin, json!("uvx"));
        assert_eq!(rest, json!(["some-server", "--", "--inner-flag"]));
    }

    /// An entry with no readable tail falls back to the snapshot — that is what
    /// keeps a hand-edited or half-written config restorable at all.
    #[test]
    fn a_wrapper_with_no_tail_falls_back_to_the_snapshot() {
        assert!(original_from_live_tail(&[json!("mcp-proxy")]).is_none());
        assert!(original_from_live_tail(&[json!("mcp-proxy"), json!("--")]).is_none());
        assert!(original_from_live_tail(&[json!("mcp-proxy"), json!("--"), json!(7)]).is_none());

        let mut root: Value = serde_json::from_str(
            r#"{"mcpServers":{"x":{"command":"tokenade","args":["mcp-proxy"],
                 "tokenadeOriginal":{"command":"npx","args":["srv"]}}}}"#,
        )
        .unwrap();
        let mut report = WrapReport::default();
        unwrap_in_root_selecting(&mut root, &|_| true, &mut report);
        assert_eq!(root["mcpServers"]["x"]["command"], json!("npx"));
        assert_eq!(root["mcpServers"]["x"]["args"], json!(["srv"]));
    }

    #[test]
    fn unwrap_restores_object_shape_exactly() {
        let p = tmp_config(
            "unwrap-obj",
            r#"{"mcpServers":{"github":{"command":"npx","args":["mcp-server-github"],"env":{"K":"v"}}}}"#,
        );
        let _ = wrap_one_config(&p, &["tokenade"]);
        let _ = unwrap_one_config(&p);
        let after: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        let entry = &after["mcpServers"]["github"];
        assert_eq!(entry["command"], json!("npx"));
        assert_eq!(entry["args"], json!(["mcp-server-github"]));
        assert_eq!(entry["env"]["K"], json!("v"));
        assert!(entry.get(ORIGINAL_KEY).is_none());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn unwrap_restores_array_shape() {
        let p = tmp_config(
            "unwrap-arr",
            r#"{"mcp":{"github":{"command":["npx","mcp-server-github"]}}}"#,
        );
        let _ = wrap_one_config(&p, &["tokenade"]);
        let _ = unwrap_one_config(&p);
        let after: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        let entry = &after["mcp"]["github"];
        assert_eq!(entry["command"], json!(["npx", "mcp-server-github"]));
        assert!(entry.get(ORIGINAL_KEY).is_none());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn skips_malformed_entry() {
        let p = tmp_config("bad", r#"{"mcpServers":{"weird":{"command":42}}}"#);
        let r = wrap_one_config(&p, &["tokenade"]);
        assert!(r.wrapped.is_empty());
        assert_eq!(r.skipped_unrecognised_shape, vec!["weird".to_string()]);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn preserves_unrelated_top_level_keys() {
        let raw = r#"{"unrelated":{"x":1},"mcpServers":{"github":{"command":"npx","args":["mcp-server-github"]}}}"#;
        let p = tmp_config("preserve", raw);
        let _ = wrap_one_config(&p, &["tokenade"]);
        let after: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(after["unrelated"]["x"], json!(1));
        let _ = std::fs::remove_file(&p);
    }

    // Symmetric uninstall: the self-entry that `install` writes
    // under `mcpServers` / `mcp` should be removed by `uninstall` so
    // agents stop trying to spawn a binary the user just removed.
    #[test]
    fn removes_tokenade_self_entry_under_mcp_servers() {
        let raw = r#"{"mcpServers":{"tokenade":{"command":"tokenade","args":["mcp"]},"github":{"command":"npx","args":["mcp-server-github"]}}}"#;
        let p = tmp_config("self_mcpservers", raw);
        let report = remove_self_entry_one_config(&p);
        assert!(report.removed);
        assert!(report.error.is_none());
        let after: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert!(after["mcpServers"].get("tokenade").is_none());
        assert_eq!(after["mcpServers"]["github"]["command"], json!("npx"));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn removes_tokenade_self_entry_under_top_level_mcp() {
        // OpenCode / Codex-style shape: top-level `mcp` key.
        let raw = r#"{"mcp":{"tokenade":{"command":["tokenade","mcp"]}}}"#;
        let p = tmp_config("self_topmcp", raw);
        let report = remove_self_entry_one_config(&p);
        assert!(report.removed);
        let after: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert!(after["mcp"].get("tokenade").is_none());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn self_entry_dry_run_does_not_touch_disk() {
        let raw = r#"{"mcpServers":{"tokenade":{"command":"tokenade","args":["mcp"]}}}"#;
        let p = tmp_config("self_dry", raw);
        let report = remove_self_entry_one_config_dry(&p);
        assert!(report.removed);
        // File should still have the entry — dry-run reports without mutating.
        let after = std::fs::read_to_string(&p).unwrap();
        assert!(after.contains("\"tokenade\""));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn self_entry_absent_reports_not_removed() {
        let raw = r#"{"mcpServers":{"github":{"command":"npx","args":["mcp-server-github"]}}}"#;
        let p = tmp_config("self_absent", raw);
        let report = remove_self_entry_one_config(&p);
        assert!(!report.removed);
        assert!(report.error.is_none());
        // File should be unchanged byte-for-byte.
        let after = std::fs::read_to_string(&p).unwrap();
        assert_eq!(after, raw);
        let _ = std::fs::remove_file(&p);
    }

    // --- zero-MCP pivot: format-aware purge helpers -------------------

    use crate::detect::McpConfigFormat;

    #[test]
    fn purge_json_mcp_servers_top_level() {
        let raw = r#"{"mcpServers":{"tokenade":{"command":"tokenade","args":["mcp"]},"github":{"command":"npx"}}}"#;
        let (out, removed) = purge_json_config(raw, McpConfigFormat::JsonMcpServers).unwrap();
        assert!(removed);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v["mcpServers"].get("tokenade").is_none());
        // A sibling server is preserved.
        assert!(v["mcpServers"].get("github").is_some());
    }

    #[test]
    fn purge_json_opencode_mcp_key() {
        // OpenCode keeps servers under `mcp`, command as an array.
        let raw = r#"{"mcp":{"tokenade":{"command":["tokenade","mcp"]}}}"#;
        let (out, removed) = purge_json_config(raw, McpConfigFormat::JsonOpenCodeMcp).unwrap();
        assert!(removed);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v["mcp"].get("tokenade").is_none());
    }

    #[test]
    fn purge_json_openclaw_nested_servers() {
        let raw = r#"{"mcp":{"servers":{"tokenade":{"command":"tokenade","args":["mcp"]}}}}"#;
        let (out, removed) = purge_json_config(raw, McpConfigFormat::JsonOpenClawMcp).unwrap();
        assert!(removed);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v["mcp"]["servers"].get("tokenade").is_none());
    }

    #[test]
    fn purge_json_claude_json_per_project_maps() {
        // ~/.claude.json carries top-level AND per-project mcpServers.
        let raw = r#"{
            "mcpServers":{"tokenade":{"command":"tokenade","args":["mcp"]}},
            "projects":{
                "/home/u/a":{"mcpServers":{"tokenade":{"command":"tokenade"}}},
                "/home/u/b":{"mcpServers":{"other":{"command":"x"}}}
            }
        }"#;
        let (out, removed) = purge_json_config(raw, McpConfigFormat::JsonMcpServers).unwrap();
        assert!(removed);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v["mcpServers"].get("tokenade").is_none());
        assert!(v["projects"]["/home/u/a"]["mcpServers"]
            .get("tokenade")
            .is_none());
        // Unrelated per-project server untouched.
        assert!(v["projects"]["/home/u/b"]["mcpServers"]
            .get("other")
            .is_some());
    }

    #[test]
    fn purge_json_idempotent_when_absent() {
        let raw = r#"{"mcpServers":{"github":{"command":"npx"}}}"#;
        let (out, removed) = purge_json_config(raw, McpConfigFormat::JsonMcpServers).unwrap();
        assert!(!removed);
        // Returned verbatim — no reserialisation churn.
        assert_eq!(out, raw);
    }

    #[test]
    fn purge_hermes_yaml_block() {
        let raw = "mcp_servers:\n  tokenade:\n    command: tokenade\n    args:\n      - mcp\n  github:\n    command: npx\n";
        let (out, removed) = purge_hermes_yaml(raw).unwrap();
        assert!(removed);
        assert!(!out.contains("tokenade"));
        // Sibling block and its body survive.
        assert!(out.contains("github:"));
        assert!(out.contains("command: npx"));
        // Trailing newline preserved.
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn purge_hermes_yaml_idempotent_when_absent() {
        let raw = "mcp_servers:\n  github:\n    command: npx\n";
        let (out, removed) = purge_hermes_yaml(raw).unwrap();
        assert!(!removed);
        assert_eq!(out, raw);
    }

    fn assert_enabled(content: &str) {
        // the result must parse-equivalently contain `- tokenade` under enabled
        assert!(
            content.contains("tokenade"),
            "expected tokenade enabled in:\n{content}"
        );
    }

    #[test]
    fn enable_plugin_absent_config_creates_block() {
        let (out, changed) = enable_plugin_in_hermes_config("", "tokenade");
        assert!(changed);
        assert_eq!(out, "plugins:\n  enabled:\n    - tokenade\n");
    }

    #[test]
    fn enable_plugin_appends_to_unrelated_config() {
        let (out, changed) = enable_plugin_in_hermes_config("model: claude-x\n", "tokenade");
        assert!(changed);
        assert!(out.starts_with("model: claude-x\n"));
        assert_enabled(&out);
    }

    #[test]
    fn enable_plugin_block_list_adds_item() {
        let raw = "plugins:\n  enabled:\n    - other\n";
        let (out, changed) = enable_plugin_in_hermes_config(raw, "tokenade");
        assert!(changed);
        assert!(out.contains("- other"));
        assert!(out.contains("    - tokenade"));
    }

    #[test]
    fn enable_plugin_idempotent_when_already_listed() {
        let raw = "plugins:\n  enabled:\n    - tokenade\n    - other\n";
        let (out, changed) = enable_plugin_in_hermes_config(raw, "tokenade");
        assert!(!changed);
        assert_eq!(out, raw);
    }

    #[test]
    fn enable_plugin_plugins_block_without_enabled() {
        let raw = "plugins:\n  disabled:\n    - foo\n";
        let (out, changed) = enable_plugin_in_hermes_config(raw, "tokenade");
        assert!(changed);
        assert!(out.contains("enabled:"));
        assert!(out.contains("- tokenade"));
        assert!(
            out.contains("disabled:"),
            "must preserve the existing block"
        );
    }

    #[test]
    fn enable_plugin_flow_list() {
        let (out, changed) =
            enable_plugin_in_hermes_config("plugins:\n  enabled: [a, b]\n", "tokenade");
        assert!(changed);
        assert!(out.contains("enabled: [a, b, tokenade]"));
        // idempotent on flow style too
        let (out2, changed2) = enable_plugin_in_hermes_config(&out, "tokenade");
        assert!(!changed2);
        assert_eq!(out2, out);
    }

    /// `claude mcp add` defaults to the `local` scope, which stores the
    /// server under `projects["<cwd>"].mcpServers` in `~/.claude.json` —
    /// the single most common place a Claude Code user's MCP servers live.
    /// The wrap path only ever read the top level, so `mcp-proxy install`
    /// reported success and wrapped nothing. Round-trip, at the real entry
    /// point.
    #[test]
    fn wrap_reaches_per_project_claude_servers() {
        let dir = std::env::temp_dir().join(format!(
            "tkn-proj-wrap-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("claude.json");
        std::fs::write(
            &cfg,
            r#"{"projects":{"/home/u/app":{"mcpServers":{"github":{"command":"npx","args":["-y","gh-mcp"]}}}}}"#,
        )
        .unwrap();

        let r = wrap_one_config(&cfg, &[]);
        assert_eq!(r.wrapped, vec!["github".to_string()], "{r:?}");

        let v: Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        let entry = &v["projects"]["/home/u/app"]["mcpServers"]["github"];
        assert!(entry.get(ORIGINAL_KEY).is_some(), "original snapshot kept");
        assert!(
            entry["args"][0].as_str().unwrap() == WRAPPER_SUBCOMMAND,
            "proxied through the wrapper: {entry}"
        );

        let r2 = unwrap_one_config(&cfg);
        assert_eq!(r2.unwrapped, vec!["github".to_string()], "{r2:?}");
        let v2: Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        let back = &v2["projects"]["/home/u/app"]["mcpServers"]["github"];
        assert_eq!(back["command"], "npx", "user command restored verbatim");
        assert_eq!(back["args"][1], "gh-mcp");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `mcp-proxy install` and `tokenade install` reach the wrap through
    /// DIFFERENT functions. Only the first learned TOML, so a full install on
    /// a machine with a Codex config printed
    /// `parse: expected value at line 1 column 1` and wrapped nothing —
    /// caught by running the real installer against a fixture home, not by
    /// any unit test, because both halves were tested through the half that
    /// worked.
    /// An empty placeholder config is not a broken one, and a config that is
    /// genuinely corrupt must STILL say so — the second half is the point.
    ///
    /// Found on a real machine 2026-08-25: `~/.gemini/config/mcp_config.json`
    /// was 0 bytes, dated 9 juin, and every `tokenade install` reported
    /// `parse: EOF while parsing a value at line 1 column 0` about it. A
    /// permanent error nobody can act on trains the reader to skip that line,
    /// and the next one is about a config that really is corrupt.
    #[test]
    fn an_empty_config_is_no_config_but_a_corrupt_one_still_speaks() {
        let dir = std::env::temp_dir().join(format!(
            "tkn-empty-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        for (name, body) in [("empty.json", ""), ("blank.json", "  \n\t\n")] {
            let cfg = dir.join(name);
            std::fs::write(&cfg, body).unwrap();
            let v = read_config_root(&cfg, "").expect("an empty file is a config with no servers");
            assert_eq!(v, serde_json::json!({}), "{name}");
            // And the placeholder is left exactly as it was found.
            assert_eq!(
                std::fs::read_to_string(&cfg).unwrap(),
                body,
                "{name} rewritten"
            );
        }

        let broken = dir.join("broken.json");
        std::fs::write(&broken, "{\"mcpServers\": {").unwrap();
        let e = read_config_root(&broken, " — left untouched")
            .expect_err("a truncated config must still be reported");
        assert!(e.starts_with("parse: "), "{e}");
        assert!(
            e.ends_with(" — left untouched"),
            "the caller's note survives: {e}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A targeted pass must NAME its backup, whatever the config format.
    /// The TOML branch took one and reported none, so a single
    /// `mcp-proxy install` told a Gemini user where to undo and a Codex user
    /// nothing — on the same run, two lines apart.
    #[test]
    fn toml_pass_reports_the_backup_it_takes() {
        let dir = std::env::temp_dir().join(format!(
            "tkn-bak-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.toml");
        std::fs::write(
            &cfg,
            "[mcp_servers.gh]\ncommand = \"npx\"\nargs = [\"gh-mcp\"]\n",
        )
        .unwrap();

        let r = toml_config_pass(&cfg, None, true, false, WrapReport::default())
            .expect("a .toml path belongs to the toml pass");
        assert_eq!(r.wrapped, vec!["gh".to_string()], "{r:?}");
        let bak = r.backup.expect("the backup must be named, not just taken");
        assert!(
            std::path::Path::new(&bak).exists(),
            "and it must actually be there: {bak}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `install` and `healthcheck` must agree on how many entries are
    /// wrapped. They did not: the inspector was JSON-only, so a Codex user
    /// saw "1 wrapped" while two servers had in fact been wrapped. A wrap
    /// nobody can see is a wrap nobody can diagnose.
    #[test]
    fn inspector_sees_toml_and_per_project_wraps() {
        let dir = std::env::temp_dir().join(format!(
            "tkn-inspect-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let toml_cfg = dir.join("config.toml");
        std::fs::write(
            &toml_cfg,
            "[mcp_servers.gh]\ncommand = \"npx\"\nargs = [\"gh-mcp\"]\n",
        )
        .unwrap();
        assert_eq!(
            wrap_one_config(&toml_cfg, &[]).wrapped,
            vec!["gh".to_string()]
        );

        let json_cfg = dir.join("claude.json");
        std::fs::write(
            &json_cfg,
            r#"{"projects":{"/p":{"mcpServers":{"pg":{"command":"npx","args":["pg-mcp"]}}}}}"#,
        )
        .unwrap();
        assert_eq!(
            wrap_one_config(&json_cfg, &[]).wrapped,
            vec!["pg".to_string()]
        );

        // The inspector walks discovery sites, not arbitrary paths, so drive
        // its two readers directly on the files just written.
        let toml_raw = std::fs::read_to_string(&toml_cfg).unwrap();
        let doc: toml::Value = toml::from_str(&toml_raw).unwrap();
        assert!(
            doc["mcp_servers"]["gh"].get(ORIGINAL_KEY).is_some(),
            "TOML wrap must be visible to a TOML reader"
        );
        let mut json_root: Value =
            serde_json::from_str(&std::fs::read_to_string(&json_cfg).unwrap()).unwrap();
        let mut seen = 0;
        for_each_server_map(&mut json_root, |m| {
            seen += m.values().filter(|v| v.get(ORIGINAL_KEY).is_some()).count();
        });
        assert_eq!(seen, 1, "per-project wrap must be visible to the walker");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A dotfiles-managed config is published as a SYMLINK into the manager's
    /// tree. Renaming a tmp file onto the link replaces the link, so the agent
    /// silently starts reading a file the manager does not own. Measured with
    /// symlinked Codex and Gemini configs: both came back as regular files.
    #[cfg(unix)]
    #[test]
    fn wrapping_writes_through_a_symlinked_config() {
        let dir = std::env::temp_dir().join(format!(
            "tkn-link-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let store = dir.join("dotfiles");
        std::fs::create_dir_all(&store).unwrap();
        let real = store.join("settings.json");
        std::fs::write(
            &real,
            r#"{"mcpServers":{"gh":{"command":"npx","args":["gh-mcp"]}}}"#,
        )
        .unwrap();
        let link = dir.join("settings.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let r = wrap_one_config(&link, &[]);
        assert_eq!(r.wrapped, vec!["gh".to_string()], "{r:?}");
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the link must survive the write"
        );
        assert!(
            std::fs::read_to_string(&real)
                .unwrap()
                .contains(WRAPPER_SUBCOMMAND),
            "the managed file itself must be the one that changed"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A save must not widen a file the user tightened. `rename` hands the
    /// destination the TMP file's umask-derived mode, so every atomic writer
    /// has to carry the old mode across — `paths::write_atomic` did, this one
    /// did not, and a 0600 agent config came back 0644.
    #[cfg(unix)]
    #[test]
    fn wrapping_preserves_a_tightened_file_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!(
            "tkn-mode-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("mcp.json");
        std::fs::write(
            &cfg,
            r#"{"mcpServers":{"gh":{"command":"npx","args":["gh-mcp"]}}}"#,
        )
        .unwrap();
        std::fs::set_permissions(&cfg, std::fs::Permissions::from_mode(0o600)).unwrap();

        let r = wrap_one_config(&cfg, &[]);
        assert_eq!(r.wrapped, vec!["gh".to_string()], "{r:?}");
        let mode = std::fs::metadata(&cfg).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "owner-only config must stay owner-only");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Reasonix keeps its servers in an ARRAY of tables, not in named
    /// sections: the server name is a KEY of the entry, so
    /// the entry is identified by its position. Also checked against the real
    /// `reasonix mcp list`, which reads back the wrapped entry then the original one
    /// after `mcp-wrap uninstall` (2026-08-21, reasonix v1.30.0).
    #[test]
    fn install_time_wrap_handles_a_toml_plugins_array() {
        let dir = std::env::temp_dir().join(format!(
            "tkn-inst-plugarr-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.toml");
        // `name` AFTER `command` in the second entry: the position in the
        // array is what identifies the entry, not the key order.
        let raw = concat!(
            "default_model = \"deepseek-flash\"\n\n",
            "[[plugins]]\nname = \"github\"\ncommand = \"npx\"\nargs = [\"-y\", \"gh-mcp\"]\n\n",
            "[[plugins]]\ncommand = \"node\"\nargs = [\"srv.mjs\"]\nname = \"local\"\n",
        );
        std::fs::write(&cfg, raw).unwrap();

        let r = wrap_one_config(&cfg, &[]);
        assert_eq!(
            r.wrapped,
            vec!["github".to_string(), "local".to_string()],
            "{r:?}"
        );
        assert!(r.error.is_none(), "{r:?}");
        let wrapped = std::fs::read_to_string(&cfg).unwrap();
        assert!(
            wrapped.contains("mcp-proxy"),
            "the launch must go through the wrapper: {wrapped}"
        );
        // The file stays valid TOML, otherwise the agent can no longer read it.
        let parsed: toml::Value = toml::from_str(&wrapped).expect("still valid TOML");
        assert_eq!(
            parsed["plugins"].as_array().unwrap().len(),
            2,
            "no entry gained or lost"
        );

        let r2 = unwrap_one_config(&cfg);
        assert_eq!(
            r2.unwrapped,
            vec!["github".to_string(), "local".to_string()],
            "{r2:?}"
        );
        let back: toml::Value = toml::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        let entries = back["plugins"].as_array().unwrap();
        assert_eq!(entries[0]["command"].as_str(), Some("npx"));
        assert_eq!(entries[1]["command"].as_str(), Some("node"));
        assert!(
            entries.iter().all(|e| e.get(ORIGINAL_KEY).is_none()),
            "the wrapper's own key must be gone after unwrap"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Our own entry does not wrap itself, and `uninstall` removes it
    /// entirely — sub-table included.
    #[test]
    fn a_plugins_array_skips_and_purges_our_own_entry() {
        let raw = concat!(
            "[[plugins]]\nname = \"tokenade\"\ncommand = \"tokenade\"\nargs = [\"mcp\"]\n\n",
            "[plugins.env]\nTOKENADE_AGENT = \"reasonix\"\n\n",
            "[[plugins]]\nname = \"github\"\ncommand = \"npx\"\nargs = [\"gh-mcp\"]\n",
        );
        let mut rep = WrapReport::default();
        transform_toml_plugins_array(raw, None, true, &mut rep).unwrap();
        assert_eq!(rep.skipped_self, vec!["tokenade".to_string()], "{rep:?}");

        let (purged, removed) = purge_reasonix_toml(raw).unwrap();
        assert!(removed, "our entry must be found");
        assert!(
            !purged.contains("tokenade"),
            "nothing of ours survives: {purged}"
        );
        assert!(
            purged.contains("github"),
            "the user's entry stays: {purged}"
        );
        let parsed: toml::Value = toml::from_str(&purged).expect("still valid TOML");
        assert_eq!(parsed["plugins"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn install_time_wrap_handles_toml_configs() {
        let dir = std::env::temp_dir().join(format!(
            "tkn-inst-toml-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.toml");
        let raw = "model = \"gpt-5.5\"\n\n[mcp_servers.github]\ncommand = \"npx\"\nargs = [\"-y\", \"gh-mcp\"]\n";
        std::fs::write(&cfg, raw).unwrap();

        let r = wrap_one_config(&cfg, &[]);
        assert_eq!(r.wrapped, vec!["github".to_string()], "{r:?}");
        assert!(r.error.is_none(), "{r:?}");

        let r2 = unwrap_one_config(&cfg);
        assert_eq!(r2.unwrapped, vec!["github".to_string()], "{r2:?}");
        assert_eq!(
            std::fs::read_to_string(&cfg).unwrap(),
            raw,
            "round trip is byte-identical"
        );

        // Dry-run previews without touching the file.
        let before = std::fs::read_to_string(&cfg).unwrap();
        let r3 = wrap_one_config_dry(&cfg, &[]);
        assert_eq!(r3.wrapped, vec!["github".to_string()]);
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The TOML shapes take the same rule from the same declarant, so an edit
    /// made while wrapped survives there too — Codex and Reasonix users get the
    /// same contract as Claude Code users.
    #[test]
    fn a_toml_edit_made_while_wrapped_survives_the_unwrap() {
        let raw = "# mine\n[mcp_servers.github]\ncommand = \"tokenade\"\n\
                   args = [\"mcp-proxy\", \"--\", \"npx\", \"-y\", \"gh-mcp@2\"]\n\
                   tokenadeOriginal = { command = \"npx\", args = [\"-y\", \"gh-mcp@1\"] }\n";
        let mut rep = WrapReport::default();
        let out = transform_toml_mcp_servers(raw, None, false, &mut rep)
            .unwrap()
            .expect("something to write");
        assert_eq!(rep.unwrapped, vec!["github".to_string()]);
        assert!(
            out.contains("gh-mcp@2"),
            "the version the entry launches today must survive: {out}"
        );
        assert!(
            !out.contains("gh-mcp@1"),
            "the snapshot must not overwrite it: {out}"
        );
        assert!(out.contains("# mine"), "user comments survive");

        // No readable tail — a hand-edited or half-written entry. The snapshot
        // is what keeps it restorable at all, so the fallback must still fire
        // on this path and not only on the JSON one.
        let raw = "[mcp_servers.github]\ncommand = \"tokenade\"\n\
                   args = [\"mcp-proxy\"]\n\
                   tokenadeOriginal = { command = \"npx\", args = [\"-y\", \"gh-mcp@1\"] }\n";
        let mut rep = WrapReport::default();
        let out = transform_toml_mcp_servers(raw, None, false, &mut rep)
            .unwrap()
            .expect("something to write");
        assert_eq!(rep.unwrapped, vec!["github".to_string()]);
        assert!(
            out.contains("command = \"npx\"") && out.contains("gh-mcp@1"),
            "with no tail to read, the snapshot is the answer: {out}"
        );
    }

    #[test]
    fn toml_wrap_round_trips_and_keeps_comments() {
        let raw = "# my config\nmodel = \"gpt-5.5\"\n\n[mcp_servers.github]\n# keep me\ncommand = \"npx\"\nargs = [\"-y\", \"gh-mcp\"]\n\n[mcp_servers.tokenade]\ncommand = \"tokenade\"\n";
        let mut rep = WrapReport::default();
        let wrapped = transform_toml_mcp_servers(raw, None, true, &mut rep)
            .unwrap()
            .expect("something to write");
        assert_eq!(rep.wrapped, vec!["github".to_string()]);
        assert_eq!(rep.skipped_self, vec!["tokenade".to_string()]);
        assert!(wrapped.contains("# my config"), "user comments survive");
        assert!(wrapped.contains("# keep me"), "in-section comments survive");
        assert!(wrapped.contains("model = \"gpt-5.5\""));
        assert!(wrapped.contains(ORIGINAL_KEY), "snapshot written");
        assert!(wrapped.contains("mcp-proxy"), "routed through the proxy");
        // Exactly one `args` key in the section — the user's line is
        // replaced, not duplicated (a duplicate key makes the file
        // unparseable, and the agent would refuse the whole config).
        assert_eq!(wrapped.matches("args = ").count(), 2, "{wrapped}");
        assert!(
            toml::from_str::<toml::Value>(&wrapped).is_ok(),
            "still valid TOML"
        );

        let mut rep2 = WrapReport::default();
        let back = transform_toml_mcp_servers(&wrapped, None, false, &mut rep2)
            .unwrap()
            .expect("unwrap writes");
        assert_eq!(rep2.unwrapped, vec!["github".to_string()]);
        assert!(back.contains("command = \"npx\""));
        assert!(back.contains("[\"-y\", \"gh-mcp\"]"));
        assert!(!back.contains(ORIGINAL_KEY), "snapshot removed");
        assert!(back.contains("# keep me"));
    }

    #[test]
    fn toml_wrap_is_idempotent_and_skips_unknown_shapes() {
        let raw = "[mcp_servers.weird]\ncommand = 42\n";
        let mut rep = WrapReport::default();
        assert!(
            transform_toml_mcp_servers(raw, None, true, &mut rep)
                .unwrap()
                .is_none(),
            "nothing to write for a shape we do not understand"
        );
        assert_eq!(rep.skipped_unrecognised_shape, vec!["weird".to_string()]);

        let ok = "[mcp_servers.a]\ncommand = \"x\"\nargs = []\n";
        let mut r1 = WrapReport::default();
        let once = transform_toml_mcp_servers(ok, None, true, &mut r1)
            .unwrap()
            .unwrap();
        let mut r2 = WrapReport::default();
        assert!(
            transform_toml_mcp_servers(&once, None, true, &mut r2)
                .unwrap()
                .is_none(),
            "second wrap is a no-op"
        );
        assert_eq!(r2.skipped_already_wrapped, vec!["a".to_string()]);
    }

    #[test]
    fn purge_grok_toml_section() {
        let raw = "[mcp_servers.tokenade]\ncommand = \"tokenade\"\nargs = [\"mcp\"]\n\n[mcp_servers.github]\ncommand = \"npx\"\n";
        let (out, removed) = purge_grok_toml(raw).unwrap();
        assert!(removed);
        assert!(!out.contains("tokenade"));
        // The next section is kept intact.
        assert!(out.contains("[mcp_servers.github]"));
        assert!(out.contains("command = \"npx\""));
    }

    /// End-to-end through the real entry point: the Codex arm must resolve
    /// to `~/.codex/config.toml` AND pick the TOML purge. Before this, it
    /// pointed at `config.json` — a file codex-cli does not read (measured on
    /// 0.144.5) — so a legacy `[mcp_servers.tokenade]` survived forever and
    /// kept the agent spawning the post-pivot `tokenade mcp` command.
    #[test]
    fn purge_reaches_codex_toml_and_spares_user_settings() {
        let home = std::env::temp_dir().join(format!(
            "tkn-codex-purge-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let dir = home.join(".codex");
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.toml");
        std::fs::write(
            &cfg,
            "model = \"gpt-5.5\"\n\n[mcp_servers.tokenade]\ncommand = \"tokenade\"\nargs = [\"mcp\"]\n\n[mcp_servers.github]\ncommand = \"npx\"\n",
        )
        .unwrap();

        let reports = purge_tokenade_mcp_entries(&home, Some(&home), false);
        assert!(
            reports
                .iter()
                .any(|r| r.config_path.ends_with("config.toml") && r.removed),
            "codex config.toml was never reached: {reports:?}"
        );

        let after = std::fs::read_to_string(&cfg).unwrap();
        assert!(!after.contains("mcp_servers.tokenade"));
        assert!(
            after.contains("model = \"gpt-5.5\""),
            "user settings survive"
        );
        assert!(
            after.contains("[mcp_servers.github]"),
            "other servers survive"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn purge_grok_toml_section_at_eof() {
        let raw =
            "[other]\nx = 1\n\n[mcp_servers.tokenade]\ncommand = \"tokenade\"\nargs = [\"mcp\"]\n";
        let (out, removed) = purge_grok_toml(raw).unwrap();
        assert!(removed);
        assert!(!out.contains("tokenade"));
        assert!(out.contains("[other]"));
    }

    #[test]
    fn purge_grok_toml_drops_env_subtable() {
        // The writer emits both the section AND a `.env` sub-table; the
        // sub-table must not survive as an orphan.
        let raw = "[cli]\nx = 1\n\n[mcp_servers.tokenade]\ncommand = \"tokenade\"\nargs = [\"mcp\"]\n\n[mcp_servers.tokenade.env]\nTOKENADE_DEBUG_LOG = \"/x\"\n\n[ui]\ny = 2\n";
        let (out, removed) = purge_grok_toml(raw).unwrap();
        assert!(removed);
        assert!(!out.contains("tokenade"));
        assert!(out.contains("[cli]"));
        assert!(out.contains("[ui]"));
        assert!(out.contains("y = 2"));
    }

    #[test]
    fn purge_grok_toml_drops_orphan_env_subtable_only() {
        // A config left in the broken state (only the `.env` orphan
        // remains) is still fully cleaned.
        let raw = "[ui]\ny = 2\n\n[mcp_servers.tokenade.env]\nTOKENADE_DEBUG_LOG = \"/x\"\n";
        let (out, removed) = purge_grok_toml(raw).unwrap();
        assert!(removed);
        assert!(!out.contains("tokenade"));
        assert!(out.contains("[ui]"));
    }

    #[test]
    fn purge_grok_toml_idempotent_when_absent() {
        let raw = "[mcp_servers.github]\ncommand = \"npx\"\n";
        let (out, removed) = purge_grok_toml(raw).unwrap();
        assert!(!removed);
        assert_eq!(out, raw);
    }

    #[test]
    fn purge_one_config_dry_run_does_not_write() {
        let raw = r#"{"mcpServers":{"tokenade":{"command":"tokenade","args":["mcp"]}}}"#;
        let p = tmp_config("purge_dry", raw);
        let report = purge_one_config(&p, McpConfigFormat::JsonMcpServers, true);
        assert!(report.removed);
        assert!(report.error.is_none());
        // File still has the entry — dry-run reports without mutating.
        let after = std::fs::read_to_string(&p).unwrap();
        assert!(after.contains("\"tokenade\""));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn purge_one_config_writes_when_present() {
        let raw = r#"{"mcpServers":{"tokenade":{"command":"tokenade","args":["mcp"]},"github":{"command":"npx"}}}"#;
        let p = tmp_config("purge_write", raw);
        let report = purge_one_config(&p, McpConfigFormat::JsonMcpServers, false);
        assert!(report.removed);
        assert!(report.error.is_none());
        let after: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert!(after["mcpServers"].get("tokenade").is_none());
        assert!(after["mcpServers"].get("github").is_some());
        let _ = std::fs::remove_file(&p);
    }
}
#[cfg(test)]
mod site_list_matches_detect {
    use super::*;
    use crate::detect::Agent;

    /// Every agent whose MCP config path the product can NAME must have a site
    /// the wrapper can WRITE.
    ///
    /// Two tables described the same fact and had drifted: `detect` answered
    /// `~/.config/opencode/mcp.json` for opencode while
    /// `discover_mcp_config_sites` had no opencode entry at all, so
    /// `mcp-wrap install` said "no MCP servers to wrap were found" about a file
    /// it had just been asked about. Measured 2026-08-20 on four agents —
    /// opencode, kilo-code, hermes, cline — and it cost the MCP box a `Partial`
    /// on each, on all three OS.
    ///
    /// Compared on the FILE NAME rather than the full path: the two tables
    /// resolve their roots differently (one takes a home, the other an
    /// `McpDiscoveryEnv`), and pinning the whole path here would assert the
    /// resolution rather than the coverage.
    #[test]
    fn every_agent_with_a_config_path_has_a_site() {
        let home = std::path::Path::new("/home/probe");
        let env = McpDiscoveryEnv {
            home: home.to_path_buf(),
            cwd: std::path::PathBuf::from("/proj"),
            os: OsKind::Linux,
        };
        let sites = discover_mcp_config_sites(&env);
        let names: Vec<String> = sites
            .iter()
            .filter_map(|s| s.path.file_name().and_then(|n| n.to_str()))
            .map(str::to_string)
            .collect();

        let mut missing = Vec::new();
        for &a in Agent::all() {
            // An agent a PLUGIN already covers must NOT have a site: the bridge
            // folds its MCP results inside the agent, and wrapping the server
            // too would be a second authority on the same bytes. That rule is
            // asserted from the other side by
            // `an_agent_with_a_plugin_is_not_also_mcp_wrapped`, and the two
            // tests together say the whole thing: cover every path we may
            // write, and never write where a bridge already reads.
            if matches!(a.coverage(), crate::detect::Coverage::Plugin) {
                continue;
            }
            let Some(p) = a.mcp_config_path(std::path::Path::new("/proj"), Some(home)) else {
                continue;
            };
            let Some(file) = p.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !names.iter().any(|n| n == file) {
                missing.push(format!("{} → {}", a.as_str(), p.display()));
            }
        }
        assert!(
            missing.is_empty(),
            "`detect` names a config these agents keep, and `discover_mcp_config_sites` \
             cannot write it:\n  {}",
            missing.join("\n  ")
        );
    }
}
#[cfg(test)]
mod bom_tolerance {
    use super::*;

    const BOM: &str = "\u{feff}";
    const CFG: &str = r#"{"mcpServers":{"demo":{"command":"node","args":["s.js"]}}}"#;

    /// A UTF-8 BOM must not make an otherwise valid agent config unreadable.
    /// PowerShell writes one by default; `json_util` already stripped it on the
    /// install side, so without this the SAME file parsed for one half of the
    /// product and not the other.
    #[test]
    fn a_bom_does_not_hide_a_valid_config() {
        let clean = parse_json_tolerant(CFG).expect("baseline parses");
        let bommed = parse_json_tolerant(&format!("{BOM}{CFG}")).expect("a BOM is not corruption");
        assert_eq!(clean, bommed, "the BOM changed what we read");
        assert!(bommed["mcpServers"]["demo"]["command"].is_string());
    }

    /// …and it must not turn corruption into silence: only the BOM is
    /// forgiven, everything else still fails loudly.
    #[test]
    fn tolerance_stops_at_the_bom() {
        assert!(parse_json_tolerant("{ this is not json").is_err());
        assert!(parse_json_tolerant(&format!("{BOM}{{ nope")).is_err());
        // A BOM in the MIDDLE is not a leading BOM and is genuinely invalid.
        assert!(parse_json_tolerant(&format!("{{{BOM}\"a\":1}}")).is_err());
    }

    /// `read_config_root` is the reader every wrap/unwrap path goes through.
    #[test]
    fn read_config_root_reads_a_bommed_file_from_disk() {
        let dir = std::env::temp_dir().join(format!(
            "tokenade-bomcfg-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("mcp.json");
        std::fs::write(&p, format!("{BOM}{CFG}")).unwrap();
        let v = read_config_root(&p, "").expect("a BOM'd config is readable");
        assert_eq!(
            v["mcpServers"]["demo"]["command"].as_str(),
            Some("node"),
            "the server behind the BOM was not seen"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
    /// A VS Code config is JSONC and tokenade must SAY so.
    #[test]
    fn a_jsonc_config_is_named_not_just_refused() {
        let dir = std::env::temp_dir().join(format!("tokenade-jsonc-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        for (body, want) in [
            (
                "{\n  // the demo server\n  \"mcpServers\": {\"demo\": {\"command\": \"node\"}}\n}",
                "`//` comment",
            ),
            (
                "{/* servers */\"mcpServers\":{\"demo\":{\"command\":\"node\"}}}",
                "`/* */` comment",
            ),
            (
                "{\"mcpServers\":{\"demo\":{\"command\":\"node\"}},}",
                "trailing comma",
            ),
        ] {
            let p = dir.join("mcp.json");
            std::fs::write(&p, body).unwrap();
            let err = read_config_root(&p, " — left untouched")
                .expect_err("JSONC is not JSON and must still be refused");
            assert!(err.contains(want), "the cause is not named: {err}");
            assert!(
                err.contains("left untouched"),
                "the caller's own note must survive: {err}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// …and a `//` inside a STRING is a URL, not a comment. This is why the
    /// detector cannot be a `contains`.
    #[test]
    fn a_url_in_a_value_is_not_mistaken_for_a_comment() {
        assert_eq!(
            looks_like_jsonc(r#"{"mcpServers":{"demo":{"url":"https://example.com/a"}}}"#),
            None
        );
        assert_eq!(
            looks_like_jsonc(r#"{"a":"/* not a comment */","b":"x,"}"#),
            None
        );
        // A comma before a closing brace only counts when nothing follows it.
        assert_eq!(looks_like_jsonc(r#"{"a":1,"b":2}"#), None);
    }

    /// TOML agents are the SAME file family, and their failure is the SILENT
    /// one.
    ///
    /// `toml::from_str` accepts a BOM, so the parse succeeds and
    /// `mcp_servers` is found — but both TOML transformers then do a
    /// LINE-BASED rewrite, and the first line reads `\u{feff}[mcp_servers.…]`,
    /// which matches no `[` header. Nothing is wrapped, nothing is written,
    /// no error is raised.
    #[test]
    fn a_bom_does_not_silently_skip_a_toml_config() {
        const T: &str = "[mcp_servers.demo]\ncommand = \"node\"\nargs = [\"s.js\"]\n";
        let mut r1 = WrapReport::default();
        let plain = transform_toml_mcp_servers(T, None, true, &mut r1)
            .expect("baseline parses")
            .expect("baseline wraps");
        let mut r2 = WrapReport::default();
        let bommed = transform_toml_mcp_servers(&format!("{BOM}{T}"), None, true, &mut r2)
            .expect("a BOM is not corruption")
            .expect("a BOM must not silently skip the wrap");
        assert_eq!(
            bommed.strip_prefix(BOM),
            Some(plain.as_str()),
            "apart from the BOM the two rewrites must be identical"
        );
    }

    /// The plugins-array shape (Reasonix) is the second TOML transformer and
    /// has the same line-based half.
    #[test]
    fn a_bom_does_not_silently_skip_a_toml_plugins_array() {
        const T: &str = "[[plugins]]\nname = \"demo\"\ncommand = \"node\"\n";
        let mut r1 = WrapReport::default();
        let plain = transform_toml_plugins_array(T, None, true, &mut r1).expect("baseline parses");
        let mut r2 = WrapReport::default();
        let bommed = transform_toml_plugins_array(&format!("{BOM}{T}"), None, true, &mut r2)
            .expect("a BOM is not corruption");
        assert_eq!(
            plain.is_some(),
            bommed.is_some(),
            "a BOM changed whether this shape is recognised at all"
        );
        if let (Some(p), Some(b)) = (plain, bommed) {
            assert_eq!(b.strip_prefix(BOM), Some(p.as_str()));
        }
    }

    /// The UNWRAP side has the same line-based half, and there a miss leaves
    /// the install HALF removed.
    ///
    /// `str::trim` is no help: U+FEFF is a format character (Cf), not
    /// whitespace, so `lines[0].trim() == "[[plugins]]"` stays false.
    #[test]
    fn a_bom_does_not_leave_a_toml_entry_behind_on_unwrap() {
        let wrapped = "[[plugins]]\nname = \"tokenade\"\ncommand = \"tokenade\"\n\
                       [[plugins]]\nname = \"keepme\"\ncommand = \"node\"\n";
        let (plain, removed_plain) = purge_reasonix_toml(wrapped).expect("baseline");
        let (bommed, removed_bom) =
            purge_reasonix_toml(&format!("{BOM}{wrapped}")).expect("a BOM is not corruption");
        assert!(removed_plain, "the fixture must have something to remove");
        assert_eq!(
            removed_plain, removed_bom,
            "a BOM changed whether the entry was found at all"
        );
        assert_eq!(bommed.strip_prefix(BOM), Some(plain.as_str()));
        assert!(bommed.contains("keepme"), "the other entry must survive");
    }

    /// The BOM is PRESERVED, not dropped. The file belongs to the agent, and
    /// silently changing bytes we were not asked to change is the habit this
    /// module exists to avoid.
    #[test]
    fn the_bom_survives_the_rewrite() {
        const T: &str = "[mcp_servers.demo]\ncommand = \"node\"\nargs = [\"s.js\"]\n";
        let mut r = WrapReport::default();
        let out = transform_toml_mcp_servers(&format!("{BOM}{T}"), None, true, &mut r)
            .expect("parses")
            .expect("wraps");
        assert!(
            out.starts_with(BOM),
            "the agent's BOM was dropped:\n{out:?}"
        );
    }
}
