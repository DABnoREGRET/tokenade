// The tests of `checks`.
//
// Spliced in by `include!` from `checks.rs`: the same module, hence the same
// private access and the same `super::` paths, without a single visibility line
// changed. This is the form this repository already uses for 33 other modules.
//
// A `//!` is impossible here: a file spliced in by `include!` is not a module,
// and an INNER doc comment has nothing to qualify.

#[cfg(test)]
pub(super) const NOT_LEVERS: &[(&str, &str)] = &[
    ("cache_hit", "a cache read, not a compaction"),
    (
        "aftertool",
        "an install-time probe of the Gemini hook wiring",
    ),
    (
        "copilot-posttool",
        "the Copilot bridge's own accounting row",
    ),
    (
        "replay",
        "a recorded session being replayed, not a live saving",
    ),
    (
        "review-context",
        "the feature CLI's packing row, not a hook lever",
    ),
    ("turn-summary", "a per-turn accounting row, not a lever"),
];

#[cfg(test)]
mod which_tokenade_runs {
    use super::*;
    use std::path::PathBuf;

    /// Silence is the whole point when there is nothing to say: a normal
    /// install is one binary, on PATH, answering here, and this check must add
    /// no line to `healthcheck` for it. A check that speaks on every machine is
    /// a check nobody reads by the third run.
    #[test]
    fn one_install_that_answers_for_itself_says_nothing() {
        let me = std::env::current_exe().unwrap();
        let r = binary_check_from(Some(&me), std::slice::from_ref(&me));
        assert_eq!(r.status, CheckStatus::Ok);
        assert!(
            r.detail.is_empty(),
            "a single install must not produce details: {:?}",
            r.detail
        );
    }

    /// The finding this exists for. Two copies on PATH, and the one the shell
    /// resolves is named — including when it is NOT the binary answering, which
    /// is the case a user cannot otherwise discover.
    #[test]
    fn a_second_copy_is_named_and_so_is_the_one_that_wins() {
        let me = std::env::current_exe().unwrap();
        let other = PathBuf::from("/somewhere/else/bin/tokenade");
        let third = PathBuf::from("/a/third/bin/tokenade");

        // We are the winner, but a second copy exists further along.
        let r = binary_check_from(Some(&me), &[me.clone(), other.clone()]);
        assert_eq!(r.status, CheckStatus::Info);
        let joined = r.detail.join("\n");
        assert!(
            joined.contains(&other.display().to_string()),
            "the later copy must be named: {joined}"
        );
        assert!(
            joined.contains("PATH order alone decides"),
            "the user must be told what decides between them: {joined}"
        );

        // We are NOT the winner: what the shell runs is somebody else.
        let r = binary_check_from(Some(&me), &[other.clone(), third.clone()]);
        assert_eq!(r.status, CheckStatus::Info);
        let joined = r.detail.join("\n");
        assert!(
            joined.contains(&other.display().to_string())
                && joined.contains("not the binary answering here"),
            "the winning copy must be named as the one that runs: {joined}"
        );
        assert!(
            joined.contains(&third.display().to_string()),
            "every copy is reported, not just the first two: {joined}"
        );
    }

    /// A `tokenade` that is nowhere on PATH is a real state — a build run out
    /// of `dist/`, or an install whose directory never made it into PATH — and
    /// the second of those breaks every agent that calls the command by name.
    #[test]
    fn nothing_on_path_is_reported_as_nothing_on_path() {
        let me = std::env::current_exe().unwrap();
        let r = binary_check_from(Some(&me), &[]);
        assert_eq!(r.status, CheckStatus::Info);
        assert!(
            r.detail.join("\n").contains("no `tokenade` is reachable"),
            "an unreachable command must say so: {:?}",
            r.detail
        );
    }
}

#[cfg(test)]
mod hooks_all_run_the_same_binary {
    use super::*;

    fn cfg(name: &str, paths: &[&str]) -> (String, Vec<String>) {
        (
            name.to_string(),
            paths.iter().map(|s| s.to_string()).collect(),
        )
    }

