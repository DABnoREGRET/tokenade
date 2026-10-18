// The tests of `crate::user_filters`, moved out of the production file.
//
// Mounted by `include!`: the tokens are spliced INTO `user_filters`, so every
// `super::` keeps its meaning. Split by `srcsweep::production_lines`.

#[cfg(test)]
mod tests {
    use super::*;

    fn compile(src: &str) -> CompiledFilter {
        parse_and_validate(src).expect("valid filter")
    }

    /// Both layers under one scratch HOME, with `cwd` moved so the PROJECT
    /// dir is reachable too. `ScratchHome` holds `ENV_LOCK` for its life, so
    /// the cwd move it brackets is serialised with every other env-touching
    /// test in the crate.
    struct Layers {
        home: crate::platform::ScratchHome,
    }
    impl Layers {
        fn new(label: &str) -> Self {
            let mut home = crate::platform::ScratchHome::new(label);
            let proj = home.path().join("repo").join(".tokenade").join("filters");
            std::fs::create_dir_all(&proj).unwrap();
            std::fs::create_dir_all(home.path().join(".tokenade").join("filters")).unwrap();
            // The project layer by its own knob, NOT by moving the process
            // working directory — that move is global and raced every test
            // that opens a relative path. `ScratchHome` holds `ENV_LOCK` for
            // its whole life, and `env()` sets this one under the same guard.
            home.env().set("TOKENADE_FILTER_PROJECT_DIR", &proj);
            Layers { home }
        }
        fn write(&self, project: bool, file: &str, src: &str) {
            let dir = if project {
                self.home
                    .path()
                    .join("repo")
                    .join(".tokenade")
                    .join("filters")
            } else {
                self.home.path().join(".tokenade").join("filters")
            };
            std::fs::write(dir.join(file), src).unwrap();
        }
    }

    const GOOD: &str = "name=\"g\"\nmatch_command=\"^g\"\nstrip_ansi=true\n";
    const BAD_RE: &str = "name=\"b\"\nmatch_command=\"^b[\"\nstrip_ansi=true\n";

    /// A file that does not parse used to vanish without a word: `load_all`
    /// dropped it and no command anywhere named it. It is now reported, WITH
    /// the reason, which is the only actionable half.
    #[test]
    fn an_unparseable_filter_is_named_with_its_reason() {
        let l = Layers::new("uf-invalid");
        l.write(false, "broken.toml", BAD_RE);
        l.write(false, "fine.toml", GOOD);
        let broken = invalid_filters();
        assert_eq!(broken.len(), 1, "got: {broken:?}");
        assert!(broken[0].0.ends_with("broken.toml"), "got: {broken:?}");
        assert!(
            broken[0].1.contains("match_command"),
            "the reason must name the field: {}",
            broken[0].1
        );
        // The valid neighbour is unaffected — a bad file is not contagious.
        let loaded: Vec<_> = survey_filters()
            .into_iter()
            .filter(|f| matches!(f.outcome, FilterOutcome::Loaded(_)))
            .collect();
        assert_eq!(loaded.len(), 1, "got: {loaded:?}");
    }

    /// Validity is asked FIRST, and of BOTH layers. A broken project file
    /// offered for trust makes `trust-filters` print "they now apply to
    /// matching command outputs" about a filter that can never apply.
    #[test]
    fn a_broken_project_filter_is_invalid_and_never_merely_untrusted() {
        let l = Layers::new("uf-order");
        l.write(true, "broken.toml", BAD_RE);
        assert_eq!(invalid_filters().len(), 1, "not reported as invalid");
        assert!(
            untrusted_project_filters().is_empty(),
            "a file that cannot parse was offered for trust: {:?}",
            untrusted_project_filters()
        );
    }

