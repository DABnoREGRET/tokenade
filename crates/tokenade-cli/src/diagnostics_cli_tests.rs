// The tests of `diagnostics_cli`.
//
// Spliced in by `include!` from `diagnostics_cli.rs`: the same module, hence the same
// private access and the same `super::` paths, without a single visibility line
// changed. This is the form this repository already uses for 33 other modules.
//
// A `//!` is impossible here: a file spliced in by `include!` is not a module,
// and an INNER doc comment has nothing to qualify.

// `discover` names traffic tokenade is not compacting. Once the hook is
// installed it compacts the named tools, so listing them is a saving already
// taken. See [`discover_opportunities`].

#[cfg(test)]
mod the_widest_window_is_all_of_history {
    //! `--days` is unbounded user input, and `days * 86_400` overflows u64
    //! above ~2.1e14 days. Both builds answered wrongly and neither said so:
    //! debug panicked and still exited 0 with an empty stdout (a `--json`
    //! caller got a parse error AND a success code), release wrapped and
    //! scanned an arbitrary window — `--days 999999999999999` reported a set
    //! of opportunities the command's own default reports as empty.

    use super::cutoff_for_days;

    const NOW: u64 = 1_800_000_000;

    #[test]
    fn a_window_wider_than_history_reaches_the_epoch() {
        for days in [u64::MAX, 999_999_999_999_999, u64::MAX / 86_400 + 1] {
            assert_eq!(
                cutoff_for_days(NOW, days),
                0,
                "--days {days} must scan everything, not an arbitrary window"
            );
        }
    }

    #[test]
    fn an_ordinary_window_is_exactly_that_many_days() {
        assert_eq!(cutoff_for_days(NOW, 1), NOW - 86_400);
        assert_eq!(cutoff_for_days(NOW, 30), NOW - 30 * 86_400);
        assert_eq!(cutoff_for_days(NOW, 0), NOW, "zero days back is now");
    }

    /// The clock does not have to be later than the window: a fixture with a
    /// small `now` is the shape every test in this file uses.
    #[test]
    fn a_window_older_than_the_clock_floors_at_the_epoch() {
        assert_eq!(cutoff_for_days(100, 30), 0);
    }

    /// Anti-vacuity: the three above would all pass on a function that
    /// returned 0 for everything.
    #[test]
    fn the_cutoff_moves_with_the_window() {
        assert!(cutoff_for_days(NOW, 1) > cutoff_for_days(NOW, 2));
        assert!(cutoff_for_days(NOW, 2) > cutoff_for_days(NOW, 3));
    }
}

#[cfg(test)]
mod discover_counts_what_came_back {
    //! `discover` exists to answer "where would tokenade help me most", and
    //! tokenade compacts what a tool RETURNS. The scanner matched the first
    //! content block carrying a `name` — which only a `tool_use` has — and
    //! charged that tool the length of the REQUEST line.
    //!
    //! Measured over this machine's last 30 days, that is not a rounding
    //! difference, it is a different ranking: Edit 38.3 MB requested against
    //! 3.0 MB returned, Write 16.2 MB against 0.5 MB, Read 13.5 MB against
    //! 30.0 MB. The report put Edit and Write second and third — the two
    //! tools whose bytes the agent wrote — and Read, second in reality,
    //! fourth.

    use super::scan_agent_logs;

