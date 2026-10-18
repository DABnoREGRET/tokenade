// The tests of `crate::stacktrace`, moved out of the production file.
//
// Mounted by `include!`: the tokens are spliced INTO `stacktrace`, so every
// `super::` keeps its meaning. Split by `srcsweep::production_lines`.

#[cfg(test)]
mod python_repeat_count {
    /// Python's own frame-repetition count must survive the fold.
    ///
    /// CPython prints `[Previous line repeated N more times]` instead of
    /// repeating a frame, and it is the only line stating the RECURSION DEPTH.
    /// It is indented, so it matched none of the classifier's branches — not a
    /// frame, not the `Traceback` header, not the unindented exception line —
    /// and was dropped in silence. Measured 2026-09-06 on the published
    /// binary: a 40-deep recursion came back showing three frames, never
    /// mentioning 37, and for a runaway recursion the depth IS the diagnosis.
    #[test]
    fn a_folded_recursion_still_says_how_deep_it_went() {
        let mut t = String::from("Traceback (most recent call last):\n");
        t.push_str("  File \"deep.py\", line 5, in <module>\n    level(40)\n");
        for _ in 0..3 {
            t.push_str("  File \"deep.py\", line 4, in level\n    return level(n - 1)\n");
        }
        t.push_str("  [Previous line repeated 37 more times]\n");
        t.push_str("  File \"deep.py\", line 3, in level\n    return d['missing']\n");
        t.push_str("KeyError: 'missing'\n");

        let out = super::compact(&t);
        assert!(
            out.contains("[Previous line repeated 37 more times]"),
            "the interpreter's own count must survive: {out}"
        );
        // The NUMBER, not a recomputed one: once the frames are folded we
        // cannot derive 37 from what is left.
        assert!(out.contains("37"), "and it must still say 37: {out}");
        // The fold still did its job around it.
        assert!(out.contains("KeyError: 'missing'"), "exception kept: {out}");
        assert!(
            out.len() < t.len(),
            "still a fold: {} vs {}",
            out.len(),
            t.len()
        );

        // Anti-vacuity: a traceback with no repetition gains no such line. A
        // fix that emitted one unconditionally would pass every arm above.
        let plain = "Traceback (most recent call last):\n  File \"a.py\", line 2, in outer\n    inner()\n  File \"a.py\", line 5, in inner\n    raise ValueError(\"nope\")\nValueError: nope\n";
        let out = super::compact(plain);
        assert!(
            !out.contains("Previous line repeated"),
            "nothing repeated, so nothing may be claimed: {out}"
        );
        assert!(out.contains("ValueError: nope"), "exception kept: {out}");
    }

    /// The predicate must match THAT line and not merely "an indented line".
    ///
    /// Python 3.11 emits caret markers under a failing expression, and
    /// exception notes are indented too. A predicate that took any indented
    /// line as the repetition note would turn those into frames.
    #[test]
    fn only_the_repetition_line_is_taken_for_one() {
        let t = "Traceback (most recent call last):\n                   File \"a.py\", line 2, in outer\n    total = a / b\n                     ~~^~~\n  File \"a.py\", line 5, in inner\n    raise ValueError(\"nope\")\n                 ZeroDivisionError: division by zero\n";
        let out = super::compact(t);
        assert!(
            !out.contains("Previous line repeated"),
            "a caret marker is not a repetition count: {out}"
        );

        // A marker without its closing bracket is not the complete shape.
        assert_eq!(
            super::python_repeat_note("  [Previous line repeated 37 more times"),
            None
        );
        assert_eq!(
            super::python_repeat_note("  [Previous line repeated 37 more times]"),
            Some("[Previous line repeated 37 more times]".to_string())
        );
        assert_eq!(super::python_repeat_note("    ~~^~~"), None);
        assert_eq!(super::python_repeat_note("  note: see the docs"), None);
    }

