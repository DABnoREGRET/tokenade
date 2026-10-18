// The tests of `crate::detect`, moved out of the production file.
//
// Mounted by `include!`: the tokens are spliced INTO `detect`, so every
// `super::` keeps its meaning. Split by `srcsweep::production_lines`.

#[cfg(test)]
mod tests {
    use super::*;

    /// A presence marker that is not absolute can never match: it is
    /// compared against real paths on disk. The Windows arms were written
    /// as bare `AppData/Roaming/...`, which `resolve` treats as
    /// root-relative (and root is empty here), so every Windows-specific
    /// marker resolved to a relative path and silently never matched —
    /// `detect_all_present` simply did not see those agents on Windows.
    /// The set of agents that get NOTHING automatic. Named explicitly
    /// rather than counted, so adding an agent forces a deliberate answer
    /// to "and what covers this one?" instead of quietly growing the
    /// silent-zero population.
    /// "Nothing automatic covers unknown" is not a sentence a user can act
    /// on. An unrecognised agent and an agent we know is unwired are
    /// different problems with different fixes, and the screens that report
    /// a zero must not merge them.
    #[test]
    fn an_unrecognised_agent_is_not_an_uncovered_one() {
        assert_eq!(Agent::Unknown.coverage(), Coverage::AgentUnknown);
        // This test needs an agent with genuinely NO channel, and the example
        // has now moved TWICE for the same reason: Windsurf left on 2026-08-08
        // when `coverage()` learnt to report the MCP proxy it could already
        // wire, and Aider left on 2026-08-11 when `ShellProxy` was added — its
        // `/run` really is folded. Odysseus is what remains, and it remains
        // only because nobody has checked how it runs commands. If somebody
        // does, this example moves a third time, which is the point: the set is
        // shrinking because the product covers more, not because the test is
        // being loosened.
        assert_eq!(Agent::Odysseus.coverage(), Coverage::Uncovered);
        assert_eq!(Agent::Aider.coverage(), Coverage::ShellProxy);
        let d = Agent::Unknown.coverage().describe();
        assert!(d.contains("no agent recognised"), "{d}");
        assert!(
            !d.contains("not wired"),
            "must not claim to know it is unwired: {d}"
        );
    }

    #[test]
    fn uncovered_agents_are_exactly_the_known_four() {
        let uncovered: Vec<&str> = Agent::all()
            .iter()
            .filter(|a| **a != Agent::Unknown)
            .filter(|a| a.coverage() == Coverage::Uncovered)
            .map(|a| a.as_str())
            .collect();
        assert_eq!(
            uncovered,
            // `copilot-vscode` joined on 2026-08-19, and it is the one entry
            // here that is uncovered ON PURPOSE rather than for want of
            // checking: a VS Code extension exposes no shell hook and no MCP
            // surface tokenade can write. It exists for rank 4 alone, and
            // this line is what stops the rank-4 row being mistaken for a
            // claim above it.
            vec!["copilot-vscode", "odysseus"],
            "the uncovered set moved — update the coverage tables with it"
        );
        assert!(
            Coverage::McpWrappable
                .describe()
                .contains("tokenade mcp-wrap install"),
            "the answer has to name the command that acts on it, under its \
             CANONICAL name: `mcp-proxy` is still accepted as an alias (D1), so \
             the old text was not wrong, only stale — and stale user-facing \
             text is how a renamed command keeps being taught"
        );
        // And nothing claims a hook without naming the event it fires on.
        for a in Agent::all() {
            if let Coverage::Hook(ev) = a.coverage() {
                assert!(!ev.is_empty(), "{a:?} claims a hook with no event");
            }
        }
    }

    /// The rank-3 answer must stay ATTACHED to the site table it is derived
    /// from. If someone adds an agent to `discover_mcp_config_sites`, this
    /// says so; if someone removes one, this says that too — instead of the
    /// agent silently going back to being told to compact by hand.
    /// The headline coverage count, derived rather than asserted.
    ///
    /// The prose count has already been wrong twice — once claiming thirteen
    /// agents uncovered, once listing Windsurf among agents with no channel
    /// while `mcp_wrap` had known its config all along. A count written
    /// in prose drifts silently; this one fails.
    #[test]
    /// No user-facing hint may tell a covered agent to compact by hand.
    ///
    /// `tokenade detect` prints `mcp_config_hint` directly under the coverage
    /// line, so the two are read together. Measured 2026-08-08, before the fix:
    ///
    ///   coverage: plugin — tool results are compacted inside the agent
    ///   mcp config hint: (no MCP — run `tokenade wrap pi …`)
    ///
    /// Pi and OpenClaw both ship a plugin, and were being told to redo by hand
    /// what the plugin already does. `install` carried the same staleness for
    /// OpenClaw — one fact, two surfaces, and only one of them was found
    /// by reading. This asserts the invariant on both kinds of covered agent,
    /// so the next agent that gains a channel cannot keep a manual hint.
    fn no_covered_agent_is_told_to_wrap_itself_by_hand() {
        for a in Agent::all() {
            let covered = matches!(a.coverage(), Coverage::Plugin | Coverage::Hook(_));
            if !covered {
                continue;
            }
            let hint = a.mcp_config_hint();
            assert!(
                !hint.contains("tokenade wrap"),
                "`{}` is covered ({}) but its hint sends the user to `tokenade wrap`: {hint}",
                a.as_str(),
                a.coverage().as_str()
            );
        }
    }