    /// One session: a tiny Write request with a huge body and a one-word
    /// answer, and a tiny Read request with a large answer. Ranking them by
    /// the request reverses them.
    fn session() -> String {
        let big_body = "x".repeat(4000);
        let big_answer = "y".repeat(9000);
        [
            format!(
                r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"t1","name":"Write","input":{{"content":"{big_body}"}}}}]}}}}"#
            ),
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}"#.to_string(),
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"Read","input":{"path":"a"}}]}}"#.to_string(),
            format!(
                r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"t2","content":[{{"type":"text","text":"{big_answer}"}}]}}]}}}}"#
            ),
        ]
        .join("\n")
    }

    fn scan(dir: &std::path::Path) -> std::collections::BTreeMap<String, super::ToolTraffic> {
        let mut by_tool = std::collections::BTreeMap::new();
        let mut sessions = 0u64;
        let mut walker = vec![dir.to_path_buf()];
        scan_agent_logs(&mut walker, &mut by_tool, &mut sessions, 0);
        assert_eq!(sessions, 1, "the fixture session was not scanned");
        by_tool
    }

    #[test]
    fn a_tool_is_charged_its_answer_not_its_request() {
        let d = std::env::temp_dir().join(format!("tkn-discover-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("s.jsonl"), session()).unwrap();

        let by_tool = scan(&d);
        let (write_bytes, write_calls) = (by_tool["Write"].bytes, by_tool["Write"].calls);
        let (read_bytes, read_calls) = (by_tool["Read"].bytes, by_tool["Read"].calls);

        assert_eq!(write_calls, 1, "one call each");
        assert_eq!(read_calls, 1);
        assert_eq!(write_bytes, 2, "Write is charged its answer (`ok`)");
        assert_eq!(read_bytes, 9000, "Read is charged the text it returned");
        assert!(
            read_bytes > write_bytes,
            "ranking by the request would put Write first"
        );

        let _ = std::fs::remove_dir_all(&d);
    }

    /// The split is attributed per TRANSCRIPT, and the flag may be read from a
    /// line that comes AFTER the calls it applies to.
    ///
    /// Both halves matter. A subagent's bytes charged to the main session hide
    /// the finding this column exists for; and attributing as the file is
    /// walked would charge a subagent's opening calls to the main session
    /// because `isSidechain` had not been seen yet.
    #[test]
    fn a_subagent_transcript_is_charged_to_the_sidechain_column() {
        let d = std::env::temp_dir().join(format!("tkn-discover-side-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        // Main session: the flag is present and false, as the host writes it.
        std::fs::write(
            d.join("main.jsonl"),
            [
                r#"{"isSidechain":false,"type":"assistant","message":{"content":[{"type":"tool_use","id":"m1","name":"Read","input":{}}]}}"#,
                r#"{"isSidechain":false,"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"m1","content":"abcd"}]}}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        // Subagent: the FIRST line carries no flag at all, so a scanner that
        // decided as it walked would charge this call to the main session.
        std::fs::write(
            d.join("agent-x.jsonl"),
            [
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"s1","name":"Read","input":{}}]}}"#,
                r#"{"isSidechain":true,"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"s1","content":"abcdefghij"}]}}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        let mut by_tool = std::collections::BTreeMap::new();
        let mut sessions = 0u64;
        let mut walker = vec![d.clone()];
        scan_agent_logs(&mut walker, &mut by_tool, &mut sessions, 0);
        assert_eq!(sessions, 2, "both fixtures must be scanned");
        let t = by_tool["Read"];
        assert_eq!((t.calls, t.bytes), (2, 14), "the totals still hold");
        assert_eq!(
            (t.side_calls, t.side_bytes),
            (1, 10),
            "the subagent's call and its bytes are the sidechain share"
        );
        assert!(
            t.side_bytes < t.bytes,
            "the share is a SUBSET of the total, never a second bucket"
        );

        let _ = std::fs::remove_dir_all(&d);
    }

    /// A result whose call is not in this session cannot be attributed, and
    /// guessing a name would put bytes under a tool that did not produce
    /// them. It is dropped, and the tool it would have been charged to keeps
    /// its honest zero.
    #[test]
    fn an_orphan_result_is_not_charged_to_anybody() {
        let d = std::env::temp_dir().join(format!("tkn-discover-orph-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("s.jsonl"),
            [
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Read","input":{}}]}}"#,
                r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"unknown-id","content":"zzzzzzzz"}]}}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        let by_tool = scan(&d);
        assert_eq!(
            (by_tool["Read"].bytes, by_tool["Read"].calls),
            (0, 1),
            "a call with no answer is still a call"
        );
        assert_eq!(by_tool.len(), 1, "an orphan invented a tool");

        let _ = std::fs::remove_dir_all(&d);
    }
}

#[cfg(test)]
mod what_discover_calls_an_opportunity {
    use super::discover_opportunities;

    fn rows() -> Vec<(String, super::ToolTraffic)> {
        [
            ("Bash", 202_499_403u64, 95_323u64),
            ("Read", 13_518_800, 9_208),
            ("WebFetch", 498_119, 315),
            (
                "mcp__plugin_playwright_playwright__browser_navigate",
                107_717,
                73,
            ),
            ("mcp__tokenade__exec_script", 391_907, 157),
            ("Edit", 38_453_542, 14_779),
        ]
        .iter()
        .map(|(n, b, c)| {
            (
                (*n).to_string(),
                super::ToolTraffic {
                    bytes: *b,
                    calls: *c,
                    ..Default::default()
                },
            )
        })
        .collect()
    }

    fn names(hook: bool) -> Vec<String> {
        discover_opportunities(&rows(), hook)
            .into_iter()
            .map(|(n, _)| n)
            .collect()
    }

    #[test]
    fn a_tool_the_installed_hook_covers_is_not_an_opportunity() {
        let got = names(true);
        for covered in ["Bash", "Read", "WebFetch"] {
            assert!(
                !got.contains(&covered.to_string()),
                "{covered} is compacted once the hook is wired, got {got:?}"
            );
        }
    }

    #[test]
    fn without_the_hook_those_same_tools_are_the_opportunity() {
        let got = names(false);
        for uncovered in ["Bash", "Read", "WebFetch"] {
            assert!(
                got.contains(&uncovered.to_string()),
                "{uncovered} must be named when nothing is wired, got {got:?}"
            );
        }
    }

    #[test]
    fn an_mcp_tool_is_an_opportunity_whatever_the_hook_does() {
        // A matcher never sees `mcp__<server>__<tool>`; only `mcp-wrap` reaches
        // it. So the hook's state cannot make this one go away.
        for hook in [true, false] {
            assert!(
                names(hook)
                    .contains(&"mcp__plugin_playwright_playwright__browser_navigate".to_string()),
                "hook_installed={hook}"
            );
        }
    }

    #[test]
    fn our_own_server_is_never_an_opportunity() {
        for hook in [true, false] {
            assert!(
                !names(hook).iter().any(|n| n.starts_with("mcp__tokenade__")),
                "tokenade's own MCP output is already compacted (hook={hook})"
            );
        }
    }

    #[test]
    fn a_tool_we_cannot_route_is_not_offered_as_one() {
        // `Edit` is the agent's own input; there is no output of ours to fold.
        for hook in [true, false] {
            assert!(!names(hook).contains(&"Edit".to_string()), "hook={hook}");
        }
    }

    #[test]
    fn the_list_stays_short_enough_to_read() {
        let many: Vec<(String, super::ToolTraffic)> = (0..20)
            .map(|i| {
                (
                    format!("mcp__srv__tool{i}"),
                    super::ToolTraffic {
                        bytes: 100,
                        calls: 1,
                        ..Default::default()
                    },
                )
            })
            .collect();
        assert_eq!(discover_opportunities(&many, true).len(), 5);
    }
}

#[cfg(test)]
mod what_the_window_actually_bounds {
    use super::{entry_in_window, scan_agent_logs};

    /// 2020-09-13T12:26:40Z, chosen so both fixtures below sit far from it and
    /// the test never reads a clock.
    const CUTOFF: u64 = 1_600_000_000;

    #[test]
    fn an_entry_without_a_usable_timestamp_is_kept() {
        let no_ts = serde_json::json!({"type": "assistant"});
        assert!(
            entry_in_window(&no_ts, CUTOFF),
            "never drop on missing evidence: another agent's dialect may omit it"
        );
        let junk = serde_json::json!({"timestamp": "yesterday afternoon"});
        assert!(
            entry_in_window(&junk, CUTOFF),
            "nor on unparseable evidence"
        );
        assert!(entry_in_window(
            &serde_json::json!({"timestamp": "2099-01-01T00:00:00.000Z"}),
            CUTOFF
        ));
        assert!(!entry_in_window(
            &serde_json::json!({"timestamp": "2020-01-01T00:00:00.000Z"}),
            CUTOFF
        ));
    }

    /// The mtime check is a cheap skip, not the window.
    ///
    /// One file, recently written, holding one call from before the cutoff and
    /// one after. Both its calls and its bytes must be counted once. Measured
    /// before this gate existed: 6 of 547 files straddled the boundary and
    /// carried 44 574 results worth 25.5 MB — 21 % of everything the report
    /// attributed to "the last 30 days".
    #[test]
    fn a_file_inside_the_window_does_not_drag_its_older_entries_in() {
        let d = std::env::temp_dir().join(format!("tkn-window-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("s.jsonl"),
            [
                r#"{"timestamp":"2020-01-01T00:00:00.000Z","type":"assistant","message":{"content":[{"type":"tool_use","id":"old","name":"Read","input":{}}]}}"#,
                r#"{"timestamp":"2020-01-01T00:00:01.000Z","type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"old","content":"aaaaaaaaaa"}]}}"#,
                r#"{"timestamp":"2099-01-01T00:00:00.000Z","type":"assistant","message":{"content":[{"type":"tool_use","id":"new","name":"Read","input":{}}]}}"#,
                r#"{"timestamp":"2099-01-01T00:00:01.000Z","type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"new","content":"bbb"}]}}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        let mut by_tool = std::collections::BTreeMap::new();
        let mut sessions = 0u64;
        let mut walker = vec![d.clone()];
        scan_agent_logs(&mut walker, &mut by_tool, &mut sessions, CUTOFF);
        let t = by_tool["Read"];
        assert_eq!(t.calls, 1, "the pre-cutoff call must not be counted");
        assert_eq!(t.bytes, 3, "nor its bytes");
        assert_eq!(sessions, 1, "the file still contributed, so it is scanned");

        // A session that contributed NOTHING is not a scanned session.
        //
        // This needs its own file: raising the cutoff instead would trip the
        // mtime pre-filter, the file would never be opened, and the assertion
        // would pass without reaching the code it is aimed at. (It did exactly
        // that at first — the mutation that counts every file survived.)
        let d2 = std::env::temp_dir().join(format!("tkn-window-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d2);
        std::fs::create_dir_all(&d2).unwrap();
        std::fs::write(
            d2.join("s.jsonl"),
            r#"{"timestamp":"2020-01-01T00:00:00.000Z","type":"assistant","message":{"content":[{"type":"tool_use","id":"o","name":"Read","input":{}}]}}"#,
        )
        .unwrap();
        let mut by2 = std::collections::BTreeMap::new();
        let mut sessions2 = 0u64;
        let mut walker2 = vec![d2.clone()];
        scan_agent_logs(&mut walker2, &mut by2, &mut sessions2, CUTOFF);
        assert!(by2.is_empty(), "everything in it is outside the window");
        assert_eq!(
            sessions2, 0,
            "a file the window empties is not a session the header may claim"
        );
        let _ = std::fs::remove_dir_all(&d2);

        let _ = std::fs::remove_dir_all(&d);
    }

    /// A call learned before the cutoff still names a result that lands after
    /// it — otherwise the window turns a straddling exchange into an orphan
    /// and silently drops its bytes.
    #[test]
    fn a_result_inside_the_window_keeps_the_name_of_a_call_outside_it() {
        let d = std::env::temp_dir().join(format!("tkn-window-straddle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("s.jsonl"),
            [
                r#"{"timestamp":"2020-01-01T00:00:00.000Z","type":"assistant","message":{"content":[{"type":"tool_use","id":"x","name":"Bash","input":{}}]}}"#,
                r#"{"timestamp":"2099-01-01T00:00:00.000Z","type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"x","content":"zzzzz"}]}}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        let mut by_tool = std::collections::BTreeMap::new();
        let mut sessions = 0u64;
        let mut walker = vec![d.clone()];
        scan_agent_logs(&mut walker, &mut by_tool, &mut sessions, CUTOFF);
        let t = by_tool["Bash"];
        assert_eq!(t.bytes, 5, "the in-window answer keeps its bytes");
        assert_eq!(
            t.calls, 0,
            "and the out-of-window call is still not counted"
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}

#[cfg(test)]
mod which_channels_live_in_subagents {
    use super::{sidechain_heavy, ToolTraffic};

    fn row(name: &str, bytes: u64, side_bytes: u64) -> (String, ToolTraffic) {
        (
            name.to_string(),
            ToolTraffic {
                bytes,
                calls: 10,
                side_bytes,
                side_calls: 5,
            },
        )
    }

    /// Both clauses are load-bearing, and the test says which is which: the
    /// ratio is what makes the main-session view unrepresentative, the floor is
    /// what stops a three-call tool from being reported as a finding.
    #[test]
    fn a_channel_is_named_only_when_most_of_it_and_enough_of_it_is_a_sidechain() {
        const MB: u64 = 1024 * 1024;
        let rows = vec![
            // Read on this machine: 91 %, far over the floor.
            row("Read", 30 * MB, 27 * MB),
            // Majority, but three calls' worth — a ratio on noise.
            row("Tiny", 900, 800),
            // Large, but the main session is where it happens.
            row("Bash", 80 * MB, 12 * MB),
            // Exactly half is not "most of it".
            row("Half", 4 * MB, 2 * MB),
        ];
        let named: Vec<String> = sidechain_heavy(&rows).into_iter().map(|(n, _)| n).collect();
        assert_eq!(named, vec!["Read".to_string()], "got {named:?}");
    }

    #[test]
    fn a_channel_with_no_subagent_traffic_is_never_named() {
        assert!(sidechain_heavy(&[row("Write", 50 * 1024 * 1024, 0)]).is_empty());
        assert!(sidechain_heavy(&[]).is_empty());
    }
}
