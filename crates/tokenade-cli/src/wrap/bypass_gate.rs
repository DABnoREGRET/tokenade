//! **When tokenade must step aside, and SAY so.**
//!
//! A silent bypass is a loss nobody measures. The gate here decides whether
//! to compact, logs wide openings, and pushes a reminder when an agent
//! bypasses repeatedly — without ever getting in the way of an interactive
//! command, nor calling itself.

use super::*;

/// #1031 — commands whose tokenade compactor is lossless at the line
/// granularity an agent cares about. Bypassing one of these for
/// "verbatim bytes" is almost always a defensive reflex, not a real
/// need — every byte the post-bypass output would contain is already
/// recoverable via `expand_ref hash=…` from the compactor's banner.
///
/// Membership criteria (be conservative — false positives block real
/// work, false negatives just lose one nudge):
///   * grep family: compactor strips common path prefix only, every
///     match line preserved verbatim;
///   * find / ls: tiny structured output, our compactor is passthrough
///     for short results and only dedups path components on long ones;
///   * wc / head / tail: tiny outputs, passthrough below threshold;
///   * awk / sed / cat: passthrough source-code-aware (#570/#590), no
///     loss for the cases an agent would actually want verbatim.
///
/// NOT in the list (intentionally): curl, jq, kubectl, git, ssh,
/// docker — these have lossy or transforming compactors where an
/// agent might legitimately want raw bytes for one specific case.
pub(crate) const SAFE_CLASS_FOR_BYPASS_REFUSAL: &[&str] = &[
    "grep", "egrep", "fgrep", "rg", "ag", "ack", "find", "ls", "wc", "head", "tail", "awk", "gawk",
    "mawk", "sed", "cat",
];

/// The same question, asked by the DOOR agents actually use.
/// `tokenade raw grep …` is refused; `TOKENADE_HOOK_DISABLED=1 grep …` was
/// not even noticed, while the transcript corpus counts 2,329 bypasses via
/// the variable against 133 via `tokenade raw`.
/// One list, two doors.
pub(crate) fn is_safe_class(prog: &str) -> bool {
    let base = std::path::Path::new(prog)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(prog)
        .trim_end_matches(".exe")
        .to_ascii_lowercase();
    SAFE_CLASS_FOR_BYPASS_REFUSAL.contains(&base.as_str())
}

#[cfg(test)]
mod a_named_recovery_route_is_not_a_refused_one {
    use super::SAFE_CLASS_FOR_BYPASS_REFUSAL;

    /// A banner that names `tokenade raw <prog>` must name a program `raw`
    /// will actually run.
    ///
    /// `raw` refuses the programs above, on purpose: re-running `grep` spends
    /// the original call's tokens again and `expand-ref` already holds what
    /// was folded. But a compactor whose banner offers `tokenade raw` as the
    /// way back to what it REMOVED — `git blame`, which strips the author and
    /// the date from every line it keeps — needs that offer to run. The list's
    /// own comment says `git` is left out deliberately for exactly this
    /// reason; nothing until now held the two together, so adding `git` to the
    /// list would have turned that banner into a dead route in silence.
    ///
    /// This is the invariant the e2e guards state as "every recovery route a
    /// banner names runs". They drive `wrap` with FILES, so they reach the
    /// content-routed compactors only — `git blame` is reached by command
    /// NAME, and `cat`ing blame-shaped bytes never routes there. Measured by
    /// mutation on 2026-09-06: stripping the banner left those guards green.
    #[test]
    fn the_programs_our_banners_offer_raw_for_are_not_refused() {
        for prog in ["git", "cargo"] {
            assert!(
                !SAFE_CLASS_FOR_BYPASS_REFUSAL.contains(&prog),
                "a compactor banner offers `tokenade raw {prog} …` as the way back to what it \
                 removed, but `{prog}` is on the bypass-refusal list, so that route now refuses. \
                 Either drop it from the list or change the banner — a named route that does not \
                 run is the failure mode those banners exist to avoid"
            );
        }
    }

    /// Anti-vacuity: the list is not empty and still refuses what it should,
    /// so the assertion above cannot pass by the list having been emptied.
    #[test]
    fn the_refusal_list_still_refuses_the_cheap_re_runs() {
        for prog in ["grep", "find", "ls", "cat"] {
            assert!(
                SAFE_CLASS_FOR_BYPASS_REFUSAL.contains(&prog),
                "`{prog}` left the refusal list — re-running it spends the original call's \
                 tokens again, which is what the list exists to stop"
            );
        }
    }
}