    /// It must survive a traceback long enough for the fold to ELIDE frames.
    ///
    /// The count is only interesting on a deep trace, which is exactly where
    /// frame folding is most aggressive — so filing it as a foldable frame
    /// would drop it in the one case it exists for.
    #[test]
    fn the_count_survives_a_traceback_deep_enough_to_be_folded() {
        let mut t = String::from("Traceback (most recent call last):\n");
        for i in 0..40 {
            t.push_str(&format!(
                "  File \"/usr/lib/python3/site-packages/framework/mod{i}.py\", line {}, in dispatch\n    return handler()\n",
                10 + i
            ));
        }
        t.push_str("  [Previous line repeated 118 more times]\n");
        t.push_str("  File \"app.py\", line 3, in handler\n    return d['missing']\n");
        t.push_str("KeyError: 'missing'\n");

        let out = super::compact(&t);
        assert!(
            out.len() < t.len() / 2,
            "the fold must actually bite here: {} vs {}",
            out.len(),
            t.len()
        );
        assert!(
            out.contains("[Previous line repeated 118 more times]"),
            "and the count must survive the folding it describes: {out}"
        );

        // And with framework frames on BOTH sides — recursion inside a
        // framework stack. Measured: filing the note as a foldable framework
        // frame loses it HERE and nowhere else, so this position is the whole
        // difference between the two ways of filing it.
        let mut t = String::from("Traceback (most recent call last):\n");
        for i in 0..40 {
            t.push_str(&format!(
                "  File \"/usr/lib/python3/site-packages/fw/m{i}.py\", line {}, in dispatch\n    return handler()\n",
                10 + i
            ));
        }
        t.push_str("  [Previous line repeated 118 more times]\n");
        for i in 0..40 {
            t.push_str(&format!(
                "  File \"/usr/lib/python3/site-packages/fw/n{i}.py\", line {}, in dispatch\n    return handler()\n",
                60 + i
            ));
        }
        t.push_str(
            "  File \"app.py\", line 3, in handler\n    return d['missing']\nKeyError: 'missing'\n",
        );
        let out = super::compact(&t);
        assert!(
            out.contains("[Previous line repeated 118 more times]"),
            "the count must survive framework frames on both sides: {out}"
        );
    }
}
#[cfg(test)]
mod tests {

    /// An address written in UPPERCASE is an address.
    ///
    /// Two traces identical up to the address must share a key. The
    /// spelling that used to live here was `0x[0-9a-fA-F]+`: it missed
    /// `0XDEADBEEF`, which Java and C both write, so two otherwise twin
    /// traces stayed two traces.
    /// (It also bit into the end of an identifier, `foo0x1f`: the word
    /// boundary fixes that, but `canonicalise` ends with a pass that masks
    /// any remaining digits, so that case cannot be observed here — it is
    /// observed in the template fold, which has no such pass.)
    #[test]
    fn an_uppercase_address_is_still_an_address() {
        let one = "\tat Foo.bar(Native Method) [0XDEADBEEF]";
        let two = "\tat Foo.bar(Native Method) [0Xcafebabe]";
        assert_eq!(
            super::canonicalise(one),
            super::canonicalise(two),
            "two traces identical up to the address do not share a key"
        );
    }

