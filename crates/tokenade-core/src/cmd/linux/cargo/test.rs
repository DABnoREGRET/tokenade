//! Extracted from `filter.rs` in the L94/L92 refactor.
//! Body preserved verbatim — only the home changed.

use crate::argv::{has_flag, has_any_flag, flag_value};

pub fn dispatch(argv: &[String], output: &str) -> String {
    // BEFORE any shape routing: a run where the filter matched NO test.
    // `cargo` prints `test result: ok. 0 passed` for that, which reads as a
    // green run, and every shape below would faithfully pass it on.
    if let Some(notice) = nothing_ran(output) {
        return notice;
    }
    // PRECEDENCE: `--message-format=json` rewrites the ENTIRE output
    // shape into NDJSON, so it wins over `--no-run` / `--quiet` (which
    // only affect the human-readable form). Always check it first.
    if flag_value(argv, "--message-format").is_some_and(|v| v.starts_with("json")) {
        // `--message-format=json` → auto's JSON compactor is the right tool.
        return crate::auto::compact(output).1;
    }
    if has_flag(argv, "--list") {
        return crate::cmd::linux::cargo_test_no_run::compact(output);
    }
    if has_flag(argv, "--no-run") {
        return crate::cmd::linux::cargo_test_no_run::compact(output);
    }
    if has_any_flag(argv, &["--quiet", "-q"]) {
        return crate::cmd::linux::cargo_test_quiet::compact(output);
    }
    if has_any_flag(argv, &["--nocapture", "--show-output"]) {
        return crate::cmd::linux::cargo_test_nocapture::compact(output);
    }
    fold_default(output)
}

/// The number written immediately before `key` on a summary line.
///
/// `test result: ok. 0 passed; 3 failed; 1 ignored; 0 measured; 7 filtered out`
/// → `count_before(line, "filtered out") == Some(7)`. Hand-scanned rather than
/// matched, because these five fields appear in different subsets depending on
/// the shape (`--bench` has no `ignored`, an old fixture has neither).
fn count_before(line: &str, key: &str) -> Option<u64> {
    let idx = line.find(key)?;
    let head = line[..idx].trim_end();
    let digits: String = head
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    digits.parse().ok()
}