/// #856 — bump the counter + emit the stderr nudge if past threshold.
/// Called from BOTH bypass paths (cmd_raw and env-disabled hook gates)
/// so the agent always sees the nudge, regardless of which mechanism
/// they used. The nudge cannot be silenced.
/// Returns the message when the nudge fires, so a caller that HAS a delivery
/// channel can use it. The `eprintln!` below reaches the agent only where hook
/// stderr is surfaced, and on a PreToolUse hook that returns "allow" it is
/// not: measured 2026-09-02 on this install, 885 recorded fires of
/// `nudge:bypass:fire` and not one of them ever seen, while the batching nudge
/// ten lines away in `bash_pretool` arrives every time because it travels in
/// `hookSpecificOutput.additionalContext`. The stderr write stays — other
/// agents do surface it, and `tokenade raw` is a plain terminal command.
/// To be called by the site that ACTUALLY put the message in front of the
/// agent — not by the one that composed it. See `maybe_emit_bypass_nudge`.
pub(crate) fn book_bypass_delivered(count: usize) {
    tokenade_core::gain::record_nudge(
        "bypass",
        "fire",
        tokenade_core::gain::NudgeMetric::Occurrence(count),
    );
}

pub(crate) fn maybe_emit_bypass_nudge() -> Option<String> {
    if let Some(count) = bump_bypass_counter() {
        // **Noticing a bypass is not talking to the agent.**
        //
        // This site used to write `nudge:bypass:fire` — 19,037 times in 30 days
        // over the real population, 38.5 % of EVERYTHING the product counts as
        // a nudge, and zero observed outcome. The cause is written ten lines
        // above: this message's channel is the stderr of a PreToolUse hook
        // that answers "allow", and Claude Code does not show it. A fire
        // counted where nothing is delivered is not zero adherence, it is a
        // measurement of nothing.
        //
        // So: this counter records an OBSERVATION, outside the nudge
        // taxonomy; `book_bypass_delivered` records the fire, and only the
        // callers that really have a channel call it.
        tokenade_core::gain::record_observation("bypass", count as usize);
        // #1031 — reframed to lead with the actual recovery path
        // (expand_ref), not with "drop bypass when unsure". Agents
        // bypass defensively when they fear the compactor mangles
        // bytes they need; the right answer to that fear is
        // `expand_ref hash=…`, not a preventive bypass. Keeping the
        // bypass-first phrasing (the prior text) reinforced the same
        // mental model that produced the unneeded bypass.
        let msg = format!(
            "[tokenade] {count} bypasses in 24h — most are unneeded. \
             Need the verbatim bytes? Run `tokenade expand-ref <hash>` \
             (the hash is printed in every compactor banner) — instant, no re-run, no bypass."
        );
        eprintln!("{msg}");
        return Some(msg);
    }
    None
}

/// #856 — Single chokepoint for all hook handlers checking
/// `TOKENADE_HOOK_DISABLED=1`. Returns true (bail) when the env var
/// is set AND emits the bypass nudge so the agent sees it even when
/// they ONLY use env-var bypass (never explicit `tokenade raw`).
pub(crate) fn hook_disabled_by_env() -> bool {
    if !tokenade_core::env_compat::is_truthy("TOKENADE_HOOK_DISABLED", "STS_HOOK_DISABLED") {
        return false;
    }
    let _ = maybe_emit_bypass_nudge();
    true
}

/// #977 — Bash one-shot env-prefix bypass detection.
///
/// `cat foo`'s parent shell evaluates `TOKENADE_HOOK_DISABLED=1 cat foo`
/// by setting the env var ONLY for the `cat` child process. The
/// Claude Code PostToolUse / PreToolUse hooks run in a SEPARATE
/// process tree spawned by the agent — they never inherit anything
/// from the user's bash command, so `hook_disabled_by_env()` returns
/// false and the hook compacts the output anyway. Pre-fix evidence:
/// the user's `tokenade gain --history` literally contained a
/// `TOKENADE_HOOK_DISABLED=1 cat .gitignore` row at 33% saved.
///
/// The docs (esp. `GLOBAL_CLAUDE_MD_BLOCK`) call this pattern a
/// "one-shot escape" — to make that claim true, the hook inspects
/// the cmdline string Claude Code hands it and honors a leading
/// bash env-prefix matching one of the supported disable env vars.
///
/// Bash env-prefix grammar: zero or more `KEY=VALUE` tokens with
/// no spaces, then the actual command. We scan from the start; any
/// `TOKENADE_HOOK_DISABLED` (or its legacy `STS_*` alias) set to a
/// truthy value before the first non-env token returns true.
pub(crate) fn cmdline_requests_hook_bypass(cmd: &str) -> bool {
    for token in cmd.split_whitespace() {
        let Some(eq) = token.find('=') else {
            // First non-env-prefix token = the actual program. Stop.
            return false;
        };
        let (key, val) = token.split_at(eq);
        let val = &val[1..]; // skip the '='
                             // Bash treats KEY= as legal (empty value). We only flag when
                             // the VALUE itself is truthy — mirrors env_compat::is_truthy
                             // case-insensitively. An empty / "0" / "false" value behaves
                             // as a no-op env-prefix; do NOT flag.
        let truthy = matches!(val.to_ascii_lowercase().as_str(), "1" | "true" | "yes");
        if truthy && matches!(key, "TOKENADE_HOOK_DISABLED" | "STS_HOOK_DISABLED") {
            return true;
        }
        // Other env-prefix (e.g. `LANG=C cmd`) — keep scanning for a
        // possible TOKENADE_HOOK_DISABLED= later in the prefix chain.
    }
    false
}