    /// `<cwd>/.tokenade/filters` ships inside a cloned repository, and this
    /// walk runs for every command. An oversized file is refused from its
    /// METADATA — the bytes are never read — and it is reported like any
    /// other unparseable file, so the healthcheck and `trust-filters` name it
    /// without a second code path.
    #[test]
    fn an_oversized_filter_file_is_refused_without_being_read() {
        let l = Layers::new("uf-huge");
        let huge = format!(
            "name=\"h\"\nmatch_command=\"^h\"\nstrip_ansi=true\n# {}\n",
            "A".repeat(MAX_FILTER_FILE_BYTES as usize)
        );
        l.write(false, "huge.toml", &huge);
        l.write(false, "fine.toml", GOOD);
        let broken = invalid_filters();
        assert_eq!(broken.len(), 1, "got: {broken:?}");
        assert!(broken[0].0.ends_with("huge.toml"), "got: {broken:?}");
        assert!(
            broken[0].1.contains("capped at"),
            "the reason must name the ceiling: {}",
            broken[0].1
        );
        // The valid neighbour still loads: an oversized file is not contagious.
        let loaded = survey_filters()
            .into_iter()
            .filter(|f| matches!(f.outcome, FilterOutcome::Loaded(_)))
            .count();
        assert_eq!(loaded, 1);
    }

    /// The ceiling itself, pinned with an ABSOLUTE size. The two guards above
    /// build their fixtures FROM `MAX_FILTER_FILE_BYTES`, so they scale with
    /// it and stay green however far it is raised — the self-referential
    /// fixture once met on the hard cap. 300 KB is over 256 KiB and
    /// under anything an inflated constant would allow.
    #[test]
    fn the_ceiling_is_where_it_says_it_is() {
        assert_eq!(
            MAX_FILTER_FILE_BYTES,
            256 * 1024,
            "the ceiling moved; the guards below are calibrated on 300 KB"
        );
        let l = Layers::new("uf-ceiling");
        l.write(
            false,
            "over.toml",
            &format!(
                "name=\"o\"\nmatch_command=\"^o\"\n# {}\n",
                "A".repeat(300_000)
            ),
        );
        assert_eq!(invalid_filters().len(), 1, "300 KB was admitted");
    }

    /// A file just UNDER the ceiling is read normally — the guard must bound
    /// the hostile case without rejecting a large but legitimate definition.
    #[test]
    fn a_large_but_legal_filter_file_still_loads() {
        let l = Layers::new("uf-large-ok");
        let pad = "A".repeat(MAX_FILTER_FILE_BYTES as usize / 2);
        l.write(
            false,
            "big.toml",
            &format!("name=\"b\"\nmatch_command=\"^b\"\nstrip_ansi=true\n# {pad}\n"),
        );
        assert!(invalid_filters().is_empty(), "a legal file was refused");
        assert_eq!(
            survey_filters()
                .into_iter()
                .filter(|f| matches!(f.outcome, FilterOutcome::Loaded(_)))
                .count(),
            1
        );
    }

    /// A valid project filter that nobody trusted is `Untrusted`, not
    /// `Invalid`: declining to trust one is a legitimate steady state and
    /// must not turn the healthcheck red.
    #[test]
    fn an_untrusted_project_filter_is_not_reported_as_broken() {
        let l = Layers::new("uf-untrusted");
        l.write(true, "ok.toml", GOOD);
        assert!(
            invalid_filters().is_empty(),
            "a parseable file called broken"
        );
        assert_eq!(untrusted_project_filters().len(), 1);
    }

    /// The name collision is a distinct outcome from a parse failure. Both
    /// used to leave through the same silent `_ => {}`.
    #[test]
    fn a_second_file_claiming_a_name_is_shadowed_not_broken() {
        let l = Layers::new("uf-shadow");
        l.write(false, "a.toml", GOOD);
        l.write(false, "b.toml", GOOD);
        let s = survey_filters();
        let loaded = s
            .iter()
            .filter(|f| matches!(f.outcome, FilterOutcome::Loaded(_)))
            .count();
        let shadowed = s
            .iter()
            .filter(|f| matches!(f.outcome, FilterOutcome::Shadowed { .. }))
            .count();
        assert_eq!((loaded, shadowed), (1, 1), "got: {s:?}");
        assert!(
            invalid_filters().is_empty(),
            "a shadowed file is not broken"
        );
    }