    #[test]
    fn the_matrix_headline_count_matches_the_tables() {
        let agents: Vec<&Agent> = Agent::all()
            .iter()
            .filter(|a| **a != Agent::Unknown)
            .collect();
        // THREE buckets, not two. Folding `ShellProxy` into "covered" would
        // undo the distinction the variant was added for (D8): an agent whose
        // shell output is folded but whose native file reads are not is neither
        // covered nor uncovered, and a headline that says "18/20" hides exactly
        // the half that is missing.
        let in_agent = agents
            .iter()
            .filter(|a| {
                matches!(
                    a.coverage(),
                    Coverage::Hook(_) | Coverage::Plugin | Coverage::McpWrappable
                )
            })
            .count();
        let shell_only: Vec<&str> = agents
            .iter()
            .filter(|a| matches!(a.coverage(), Coverage::ShellProxy))
            .map(|a| a.as_str())
            .collect();
        let uncovered: Vec<&str> = agents
            .iter()
            .filter(|a| {
                matches!(
                    a.coverage(),
                    Coverage::Uncovered | Coverage::ViaDrivenAgents
                )
            })
            .map(|a| a.as_str())
            .collect();
        assert_eq!(
            (in_agent, agents.len()),
            (15, 19),
            "the headline says 15 covered of 19 — update BOTH or neither"
        );
        assert_eq!(
            shell_only,
            vec!["aider"],
            "the shell-only set moved — update the coverage tables with it"
        );
        assert_eq!(
            uncovered,
            vec!["t3-code", "copilot-vscode", "odysseus"],
            "the no-channel list moved; the coverage tables name it explicitly"
        );
    }

    #[test]
    fn every_agent_with_a_known_mcp_site_and_no_hook_gets_the_rank_three_answer() {
        let with_site: Vec<&str> = Agent::all()
            .iter()
            .filter(|a| **a != Agent::Unknown)
            .filter(|a| crate::mcp_wrap::agent_has_known_mcp_site(a.as_str()))
            .filter(|a| !a.has_plugin() && a.integration().hook_name.is_none())
            .map(|a| a.as_str())
            .collect();
        assert_eq!(
            with_site,
            // antigravity joined on 2026-08-10 and LEFT on 2026-08-25. It was
            // here because the IDE's `language_server` is a compiled binary
            // with no hook and no plugin API, leaving only the Codeium
            // `~/.gemini/config/mcp_config.json` it reads. That was a fact
            // about the IDE read as a fact about the product: the same vendor
            // ships `agy`, a CLI with a NATIVE hook system, and its contract is
            // in the shipped binary. Rank 1 now; windsurf left with the
            // product (2026-09) and came back as devin-desktop, its fork, which
            // kept `~/.codeium/windsurf/mcp_config.json` and nothing else.
            vec!["devin-desktop"],
            "the set of agents whose ONLY channel is the MCP proxy moved — \
             update the coverage tables with it"
        );
        for a in Agent::all() {
            if crate::mcp_wrap::agent_has_known_mcp_site(a.as_str())
                && !a.has_plugin()
                && a.integration().hook_name.is_none()
            {
                assert_eq!(a.coverage(), Coverage::McpWrappable, "{a:?}");
            }
        }
    }

    /// The Claude Code presence marker must follow a relocated config.
    ///
    /// `presence_markers` decides whether `install` believes the agent exists,
    /// and it looked only at `~/.claude`. Measured 2026-08-31 on a clean
    /// scratch home with `CLAUDE_CONFIG_DIR` pointing elsewhere:
    /// `tokenade install --only claude-code` answered "no AI coding agent
    /// detected on this machine" and wrote nothing, on a machine where Claude
    /// Code was configured and running. Not a degraded install — no install.
    #[test]
    fn the_claude_marker_follows_a_relocated_config_dir() {
        let mut h = crate::platform::ScratchHome::new("marker-config-dir");
        let home = h.path().to_path_buf();

        let default = Agent::ClaudeCode.presence_markers(Some(&home));
        assert!(
            default.contains(&home.join(".claude")),
            "the default marker must still be ~/.claude: {default:?}"
        );

        let alt = home.join("moved-config");
        h.env().set("CLAUDE_CONFIG_DIR", &alt);
        let moved = Agent::ClaudeCode.presence_markers(Some(&home));
        assert!(
            moved.contains(&alt),
            "a relocated config must be looked for where it IS: {moved:?}"
        );
        assert!(
            !moved.contains(&home.join(".claude")),
            "and not also where it is not, or an empty ~/.claude would answer \
             for a config that moved: {moved:?}"
        );
    }