/// Was a bypass honoured ANYWHERE in the command?
///
/// `cmdline_requests_hook_bypass` reads a LEADING env prefix, which is the
/// grammar bash uses — and the whole command's leading prefix is not the only
/// place one appears. The wrap path already knows this: it asks the same
/// question of every chain segment, so `cd /repo` on one line and
/// `TOKENADE_HOOK_DISABLED=1 cargo test` on the next leaves that segment
/// unwrapped, exactly as asked. The COUNTING did not follow. Measured over
/// 89 633 commands: 2 948 carry a bypass in some segment and only 458 are in
/// the leading position, so the nudge that exists to catch over-bypassing saw
/// one in six of them — and its threshold is ten a day.
///
/// Reuses the segmenter the wrap path uses, and the grammar above, so a third
/// spelling of either cannot drift from the two that decide behaviour.
/// Returns the segment that asked, so the caller can ask ITS program what it
/// is — `invokes_tokenade_itself` on the whole command reads `cd` and answers
/// about the wrong thing.
pub(crate) fn bypassed_segment(cmd: &str) -> Option<&str> {
    if cmdline_requests_hook_bypass(cmd) {
        return Some(cmd);
    }
    tokenade_core::command_rewrite::split_chain_segments(cmd)?
        .into_iter()
        .map(|seg| seg.cmd.trim())
        .find(|seg| cmdline_requests_hook_bypass(seg))
}

/// Is the command behind the env prefix tokenade itself?
///
/// `TOKENADE_HOOK_DISABLED=1 tokenade wrap '<cmd>'` is not a bypass: the hook
/// is off precisely BECAUSE tokenade is being invoked directly, and the output
/// gets compacted either way. Counting it as one both inflates the "lost
/// savings" figure and fires the over-bypass nudge at someone doing the right
/// thing — and a nudge that cries wolf gets ignored, then removed.
///
/// The callers already knew this: both sites follow the bypass check with
/// `if cmd.starts_with("tokenade ") { return 0 }` ("don't double-wrap"). The
/// bypass tick simply ran first and returned before reaching it, so the SAME
/// command counted as a bypass with the env prefix and not without it.
/// Measured 2026-08-04: 19 of 19 bypasses in a 13 h window were
/// `TOKENADE_HOOK_DISABLED=1 …/tokenade wrap …`.
///
/// `tokenade raw` is deliberately covered too — it is already excluded from
/// this counter when written without the env prefix, so this makes the two
/// spellings agree rather than dropping accounting that was happening.
pub(crate) fn invokes_tokenade_itself(cmd: &str) -> bool {
    for token in cmd.split_whitespace() {
        if token.contains('=') && !token.contains('/') {
            continue; // env-assignment prefix (`FOO=bar`), keep scanning
        }
        let name = token.rsplit(['/', '\\']).next().unwrap_or(token);
        return name == "tokenade" || name == "tokenade.exe";
    }
    false
}

pub(crate) const BYPASS_NUDGE_THRESHOLD: u64 = 10;
pub(crate) const BYPASS_NUDGE_WINDOW_SECS: u64 = 24 * 3600;

/// Pure half of `maybe_emit_bypass_nudge`. Given `now` (unix epoch),
/// previous `(window_start, count)` if any, and a threshold, returns
/// `((new_window_start, new_count), should_nudge)`. The window rolls
/// when `now - window_start > BYPASS_NUDGE_WINDOW_SECS`. Nudge fires
/// every time count is at or past the threshold (loud on purpose — an
/// agent over-bypassing should notice on every bypass past N).
pub(crate) fn bypass_nudge_step(
    now: u64,
    prev: Option<(u64, u64)>,
    threshold: u64,
) -> ((u64, u64), bool) {
    let (new_window, new_count) = match prev {
        Some((ws, c)) if now.saturating_sub(ws) <= BYPASS_NUDGE_WINDOW_SECS => (ws, c + 1),
        _ => (now, 1),
    };
    let should_nudge = new_count >= threshold;
    ((new_window, new_count), should_nudge)
}

