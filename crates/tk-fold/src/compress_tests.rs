// The tests of `compress`.
//
// Spliced in by `include!` from `compress.rs`: the same module, hence the same
// private access and the same `super::` paths, without a single visibility line
// changed. This is the shape this repo already uses for 33 other modules.
//
// A `//!` is impossible here: a file spliced in by `include!` is not a
// module, and an INNER doc comment has nothing to qualify.

#[cfg(test)]
mod verdict_tests {
    use super::*;

    /// The verdict a runner prints decides, not the words in the test's name.
    ///
    /// MEASURED 2026-09-04 on a real `cargo nextest run` of 8 884 tests with
    /// ZERO failures: the fold announced "10 kept below because they look like
    /// failures" and listed ten PASSING tests, every one of them named for what
    /// it defends (`a_pipeline_that_hid_a_failure_says_so`). Inventing failures
    /// is the mirror of losing them.
    #[test]
    fn a_passing_verdict_beats_the_words_in_the_test_name() {
        for l in [
            "        PASS [   0.006s] (   1/8884) tokenade-cli::e2e misc::a_pipeline_that_hid_a_failure_says_so",
            "        PASS [   0.021s] ( 163/8884) tokenade-cli::e2e web::read_prompt_without_a_value_is_a_clean_error",
            "test tests::an_error_line_survives_the_fold ... ok",
            "--- PASS: TestErrorHandling (0.00s)",
            "tests/x.py::test_timeout_is_reported PASSED   [ 20%]",
            "\u{2713} handles a failed request (3 ms)",
        ] {
            assert!(!is_error_line(l), "a passing test read as a failure: {l}");
        }
    }