    #[test]
    fn presence_markers_are_all_home_anchored() {
        // The fixture has to be absolute ON THE PLATFORM THE TEST RUNS ON, and
        // `/fake/home` is not: Windows calls a path without a drive letter
        // relative, so `Path::join` produced `/fake/home\.claude` and the
        // assertion failed on the fixture rather than on the property.
        //
        // Found on a real Windows VM 2026-08-17, not in CI. The workflow's own
        // comment says the cross-check "does NOT run a single test on Windows
        // and must not be read as if it did" — this is the first thing that
        // sentence was protecting against, and it stood for as long as nobody
        // ran the suite there.
        //
        // It is not cosmetic: a Windows test run fails on this one assertion
        // and stops the build before it produces a binary.
        #[cfg(windows)]
        let home = Path::new(r"C:\fake\home");
        #[cfg(not(windows))]
        let home = Path::new("/fake/home");
        for a in Agent::all() {
            for m in a.presence_markers(Some(home)) {
                assert!(
                    m.is_absolute(),
                    "{a:?} marker {} is relative — it can never match a real path",
                    m.display()
                );
            }
        }
    }

    #[test]
    fn from_slug_round_trips_every_agent_and_rejects_junk() {
        for a in Agent::all() {
            assert_eq!(
                Agent::from_slug(a.as_str()),
                Some(*a),
                "slug {}",
                a.as_str()
            );
        }
        assert_eq!(Agent::from_slug(""), None);
        assert_eq!(Agent::from_slug("not-an-agent"), None);
        // the TOKENADE_AGENT override path depends on this mapping (hermes plugin)
        assert_eq!(Agent::from_slug("hermes"), Some(Agent::Hermes));
    }