    #[test]
    fn head_tail_keeps_outcome_line() {
        let f = compile("name=\"x\"\nmatch_command=\"^x\"\nhead_lines=2\ntail_lines=1\n");
        let out = f.apply("a\nb\nc\nd\ne\nLAST");
        assert!(out.starts_with("a\nb\n"), "head kept: {out:?}");
        assert!(out.trim_end().ends_with("LAST"), "tail kept: {out:?}");
        assert!(out.contains("omitted"), "omission marker: {out:?}");
    }

    #[test]
    fn strip_lines_and_on_empty() {
        let f = compile(
            "name=\"x\"\nmatch_command=\"^x\"\nstrip_lines_matching=[\"^DEBUG\"]\non_empty=\"ok\"\n",
        );
        assert_eq!(f.apply("DEBUG a\nDEBUG b"), "ok");
        assert_eq!(f.apply("DEBUG a\nkeep"), "keep");
    }

    #[test]
    fn phases_state_machine_keeps_failure_section() {
        let f = compile(
            "name=\"cargo\"\nmatch_command=\"^cargo test\"\n\
             [[phases]]\nkeep=\"warning:\"\nuntil=\"Running\"\n\
             [[phases]]\nkeep=\"FAILED|test result\"\n",
        );
        let raw = "Compiling foo\nCompiling bar\nwarning: unused x\nCompiling baz\n\
                   Running tests\nrunning 3 tests\ntest a ... ok\ntest b ... FAILED\n\
                   test result: FAILED. 2 passed; 1 failed";
        let out = f.apply(raw);
        assert!(
            out.contains("warning: unused x"),
            "phase0 warning kept: {out:?}"
        );
        assert!(
            !out.contains("Compiling foo"),
            "phase0 compile spew dropped: {out:?}"
        );
        assert!(
            out.contains("test b ... FAILED"),
            "phase1 failure kept: {out:?}"
        );
        assert!(
            out.contains("test result: FAILED"),
            "phase1 summary kept: {out:?}"
        );
        assert!(
            !out.contains("test a ... ok"),
            "phase1 passing test dropped: {out:?}"
        );
    }

    #[test]
    fn match_output_collapses_whole_output() {
        let f = compile(
            "name=\"b\"\nmatch_command=\"^cargo build\"\nmatch_output=[{pattern=\"Finished\",summary=\"cargo: ok\"}]\n",
        );
        assert_eq!(f.apply("Compiling foo\nFinished in 2s"), "cargo: ok");
    }

    #[test]
    fn replace_and_strip_ansi() {
        let f = compile(
            "name=\"x\"\nmatch_command=\"^x\"\nstrip_ansi=true\nreplace=[{from=\"foo\",to=\"F\"}]\n",
        );
        assert_eq!(f.apply("\x1b[31mfoo\x1b[0m bar"), "F bar");
    }

    #[test]
    fn embedded_tests_run_at_validation() {
        // A filter whose self-test passes compiles…
        assert!(parse_and_validate(
            "name=\"x\"\nmatch_command=\"^x\"\nstrip_lines_matching=[\"^DEBUG\"]\n\
             [[tests]]\ninput=\"DEBUG a\\nkeep\"\nexpect_contains=\"keep\"\n"
        )
        .is_ok());
        // …and one whose self-test fails is rejected with the reason.
        let err = parse_and_validate(
            "name=\"x\"\nmatch_command=\"^x\"\nstrip_lines_matching=[\"^DEBUG\"]\n\
             [[tests]]\ninput=\"DEBUG a\"\nexpect_contains=\"keep\"\n",
        )
        .unwrap_err();
        assert!(err.contains("test #0 failed"), "got: {err}");
    }

    #[test]
    fn invalid_regex_and_noop_rejected() {
        assert!(parse_and_validate("name=\"x\"\nmatch_command=\"(\"\n").is_err());
        assert!(parse_and_validate("name=\"x\"\nmatch_command=\"^x\"\n")
            .unwrap_err()
            .contains("no operations"));
    }