#[cfg(all(test, unix))]
#[cfg(test)]
mod binary_shortcircuit_tests {
    use std::os::unix::process::ExitStatusExt as _;

    fn out(stdout: &[u8]) -> std::process::Output {
        std::process::Output {
            status: std::process::ExitStatus::from_raw(0),
            stdout: stdout.to_vec(),
            stderr: Vec::new(),
        }
    }

    /// `serve_binary` answers for BINARY stdout and stays out of the way for
    /// text. It is the gate in front of the whole compactor pipeline, so an
    /// unconditional `Some` here silently disables compaction for every
    /// command — which is exactly what happened when this body was lifted out
    /// of `cmd_wrap` without its `detect_binary` test. Only the e2e suite
    /// noticed; nothing below `cmd_wrap` was wrong, so no unit test failed.
    #[test]
    fn text_output_never_takes_the_binary_short_circuit() {
        let log: String = (1..=50)
            .map(|i| format!("2026-01-01 INFO worker {i} ok\n"))
            .collect();
        assert!(
            super::serve_binary(&out(log.as_bytes())).is_none(),
            "plain text must fall through to the compactor"
        );
        assert!(
            super::serve_binary(&out(b"")).is_none(),
            "empty is not binary"
        );
    }

    #[test]
    fn a_recognisable_binary_blob_still_short_circuits() {
        let png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x06\x00\x00\x00\x1f\x15\xc4\x89";
        assert!(
            super::serve_binary(&out(png)).is_some(),
            "a PNG must not be handed to a text compactor"
        );
    }
}

#[cfg(test)]
mod swallowed_failure {
    use super::*;

    // ── the marker set ──────────────────────────────────────────────────
    #[test]
    fn a_green_cargo_summary_is_not_failure_evidence() {
        // THE trap this whole check is calibrated against: every passing run
        // ends in "0 failed". Counting `\d+ failed` reported 2797 swallowed
        // failures where 830 were real.
        let green = "test result: ok. 7386 passed; 0 failed; 0 ignored; 0 measured";
        assert_eq!(failure_evidence(green), None, "{green:?} is a PASSING run");
        assert_eq!(failure_evidence("1 passed, 0 failed"), None);
        assert_eq!(failure_evidence("errors: 0\nfailed: 0"), None);
    }

    #[test]
    fn the_markers_the_corpus_actually_produced_are_evidence() {
        for m in [
            "running 3 tests\ntest result: FAILED. 2 passed; 1 failed",
            "error[E0308]: mismatched types",
            "src/a.ts(3,1): error TS2345: bad arg",
            "npm ERR! code ELIFECYCLE",
            "[ERROR] BUILD FAILURE",
        ] {
            assert!(failure_evidence(m).is_some(), "should be evidence: {m:?}");
        }
    }

    /// A build tool refusing the command it was given closes with its own
    /// `error:` line, and naming those one at a time does not converge: 233
    /// results across the whole corpus carry one that no listed marker sees.
    #[test]
    fn a_build_tool_refusing_the_command_is_evidence() {
        for m in [
            "error: test failed, to rerun pass `-p tokenade-core --lib`",
            "error: no library targets found in package `tokenade-cli`",
            "error: unexpected argument '--zzz' found",
            "error: unrecognized subcommand 'buidl'",
            "error: no bin target named `x` in default-run packages",
            "error: failed to run custom build command for `ring v0.17`",
            "warming up\nerror: the following required arguments were not provided:",
        ] {
            assert!(failure_evidence(m).is_some(), "should be evidence: {m:?}");
        }
    }

    /// `go test` closes a failing package with `FAIL\tpkg`, jest with
    /// `FAIL path/to/spec`. 210 results in the corpus carry one.
    #[test]
    fn a_failing_test_runner_line_is_evidence() {
        for m in [
            "ok  \tfmt\t0.2s\nFAIL\tgithub.com/x/y\t0.4s",
            "FAIL src/app.test.js",
            "FAIL validate           0.6s  1 error(s), 0 warning(s)",
        ] {
            assert!(failure_evidence(m).is_some(), "should be evidence: {m:?}");
        }
    }