    /// The finding: two installs, every path resolving, every command the right
    /// shape, and the split invisible. Measured on the development machine as
    /// eight hooks on 0.5.1 and three on 1.1.7.
    #[test]
    fn a_split_across_two_installs_is_named_with_both_paths() {
        let r = hook_binaries_verdict(
            &[
                cfg(
                    "/h/.claude/settings.json",
                    &["/h/.cargo/bin/tokenade", "/h/.local/bin/tokenade"],
                ),
                cfg("/h/.codex/config.toml", &["/h/.cargo/bin/tokenade"]),
            ],
            None,
        );
        assert_eq!(r.status, CheckStatus::Info);
        assert!(
            r.message.contains("split across 2"),
            "the count is the headline: {}",
            r.message
        );
        let joined = r.detail.join("\n");
        for want in [
            "/h/.cargo/bin/tokenade",
            "/h/.local/bin/tokenade",
            "/h/.claude/settings.json",
            "/h/.codex/config.toml",
        ] {
            assert!(joined.contains(want), "{want} must be named: {joined}");
        }
        assert!(
            !joined.contains("hook path(s)"),
            "the detail must name the config files, not a count of spellings: {joined}"
        );
        assert!(
            joined.contains("tokenade install"),
            "the one-line fix must be named: {joined}"
        );
    }

    /// Silence when there is nothing to say — one install, however many hooks
    /// and however many agent configs point at it.
    #[test]
    fn one_install_across_many_agents_says_nothing() {
        let r = hook_binaries_verdict(
            &[
                cfg("/h/.claude/settings.json", &["/h/.local/bin/tokenade"]),
                cfg("/h/.codex/config.toml", &["/h/.local/bin/tokenade"]),
                cfg("/h/.gemini/settings.json", &["/h/.local/bin/tokenade"]),
            ],
            None,
        );
        assert_eq!(r.status, CheckStatus::Ok);
        assert!(r.detail.is_empty(), "nothing to report: {:?}", r.detail);

        // No hooks at all is still not this check's subject — but `Ok` is not
        // how you say nothing. An OK row PRINTS, so the empty set came back as
        // `OK  every hook runs the same tokenade binary` on a machine with no
        // hooks at all, i.e. one where tokenade was compacting nothing.
        // Measured 2026-09-03 on a HOME with no agent config: that line and
        // `OK all agent-config hook paths resolve` sat three lines above the
        // only line about the real state, which was an INFO saying settings
        // .json was "not present". Info, and a message that claims nothing,
        // still leaves the verdict to the check that owns it.
        let empty = hook_binaries_verdict(&[], None);
        assert_eq!(empty.status, CheckStatus::Info);
        assert!(
            !empty.message.contains("every hook runs the same"),
            "an empty set must not claim agreement: {}",
            empty.message
        );
    }