    #[test]
    fn match_command_selects() {
        let f = compile("name=\"x\"\nmatch_command=\"^mytool\\\\s+status\"\nmax_lines=1\n");
        assert!(f.match_re.is_match("mytool status --all"));
        assert!(!f.match_re.is_match("othertool status"));
    }

    // ── Feature 1: longest-match-span selection ──
    #[test]
    fn expand_command_templates_roundtrip_and_unknown() {
        // No template → byte-identical (retro-compat).
        assert_eq!(
            expand_command_templates("^npm run build").unwrap(),
            "^npm run build"
        );
        // Known template expands to its fragment.
        let out = expand_command_templates("^{{runner}} .*build").unwrap();
        assert!(out.contains("npx|bunx"), "expanded: {out}");
        assert!(!out.contains("{{"), "no residual token: {out}");
        // Unknown → clear error, not a silent no-op.
        let e = expand_command_templates("^{{bogus}}").unwrap_err();
        assert!(
            e.contains("unknown template") && e.contains("bogus"),
            "got: {e}"
        );
    }

    #[test]
    fn template_filter_matches_all_runners() {
        let f = compile(
            "name=\"js-build\"\nmatch_command=\"^{{runner}} .*\\\\bbuild\\\\b\"\nmax_lines=1\n",
        );
        assert!(f.match_re.is_match("npx vite build --mode prod"));
        assert!(f.match_re.is_match("pnpm dlx turbo build"));
        assert!(!f.match_re.is_match("npm run build")); // npm is not a {{runner}}
    }