    /// Both new rules read the START of a line only. Mid-sentence prose and a
    /// path like `src/error.rs` are not a build tool refusing anything, and a
    /// run that mentions FAILSAFE has not failed.
    #[test]
    fn the_same_words_inside_a_line_are_not_evidence() {
        for m in [
            "compiled src/error.rs in 0.4s",
            "see the error: section of the manual",
            "  error: indented, so it is a nested diagnostic body",
            "FAILSAFE mode enabled",
            "FAILOVER complete",
            "the test did not FAIL today",
        ] {
            assert_eq!(failure_evidence(m), None, "should NOT be evidence: {m:?}");
        }
    }

    /// The disclosure quotes what happened, not the rule that caught it.
    #[test]
    fn the_evidence_is_the_line_not_the_prefix() {
        let line = "error: no bin target named `zz` in default-run packages";
        assert_eq!(
            failure_evidence(&format!("noise\n{line}\nmore")),
            Some(line)
        );
        // A listed marker still answers with itself, as it always did.
        assert_eq!(
            failure_evidence("running\ntest result: FAILED. 1 failed"),
            Some("test result: FAILED")
        );
    }

    /// A build line can be thousands of columns wide; the message must not be.
    #[test]
    fn a_long_line_is_clipped_on_a_character_boundary() {
        let long = format!("error: {}", "é".repeat(400));
        let got = failure_evidence(&long).expect("evidence");
        assert!(got.len() < long.len(), "must be clipped");
        assert!(
            long.starts_with(got),
            "the clip must be a prefix of the line"
        );
        assert_eq!(got.chars().count(), 96, "clipped by CHARACTERS, not bytes");
    }

    // ── the allowlist ───────────────────────────────────────────────────
    #[test]
    fn only_a_command_that_runs_a_build_or_test_can_be_contradicted() {
        assert!(runs_a_build_or_test("cargo test -p core 2>&1 | tail -40"));
        assert!(runs_a_build_or_test("cd /x && npm run build | head"));
        assert!(
            runs_a_build_or_test("/usr/bin/cargo build"),
            "absolute path"
        );
        assert!(
            runs_a_build_or_test("RUST_LOG=debug cargo test"),
            "env prefix"
        );
        // Searching FOR the marker is not running the tool that emits it.
        assert!(!runs_a_build_or_test(
            "grep 'test result: FAILED' build.log"
        ));
        assert!(!runs_a_build_or_test("rg 'error\\[E0308\\]' src/"));
        assert!(
            !runs_a_build_or_test("cat cargo-notes.txt"),
            "cargo as an ARG"
        );
    }

    /// Three markers the list did not carry, each measured on the corpus:
    /// a rust panic (54 cases once the commands that GREP for the word are
    /// excluded, which is 5/6 of the raw count), a rustc error with no
    /// diagnostic code, and an uncaught python exception.
    #[test]
    fn a_panic_an_uncoded_rustc_error_and_a_python_traceback_are_evidence() {
        for m in [
            "thread 'main' panicked at src/lib.rs:4:9:\nassertion failed",
            "error: could not compile `core` (lib test) due to 3 previous errors",
            "Traceback (most recent call last):\n  File \"x.py\"\nAssertionError",
        ] {
            assert!(failure_evidence(m).is_some(), "should be evidence: {m:?}");
        }
    }

    /// A wrapper runs the build; the build is still what ran. `timeout` was
    /// the common one — it takes a duration BEFORE the program, so neither
    /// the flag skip nor the env-assignment skip reached past it.
    #[test]
    fn a_wrapper_does_not_hide_the_program_it_runs() {
        assert!(runs_a_build_or_test("timeout 60 cargo test | tail -5"));
        assert!(runs_a_build_or_test("timeout 90s cargo build"), "suffixed");
        assert!(runs_a_build_or_test(
            "timeout --preserve-status 5 cargo test"
        ));
        assert!(runs_a_build_or_test("env FOO=1 cargo test"));
        assert!(runs_a_build_or_test("nice -n 19 make"));
        // The wrapper is not itself a build, and neither is what it runs here.
        assert!(!runs_a_build_or_test("timeout 60 sleep 120"));
        assert!(!runs_a_build_or_test("sudo systemctl restart nginx"));
        // A stage that is nothing BUT wrappers must decide, not panic.
        assert!(!runs_a_build_or_test("timeout 60"));
        assert!(!runs_a_build_or_test("env"));
    }

    #[test]
    fn a_stage_leader_is_not_an_argument() {
        // `cargo` here is the argument of `echo`, not a stage leader.
        assert!(!runs_a_build_or_test("echo cargo test"));
        // ...but put it after a pipe and it IS one.
        assert!(runs_a_build_or_test("echo x | cargo run"));
    }
}