    #[test]
    fn unknown_when_nothing_present() {
        let dir = std::env::temp_dir().join(format!("tokenade-detect-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        assert_eq!(from_filesystem(&dir), Agent::Unknown);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn claude_from_marker() {
        let dir = std::env::temp_dir().join(format!("tokenade-detect2-{}", std::process::id()));
        let _ = std::fs::create_dir_all(dir.join(".claude"));
        assert_eq!(from_filesystem(&dir), Agent::ClaudeCode);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_paths_differ_across_agents() {
        let root = Path::new("/tmp/fake");
        let home = Path::new("/home/me");
        let claude = Agent::ClaudeCode.mcp_config_path(root, Some(home)).unwrap();
        let cursor = Agent::Cursor.mcp_config_path(root, Some(home)).unwrap();
        let opencode = Agent::OpenCode.mcp_config_path(root, Some(home)).unwrap();
        assert_ne!(claude, cursor);
        assert_ne!(cursor, opencode);
        assert!(claude.ends_with(".mcp.json"));
        assert!(cursor.ends_with(".cursor/mcp.json"));
        // OpenCode's MCP entry lives inside its main `opencode.json`
        // config (under the `mcp` key) — distinct from the project-local
        // Cursor `.cursor/mcp.json`. Path varies by OS:
        //   Linux/BSD:  ~/.config/opencode/opencode.json
        //   macOS:      ~/Library/Application Support/opencode/opencode.json
        //   Windows:    AppData/Roaming/opencode/opencode.json
        assert!(
            opencode
                .to_string_lossy()
                .contains("opencode/opencode.json"),
            "opencode path was: {opencode:?}"
        );
    }

    #[test]
    fn detect_all_present_finds_planted_agents() {
        // Create a fake home with claude + cursor markers.
        let home = std::env::temp_dir().join(format!(
            "tokenade-detect-all-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::create_dir_all(home.join(".cursor")).unwrap();
        let found = detect_all_present(Some(&home));
        assert!(found.contains(&Agent::ClaudeCode));
        assert!(found.contains(&Agent::Cursor));
        assert!(!found.contains(&Agent::Codex));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn shell_hook_matches_what_the_installer_wires() {
        // Every `true` here must correspond to an `install_*_hooks`
        // function that actually writes a hook config for that agent.
        assert!(Agent::ClaudeCode.integration().shell_hook);
        assert!(
            Agent::Codex.integration().shell_hook,
            "install_codex_passive_hooks writes ~/.codex/hooks.json"
        );
        assert!(Agent::Droid.integration().shell_hook);
        assert!(Agent::Grok.integration().shell_hook);
        assert!(Agent::Qwen.integration().shell_hook);
        // No hook installer exists for these.
        assert!(
            Agent::Cursor.integration().shell_hook,
            "install_cursor_hooks writes ~/.cursor/hooks.json"
        );
        assert!(!Agent::OpenCode.integration().shell_hook);
        assert!(!Agent::DevinDesktop.integration().shell_hook);
    }

    /// The hint and the path must name the SAME file.
    ///
    /// `mcp_config_path` is what the product writes; `mcp_config_hint` is what
    /// it tells an operator to open. They disagreed for opencode
    /// (`mcp.json` vs `opencode.json`) and hermes (`~/.config/hermes/mcp.json`
    /// vs `~/.hermes/config.yaml`) — two answers to one question, and the
    /// user-facing one was the wrong one in both cases.
    ///
    /// Compared on the file name: the hint is a `&'static str` written for a
    /// human and the path is resolved per OS, so the directory spelling
    /// legitimately differs.
    #[test]
    fn the_hint_and_the_path_name_the_same_file() {
        let home = Path::new("/home/probe");
        let mut wrong = Vec::new();
        for &a in Agent::all() {
            let Some(p) = a.mcp_config_path(Path::new("/proj"), Some(home)) else {
                continue;
            };
            let hint = a.mcp_config_hint();
            // Prose answers ("(none — …)") belong to agents with no path, and
            // the `let else` above already skipped those.
            if hint.starts_with('(') {
                wrong.push(format!(
                    "{}: has a path but the hint is prose: {hint}",
                    a.as_str()
                ));
                continue;
            }
            let file = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            if !hint.ends_with(file) {
                wrong.push(format!(
                    "{}: writes {file}, tells the user {hint}",
                    a.as_str()
                ));
            }
        }
        assert!(
            wrong.is_empty(),
            "hint and path disagree:\n  {}",
            wrong.join("\n  ")
        );
    }

    #[test]
    fn all_mcp_capable_agents_support_mcp() {
        // Aider and OpenClaw are wrap-only; the rest expose MCP.
        for a in Agent::all() {
            let supports_mcp = a.integration().mcp;
            // Aider, Pi, OpenClaw, Odysseus and Droid can't have MCP
            // auto-registered by tokenade (Aider/Pi/OpenClaw: no MCP
            // contract; Odysseus: UI-only config; Droid: MCP surface not
            // verified yet — hooks only).
            if matches!(
                a,
                Agent::Aider
                    | Agent::Pi
                    | Agent::OpenClaw
                    | Agent::Odysseus
                    // orchestrator — no MCP surface of its own
                    | Agent::T3Code
                    // Copilot's VS Code half registers MCP servers through VS
                    // Code's own `mcp.json`, which is VS Code's surface and
                    // not Copilot's. Rank 4 only.
                    | Agent::CopilotVsCode
            ) {
                assert!(
                    !supports_mcp,
                    "{} should NOT report MCP support",
                    a.as_str()
                );
            } else {
                assert!(supports_mcp, "{} should support MCP", a.as_str());
            }
        }
    }

    #[test]
    fn pi_is_wrap_only_and_filesystem_detected() {
        assert_eq!(Agent::Pi.as_str(), "pi");
        assert!(!Agent::Pi.integration().mcp, "pi is wrap-only (no MCP)");
        assert!(Agent::Pi
            .mcp_config_path(Path::new(""), Some(Path::new("/home/me")))
            .is_none());
        assert!(Agent::all().contains(&Agent::Pi), "installer must see pi");
        // a project-local `.pi/` dir identifies pi.
        let dir = std::env::temp_dir().join(format!("tok-pi-detect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".pi")).unwrap();
        assert_eq!(from_filesystem(&dir), Agent::Pi);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn new_agents_have_distinct_config_paths() {
        let home = Path::new("/home/me");
        let cline = Agent::Cline.mcp_config_path(Path::new(""), Some(home));
        let openclaw = Agent::OpenClaw.mcp_config_path(Path::new(""), Some(home));
        let aider = Agent::Aider.mcp_config_path(Path::new(""), Some(home));
        assert!(cline.is_some());
        // Aider + OpenClaw have no MCP config path (wrap-only).
        assert!(aider.is_none());
        assert!(openclaw.is_none());
        let cline_p = cline.unwrap();
        assert!(cline_p.to_string_lossy().contains("saoudrizwan.claude-dev"));
    }

    #[test]
    fn aider_presence_markers_cover_history_files() {
        let home = Path::new("/home/me");
        let markers = Agent::Aider.presence_markers(Some(home));
        assert!(markers
            .iter()
            .any(|p| p.to_string_lossy().contains(".aider.chat.history")));
    }
}