    #[test]
    fn longest_match_span_wins_over_broad() {
        // Simulate two loaded filters: broad `^git` vs specific `^git status`.
        let broad = compile("name=\"git\"\nmatch_command=\"^git\\\\b\"\nmax_lines=1\n");
        let specific =
            compile("name=\"git-status\"\nmatch_command=\"^git status\\\\b\"\nmax_lines=1\n");
        // Reproduce match_for_command's selection over an explicit slice.
        fn pick<'a>(fs: &[&'a CompiledFilter], cmd: &str) -> Option<&'a CompiledFilter> {
            let mut best: Option<(&CompiledFilter, usize)> = None;
            for f in fs {
                let Some(m) = f.match_re.find(cmd) else {
                    continue;
                };
                let span = m.end() - m.start();
                match best {
                    None => best = Some((f, span)),
                    Some((c, cs)) => {
                        if span > cs || (span == cs && f.priority > c.priority) {
                            best = Some((f, span));
                        }
                    }
                }
            }
            best.map(|(f, _)| f)
        }
        // Broad loaded FIRST, yet the specific (longer span) wins.
        let order = [&broad, &specific];
        assert_eq!(pick(&order, "git status -s").unwrap().name, "git-status");
        // A command only the broad one matches still routes to broad.
        assert_eq!(pick(&order, "git log").unwrap().name, "git");
    }

    #[test]
    fn priority_breaks_span_ties() {
        let lo = compile("name=\"lo\"\nmatch_command=\"^tool\\\\b\"\npriority=1\nmax_lines=1\n");
        let hi = compile("name=\"hi\"\nmatch_command=\"^tool\\\\b\"\npriority=5\nmax_lines=1\n");
        assert_eq!(lo.priority, 1);
        assert!(
            hi.priority > lo.priority,
            "equal span → higher priority wins"
        );
    }

    // ── Feature 3: scaffold-from-transcript ──
    #[test]
    fn draft_from_sample_is_valid_and_agnostic() {
        // 60 noisy lines with blanks → strip blanks + cap lines.
        let mut sample = String::new();
        for i in 0..60 {
            sample.push_str(&format!("line {i}\n\n"));
        }
        let toml = draft_filter_toml("mytool build --release", &sample).unwrap();
        assert!(toml.contains("draft = true"), "carries draft marker");
        assert!(
            toml.contains("match_command = \"^mytool build\\\\b\""),
            "cmd-derived match: {toml}"
        );
        assert!(
            toml.contains("strip_lines_matching = [\"^\\\\s*$\"]"),
            "blank-line strip: {toml}"
        );
        assert!(toml.contains("max_lines = 40"), "line cap: {toml}");
        // Content-agnostic: no sample line body leaks into the SPEC (only the
        // test's input/expect_equals may contain it).
        let spec_region = toml.split("[[tests]]").next().unwrap();
        assert!(
            !spec_region.contains("line 7"),
            "spec must not key on sample bytes"
        );
        // The generated draft parses (self-test seeded from the sample passes).
        assert!(
            parse_and_validate(&toml).is_ok(),
            "generated draft is valid TOML+filter"
        );
    }

    #[test]
    fn draft_complete_json_gets_no_line_transform() {
        let sample = "{\n  \"a\": 1,\n  \"b\": [1,2,3]\n}\n";
        let toml = draft_filter_toml("mytool dump", sample).unwrap();
        assert!(
            !toml.contains("max_lines"),
            "JSON must not get a line cap: {toml}"
        );
        assert!(
            !toml.contains("strip_lines_matching"),
            "JSON keeps all lines: {toml}"
        );
    }

    #[test]
    fn draft_roundtrips_to_installable_when_marker_removed() {
        let sample = "a\n\nb\n";
        let toml = draft_filter_toml("mytool run", sample).unwrap();
        // Author removes the marker → filter compiles and is no longer a draft.
        let installable = toml.replace("draft = true\n", "");
        let f = parse_and_validate(&installable).expect("installs after review");
        assert!(!f.draft);
    }

    /// The draft goes to STDOUT, so an unbounded fixture is unbounded agent
    /// context, and the header asks the author to review a file nobody reviews
    /// at three megabytes.
    ///
    /// MEASURED 2026-08-04: a 3,16 MB sample drafted a 3,22 MB TOML.
    #[test]
    fn a_huge_sample_does_not_draft_a_huge_fixture() {
        let sample: String = (0..60_000)
            .map(|i| format!("2026-08-04 INFO worker={i} status=ok\n"))
            .collect();
        let out = draft_filter_toml("deploy prod", &sample).unwrap();
        assert!(
            out.len() < 40_000,
            "the fixture is still unbounded: {} bytes from a {} byte sample",
            out.len(),
            sample.len()
        );
        assert!(
            out.contains("trimmed to its first 200 of 60000 line(s)"),
            "the trim is silent — the author cannot tell the fixture is partial: {}",
            out.lines().take(8).collect::<Vec<_>>().join("\n")
        );
        // Self-consistency is the reason the sample is cut BEFORE anything is
        // derived from it: a draft whose expected output was computed from
        // bytes the test no longer feeds in would fail the moment it is run.
        let input = out
            .lines()
            .find(|l| l.starts_with("input = "))
            .expect("no input line");
        assert!(
            input.matches("worker=199").count() == 1 && !input.contains("worker=200"),
            "input does not stop at the documented line count"
        );
    }

    /// A sample small enough to embed whole must not gain the note — it would
    /// claim a truncation that never happened.
    #[test]
    fn a_small_sample_is_embedded_whole_and_says_nothing() {
        let out = draft_filter_toml("deploy prod", "one\ntwo\nthree\n").unwrap();
        assert!(
            !out.contains("trimmed to its first"),
            "claimed a trim that never ran: {out}"
        );
        assert!(
            out.contains("three"),
            "lost the tail of a small sample: {out}"
        );
    }
}
#[cfg(test)]
mod one_meaning_for_an_anchor {
    use super::*;

    fn filter_with(body: &str) -> CompiledFilter {
        let toml = format!("name = \"probe\"\nmatch_command = \"^probe\"\n{body}");
        let f: UserFilter = toml::from_str(&toml).expect("parse");
        f.compile().expect("compile")
    }