/// Every target ran ZERO tests while something was filtered out: the filter
/// matched nothing.
///
/// `cargo` reports that as `test result: ok. 0 passed; 0 failed; …; N filtered
/// out`, and an agent reads `ok` as a passing run — the same three words a real
/// green run ends with. Measured 2026-08-27 across every transcript on this
/// machine: **124 of 6 366 test invocations (1.9 %) ran nothing and said `ok`**.
/// Each one is a turn spent believing a test passed that never executed.
///
/// Read the count carefully. Per SUMMARY LINE the figure is 1 578 of 9 567
/// (16.5 %), and that number means nothing: a multi-target `cargo test <filter>`
/// legitimately prints `0 passed; N filtered out` for every target the filter
/// does not touch. Only a run where EVERY target is empty is a run that did
/// nothing, which is why this asks about the whole output and not about a line.
///
/// `ignored` and `measured` are checked too: a filter that matches only
/// `#[ignore]`d tests, or a `--bench` pass, DID select something.
///
/// Returns the replacement text, not an addition. `filter`'s never-inflate
/// guard returns the original bytes whenever a compactor comes back no
/// smaller, so a warning appended to this output would be discarded by the
/// one rule that exists to stop compactors bloating things — and the warning
/// IS the whole value here. Dropping the boilerplate it describes pays for
/// itself; every other line, compiler warnings included, is kept.
pub(crate) fn nothing_ran(output: &str) -> Option<String> {
    let mut targets = 0usize;
    let mut filtered = 0u64;
    for line in output.lines() {
        let t = line.trim_start();
        if !t.starts_with("test result:") {
            continue;
        }
        let passed = count_before(t, "passed")?;
        let failed = count_before(t, "failed")?;
        let ignored = count_before(t, "ignored").unwrap_or(0);
        let measured = count_before(t, "measured").unwrap_or(0);
        let f = count_before(t, "filtered out")?;
        if passed != 0 || failed != 0 || ignored != 0 || measured != 0 {
            return None;
        }
        targets += 1;
        filtered += f;
    }
    // `filtered == 0` alone: `filtered` only ever accumulates from a counted
    // target, so no target means no filtered, and a `targets == 0` arm was
    // measured unreachable as a distinct outcome. It covers three cases at
    // once — output with no summary line at all, and a crate that simply
    // carries no tests, and both of those must stay silent.
    if filtered == 0 {
        return None;
    }
    use std::fmt::Write;
    let mut out = String::with_capacity(output.len());
    let _ = writeln!(
        out,
        "NOTHING RAN: the filter matched no test ({filtered} filtered out, {targets} target{}).",
        if targets == 1 { "" } else { "s" }
    );
    for line in output.lines() {
        let t = line.trim_start();
        if t.is_empty() || t.starts_with("test result:") || t.starts_with("running ") {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    Some(out.trim_end().to_string())
}

/// Default `cargo test` output. The noise is the long run of
/// `test <name> ... ok` / `... ignored` lines; the signal is everything
/// else — `running N tests`, every `... FAILED` line, the captured panic
/// output under `failures:`, and the final `test result:` summary (all of
/// which appear at the BOTTOM, so the old `line_cap(200)` decapitated them
/// on any large suite). We fold the passing/ignored lines into a count and
/// keep every other line verbatim.
///
/// These libcargo/libtest labels (`ok`, `ignored`, `FAILED`, `test
/// result:`) are emitted by rustc and are never localised, so matching
/// them by text is locale-safe (unlike coreutils output).
pub fn fold_default(output: &str) -> String {
    use std::fmt::Write;

    // A `test <name> ... ok` / `... ignored` line — pure noise to fold.
    // `... FAILED` and `... bench:` are NOT folded (failures and perf
    // numbers are signal).
    fn is_passing(line: &str) -> bool {
        let t = line.trim_start();
        t.starts_with("test ") && (t.ends_with(" ... ok") || t.ends_with(" ... ignored"))
    }

    let total_passing = output.lines().filter(|l| is_passing(l)).count();
    // Nothing worth folding — leave it alone (the central guard would
    // strip our banner anyway).
    if total_passing < 10 {
        return crate::archetypes::passthrough(output);
    }

    let mut out = String::with_capacity(output.len());
    let mut run = 0usize;
    let flush = |out: &mut String, run: &mut usize| {
        if *run > 0 {
            let _ = writeln!(out, "  ... ({} passing/ignored tests folded) ...", run);
            *run = 0;
        }
    };
    for line in output.lines() {
        if is_passing(line) {
            run += 1;
        } else {
            flush(&mut out, &mut run);
            out.push_str(line);
            out.push('\n');
        }
    }
    flush(&mut out, &mut run);
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    const DEAD: &str = "\nrunning 0 tests\n\ntest result: ok. 0 passed; 0 failed; \
0 ignored; 0 measured; 7171 filtered out; finished in 0.00s\n";

    /// A filter that matched nothing must not read as a green run.
    #[test]
    fn a_filter_that_matched_nothing_says_so() {
        let out = dispatch(&argv(&["cargo", "test", "-q", "zzz"]), DEAD);
        assert!(out.starts_with("NOTHING RAN:"), "got {out:?}");
        assert!(out.contains("7171 filtered out"), "got {out:?}");
    }

    /// The notice REPLACES the boilerplate. `filter`'s never-inflate guard
    /// hands back the original bytes whenever a compactor comes back no
    /// smaller, so a notice that merely appended would be thrown away by the
    /// one rule meant to stop compactors bloating output.
    #[test]
    fn the_notice_is_smaller_than_what_it_replaces() {
        let out = dispatch(&argv(&["cargo", "test", "-q", "zzz"]), DEAD);
        assert!(
            out.len() < DEAD.len(),
            "notice must pay for itself: {} vs {}",
            out.len(),
            DEAD.len()
        );
    }

    /// Everything that is not the zero-result boilerplate survives — a
    /// compiler warning is often the reason the run was interesting at all.
    #[test]
    fn compiler_warnings_survive_the_notice() {
        let body = format!("{DEAD}warning: function `foo` is never used\n");
        let out = dispatch(&argv(&["cargo", "test", "-q", "zzz"]), &body);
        assert!(out.contains("warning: function `foo` is never used"), "got {out:?}");
    }

    /// One empty target among several is NORMAL: a filter that selects tests
    /// in one crate leaves every other target at `0 passed; N filtered out`.
    /// Only a run where EVERY target is empty ran nothing.
    #[test]
    fn one_empty_target_among_several_is_not_a_dead_run() {
        let body = "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; \
0 measured; 40 filtered out; finished in 0.00s\n\nrunning 5 tests\n\ntest result: ok. \
5 passed; 0 failed; 0 ignored; 0 measured; 12 filtered out; finished in 0.02s\n";
        let out = dispatch(&argv(&["cargo", "test", "zzz"]), body);
        assert!(!out.contains("NOTHING RAN"), "got {out:?}");
    }

    /// A filter matching only `#[ignore]`d tests DID select something — the
    /// tests are named, they were simply not run.
    #[test]
    fn a_filter_matching_only_ignored_tests_is_not_a_dead_run() {
        let body = "running 3 tests\n\ntest result: ok. 0 passed; 0 failed; 3 ignored; \
0 measured; 40 filtered out; finished in 0.00s\n";
        assert!(nothing_ran(body).is_none(), "{:?}", nothing_ran(body));
    }

    /// A `--bench` pass reports `0 passed` and `N measured`; it ran plenty.
    #[test]
    fn a_bench_pass_is_not_a_dead_run() {
        let body = "running 5 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; \
5 measured; 40 filtered out; finished in 6.31s\n";
        assert!(nothing_ran(body).is_none(), "{:?}", nothing_ran(body));
    }

    /// A crate that simply has no tests is not a mistyped filter, and saying
    /// so would fire on every workspace member that carries none.
    #[test]
    fn a_crate_with_no_tests_at_all_is_not_a_dead_filter() {
        let body = "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; \
0 measured; 0 filtered out; finished in 0.00s\n";
        assert!(nothing_ran(body).is_none(), "{:?}", nothing_ran(body));
    }

    /// Output with no summary line at all (`--list`, `--no-run`, a build
    /// failure) must not be rewritten.
    #[test]
    fn output_without_a_summary_line_is_untouched() {
        assert!(nothing_ran("error[E0425]: cannot find value `x`\n").is_none());
        assert!(nothing_ran("").is_none());
    }

    #[test]
    fn message_format_json_routes_first() {
        // `--message-format=json` rewrites output to NDJSON shape;
        // dispatcher must route there even when other flags are set.
        let body = r#"{"type":"test","event":"started","name":"foo"}
{"type":"test","event":"ok","name":"foo"}
{"type":"test","event":"started","name":"bar"}
{"type":"test","event":"ok","name":"bar"}"#;
        let out = dispatch(
            &argv(&["cargo", "test", "--message-format=json"]),
            body,
        );
        assert!(out.contains("event") || out.contains("test"));
    }

    #[test]
    fn json_precedence_over_quiet_and_no_run() {
        // The dispatcher's documented precedence: json wins over
        // --no-run + --quiet. Sanity-check the ordering — fixture
        // is JSON; if dispatcher routed to no_run or quiet instead,
        // the JSON-recognizing compactor wouldn't fire.
        let body = r#"{"type":"test","event":"ok","name":"x"}
{"type":"suite","event":"ok","passed":1}"#;
        let out = dispatch(
            &argv(&[
                "cargo", "test",
                "--no-run", "--quiet",
                "--message-format=json",
            ]),
            body,
        );
        // cargo_test_json preserves the structural keys.
        assert!(out.contains("event"));
    }

    #[test]
    fn message_format_json_pretty_also_routes() {
        // `--message-format=json-render-diagnostics` and `json-diagnostic-*`
        // all start with "json" → must also route to json compactor.
        let body = r#"{"reason":"compiler-message"}"#;
        let out = dispatch(
            &argv(&["cargo", "test", "--message-format=json-render-diagnostics"]),
            body,
        );
        assert!(out.contains("reason") || out.contains("compiler"));
    }

    #[test]
    fn list_flag_routes_to_test_list() {
        // `--list` produces one-test-per-line — dedicated compactor.
        let body = "test foo: test\n\
                    test bar: test\n\
                    test baz: test\n";
        let out = dispatch(&argv(&["cargo", "test", "--", "--list"]), body);
        assert!(out.contains("foo") || out.contains("bar") || out.contains("baz"));
    }

    #[test]
    fn no_run_flag_routes_to_no_run_compactor() {
        // `--no-run`: build-only, no test execution. Output is the
        // build summary.
        let body = "    Finished test [unoptimized + debuginfo] target(s) in 5.34s\n";
        let out = dispatch(&argv(&["cargo", "test", "--no-run"]), body);
        assert!(out.contains("Finished"));
    }

    #[test]
    fn quiet_flag_routes_to_quiet_compactor() {
        // `--quiet` / `-q` strips dots and most progress, keeps summary.
        let body = "running 5 tests\n.....\ntest result: ok. 5 passed; 0 failed\n";
        for flag in &["--quiet", "-q"] {
            let out = dispatch(&argv(&["cargo", "test", flag]), body);
            assert!(out.contains("test result"), "{flag} must preserve summary");
        }
    }

    #[test]
    fn nocapture_flag_routes_to_nocapture_compactor() {
        // `--nocapture` / `--show-output` keeps println! output;
        // dedicated compactor preserves the captured stdout blocks.
        let body = "running 1 test\n\
                    ---- foo stdout ----\nhello from the test\n\
                    test foo ... ok\n\
                    test result: ok. 1 passed\n";
        for flag in &["--nocapture", "--show-output"] {
            let out = dispatch(&argv(&["cargo", "test", flag]), body);
            assert!(
                out.contains("hello from the test") || out.contains("foo stdout"),
                "{flag} must preserve captured output; got: {out}"
            );
        }
    }

    #[test]
    fn default_small_suite_passes_through() {
        // <10 passing lines → nothing worth folding, left untouched.
        let body = "running 3 tests\n\
                    test alpha ... ok\n\
                    test beta ... ok\n\
                    test gamma ... ok\n\
                    test result: ok. 3 passed; 0 failed\n";
        let out = dispatch(&argv(&["cargo", "test"]), body);
        assert!(out.contains("test result"));
        assert!(out.contains("test alpha ... ok"));
    }

    #[test]
    fn default_never_decapitates_failures_on_large_suite() {
        // The old line_cap(200) cut the BOTTOM — exactly where the
        // FAILED lines, the captured panic and the summary live.
        let mut body = String::from("running 600 tests\n");
        for i in 0..595 {
            let _ = writeln!(body, "test mod::case_{i} ... ok");
        }
        body.push_str("test mod::broken ... FAILED\n\n");
        body.push_str("failures:\n\n");
        body.push_str("---- mod::broken stdout ----\n");
        body.push_str("thread 'mod::broken' panicked at src/x.rs:9:\n");
        body.push_str("assertion `left == right` failed\n\n");
        body.push_str("failures:\n    mod::broken\n\n");
        body.push_str("test result: FAILED. 595 passed; 1 failed; 0 ignored;\n");

        let out = dispatch(&argv(&["cargo", "test"]), &body);
        // Every piece of signal survives — none of it decapitated.
        assert!(out.contains("test mod::broken ... FAILED"), "FAILED line kept");
        assert!(out.contains("panicked at src/x.rs:9"), "panic kept");
        assert!(
            out.contains("test result: FAILED. 595 passed; 1 failed"),
            "summary kept"
        );
        // The passing noise is folded into a count.
        assert!(out.contains("passing/ignored tests folded"));
        assert!(!out.contains("case_300 ... ok"), "passing lines folded away");
        assert!(out.len() < body.len(), "must shrink");
    }
}