    /// Agreeing is only half the question. A machine whose every hook points at
    /// ONE stale tokenade is the split-binary state without the split that made it
    /// visible — and the first version of this check answered `every hook runs
    /// the same tokenade binary` to it: true, and silent in exactly the case
    /// the check exists for.
    #[test]
    fn agreeing_on_a_binary_nobody_types_is_still_worth_saying() {
        let tmp = std::env::temp_dir().join(format!("tokenade-agree-{}", std::process::id()));
        let (old, new) = (tmp.join("old"), tmp.join("new"));
        for d in [&old, &new] {
            std::fs::create_dir_all(d).unwrap();
            std::fs::write(d.join("tokenade"), b"x").unwrap();
        }
        let hooks = old.join("tokenade").display().to_string();
        let winner = new.join("tokenade");

        // All hooks agree — on the binary that is NOT what a bare `tokenade`
        // runs.
        let r = hook_binaries_verdict(
            &[cfg("/h/.claude/settings.json", &[&hooks])],
            Some(winner.as_path()),
        );
        assert_eq!(r.status, CheckStatus::Info);
        assert!(
            r.message.contains("not the one on your PATH"),
            "the second half of the question must be asked: {}",
            r.message
        );
        let joined = r.detail.join("\n");
        assert!(
            joined.contains(&hooks) && joined.contains(&winner.display().to_string()),
            "both binaries must be named: {joined}"
        );
        assert!(
            joined.contains("tokenade install"),
            "the one-line fix must be named: {joined}"
        );

        // Agreeing on the PATH winner is the healthy case and stays silent.
        let r = hook_binaries_verdict(
            &[cfg(
                "/h/.claude/settings.json",
                &[&winner.display().to_string()],
            )],
            Some(winner.as_path()),
        );
        assert_eq!(r.status, CheckStatus::Ok, "{:?}", r.detail);

        // Nothing on PATH: the question cannot be asked, so it is not answered.
        let r = hook_binaries_verdict(&[cfg("/h/.claude/settings.json", &[&hooks])], None);
        assert_eq!(
            r.status,
            CheckStatus::Ok,
            "with no PATH winner there is nothing to compare against"
        );

        // A hook path that does not resolve at all cannot be the binary you
        // type, and the comparison must say so rather than fall back to
        // "assume they match" — that fallback is the difference between
        // reporting a dead install and blessing it.
        let r = hook_binaries_verdict(
            &[cfg("/h/.claude/settings.json", &["/gone/bin/tokenade"])],
            Some(winner.as_path()),
        );
        assert_eq!(
            r.status,
            CheckStatus::Info,
            "an unresolvable hook path is not the PATH winner: {:?}",
            r.detail
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The wiring, not just the rule: `check_hook_binaries_agree` must hand the
    /// verdict the real PATH winner. A mutation that passes `None` there leaves
    /// every pure-function test green while the check goes silent on the
    /// machine it runs on — the fix living in a line no test crosses.
    #[test]
    fn the_real_check_hands_over_the_path_winner() {
        if tokenade_core::platform::tokenade_on_path().is_empty() {
            return; // No tokenade on this PATH — nothing to compare against.
        }
        let home = std::env::temp_dir().join(format!("tokenade-wire-{}", std::process::id()));
        let claude = home.join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::write(
            claude.join("settings.json"),
            r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"sh -c 'exec '/definitely/not/on/path/tokenade' hookwrap'"}]}]}}"#,
        )
        .unwrap();
        let r = check_hook_binaries_agree(&home);
        assert_eq!(
            r.status,
            CheckStatus::Info,
            "hooks on a binary nobody types must be reported through the real call: {:?}",
            r.detail
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A symlink farm is one install written two ways. Reporting it as a split
    /// would send the user rewriting a config that is already correct.
    #[test]
    fn the_same_binary_reached_by_a_symlink_is_not_a_split() {
        let tmp = std::env::temp_dir().join(format!("tokenade-hookid-{}", std::process::id()));
        let (real, link) = (tmp.join("real"), tmp.join("link"));
        std::fs::create_dir_all(&real).unwrap();
        std::fs::create_dir_all(&link).unwrap();
        std::fs::write(real.join("tokenade"), b"x").unwrap();
        #[cfg(unix)]
        {
            let l = link.join("tokenade");
            let _ = std::fs::remove_file(&l);
            std::os::unix::fs::symlink(real.join("tokenade"), &l).unwrap();
            let r = hook_binaries_verdict(
                &[cfg(
                    "/h/.claude/settings.json",
                    &[
                        &real.join("tokenade").display().to_string(),
                        &l.display().to_string(),
                    ],
                )],
                None,
            );
            assert_eq!(
                r.status,
                CheckStatus::Ok,
                "a symlink to the same binary is one install: {:?}",
                r.detail
            );
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The extraction now answers two questions from one scan, and the second
    /// one only works if it keeps the paths that DO exist — the very ones the
    /// stale check throws away.
    #[test]
    fn the_extraction_keeps_paths_that_exist_and_the_stale_check_still_drops_them() {
        // The shape the installer actually writes, with a path that EXISTS.
        // The stale check drops it by design; the agreement check needs it, and
        // that is the whole reason the two were split.
        let dir = std::env::temp_dir().join(format!("tokenade-extract-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let live = dir.join("tokenade");
        std::fs::write(&live, b"x").unwrap();
        let raw = format!(
            "{{\"command\": \"sh -c 'exec '\\\\''{}'\\\\'' hookwrap'\"}}",
            live.display()
        );
        assert_eq!(
            hook_binary_paths(&raw),
            vec![live.display().to_string()],
            "the installed `sh -c` shape must yield its binary path"
        );
        assert!(
            stale_hook_binary_paths(&raw).is_empty(),
            "a path that exists is not stale"
        );
        let _ = std::fs::remove_dir_all(&dir);
        let raw = "\"sh -c 'exec '/nope/bin/tokenade' hookwrap'\" \"/also/gone/tokenade\"";
        let all = hook_binary_paths(raw);
        assert_eq!(
            all,
            vec![
                "/also/gone/tokenade".to_string(),
                "/nope/bin/tokenade".to_string()
            ],
            "every tokenade path is collected, existing or not"
        );
        assert_eq!(
            stale_hook_binary_paths(raw),
            all,
            "none of these exist, so the stale check reports all of them"
        );
    }
}

#[cfg(test)]
mod lever_table_covers_the_product {
    use super::*;

    #[test]
    fn the_lever_table_covers_every_op_the_product_books() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root");
        // `record(source, op, …)` and `record_classed(source, op, class, …)`
        // both spell the op as the SECOND string literal of the call. Scanned by
        // hand rather than with a regex crate this workspace does not carry —
        // and, like the lever sweep, by looking FORWARD from the call site instead
        // of trying to match a whole call in one pattern, which a line break
        // between arguments defeats.
        fn ops_in(src: &str) -> Vec<String> {
            let mut out = Vec::new();
            // A file's own `#[cfg(test)] mod tests` invents ops
            // (`UsageEvent { op: "x", … }`), and the per-FILE exclusion below
            // only skips whole test files. Remove the inline modules by
            // matching their braces — NOT by truncating at the first one: a
            // module can sit in the middle of a file, and cutting there took
            // `compactors.rs`'s own `read_dedup` with it. When the braces do
            // not balance, nothing is removed: reading a test op is a failing
            // guard someone looks at, dropping shipped code is a silent hole.
            // ONE declarer for "the half of a file that ships"
            // (`tokenade_core::srcsweep`): this guard had grown its own,
            // and so had the e2e suite, each with its own brace bug.
            let src = &tokenade_core::srcsweep::production_source(src);
            for marker in ["::record(", "::record_classed("] {
                let mut from = 0usize;
                while let Some(i) = src[from..].find(marker) {
                    let start = from + i + marker.len();
                    from = start;
                    // The two string literals that open the call.
                    let win = &src[start..src.len().min(start + 240)];
                    let mut parts = win.split('"');
                    let (Some(_before), Some(_source), Some(between), Some(op)) =
                        (parts.next(), parts.next(), parts.next(), parts.next())
                    else {
                        continue;
                    };
                    // The op must be the argument RIGHT AFTER the source. When
                    // it is a variable (`record_classed("cli", op, label,
                    // "ci_heuristic", …)`) the next literal is something else
                    // entirely, and taking it would put a CLASS in the op list.
                    if between.trim() != "," {
                        continue;
                    }
                    // An op is a bare identifier; anything with a space or a
                    // path separator is a label that landed in this position
                    // because the call is shaped differently.
                    if !op.is_empty()
                        && op.len() <= 40
                        && op.bytes().all(|b| {
                            b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-:".contains(&b)
                        })
                    {
                        out.push(op.to_string());
                    }
                }
            }
            // A THIRD way to book an op, which this sweep did not know: a
            // struct literal handed to a helper that records for you
            // (`DedupSite { channel: "read", op: "read_dedup", … }`). Two live
            // levers escaped through it — `read_dedup` and `cmd_dedup` — and
            // the lever table's note then called `read_dedup` "a spelling no
            // current code path books", which is exactly the sentence a gap
            // in a completeness guard writes for you.
            let mut from = 0usize;
            while let Some(i) = src[from..].find("op: \"") {
                let start = from + i + "op: \"".len();
                from = start;
                let Some(end) = src[start..].find('"') else {
                    break;
                };
                let op = &src[start..start + end];
                if !op.is_empty()
                    && op.len() <= 40
                    && op.bytes().all(|b| {
                        b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-:".contains(&b)
                    })
                {
                    out.push(op.to_string());
                }
            }
            out
        }
        let mut found: std::collections::BTreeSet<String> = Default::default();
        let mut scanned = 0usize;
        for crate_dir in ["crates/tokenade-cli/src", "crates/tokenade-core/src"] {
            let mut stack = vec![root.join(crate_dir)];
            while let Some(dir) = stack.pop() {
                let Ok(rd) = std::fs::read_dir(&dir) else {
                    continue;
                };
                for e in rd.flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        stack.push(p);
                        continue;
                    }
                    if p.extension().and_then(|x| x.to_str()) != Some("rs") {
                        continue;
                    }
                    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    // Test code invents ops (`test:zero-drop-PID`); the guard is
                    // about what SHIPS.
                    if name.ends_with("_tests.rs")
                        || p.components().any(|c| c.as_os_str() == "tests")
                    {
                        continue;
                    }
                    let Ok(src) = std::fs::read_to_string(&p) else {
                        continue;
                    };
                    scanned += 1;
                    found.extend(ops_in(&src));
                }
            }
        }
        assert!(
            scanned > 100,
            "the sweep did not reach the source ({scanned} files) — it proves nothing"
        );
        assert!(
            found.len() >= 20,
            "the sweep found only {} op literals; the extractor has stopped matching \
             the way ops are booked",
            found.len()
        );
        // The counts above cannot notice a whole SHAPE going unread: once an
        // op is in LEVERS, dropping the scan that finds it leaves nothing
        // unwatched and the check goes green again. Name one op per shape.
        for (op, shape) in [
            ("read_identity_dedup", "a `record(source, op, …)` call"),
            ("read_dedup", "an `op:` field in a struct literal"),
        ] {
            assert!(
                found.contains(op),
                "the sweep no longer reads {shape} — `{op}` was not found"
            );
        }
        // …and the inline `#[cfg(test)] mod tests` of a shipped file must stay
        // out: a test module may build a `UsageEvent { op: "x", … }`.
        assert!(
            !found.contains("x"),
            "an op invented by a file's own test module reached the sweep"
        );
        let covered = |op: &str| {
            LEVERS.iter().any(|(needle, _)| op.contains(needle))
                || NOT_LEVERS.iter().any(|(name, _)| op == *name)
        };
        let orphans: Vec<&String> = found.iter().filter(|o| !covered(o)).collect();
        assert!(
            orphans.is_empty(),
            "op(s) booked by the product that `check_dead_levers` watches by nothing — \
             add them to LEVERS, or to NOT_LEVERS with the reason:\n  {}",
            orphans
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("\n  ")
        );
    }

    /// Twenty-two silent levers is not twenty-two findings, and a wall of them
    /// is a report nobody reads to the end. Both halves of that — the single
    /// cause and the capped list — are behaviour, so both are pinned here:
    /// the first version of this change had neither under test, and the
    /// mutations said so.
    fn home_with_ops(ops: &[&str]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tokenade-levers-{}-{}",
            std::process::id(),
            ops.len()
        ));
        let t = std::fs::remove_dir_all(&dir);
        drop(t);
        std::fs::create_dir_all(dir.join(".tokenade")).unwrap();
        let now = tokenade_core::platform::now_unix_secs();
        let body: String = ops
            .iter()
            .map(|op| format!("{{\"ts\":{now},\"op\":\"{op}\",\"saved\":1}}\n"))
            .collect();
        std::fs::write(dir.join(".tokenade").join("gain.jsonl"), body).unwrap();
        dir
    }

    #[test]
    fn a_mostly_silent_table_is_reported_as_one_cause_and_not_as_a_wall() {
        let home = home_with_ops(&["read_identity_dedup", "exec_dedup"]);
        let r = check_dead_levers(&home);
        assert_eq!(r.status, CheckStatus::Info);
        assert!(
            r.message.contains("one cause, not"),
            "most of the table at zero is one finding: {}",
            r.message
        );
        assert!(
            r.detail.len() <= 8,
            "the list is evidence, not the report — got {} lines",
            r.detail.len()
        );
        let joined = r.detail.join("\n");
        assert!(
            joined.contains("… and ") && joined.contains(" more"),
            "the lines not shown must still be counted: {joined}"
        );
        assert!(
            joined.contains("hook_binaries_agree"),
            "the usual cause is a check we already have; point at it: {joined}"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// The other side: a handful of quiet levers on an otherwise busy machine
    /// IS a per-lever finding, and must keep the wording that says so.
    #[test]
    fn a_few_quiet_levers_are_still_reported_one_by_one() {
        let live: Vec<&str> = LEVERS.iter().skip(2).map(|(needle, _)| *needle).collect();
        let home = home_with_ops(&live);
        let r = check_dead_levers(&home);
        assert_eq!(r.status, CheckStatus::Info);
        assert!(
            r.message.contains("possible dead routing"),
            "two quiet levers is the per-lever case: {}",
            r.message
        );
        assert!(
            !r.message.contains("one cause"),
            "and must NOT claim a single upstream cause: {}",
            r.message
        );
        assert_eq!(r.detail.len(), 2, "both are named: {:?}", r.detail);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// The two lists must stay disjoint and non-empty — a name in both would
    /// make "we decided this is not a lever" and "we watch this lever" the same
    /// statement.
    #[test]
    fn a_lever_is_never_also_a_declared_non_lever() {
        assert!(!LEVERS.is_empty() && !NOT_LEVERS.is_empty());
        for (name, _) in NOT_LEVERS {
            assert!(
                !LEVERS.iter().any(|(needle, _)| name.contains(needle)),
                "{name} is declared as not-a-lever and is also watched as one"
            );
        }
        for (_, reason) in NOT_LEVERS {
            assert!(!reason.is_empty(), "every exclusion carries its reason");
        }
    }
}

#[cfg(test)]
mod a_bridge_is_only_firing_if_it_fired_recently {
    use super::*;

    fn secs_ago(d: u64) -> Option<u64> {
        Some(tokenade_core::platform::now_unix_secs().saturating_sub(d))
    }

    /// Both bridge checks spoke in the present tense from evidence with no time
    /// bound: the ledger scan asked whether an `agent` row had EVER been
    /// written, and the Hermes check asked whether a marker file exists — a
    /// file written once and never removed. A bridge that stopped loading
    /// months ago read `plugin active — tool-result compaction firing`.
    #[test]
    fn a_bridge_silent_for_months_is_not_firing() {
        let r = bridge_verdict("plugin-bridge", "opencode", secs_ago(90 * 86_400));
        assert_eq!(r.status, CheckStatus::Info);
        assert!(
            r.message.contains("last compacted") && r.message.contains("90d"),
            "say when it last worked: {}",
            r.message
        );
        assert!(
            !r.message.contains("firing"),
            "and stop claiming it is firing: {}",
            r.message
        );
        assert!(
            r.message.contains("shell-proxy"),
            "the user must know what still works: {}",
            r.message
        );
    }

    /// Recent enough is still OK, and the wording is unchanged for it — this
    /// change is about the claim being false, not about adding noise to the
    /// case that was always right.
    #[test]
    fn a_bridge_that_worked_today_still_reads_active() {
        for age in [0, 3600, 29 * 86_400] {
            let r = bridge_verdict("plugin-bridge", "openclaw", secs_ago(age));
            assert_eq!(r.status, CheckStatus::Ok, "age {age}: {:?}", r.message);
            assert!(r.message.contains("active — tool-result compaction firing"));
        }
    }

    /// Never seen is a different message from gone quiet: one says start it,
    /// the other says it stopped. Collapsing them would tell somebody whose
    /// bridge broke to "start it once", which they have already done.
    #[test]
    fn never_seen_and_gone_quiet_are_different_answers() {
        let never = bridge_verdict("plugin-bridge", "cline", None);
        assert_eq!(never.status, CheckStatus::Info);
        assert!(never.message.contains("no saving was ever recorded"));
        assert!(!never.message.contains("last compacted"));

        // A zero timestamp is "no timestamp", not 1970 — ledger rows written
        // before the field existed carry it.
        let zero = bridge_verdict("plugin-bridge", "cline", Some(0));
        assert_eq!(zero.message, never.message, "0 is absence, not an age");
    }

    /// The two call sites that FEED the rule, not just the rule: the ledger's
    /// `ts` and the marker file's mtime. Mutating either to a constant leaves
    /// every test above green while every bridge on the machine reports as
    /// decades silent — the fix living in a line no test crosses.
    #[test]
    fn the_hermes_marker_age_reaches_the_rule() {
        let home = std::env::temp_dir().join(format!("tokenade-hermes-{}", std::process::id()));
        let dir = crate::install_cli::hermes_home_dir(&home)
            .join("plugins")
            .join("tokenade");
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["__init__.py", "hook-registered", "hook-fired"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        // Fresh markers: the healthy case.
        let r = check_hermes_plugin(&home).expect("the plugin is installed");
        assert_eq!(r.status, CheckStatus::Ok, "{}", r.message);

        // Same markers, `hook-fired` backdated well past the window. The file
        // still EXISTS — which is all the check used to ask.
        let old = std::time::SystemTime::now()
            .checked_sub(std::time::Duration::from_secs(
                (BRIDGE_SILENT_DAYS + 45) * 86_400,
            ))
            .unwrap();
        let f = std::fs::OpenOptions::new()
            .write(true)
            .open(dir.join("hook-fired"))
            .unwrap();
        f.set_modified(old).unwrap();
        drop(f);
        let r = check_hermes_plugin(&home).expect("the plugin is still installed");
        assert_eq!(
            r.status,
            CheckStatus::Info,
            "a marker written months ago is not a hook firing now: {}",
            r.message
        );
        assert!(r.message.contains("last compacted"), "{}", r.message);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// The ledger side of the same wiring: an `agent` row's timestamp is what
    /// dates the bridge, and reading only its presence dates every bridge to
    /// the epoch.
    #[test]
    fn the_ledger_timestamp_reaches_the_rule() {
        let home = std::env::temp_dir().join(format!("tokenade-ledger-{}", std::process::id()));
        let dir = home.join(".tokenade").join("openclaw-plugin");
        std::fs::create_dir_all(&dir).unwrap();
        // The marker `plugin_bridges` looks for, whatever it is named.
        let marker = crate::install_cli::plugin_bridges(&home)
            .iter()
            .find(|(label, _, _, _)| *label == "openclaw")
            .map(|(_, m, _, _)| *m)
            .expect("openclaw is a known bridge");
        std::fs::write(dir.join(marker), b"x").unwrap();

        let now = tokenade_core::platform::now_unix_secs();
        let write_row = |ts: u64| {
            std::fs::write(
                home.join(".tokenade").join("gain.jsonl"),
                format!(
                    "{{\"ts\":{ts},\"op\":\"wrap\",\"agent\":\"openclaw\",\
                     \"tokens_before\":100,\"tokens_after\":10,\"saved\":90}}\n"
                ),
            )
            .unwrap();
        };

        write_row(now);
        let fresh = check_plugin_bridges(&home);
        assert!(
            fresh.iter().any(|r| r.status == CheckStatus::Ok),
            "a saving recorded now is a working bridge: {:?}",
            fresh.iter().map(|r| &r.message).collect::<Vec<_>>()
        );

        write_row(now.saturating_sub((BRIDGE_SILENT_DAYS + 45) * 86_400));
        let stale = check_plugin_bridges(&home);
        assert!(
            stale
                .iter()
                .any(|r| r.status == CheckStatus::Info && r.message.contains("last compacted")),
            "a saving from months ago is not a bridge firing now: {:?}",
            stale.iter().map(|r| &r.message).collect::<Vec<_>>()
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A stamp ahead of the clock is a clock that moved. It must not underflow
    /// into a colossal age and report a working bridge as decades silent.
    #[test]
    fn a_timestamp_from_the_future_is_not_an_age() {
        let ahead = tokenade_core::platform::now_unix_secs().saturating_add(86_400);
        let r = bridge_verdict("plugin-bridge", "pi", Some(ahead));
        assert_eq!(r.status, CheckStatus::Ok, "{}", r.message);
    }
}