    /// The other half, and the half that must never be traded for the first:
    /// a line whose verdict says it FAILED is an error line even when nothing
    /// on it matches an ordinary keyword. `this_one_really_fails` contains no
    /// keyword — `fail` alone is too broad to be one (`failover`, `failsafe`).
    #[test]
    fn a_failing_verdict_is_an_error_line_on_its_own() {
        for l in [
            "        FAIL [   0.004s] (4/5) ntfix tests::this_one_really_fails",
            "        SIGSEGV [   0.1s] (9/9) ntfix tests::maps_a_region",
            "        TIMEOUT [  60.0s] (9/9) ntfix tests::waits_for_the_lock",
            "test tests::plain_arithmetic ... FAILED",
            "--- FAIL: TestOk (0.00s)",
            "tests/x.py::test_ok FAILED",
        ] {
            assert!(is_error_line(l), "a real failure was not read as one: {l}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn squeeze_collapses_blanks() {
        assert_eq!(squeeze("a\n\n\n\nb"), "a\n\nb");
    }
    #[test]
    fn is_error_line_matches_keywords_case_insensitively() {
        assert!(is_error_line("ERROR: build failed"));
        assert!(is_error_line("thread 'main' panicked"));
        assert!(is_error_line("  WARN: deprecated API"));
        assert!(is_error_line("Segmentation fault (SIGSEGV)"));
        assert!(is_error_line("process timed out after 30s"));
        // Plain informational lines must not match.
        assert!(!is_error_line("Compiling tokenade-core v0.3.4"));
        assert!(!is_error_line("   Finished in 2.1s"));
        assert!(!is_error_line(""));
    }
    /// The last-resort fold gives what fits whole: `pip freeze` lost 52 of
    /// 371 packages to the 180-line sampling.
    #[test]
    fn the_generic_fold_gives_what_fits_whole() {
        let s: String = (0..371)
            .map(|i| format!("package-{i}==1.{i}.0\n"))
            .collect();
        {
            let _g = tk_base::delivery::threshold_on_this_thread(100_000);
            assert!(!generic(&s).contains("elided by tokenade"));
        }
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        assert!(
            generic(&s).contains("elided by tokenade"),
            "past the limit it still folds"
        );
    }

    /// A listing cut in the middle is cut ONCE: commit subjects that say
    /// `error` are entries, not failures to rescue (`git reflog -80`, 2026-10-04).
    #[test]
    fn a_listing_is_cut_without_rescuing_its_entries() {
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        let s: String = (0..200)
            .map(|i| {
                if i % 9 == 0 {
                    format!("abc{i:04} HEAD@{{{i}}}: commit: fix error in parser {i}\n")
                } else {
                    format!("abc{i:04} HEAD@{{{i}}}: commit: step {i}\n")
                }
            })
            .collect();
        let out = whole_or_truncate_middle(&s, 40, 5);
        assert_eq!(out.matches("elided by tokenade").count(), 1, "{out}");
        assert!(!out.contains("parser 99"), "{out}");
        assert!(
            truncate_middle(&s, 40, 5).contains("parser 99"),
            "a log cut still rescues its failures"
        );
    }

    /// `cargo tree` cut in the middle rescued `thiserror` and `quick-error` as
    /// its errors: a lower-case keyword inside a name is the name.
    #[test]
    fn a_keyword_inside_a_name_is_not_an_error() {
        for l in [
            "4thiserror v2.0.18 (*)",
            "│ │ ├── quick-error v2.0.1",
            "├── error-ex@1.3.4",
            "connect_timeout = 30",
            "├── abort-controller@3.0.0",
        ] {
            assert!(!is_error_line(l), "a name read as a failure: {l}");
        }
        for l in [
            "TypeError: x is not a function",
            "java.lang.NullPointerException",
            "ERROR_FILE_NOT_FOUND",
            "error: could not compile `thiserror`",
            "npm ERR! code ETARGET",
            "{\"level\":\"error\",\"msg\":\"disk_full\"}",
        ] {
            assert!(is_error_line(l), "a real failure was not read as one: {l}");
        }
    }
    #[test]
    fn smart_fold_short_input_is_identity() {
        let lines = ["a", "b", "c"];
        assert_eq!(smart_fold_indices(&lines, 10, 2, 2), vec![0, 1, 2]);
    }
    #[test]
    fn smart_fold_is_strict_budget() {
        // 100 lines, max 10 — result must never exceed the budget even when
        // many lines look like errors.
        let lines: Vec<String> = (0..100)
            .map(|i| {
                if i % 2 == 0 {
                    format!("error {i}")
                } else {
                    format!("ok {i}")
                }
            })
            .collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let idx = smart_fold_indices(&refs, 10, 3, 2);
        assert!(idx.len() <= 10, "budget exceeded: {}", idx.len());
        // Sorted + de-duplicated.
        assert!(idx.windows(2).all(|w| w[0] < w[1]));
    }
    #[test]
    fn smart_fold_keeps_head_tail_and_rescues_errors() {
        let mut lines: Vec<String> = (0..50).map(|i| format!("info line {i}")).collect();
        lines[25] = "fatal: disk corruption detected".to_string();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let idx = smart_fold_indices(&refs, 12, 3, 2);
        // Head guaranteed.
        assert!(idx.contains(&0) && idx.contains(&1) && idx.contains(&2));
        // Tail guaranteed.
        assert!(idx.contains(&48) && idx.contains(&49));
        // The buried error survives.
        assert!(idx.contains(&25), "error line was dropped: {idx:?}");
    }
    #[test]
    fn coverage_knee_saturates_on_repetitive_input() {
        // First 5 lines are distinct, the rest are identical → coverage
        // flattens early, so the knee should land well before the end.
        let mut lines: Vec<&str> = vec![
            "alpha beta gamma",
            "delta epsilon zeta",
            "eta theta iota",
            "kappa lambda mu",
            "nu xi omicron",
        ];
        lines.extend(std::iter::repeat_n("same same same", 45));
        let knee = coverage_knee(&lines, 50);
        assert!(knee < 20, "expected early knee, got {knee}");
        assert!(knee >= 1);
    }
    #[test]
    fn coverage_knee_keeps_all_on_diverse_input() {
        // Every line is unique → ~linear coverage → no real knee → keep max.
        let lines: Vec<String> = (0..40)
            .map(|i| format!("unique token {i} alpha{i} beta{i} gamma{i}"))
            .collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let knee = coverage_knee(&refs, 40);
        assert_eq!(knee, 40, "diverse input must not be over-truncated");
    }
    #[test]
    fn coverage_knee_respects_max_clamp() {
        let lines: Vec<String> = (0..100)
            .map(|i| format!("unique {i} a{i} b{i} c{i}"))
            .collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        assert!(coverage_knee(&refs, 20) <= 20);
    }
}

#[cfg(test)]
mod truncate_middle_tests {
    use super::truncate_middle;

    #[test]
    fn truncate_middle_short_input_is_identity() {
        let s = "one\ntwo\nthree";
        assert_eq!(truncate_middle(s, 5, 5), s);
    }

    #[test]
    fn truncate_middle_keeping_holds_the_extra_lines_in_place() {
        let s: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let out = crate::compress::truncate_middle_keeping(&s, 10, 5, &[40, 41, 70, 500]);
        let kept: Vec<&str> = out.lines().filter(|l| l.starts_with("line ")).collect();
        assert!(
            kept.contains(&"line 40") && kept.contains(&"line 41") && kept.contains(&"line 70"),
            "{out}"
        );
        let at = |n: &str| out.find(n).unwrap();
        assert!(
            at("line 0\n") < at("line 40") && at("line 70") < at("line 95"),
            "{out}"
        );
        assert_eq!(
            crate::compress::truncate_middle_keeping(&s, 10, 5, &[]),
            truncate_middle(&s, 10, 5)
        );
    }

    #[test]
    fn truncate_middle_keeps_head_and_tail() {
        let lines: Vec<String> = (0..200)
            .map(|i| format!("distinct line {i} alpha{i} beta{i}"))
            .collect();
        let input = lines.join("\n");
        let out = truncate_middle(&input, 20, 10);
        // First and last lines survive.
        assert!(out.contains("distinct line 0 "));
        assert!(out.contains("distinct line 199 "));
        // Middle is elided.
        assert!(out.contains("elided by tokenade"));
        assert!(!out.contains("distinct line 100 "));
    }

    #[test]
    fn truncate_middle_rescues_error_from_middle() {
        let mut lines: Vec<String> = (0..200)
            .map(|i| format!("distinct line {i} a{i} b{i}"))
            .collect();
        lines[120] = "panic: index out of bounds".to_string();
        let input = lines.join("\n");
        let out = truncate_middle(&input, 20, 10);
        assert!(
            out.contains("panic: index out of bounds"),
            "error buried in the elided middle must be rescued"
        );
    }

    #[test]
    fn truncate_middle_adaptive_head_shrinks_on_repetitive_input() {
        // Two distinct head lines, then a long run of identical lines, then a
        // distinct tail. The adaptive head should not echo the whole head
        // window of identical lines.
        let mut lines: Vec<String> = vec![
            "header alpha beta gamma".to_string(),
            "header delta epsilon zeta".to_string(),
        ];
        for _ in 0..300 {
            lines.push("repeated noise noise noise".to_string());
        }
        lines.push("tail unique marker line".to_string());
        let input = lines.join("\n");
        let out = truncate_middle(&input, 80, 5);
        // Adaptive head means we keep far fewer than 80 head lines.
        let repeated = out.matches("repeated noise noise noise").count();
        assert!(
            repeated < 40,
            "adaptive head failed to shrink: {repeated} repeated lines kept"
        );
        assert!(out.contains("tail unique marker line"));
    }
}