    /// `^` means the start of a LINE in every list of the format, because
    /// `strip_lines_matching` has always been applied line by line and a reader
    /// takes the other two the same way. MEASURED 2026-09-04: a filter that
    /// installs and validates cleanly did NOTHING — `match_output.pattern =
    /// "^ALL SUITES PASSED"` never fired on output whose last line is exactly
    /// that, and an anchored `replace.from` rewrote nothing.
    #[test]
    fn an_anchor_means_a_line_in_every_list() {
        let sample = "alpha\nPASS (17ms) case_a\nomega\n";

        let f =
            filter_with("replace = [ { from = \"^PASS \\\\((\\\\d+)ms\\\\) \", to = \"ok \" } ]\n");
        assert!(
            f.apply(sample).contains("ok case_a"),
            "an anchored replace must rewrite the line it names"
        );

        let f =
            filter_with("match_output = [ { pattern = \"^omega\", summary = \"probe: done\" } ]\n");
        assert_eq!(f.apply(sample).trim(), "probe: done");

        // And the list that already worked keeps working.
        let f = filter_with("strip_lines_matching = [ \"^omega\" ]\n");
        assert!(!f.apply(sample).contains("omega"));
    }

    /// An anchor is not a wildcard: `^omega` must not match `not omega`.
    #[test]
    fn an_anchor_still_anchors() {
        let sample = "not omega here\nalpha\n";
        let f =
            filter_with("match_output = [ { pattern = \"^omega\", summary = \"probe: done\" } ]\n");
        assert_eq!(
            f.apply(sample),
            sample,
            "a mid-line match must not fire a whole-output summary"
        );
    }
}
#[cfg(test)]
mod a_typo_is_a_refusal {
    use super::*;

    fn parse(body: &str) -> Result<CompiledFilter, String> {
        let toml = format!("name = \"probe\"\nmatch_command = \"^probe\"\n{body}");
        let f: UserFilter = toml::from_str(&toml).map_err(|e| e.to_string())?;
        f.compile()
    }

    /// The format's whole selling point is that a filter is validated before
    /// it is installed. A typo went through all of it. MEASURED 2026-09-04:
    /// `strip_line_matching` (singular) installed cleanly and folded nothing,
    /// and `expect` instead of `expect_contains` installed cleanly with the
    /// author's self-test never run.
    #[test]
    fn a_misspelled_field_is_named_instead_of_ignored() {
        let e = parse("strip_line_matching = [ \"^DEBUG\" ]\n").unwrap_err();
        assert!(
            e.contains("unknown field") && e.contains("strip_line_matching"),
            "the refusal must NAME the field: {e}"
        );

        // The same inside a `[[tests]]` table, which is its own struct — and
        // the error has to be the UNKNOWN FIELD, not the "asserts nothing"
        // check downstream, which would fire on the same file for a different
        // reason and hide that the typo was accepted.
        let e = parse(
            "strip_lines_matching = [ \"^DEBUG\" ]\n\n[[tests]]\ninput = \"DEBUG a\\nkeep\\n\"\nexpect = \"keep\"\n",
        )
        .unwrap_err();
        assert!(
            e.contains("unknown field") && e.contains("`expect`"),
            "the refusal must NAME the field: {e}"
        );
    }

    /// And a case that checks nothing is not a test: the install is gated on
    /// these, so an empty one is a gate the author believes they wrote.
    #[test]
    fn a_test_case_with_no_assertion_is_refused() {
        let e = parse(
            "strip_lines_matching = [ \"^DEBUG\" ]\n\n[[tests]]\ninput = \"DEBUG a\\nkeep\\n\"\n",
        )
        .unwrap_err();
        assert!(e.contains("asserts nothing"), "{e}");
    }

    /// The other direction, or the rule would be "refuse everything": a
    /// correct filter with a real self-test still compiles, and a failing one
    /// still fails for its own reason.
    #[test]
    fn a_correct_filter_still_installs_and_a_failing_test_still_fails() {
        let good = "strip_lines_matching = [ \"^DEBUG\" ]\n\n[[tests]]\ninput = \"DEBUG a\\nkeep\\n\"\nexpect_contains = \"keep\"\n";
        assert!(parse(good).is_ok());
        let bad = "strip_lines_matching = [ \"^DEBUG\" ]\n\n[[tests]]\ninput = \"DEBUG a\\nkeep\\n\"\nexpect_contains = \"never there\"\n";
        let e = parse(bad).unwrap_err();
        assert!(e.contains("does not contain"), "{e}");
    }
}