    /// What follows the last frame is the OUTCOME, and it must survive.
    ///
    /// Fixture is the REAL output of `node -e 'require("nosuch_xyz")'`,
    /// byte for byte with the working directory replaced. A hand-written
    /// approximation of it did NOT reproduce the defect: the first version
    /// of this test passed with the fix removed, while the same mutation
    /// on the real bytes dropped `code: 'MODULE_NOT_FOUND'` exactly as
    /// reported. Measured on the real command: 262 tokens to 191, and what
    /// left was the half an agent greps for.
    ///
    /// The two halves oppose: the frames must still FOLD (or this passes
    /// by not compacting) and the tail must still be THERE (or it passes
    /// by folding everything away).
    #[test]
    fn a_js_trace_keeps_what_follows_its_frames() {
        let trace = NODE_MODULE_NOT_FOUND;
        let out = compact_js(trace);

        for kept in [
            "Error: Cannot find module 'nosuch_xyz'",
            "code: 'MODULE_NOT_FOUND'",
            "requireStack:",
            "Node.js v22.21.1",
        ] {
            assert!(out.contains(kept), "the trace lost `{kept}`:\n{out}");
        }
        // This fixture carries ten frames, which is under the fold's own
        // bar, so its saving comes from trimming rather than folding —
        // asserting a fold here would assert something the fixture cannot
        // show. What it CAN show is that the compactor ran at all.
        assert!(
            out.len() < trace.len(),
            "the compactor returned its input, so this proves nothing"
        );
        // A tail is appended, and appending must not leave the output
        // ending in whitespace — the caller measures and compares this.
        assert_eq!(out, out.trim_end(), "the tail was appended untrimmed");

        // The fold itself gets its own fixture, with enough frames to
        // reach the bar, so "the tail survived" is never satisfied by a
        // compactor that stopped compacting.
        // The frame paths must be ones the JS frame regex actually
        // matches: its path group excludes `:`, so `node:internal/...`
        // frames never reach `fold` at all — the first version of this
        // fixture used them and was measuring `generic_compact` instead.
        let deep: String = "Error: boom\n".to_string()
            + "    at handler (/app/src/handler.js:12:9)\n"
            + &(0..40)
                .map(|i| {
                    format!(
                        "    at layer{i} (/app/node_modules/express/lib/router.js:{}:13)\n",
                        100 + i
                    )
                })
                .collect::<String>()
            + "  code: 'DEEP'\n";
        let out_deep = compact_js(&deep);
        let frames_in = deep
            .lines()
            .filter(|l| l.trim_start().starts_with("at "))
            .count();
        let frames_out = out_deep
            .lines()
            .filter(|l| l.trim_start().starts_with("at "))
            .count();
        assert!(
            frames_out < frames_in,
            "forty frames were not folded: {frames_in} in, {frames_out} out"
        );
        assert!(
            out_deep.contains("code: 'DEEP'"),
            "the tail was lost once the frames folded:\n{out_deep}"
        );

        // And a trace with nothing after its frames gains no empty tail.
        let bare: String = "Error: boom\n".to_string()
            + &(0..24)
                .map(|i| {
                    format!(
                        "    at Function._load{i} (node:internal/x.js:{}:1)\n",
                        10 + i
                    )
                })
                .collect::<String>();
        let out = compact_js(&bare);
        assert!(
            !out.ends_with('\n') && !out.ends_with(' '),
            "a trace with no tail gained trailing whitespace: {out:?}"
        );
    }

    /// Real bytes, captured 2026-09-02 from node v22.21.1.
    const NODE_MODULE_NOT_FOUND: &str = "node:internal/modules/cjs/loader:1386\n  throw err;\n  ^\n\nError: Cannot find module 'nosuch_xyz'\nRequire stack:\n- /app/[eval]\n    at Function._resolveFilename (node:internal/modules/cjs/loader:1383:15)\n    at defaultResolveImpl (node:internal/modules/cjs/loader:1025:19)\n    at resolveForCJSWithHooks (node:internal/modules/cjs/loader:1030:22)\n    at Function._load (node:internal/modules/cjs/loader:1192:37)\n    at TracingChannel.traceSync (node:diagnostics_channel:328:14)\n    at wrapModuleLoad (node:internal/modules/cjs/loader:237:24)\n    at Module.require (node:internal/modules/cjs/loader:1463:12)\n    at require (node:internal/modules/helpers:147:16)\n    at [eval]:1:1\n    at runScriptInThisContext (node:internal/vm:209:10) {\n  code: 'MODULE_NOT_FOUND',\n  requireStack: [ '/app/[eval]' ]\n}\n\nNode.js v22.21.1\n";
    use super::*;

    #[test]
    fn detects_python_trace() {
        let t = "Traceback (most recent call last):\n  File \"x.py\", line 1, in <module>\n    foo()\nValueError: bad\n";
        assert_eq!(detect_lang(t), Lang::Python);
    }

    #[test]
    fn detects_rust_panic() {
        let t = "thread 'main' panicked at src/main.rs:1:1:\nbad\n";
        assert_eq!(detect_lang(t), Lang::Rust);
    }

    #[test]
    fn enrich_inlines_throwing_frame_signature() {
        // Real source file the frame points into.
        let dir = std::env::temp_dir().join(format!("tok-stackenrich-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let src = "def outer():\n    return boom(1, 2)\n\ndef boom(x, y):\n    raise ValueError(\"nope\")\n";
        std::fs::write(dir.join("app.py"), src).unwrap();
        let app = dir.join("app.py");
        // Python traceback whose (last) user frame is line 5, inside `boom`.
        let trace = format!(
            "Traceback (most recent call last):\n  File \"{}\", line 2, in outer\n    return boom(1, 2)\n  File \"{}\", line 5, in boom\n    raise ValueError(\"nope\")\nValueError: nope\n",
            app.display(),
            app.display()
        );
        let compacted = compact(&trace);
        let enriched = enrich_top_user_frame(&compacted, &dir);
        assert!(
            enriched.contains("def boom(x, y):"),
            "expected boom signature inlined; got:\n{enriched}"
        );
        // Exactly one signature line added.
        assert_eq!(enriched.matches("    ↳ ").count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn enrich_is_noop_for_vendor_and_unresolvable_frames() {
        // Vendor path → skipped; non-existent path → unresolvable. Either way
        // the trace is returned unchanged (no signature line).
        let trace = "Traceback (most recent call last):\n  File \"/usr/lib/python3.12/site-packages/x/y.py\", line 9, in inner\n    boom()\n  File \"nope_does_not_exist.py\", line 3, in main\n    inner()\nValueError: bad\n";
        let compacted = compact(trace);
        let enriched = enrich_top_user_frame(&compacted, std::path::Path::new("/tmp"));
        assert_eq!(enriched, compacted, "should be unchanged");
        assert!(!enriched.contains("↳"));
    }

    #[test]
    fn detects_java_trace() {
        let t = "Exception in thread \"main\" java.lang.RuntimeException: bad\n\tat com.example.Foo.bar(Foo.java:42)\n";
        assert_eq!(detect_lang(t), Lang::Java);
    }

    #[test]
    fn python_keeps_user_frames_folds_framework() {
        // 100 framework frames + 1 user frame + the error message.
        let mut t = String::from("Traceback (most recent call last):\n");
        for _ in 0..100 {
            t.push_str("  File \"/usr/lib/python3.12/site-packages/django/handlers/exception.py\", line 55, in inner\n");
            t.push_str("    return func(*args, **kwargs)\n");
        }
        t.push_str("  File \"/home/me/app/views.py\", line 13, in detail\n");
        t.push_str("    obj = Item.objects.get(pk=pk)\n");
        t.push_str("ValueError: no such item\n");
        let out = compact(&t);
        assert!(out.contains("/home/me/app/views.py:13 in detail"));
        assert!(out.contains("framework frame(s)"));
        assert!(out.contains("ValueError: no such item"));
        let savings = crate::Savings::new(
            crate::tokenizer::estimate(&t),
            crate::tokenizer::estimate(&out),
        );
        assert!(savings.percent() > 80.0, "got {:?}", savings);
    }

    #[test]
    fn js_node_modules_collapsed() {
        let t = "Error: bad\n\
            at handler (/home/me/app/server.js:10:5)\n\
            at /node_modules/express/lib/router/route.js:144:13\n\
            at /node_modules/express/lib/router/route.js:117:3\n\
            at /node_modules/express/lib/router/index.js:284:15\n\
            at /node_modules/express/lib/router/index.js:365:14\n\
            at /node_modules/express/lib/router/index.js:174:7\n\
            at /node_modules/express/lib/router/index.js:47:12\n\
            at /node_modules/express/lib/router/layer.js:95:5\n\
            at /node_modules/express/lib/application.js:174:3\n\
            at /node_modules/express/lib/express.js:39:9\n";
        let out = compact(t);
        assert!(out.contains("/home/me/app/server.js"));
        assert!(out.contains("framework"));
    }

    /// A run of framework frames no longer than `2*BORDER` is worth less than
    /// the marker that would replace it — and on a trace with no user frame
    /// it is the whole trace. A real 4-line `drizzle-kit` trace came back as
    /// one frame and `… 3 framework frame(s)`.
    #[test]
    fn a_short_framework_run_is_kept() {
        let t = "    at process.processTicksAndRejections (node:internal/process/task_queues:105:5)\n\
            at async prepareAndMigratePg (/srv/app/node_modules/.pnpm/drizzle-kit@0.31.10/node_modules/drizzle-kit/bin.cjs:32191:42)\n\
            at async Object.handler (/srv/app/node_modules/.pnpm/drizzle-kit@0.31.10/node_modules/drizzle-kit/bin.cjs:91340:9)\n\
            at async run (/srv/app/node_modules/.pnpm/drizzle-kit@0.31.10/node_modules/drizzle-kit/bin.cjs:91472:7)\n";
        let frames: Vec<(bool, String)> =
            t.lines().map(|l| (false, l.trim().to_string())).collect();
        let out = fold(frames.clone());
        assert_eq!(out.len(), 4, "{out:?}");
        assert!(
            out.iter().any(|l| l.contains("prepareAndMigratePg")),
            "{out:?}"
        );
        // A long run still folds.
        let long: Vec<(bool, String)> = (0..9)
            .map(|i| (false, format!("at /node_modules/x/lib/f{i}.js:{i}:1")))
            .collect();
        assert!(fold(long)
            .iter()
            .any(|l| l.contains("9 framework frame(s)")));
    }

    #[test]
    fn unknown_format_falls_back_safely() {
        let t = "random text that isn't a stack trace at all, just to check we don't panic.";
        let _ = compact(t);
    }

    #[test]
    fn savings_with_helper() {
        let mut t = String::from("Error: x\n");
        for _ in 0..200 {
            t.push_str("    at /node_modules/foo/bar.js:1:1\n");
        }
        t.push_str("    at /home/me/app.js:42:7\n");
        let (out, sav) = compact_with_savings(&t);
        assert!(out.contains("/home/me/app.js:42"));
        assert!(sav.percent() > 60.0, "got {:?}", sav);
    }

    // ---- Adversarial tests for dedup_blocks ------------------------------

    #[test]
    fn dedup_collapses_50_identical_pytest_failures() {
        // Synthesise a pytest output where 50 parametrised cases all fail the
        // same way. Each block ~600 chars; raw output ~30 KB; deduped should
        // keep the first verbatim + an `×50` marker.
        let mut t = String::from(
            "=================================== FAILURES ===================================\n",
        );
        for i in 0..50 {
            let _ = write!(
                t,
                "________________________________ test_x[case{i}] ____________________________________\n\n\
                 def test_x(case):\n\
                 >       assert case.is_valid()\n\
                 E       AssertionError: assert False\n\
                 E        +  where False = <Case 'case{i}'>.is_valid()\n\n\
                 tests/test_x.py:42: AssertionError\n"
            );
        }
        let (out, sav) = compact_with_savings(&t);
        assert!(
            out.contains("identical traceback"),
            "expected dedup marker, got:\n{out}"
        );
        // The first case should appear by name; the 49th should NOT appear in
        // full — only listed as a sibling.
        assert!(out.contains("test_x[case0]"));
        assert!(
            !out.contains("test_x[case10]") || out.contains("(also: "),
            "case10 should only appear as a sibling reference"
        );
        assert!(
            sav.percent() > 80.0,
            "expected >80% savings, got {:.1}%",
            sav.percent()
        );
    }

    #[test]
    fn dedup_keeps_distinct_tracebacks() {
        // 5 tests fail with the SAME error, 1 fails with a DIFFERENT error.
        // The distinct one must appear in full.
        let mut t = String::from(
            "=================================== FAILURES ===================================\n",
        );
        for i in 0..5 {
            let _ = write!(
                t,
                "________________________________ test_same[{i}] ____________________________________\n\n\
                 def test_same(x):\n\
                 >       assert x == 1\n\
                 E       AssertionError: assert 0 == 1\n\
                 tests/a.py:10: AssertionError\n"
            );
        }
        t.push_str(
            "________________________________ test_unique ____________________________________\n\n\
             def test_unique():\n\
             >       raise RuntimeError('completely different bug')\n\
             E       RuntimeError: completely different bug\n\
             tests/b.py:20: RuntimeError\n",
        );
        let out = compact(&t);
        assert!(out.contains("AssertionError"));
        assert!(
            out.contains("completely different bug"),
            "unique traceback must be preserved verbatim, got:\n{out}"
        );
    }

    #[test]
    fn dedup_never_inflates_single_block_input() {
        // A normal single-test pytest output must not be touched (or only
        // shortened) by the dedup pass.
        let t = "tests/foo.py::test_one FAILED\n\
                 ================ FAILURES =================\n\
                 ____________ test_one ___________\n\
                 \n\
                 def test_one():\n\
                 >       assert 1 == 2\n\
                 E       AssertionError\n";
        let out = dedup_blocks(t);
        assert!(out.len() <= t.len() + 4, "dedup must not grow input");
    }

    #[test]
    fn dedup_handles_cargo_test_separators() {
        // cargo test failure output: each failing test gets a `---- name stdout ----`.
        let mut t = String::from("failures:\n\n");
        for i in 0..20 {
            let _ = write!(
                t,
                "---- module::test_case_{i} stdout ----\n\
                 thread 'module::test_case_{i}' panicked at src/lib.rs:99:5:\n\
                 assertion `left == right` failed\n\
                   left: 0\n\
                  right: 1\n\
                 note: run with `RUST_BACKTRACE=1` environment variable\n\n"
            );
        }
        let out = dedup_blocks(&t);
        assert!(
            out.contains("identical traceback"),
            "cargo-test blocks should be deduped, got:\n{out}"
        );
        let sav = crate::Savings::new(
            crate::tokenizer::estimate(&t),
            crate::tokenizer::estimate(&out),
        );
        assert!(
            sav.percent() > 70.0,
            "expected >70% savings, got {:.1}%",
            sav.percent()
        );
    }

    #[test]
    fn dedup_robust_against_malformed_separator_lookalike() {
        // A README with `___` art and `---` rules MUST NOT be torn apart by
        // dedup_blocks pretending it's a pytest output.
        let t = "# Title\n\n___\n\nSome prose.\n\n___\n\nMore prose.\n";
        let out = dedup_blocks(t);
        assert_eq!(out, t);
    }

    // ---- Adversarial: failure family grouping (Tokenless-inspired) -------

    #[test]
    fn classify_missing_module_pytest() {
        let b = "ModuleNotFoundError: No module named 'foo'";
        assert_eq!(FailureFamily::classify(b), FailureFamily::MissingModule);
    }

    #[test]
    fn classify_async_timeout_jest() {
        let b = "Exceeded timeout of 5000 ms for a test.";
        assert_eq!(FailureFamily::classify(b), FailureFamily::AsyncTimeout);
    }

    #[test]
    fn classify_undefined_access_js() {
        let b = "TypeError: Cannot read properties of undefined (reading 'name')";
        assert_eq!(FailureFamily::classify(b), FailureFamily::UndefinedAccess);
    }

    #[test]
    fn classify_assertion_pytest() {
        let b = "E       AssertionError: assert 1 == 2";
        assert_eq!(FailureFamily::classify(b), FailureFamily::Assertion);
    }

    #[test]
    fn classify_unknown_falls_back_to_other() {
        let b = "weird unexplained output";
        assert_eq!(FailureFamily::classify(b), FailureFamily::Other);
    }

    #[test]
    fn group_collapses_50_assertion_failures_into_one_line() {
        let mut t = String::from(
            "=================================== FAILURES ===================================\n",
        );
        for i in 0..50 {
            let _ = write!(
                t,
                "________________________________ test_x[case{i}] ____________________________________\n\n\
                 def test_x(case):\n>       assert case.is_valid()\nE       AssertionError: assert False\ntests/test_x.py:42: AssertionError\n"
            );
        }
        let out = group_by_failure_family(&t);
        assert!(
            out.contains("[Assertion failure] ×50"),
            "expected one line with ×50, got:\n{out}"
        );
        // Three example labels separated by `|`.
        assert!(out.contains("test_x[case0]"));
        assert!(out.contains("test_x[case1]"));
        assert!(out.contains("test_x[case2]"));
        // 47 more — counted, not listed.
        assert!(out.contains("+47 more"));
    }

    #[test]
    fn group_separates_mixed_families() {
        let mut t = String::from(
            "=================================== FAILURES ===================================\n",
        );
        for i in 0..5 {
            let _ = write!(
                t,
                "________________________________ test_assert_{i} ____________________________________\n\nE       AssertionError: nope\n"
            );
        }
        for i in 0..3 {
            let _ = write!(
                t,
                "________________________________ test_timeout_{i} ____________________________________\n\nException: timed out after 5000 ms\n"
            );
        }
        let out = group_by_failure_family(&t);
        assert!(out.contains("[Assertion failure] ×5"));
        assert!(out.contains("[Async timeout] ×3"));
    }

    #[test]
    fn group_passes_through_when_no_known_family() {
        let t =
            "=== FAILURES ===\n____ unknown ____\n\nweird unexplained problem\n____ unknown_2 ____\n\nstill weird\n";
        let out = group_by_failure_family(t);
        // Every block classified as Other → bail out and return input.
        assert_eq!(out, t);
    }

    #[test]
    fn group_does_not_collapse_single_block() {
        let t = "=== FAILURES ===\n____ test_only ____\n\nE       AssertionError: x\n";
        let out = group_by_failure_family(t);
        // Only one block — pass through.
        assert_eq!(out, t);
    }

    #[test]
    fn group_real_savings_on_repetitive_pytest_failure() {
        // 200 identical assertion failures over a parametrized test → the
        // grouped output should be a tiny fraction of the original.
        let mut t = String::from(
            "=================================== FAILURES ===================================\n",
        );
        for i in 0..200 {
            let _ = write!(
                t,
                "________________________________ test_param[{i}] ____________________________________\n\n\
                 def test_param(x):\n>       assert x > 0\nE       AssertionError: assert False\n   +  where False = {i} > 0\n\
                 tests/test_param.py:42: AssertionError\n\n"
            );
        }
        let sav = crate::Savings::new(
            crate::tokenizer::estimate(&t),
            crate::tokenizer::estimate(&group_by_failure_family(&t)),
        );
        assert!(
            sav.percent() > 95.0,
            "expected >95% savings, got {:.1}%",
            sav.percent()
        );
    }
}
#[cfg(test)]
mod prose_that_mentions_a_path_is_not_a_frame {
    //! `text.contains("at /")` — three characters, unanchored. A
    //! `docker-compose.yml` whose header comment says "Repos stored in
    //! /work/" was folded as a JavaScript stack trace: 28 709 bytes in,
    //! **854 out**, every service, volume and network gone.

    const COMPOSE: &str = "# Development Docker Compose\n# Repos are stored at /work/ and metadata at /work/.codebase\nservices:\n  qdrant:\n    image: qdrant/qdrant:latest\n    volumes:\n      - qdrant_storage:/qdrant/storage\n    networks:\n      - dev-network\n  api:\n    build: .\n    environment:\n      - OPENLIT_ENABLED=0\n    networks:\n      - dev-network\nvolumes:\n  qdrant_storage:\nnetworks:\n  dev-network:\n";

    #[test]
    fn a_comment_mentioning_a_path_is_not_a_javascript_trace() {
        assert_ne!(
            format!("{:?}", super::detect_lang(COMPOSE)),
            "JavaScript",
            "`stored at /work/` is prose"
        );
    }

    #[test]
    fn the_compose_file_keeps_its_services() {
        let out = crate::auto::compact(COMPOSE).1;
        for needle in ["qdrant/qdrant:latest", "dev-network", "OPENLIT_ENABLED"] {
            assert!(out.contains(needle), "{needle:?} was dropped");
        }
    }

    #[test]
    fn a_real_frame_without_line_numbers_still_detects() {
        let js = "Error: boom\n    at /srv/app/index.js\n    at /srv/app/lib/run.js\n";
        assert_eq!(format!("{:?}", super::detect_lang(js)), "JavaScript");
    }

    #[test]
    fn a_normal_v8_trace_still_detects() {
        let js = "TypeError: x is not a function\n    at handler (/srv/app/index.js:12:5)\n    at run (/srv/app/lib/run.js:44:9)\n";
        assert_eq!(format!("{:?}", super::detect_lang(js)), "JavaScript");
    }
}
