//! End-to-end tests driving `tokenade misc`.

use super::*;

#[test]
fn e2e_version_flag_works() {
    let h = TempHome::new("version");
    let (stdout, _err, code) = run(&["--version"], h.path());
    assert_eq!(code, 0, "binary should exit 0 on --version");
    assert!(stdout.contains("tokenade"));
}

#[test]
fn disclose_read_returns_the_bytes_not_the_file_extension() {
    // `read_stash_by_hash` returns `(content, extension)`. The `read` arm
    // destructured it as `(_meta, body)` and printed the SECOND element, so a
    // 43k-token JSON blob came back as the four characters `json`, exit code 0.
    // Every kind had its own wrong answer — `mk` for a log, `tab.txt` for a
    // table — which is why this asserts on the CONTENT, not on "not json".
    let h = TempHome::new("disclose-read");
    let needle = "MARKER_UNIQUE_STASHED_LINE";
    let blob: String = (0..4000)
        .map(|i| {
            format!(
                "2026-09-02T10:00:00Z INFO worker[{}] id={i} {needle}\n",
                i % 8
            )
        })
        .collect();
    let (stashed, _e, code) =
        run_with_stdin(&["disclose", "stash", "--source", "shell"], h.path(), &blob);
    assert_eq!(code, 0, "stash must succeed");
    // The hash is looked up STRIPPED of its formatting. The stash footer now
    // spells the same menu as the fold footer — a single declarer,
    // `disclose::recovery_menu` — and that one writes the remedy as a code span:
    // `` `tokenade expand-ref <hash>` ``. Splitting on whitespace therefore
    // brought back `<hash>` with its backtick attached, thirteen characters
    // instead of twelve, and this test was the first consumer to notice.
    let hash = stashed
        .split_whitespace()
        .map(|t| t.trim_matches('`'))
        .find(|t| t.len() == 12 && t.chars().all(|c| c.is_ascii_hexdigit()))
        .unwrap_or_else(|| panic!("no stash hash in banner: {stashed}"))
        .to_string();

    let (out, _err, code) = run(&["disclose", "read", &hash], h.path());
    assert_eq!(code, 0, "a known hash must succeed");
    assert!(
        out.contains(needle),
        "disclose read returned no stashed content: {:?}",
        &out[..out.len().min(120)]
    );
    // The extension is what the bug printed. Guard the exact failure shape:
    // a body that IS just the extension is a regression even if non-empty.
    assert!(
        out.trim() != "txt"
            && out.trim() != "mk"
            && out.trim() != "json"
            && out.trim() != "tab.txt",
        "disclose read printed the stash file extension: {out:?}"
    );

    // The unknown-hash path was never broken — pin it so a fix here cannot
    // trade one arm for the other.
    let (_o, err, code) = run(&["disclose", "read", "ffffffffffff"], h.path());
    assert_eq!(code, 1, "an unknown hash must fail");
    assert!(
        err.contains("no stash for hash"),
        "unknown hash said: {err}"
    );
}

#[test]
fn a_ref_that_was_never_a_hash_is_not_blamed_on_expiry() {
    let home = TempHome::new("expand-ref-diag");
    for bad in ["zz", "D17A8E85A3DB", "d17a8e85a3dbextra"] {
        let (_out, err, code) = run(&["expand-ref", bad], home.path());
        assert_eq!(code, 1, "{bad} must fail");
        assert!(err.contains("is not a ref hash"), "{bad} answered: {err}");
        assert!(
            !err.contains("expired"),
            "{bad} must not blame expiry: {err}"
        );
    }
    // A WELL-FORMED hash that is simply not there is not treated as a typo:
    // it says what the store holds, and it NAMES the next step.
    //
    // This message used to hesitate ("may have expired … or was never
    // registered") and named no next step, while 41 recoveries out of 436
    // come back empty-handed across 758 real transcripts. The hesitation is
    // replaced by a fact measured on the store, and by the only path left
    // once the bytes are gone.
    let (_out, err, code) = run(&["expand-ref", "deadbeef0000"], home.path());
    assert_eq!(code, 1);
    assert!(
        !err.contains("is not a ref hash"),
        "a well-formed hash is not a typo: {err}"
    );
    assert!(
        err.contains("the stash is empty"),
        "the message does not say what the store holds: {err}"
    );
    assert!(
        err.contains("re-run the command") && err.contains("tokenade raw"),
        "no next step named: {err}"
    );
}

/// Data piped into `tokenade exec` must reach the script.
///
/// It did not, on any version before this one, and it did not FAIL either:
/// `printf 'a\nb\n' | tokenade exec --lang sh --script 'wc -l'` answered `0`.
/// The cause is structural — `sh`/`bash` are driven with `-s`, so the child's
/// stdin carries the SCRIPT, and the `-c`-style interpreters had their stdin
/// closed outright. An agent piping a command's output in got a confident
/// wrong number rather than a complaint, which is the worst shape a defect
/// can take in a tool whose whole purpose is computing over data.
///
/// Both interpreter families are exercised because they fail for DIFFERENT
/// reasons and are fixed by different means: `Arg` mode inherits stdin,
/// `Stdin` mode additionally switches the script to `-c` to free the channel.
#[test]
fn e2e_piped_stdin_reaches_the_exec_script() {
    let h = TempHome::new("exec-stdin");
    let (out, err, code) = run_with_stdin(
        &["exec", "--lang", "sh", "--script", "wc -l"],
        h.path(),
        "a\nb\nc\n",
    );
    assert_eq!(code, 0, "sh: {err}");
    assert!(
        out.contains('3'),
        "piped stdin never reached the sh script (got {out:?})"
    );

    let (out, err, code) = run_with_stdin(
        &[
            "exec",
            "--lang",
            "python",
            "--script",
            "import sys; print(len(sys.stdin.readlines()))",
        ],
        h.path(),
        "x\ny\n",
    );
    assert_eq!(code, 0, "python: {err}");
    assert!(
        out.contains('2'),
        "piped stdin never reached the python script (got {out:?})"
    );
}

/// `exec` compacts and CAPS, and between those two there was nothing.
///
/// Output the compactors cannot fold — a log whose every line differs — was
/// delivered at the 1 MiB cap: ~260 k tokens, a whole context window, from the
/// command whose documented promise is that only its stdout reaches you.
/// Measured 2026-08-27 on 9 368 890 bytes of varied log lines: `exec` returned
/// 1 048 678 where `wrap` on the SAME bytes returned 3 013.
///
/// Both halves are asserted, because a cap that folds everything is the other
/// way to fail: a small answer must still arrive whole and exact.
#[test]
fn e2e_exec_stashes_an_output_it_could_not_fold() {
    let h = TempHome::new("exec-stash");
    // Every line distinct AND not templatable. Numbers are the wrong fixture
    // here: the value-folding compactor collapses `req={i}` into one
    // `req=<N2>` line with a count, which is a correct fold and leaves nothing
    // for the stash to do. Distinct WORDS, derived from the index in base-26
    // so the fixture stays deterministic, defeat that.
    let script = "\nimport string\nA = string.ascii_lowercase\ndef w(n):\n    s = ''\n    for _ in range(6):\n        s += A[n % 26]\n        n //= 26\n    return s\nfor i in range(60000):\n    print(f'2026-08-27T10:00:00Z INFO worker={w(i)} req={w(i*7)} path=/api/v1/{w(i*13)}')\n";
    let (out, err, code) = run(&["exec", "--lang", "python", "--script", script], h.path());
    assert_eq!(code, 0, "{err}");
    assert!(
        out.len() < 100_000,
        "an unfoldable output must be stashed, not delivered ({} bytes)",
        out.len()
    );
    assert!(
        out.contains("disclose:summary") || out.contains("stashed"),
        "and it must say where the rest went (got {:?})",
        &out[..out.len().min(200)]
    );

    let (out, err, code) = run(
        &["exec", "--lang", "python", "--script", "print(6 * 7)"],
        h.path(),
    );
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.trim(), "42", "a small answer must arrive whole");

    // …and `--no-compact` means what it says. The stash is a compaction, so it
    // answers to the same opt-out — a caller asking for the script's own bytes
    // gets them. Caught by the SIGPIPE guard, whose fixture needs a write big
    // enough to fill several pipe buffers and got a stash pointer instead.
    let (out, err, code) = run(
        &[
            "exec",
            "--lang",
            "bash",
            "--script",
            "seq 1 100000",
            "--no-compact",
        ],
        h.path(),
    );
    assert_eq!(code, 0, "{err}");
    assert!(
        out.len() > 500_000,
        "--no-compact must deliver the bytes, got {} of ~580 000",
        out.len()
    );
}

/// A script too large to travel as an argv element must still RUN when a pipe
/// is present — the `-c` switch above must not become a size limit that `-s`
/// never had.
///
/// Measured while writing it: `sh -c` with a 500 KiB script dies on
/// `Argument list too long`, and `--script-file` can carry far more than
/// `--script` ever could. Since an agent's stdin is never a terminal, an
/// unbounded rule would have routed EVERY such call down the `-c` path. Above
/// the ceiling `-s` is kept, so the script runs and stdin stays the script's —
/// the one uncovered combination, asserted here so it stays deliberate.
#[test]
fn e2e_a_huge_script_still_runs_when_stdin_is_piped() {
    let h = TempHome::new("exec-big-script");
    let big = h.path().join("big.sh");
    let mut body = String::with_capacity(200 * 1024 + 32);
    body.push_str("# ");
    body.push_str(&"x".repeat(200 * 1024));
    body.push_str("\necho huge-script-ran\n");
    std::fs::write(&big, &body).unwrap();

    let (out, err, code) = run_with_stdin(
        &[
            "exec",
            "--lang",
            "sh",
            "--script-file",
            big.to_str().unwrap(),
        ],
        h.path(),
        "ignored\n",
    );
    assert_eq!(code, 0, "huge script failed: {err}");
    assert!(
        out.contains("huge-script-ran"),
        "the size ceiling broke a script that `-s` used to run (got {out:?}, err {err:?})"
    );
}

/// If a handler bothers to answer `--help`, `--help` must REACH it.
///
/// `cmd_forwards_help` is an opt-in list, and everything absent from it is
/// intercepted by the global handler, which prints a one-line usage plus
/// "this command has no dedicated help yet" — about commands that DO have
/// help, sitting unreachable inside the handler. Measured 2026-08-08: 13 such
/// commands, including `login`, whose own usage is the only place
/// `--no-browser` is documented, and `web`/`search`, which the scaffold tells
/// every agent to use.
///
/// The roster is derived from the SOURCE and checked against the BINARY, so a
/// handler that starts answering `--help` tomorrow either gets routed or fails
/// here. That matters more than the 13: two defects this week came from a list
/// somebody had to remember to update.
#[test]
fn a_handler_that_answers_help_actually_receives_it() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources: Vec<String> = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                if let Ok(s) = std::fs::read_to_string(&p) {
                    sources.push(s);
                }
            }
        }
    }
    assert!(
        sources.len() > 20,
        "found only {} source files — the walker stopped walking",
        sources.len()
    );

    // Does this handler answer `--help` itself?
    let handles_help = |handler: &str| -> bool {
        sources.iter().any(|s| {
            let Some(i) = s.find(&format!("fn {handler}(")) else {
                return false;
            };
            let rest = &s[i..];
            let end = rest[1..]
                .find("\nfn ")
                .map(|x| x + 1)
                .unwrap_or(rest.len())
                .min(
                    rest[1..]
                        .find("\npub(crate) fn ")
                        .map(|x| x + 1)
                        .unwrap_or(rest.len()),
                );
            rest[..end].contains("asks_for_help")
        })
    };

    let main_rs = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs"),
    )
    .expect("main.rs");
    let mut checked = 0usize;
    let mut unreachable: Vec<String> = Vec::new();
    for line in main_rs.lines() {
        let t = line.trim();
        let Some((pats, body)) = t.split_once("=>") else {
            continue;
        };
        if !pats.trim_start().starts_with('"') {
            continue;
        }
        let Some(handler) = body.split('(').next().map(str::trim) else {
            continue;
        };
        if !handler.starts_with("cmd_") || !handles_help(handler) {
            continue;
        }
        for name in pats.split('|').filter_map(|p| {
            p.trim()
                .strip_prefix('"')
                .and_then(|p| p.strip_suffix('"'))
                .map(str::to_string)
        }) {
            let h = TempHome::new("help-reach");
            let (out, err, _) = run(&[&name, "--help"], h.path());
            checked += 1;
            if format!("{out}{err}").contains("no dedicated help yet") {
                unreachable.push(format!("{name} ({handler})"));
            }
        }
    }
    // Anti-vacuity: a parser that matched nothing would pass in silence.
    assert!(
        checked >= 10,
        "only {checked} commands were checked — the dispatch parser stopped \
         matching the source it reads"
    );
    assert!(
        unreachable.is_empty(),
        "these handlers answer `--help` but never receive it (the global \
         handler claims they have no help): {unreachable:?}"
    );
}

/// A FLAG must never become part of the QUESTION.
///
/// `ask` stripped `--output-mode/--format/-o` and `--budget` from the query
/// with a comment stating the principle. `--limit`/`-n` was added later and
/// never joined that list, so `ask "where is auth" --limit 5` searched for
/// "where is auth --limit 5". Measured against 1.1.7: a nonsense symbol plus
/// `--limit 3` returned `12+` matches — the flag itself did the matching —
/// where the fixed binary answers `no match`. Worse than noise: confident
/// results for a question nobody asked.
///
/// The fix was to REUSE the validator `query` has had since #680 rather than
/// extend a second list, so this also pins the typo behaviour it brings with
/// it. Two lists that must agree are the defect; one declaration is the fix.
#[test]
fn e2e_a_semantic_flag_never_leaks_into_the_query() {
    let h = TempHome::new("ask-flag-leak");
    std::fs::write(h.path().join("a.rs"), "pub fn alpha() {}\n").unwrap();

    let (out, err, _) = run(
        &["ask", "zzzqqqnosuchsymbol", "--limit", "3", "-o", "count"],
        h.path(),
    );
    let all = format!("{out}{err}");
    assert!(
        all.contains("zzzqqqnosuchsymbol"),
        "expected the query echoed back, got:\n{all}"
    );
    assert!(
        !all.contains("--limit"),
        "the flag leaked into the searched query:\n{all}"
    );

    // Same declaration, second hole: a typo must be REFUSED, not searched.
    let (out, err, code) = run(&["ask", "alpha", "--bogus"], h.path());
    let all = format!("{out}{err}");
    assert_eq!(code, 2, "a bad flag must fail, got {code}:\n{all}");
    assert!(
        all.contains("unknown flag"),
        "expected an unknown-flag refusal, got:\n{all}"
    );
}

/// A mistyped flag must not become a paid web search.
///
/// `cmd_search` ended its match with `other => queries.push(other)`, so
/// `tokenade search --josn "rust async"` ran TWO searches — one for `--josn` —
/// spending network and quota on a typo and blending its results into the
/// answer. Rejection happens before any request, which is what makes this test
/// safe to run offline.
///
/// Flag-SHAPED, not hyphen-leading: `tokenade search "-Wall meaning"` is a
/// legitimate one-argument query beginning with a dash, and the `--` prefix is
/// what keeps it a query. Breaking a working invocation to fix a typo would be
/// the worse trade.
#[test]
fn e2e_a_mistyped_search_flag_is_refused_not_searched() {
    let h = TempHome::new("search-typo");
    let (out, err, code) = run(&["search", "--josn", "rust async"], h.path());
    let all = format!("{out}{err}");
    assert_eq!(code, 2, "a mistyped flag must fail, got {code}:\n{all}");
    assert!(
        all.contains("unknown flag") && all.contains("--josn"),
        "expected the typo named in the refusal, got:\n{all}"
    );

    // `semantic` has the same ending, and rejects before loading any model.
    let (out, err, code) = run(&["semantic", "alpha", "--bogus"], h.path());
    let all = format!("{out}{err}");
    assert_eq!(code, 2, "semantic must refuse too, got {code}:\n{all}");
    assert!(
        all.contains("unknown flag") || all.contains("unknown"),
        "expected an unknown-flag refusal, got:\n{all}"
    );
}

/// A parameter that was retired must be REFUSED by name, not fetched.
///
/// `kind=page` / `kind=serp` were MCP tool parameters, dropped with the
/// zero-MCP pivot. The retirement was enforced on the scaffolded docs and
/// nowhere else, so the command met both spellings as strangers: `--kind page`
/// got the generic unknown-flag shrug, and the bare `kind=serp` fell into the
/// POSITIONAL arm and became a second fetch target. `tokenade web page.html
/// kind=serp` therefore served the page, printed `kind=serp: fetch/read failed`
/// about a target nobody named, and exited 1 — a run that worked, reported as
/// a failure to anything reading the exit code.
///
/// Measured 2026-08-31: 19 real `web` calls across 10 sessions on this machine
/// carry one of the two spellings, which is a doc that taught it for years
/// rather than a caller guessing. Both spellings are asserted, because the two
/// arrived by different arms and a fix to one is not a fix to the other.
#[test]
fn e2e_the_retired_kind_parameter_is_named_not_fetched() {
    let h = TempHome::new("web-kind");
    let page = h.path().join("p.html");
    std::fs::write(&page, "<html><body><h1>Hi</h1><p>body</p></body></html>").unwrap();
    let p = page.to_str().unwrap().to_string();

    for args in [
        vec!["web", p.as_str(), "kind=serp"],
        vec!["web", "--kind", "page", p.as_str()],
    ] {
        let (out, err, code) = run(&args, h.path());
        let all = format!("{out}{err}");
        assert_eq!(code, 2, "{args:?} must refuse, got {code}:\n{all}");
        assert!(
            all.contains("no longer exists"),
            "{args:?} must say the parameter is retired, got:\n{all}"
        );
        assert!(
            !all.contains("fetch/read failed"),
            "{args:?} must not treat the parameter as a fetch target:\n{all}"
        );
        assert!(
            out.trim().is_empty(),
            "{args:?} must not serve the page under a refusal:\n{out}"
        );
    }

    // And the page itself still reads, so the refusal did not eat a real target.
    let (out, _err, code) = run(&["web", p.as_str()], h.path());
    assert_eq!(code, 0, "a clean fetch must still succeed");
    assert!(out.contains("Hi"), "the page must still be served:\n{out}");
}

/// A command that takes no positionals must REFUSE one, not drop it.
///
/// `receipt`, `audit-gains` and `list-commands` returned exit 0 on a bare
/// bogus argument — reporting success on something they never did — while nine
/// sibling commands already refused exactly that. And `add-filter <path>` fell
/// through to the stdin branch, so an empty non-tty stdin produced a TOML parse
/// error about a document nobody wrote, byte-identical with and without the
/// argument.
///
/// Each command is exercised on FOUR shapes, not just the one that motivated the
/// fix: the first attempt reused `collect_positionals_with_values`, which
/// REQUIRES a positional, and turned bare `tokenade receipt` into a usage error.
/// Verifying only the failing input would have shipped that.
#[test]
fn a_command_with_no_positionals_refuses_one_instead_of_dropping_it() {
    let h = TempHome::new("no-positionals");
    // (command, a flag that must still work)
    let cases: [(&str, &str); 3] = [
        ("receipt", "--json"),
        ("audit-gains", "--json"),
        ("list-commands", ""),
    ];
    for (cmd, ok_flag) in cases {
        let (_o, _e, bare) = run(&[cmd], h.path());
        assert_eq!(bare, 0, "[{cmd}] must still work with NO arguments");
        if !ok_flag.is_empty() {
            let (_o, _e, flagged) = run(&[cmd, ok_flag], h.path());
            assert_eq!(flagged, 0, "[{cmd} {ok_flag}] must still work");
        }
        let (_o, err, code) = run(&[cmd, "__tokenade_bogus__"], h.path());
        assert_eq!(
            code, 2,
            "[{cmd}] a bare bogus argument must be refused, not silently dropped"
        );
        assert!(
            err.contains("__tokenade_bogus__"),
            "[{cmd}] the refusal must NAME the argument it rejected, got: {err}"
        );
    }

    // `add-filter`: an empty stdin and a stray positional are two different
    // situations and must not read identically — that is the criterion.
    let (_o, no_stdin, c1) = run(&["add-filter"], h.path());
    assert_eq!(c1, 2);
    let (_o, positional, c2) = run(&["add-filter", "__tokenade_bogus__"], h.path());
    assert_eq!(c2, 2);
    assert!(
        !no_stdin.contains("TOML parse error"),
        "an empty stdin is not an invalid filter; it must not surface the parser: {no_stdin}"
    );
    assert!(
        no_stdin.contains("--file"),
        "the refusal must name the way in: {no_stdin}"
    );
    assert!(
        positional.contains("__tokenade_bogus__"),
        "the stray argument must be named: {positional}"
    );
    assert_ne!(
        no_stdin.trim(),
        positional.trim(),
        "two situations that MUST differ produced the same text"
    );
}

/// A removed command must name its replacement, not read as a typo.
///
/// `tokenade proxy` was an argv-only variant of `wrap`, and its main consumer
/// was the PATH shim — which is gone. Removed 2026-08-10 so the name is free
/// for the LLM proxy. Someone whose script or muscle memory still types it gets
/// a sentence that fixes them, not `unknown command 'proxy'`.
#[test]
fn a_removed_command_names_its_replacement() {
    let h = TempHome::new("removed-proxy");
    let (_o, err, code) = run(&["proxy", "echo", "hi"], h.path());
    assert_eq!(code, 2, "a removed command must fail, not silently run");
    assert!(
        err.contains("removed"),
        "the refusal must say it was removed: {err}"
    );
    assert!(
        err.contains("tokenade wrap"),
        "the refusal must NAME the replacement: {err}"
    );
    // And the old MCP name keeps working, because it is written into every
    // shipped CLAUDE.md — renaming it without an alias breaks that file silently.
    for name in ["mcp-wrap", "mcp-proxy"] {
        let (out, _e, c) = run(&[name, "--help"], h.path());
        assert_eq!(c, 0, "{name} --help must work");
        assert!(
            out.contains("mcp-wrap"),
            "{name} --help must present the CURRENT name: {out}"
        );
    }
}

/// `tokenade read <source-file>` used to hand the file back whole while the
/// Read HOOK stripped its comments — the same bytes, two answers, decided by
/// which door they came through. Drives the real binary, so it fails if the
/// call site is removed and the helper is merely left behind.
#[test]
fn read_strips_comments_from_source_the_way_the_hook_does() {
    let home = crate::TempHome::new("read-source-strip");
    // Outside any git work tree: inside one, the editable-source exception
    // applies and raw bytes are the right answer.
    let f = home.path().join("handler.go");
    let body: String = (0..200)
        .map(|i| format!("// what handler {i} does, at some length so it counts\nfunc handler{i}() {{ return }}\n"))
        .collect();
    std::fs::write(&f, &body).unwrap();
    let (out, _err, rc) = crate::run(&["read", f.to_str().unwrap()], home.path());
    assert_eq!(rc, 0);
    assert!(
        out.contains("[tokenade:read-code-strip lang=go"),
        "the content path must declare the strip: {}",
        out.lines().next().unwrap_or("")
    );
    assert!(
        !out.contains("what handler 7 does"),
        "the comments are what gets stripped"
    );
    assert!(out.contains("func handler7()"), "the code must survive");
    assert!(out.len() < body.len(), "{} vs {}", out.len(), body.len());
}

/// The question used to be dropped in SILENCE on exactly the files worth
/// asking one about. `try_source_strip` returns from `cmd_auto` before the
/// retrieval dispatch runs, and it never received `--prompt` at all: measured
/// 2026-08-31 on a 24 006-token `.rs`, stdout was BYTE-IDENTICAL with and
/// without the flag (56 300 both ways) and stderr was empty. The advisory that
/// used to say "--prompt was NOT applied" had been removed on the ground that
/// the dispatch downstream now answers the question — true on the text path,
/// and this path never reaches it.
///
/// Byte-identity is the assertion, because that is the shape of the defect: a
/// weaker "is it shorter" would also pass on a build that merely folded harder.
#[test]
fn a_question_reaches_the_source_strip_path() {
    let home = crate::TempHome::new("read-source-scoped");
    // Outside any git work tree, and above the editable-source ceiling: those
    // two together are what selects the strip path over the fall-through.
    let f = home.path().join("handler.go");
    let mut body: String = (0..400)
        .map(|i| format!("// what handler {i} does, at some length so it counts\nfunc handler{i}() {{ return }}\n"))
        .collect();
    body.push_str(
        "// the pelican reconciler drains the queue\nfunc pelicanReconciler() { drainQueue() }\n",
    );
    std::fs::write(&f, &body).unwrap();
    let path = f.to_str().unwrap();

    let (whole, _e, rc) = crate::run(&["read", path], home.path());
    assert_eq!(rc, 0);
    let (answer, _e, rc) = crate::run(
        &["read", path, "--prompt", "what does pelicanReconciler do"],
        home.path(),
    );
    assert_eq!(rc, 0);

    assert_ne!(
        whole, answer,
        "the question changed nothing — it was dropped on the way in"
    );
    assert!(
        answer.len() < whole.len() / 2,
        "an answer must be a fraction of the delivery: {} vs {}",
        answer.len(),
        whole.len()
    );
    assert!(
        answer.contains("pelicanReconciler"),
        "the answer must carry what was asked about: {answer}"
    );
    // And it must SAY it answered, or a scoped extract reads as the whole file.
    assert!(
        answer.contains("mode=comments+scoped"),
        "the banner must declare the retrieval: {}",
        answer.lines().next().unwrap_or("")
    );
    assert!(
        whole.contains("mode=comments]"),
        "no question, no retrieval: {}",
        whole.lines().next().unwrap_or("")
    );
}

/// A typo must not rewrite the agent's MCP config for every server it has.
///
/// `first_positional` skipped every `-`-prefixed token, so an unimplemented
/// flag was DISCARDED and the subcommand ran with no server name — which means
/// "all of them". Measured 2026-08-31 on a config holding two servers,
/// `mcp-wrap uninstall --zzbogus` restored BOTH and reported success at exit 0.
/// `--all` is documented and "worked" for exactly the same reason it was
/// thrown away, so the real flag was never implemented either.
#[test]
fn a_typo_does_not_unwrap_every_mcp_server() {
    let home = crate::TempHome::new("mcp-wrap-typo");
    let cfg = home.path().join(".claude.json");
    let original = r#"{"mcpServers":{"alpha":{"command":"/bin/echo","args":["alpha"]},"beta":{"command":"/bin/echo","args":["beta"]}}}"#;
    std::fs::write(&cfg, original).unwrap();

    let (_o, _e, rc) = crate::run(&["mcp-wrap", "install"], home.path());
    assert_eq!(rc, 0, "setup: both servers must wrap");
    let wrapped = std::fs::read_to_string(&cfg).unwrap();
    assert!(
        !wrapped.contains("\"command\":\"/bin/echo\""),
        "setup: nothing was wrapped: {wrapped}"
    );

    // The defect: a flag nobody implemented, on a config-mutating subcommand.
    let (_o, err, rc) = crate::run(&["mcp-wrap", "uninstall", "--zzbogus"], home.path());
    assert_eq!(rc, 2, "an unimplemented flag must refuse, not act: {err}");
    assert!(
        err.contains("unknown flag '--zzbogus'"),
        "the refusal must name the token: {err}"
    );
    assert_eq!(
        std::fs::read_to_string(&cfg).unwrap(),
        wrapped,
        "the config was rewritten by a flag the command does not implement"
    );

    // A typo CLOSE to the real flag is named, not guessed at silently.
    let (_o, err, rc) = crate::run(&["mcp-wrap", "uninstall", "--al"], home.path());
    assert_eq!(rc, 2);
    assert!(
        err.contains("did you mean `--all`?"),
        "one character from the real flag, and it is not offered: {err}"
    );

    // Two server names is not a bigger request, it is a mistake: the second
    // was silently dropped and the first acted on alone.
    let (_o, err, rc) = crate::run(&["mcp-wrap", "install", "alpha", "beta"], home.path());
    assert_eq!(rc, 2, "a second server name must refuse: {err}");
    assert!(
        err.contains("'beta'"),
        "the refusal must name the argument it could not take: {err}"
    );

    // And the DOCUMENTED flag still does what its help says.
    let (_o, _e, rc) = crate::run(&["mcp-wrap", "uninstall", "--all"], home.path());
    assert_eq!(rc, 0);
    let restored = std::fs::read_to_string(&cfg).unwrap();
    assert!(
        restored.contains("/bin/echo"),
        "`--all` is documented and must still restore everything: {restored}"
    );
}

/// The flag sweep, one level lower again. `mistake list` reads exactly one flag
/// and discarded every other spelling in silence — so `--jsonl`, or `--json=x`,
/// listed the human table at exit 0 to a caller that had asked for JSON.
/// Nothing here mutates; the cost is a wrong answer rather than a wrong action,
/// and it is still the answer the rest of the CLI refuses to give.
#[test]
fn mistake_list_refuses_a_flag_it_does_not_implement() {
    let home = crate::TempHome::new("mistake-flag");
    for form in ["--zzbogus", "--jsonl"] {
        let (out, err, rc) = crate::run(&["mistake", "list", form], home.path());
        assert_eq!(rc, 2, "`{form}` was accepted; stdout was: {out}");
        assert!(
            err.contains(&format!("unknown flag '{form}'")),
            "the refusal must name the token: {err}"
        );
        assert!(
            err.contains("tokenade mistake list"),
            "the refusal must name the command it came from: {err}"
        );
    }
    // The real flag, the bare command, and the implicit `list` all still work.
    let (out, _e, rc) = crate::run(&["mistake", "list", "--json"], home.path());
    assert_eq!(rc, 0, "`--json` is a real flag here");
    assert!(
        out.trim_start().starts_with('['),
        "expected JSON, got: {out}"
    );
    let (_o, _e, rc) = crate::run(&["mistake"], home.path());
    assert_eq!(rc, 0, "a bare `mistake` lists");
    let (_o, _e, rc) = crate::run(&["mistake", "list"], home.path());
    assert_eq!(rc, 0);
}

/// A machine reader asking for JSON must not be handed the human table at
/// exit 0. `mcp-wrap report` has no `--json`, and said so by ignoring it.
#[test]
fn mcp_wrap_report_refuses_a_flag_it_does_not_implement() {
    let home = crate::TempHome::new("mcp-wrap-report-flag");
    let (out, err, rc) = crate::run(&["mcp-wrap", "report", "--json"], home.path());
    assert_eq!(rc, 2, "stdout was: {out}");
    assert!(
        err.contains("unknown flag '--json'"),
        "the refusal must name the token: {err}"
    );
    // The way out has to lead somewhere: the referral is a real help page.
    let (help, _e, rc) = crate::run(&["mcp-wrap", "report", "--help"], home.path());
    assert_eq!(rc, 0);
    assert!(
        help.contains("mcp-wrap report"),
        "the refusal points at a help page that does not describe it: {help}"
    );
}

/// The strip path returns from `cmd_auto` before the recording at the bottom
/// of it, so `tokenade read <source>` folded 8 968 tokens to 3 839 and wrote
/// NOTHING to the ledger. The Read HOOK credits the same bytes
/// (`read_code_strip_lite`), so one rule behind two doors disagreed about
/// whether the saving had happened — it counted when the agent opened the file
/// and vanished when it asked us to.
#[test]
fn the_source_strip_books_what_it_saved() {
    let home = crate::TempHome::new("read-source-credit");
    let f = home.path().join("svc.go");
    let mut body: String = (0..400)
        .map(|i| format!("// what handler {i} does, at some length so it counts\nfunc handler{i}() {{ return }}\n"))
        .collect();
    body.push_str(
        "// the pelican reconciler drains the queue\nfunc pelicanReconciler() { drainQueue() }\n",
    );
    std::fs::write(&f, &body).unwrap();
    let path = f.to_str().unwrap();

    let (_o, _e, rc) = crate::run(&["read", path], home.path());
    assert_eq!(rc, 0);
    let (_o, _e, rc) = crate::run(
        &["read", path, "--prompt", "what does pelicanReconciler do"],
        home.path(),
    );
    assert_eq!(rc, 0);

    let log = std::fs::read_to_string(home.path().join(".tokenade").join("gain.jsonl"))
        .unwrap_or_default();
    let rows: Vec<serde_json::Value> = log
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("valid JSON record"))
        .collect();
    for want in ["auto:code-strip", "auto:code-strip:scoped"] {
        let row = rows
            .iter()
            .find(|r| r["label"] == want)
            .unwrap_or_else(|| panic!("no `{want}` row. Ledger:\n{log}"));
        let before = row["before"].as_u64().unwrap_or(0);
        let after = row["after"].as_u64().unwrap_or(0);
        assert!(
            before > after && after > 0,
            "`{want}` must book a real fold, got {before}->{after}"
        );
    }
    // The scoped row has to claim MORE than the plain one, or retrieval is
    // being credited with the delivery it replaced.
    let saved = |label: &str| -> u64 {
        rows.iter()
            .find(|r| r["label"] == label)
            .map(|r| r["before"].as_u64().unwrap_or(0) - r["after"].as_u64().unwrap_or(0))
            .unwrap_or(0)
    };
    assert!(
        saved("auto:code-strip:scoped") > saved("auto:code-strip"),
        "an answer is smaller than the file it came from: {} vs {}",
        saved("auto:code-strip:scoped"),
        saved("auto:code-strip")
    );
}

/// A blank `--prompt` is not a question. `--prompt "$Q"` with `Q` unset
/// expands to an empty string, and treating that as an ask sends the extractor
/// a query with no anchor — which then declines, and the delivery comes back
/// under "nothing in this source answers that question" when nothing was asked.
#[test]
fn a_blank_question_is_not_a_question() {
    let home = crate::TempHome::new("read-blank-prompt");
    let f = home.path().join("svc.go");
    let body: String = (0..400)
        .map(|i| format!("// what handler {i} does, at some length so it counts\nfunc handler{i}() {{ return }}\n"))
        .collect();
    std::fs::write(&f, &body).unwrap();
    let path = f.to_str().unwrap();

    let (whole, _e, rc) = crate::run(&["read", path], home.path());
    assert_eq!(rc, 0);
    let (blank, _e, rc) = crate::run(&["read", path, "--prompt", "   "], home.path());
    assert_eq!(rc, 0);
    assert_eq!(
        whole, blank,
        "a whitespace-only prompt must deliver exactly what no prompt delivers"
    );
    assert!(
        !blank.contains("nothing in this source answers"),
        "nothing was asked, so nothing went unanswered: {}",
        blank.lines().take(2).collect::<Vec<_>>().join(" / ")
    );
}

/// The banner has to count what is BELOW it. `d.after` describes the whole
/// stripped file, so reusing it for a scoped extract announces a 3 839-token
/// delivery above 393 tokens of answer — the reader sizes the rest of the turn
/// on a number that is an order of magnitude wrong.
#[test]
fn the_scoped_banner_counts_what_follows() {
    let home = crate::TempHome::new("read-scoped-banner");
    let f = home.path().join("svc.go");
    let mut body: String = (0..400)
        .map(|i| format!("// what handler {i} does, at some length so it counts\nfunc handler{i}() {{ return }}\n"))
        .collect();
    body.push_str(
        "// the pelican reconciler drains the queue\nfunc pelicanReconciler() { drainQueue() }\n",
    );
    std::fs::write(&f, &body).unwrap();

    let (answer, _e, rc) = crate::run(
        &[
            "read",
            f.to_str().unwrap(),
            "--prompt",
            "what does pelicanReconciler do",
        ],
        home.path(),
    );
    assert_eq!(rc, 0);
    let banner = answer
        .lines()
        .find(|l| l.contains("read-code-strip"))
        .unwrap_or_else(|| panic!("no banner in: {answer}"));
    // "(8968→393 tokens, -95%)" — take the arrow's right-hand side.
    let claimed: usize = banner
        .split('\u{2192}')
        .nth(1)
        .and_then(|t| t.split_whitespace().next())
        .and_then(|t| t.parse().ok())
        .unwrap_or_else(|| panic!("no token count in: {banner}"));
    // ~4 chars per token is the coarse rule the estimator follows; a count that
    // describes the whole file instead of the answer misses by 10x, not by 2x.
    let delivered = answer.len() / 4;
    assert!(
        claimed <= delivered.max(64) * 3,
        "banner claims {claimed} tokens over {} bytes of answer: {banner}",
        answer.len()
    );
}

/// #287. The host cuts a Bash result at its own cap and marks the cut nowhere,
/// so a passthrough result reaches the model ending mid-token with nothing to
/// say the tail is missing. Drives the real binary through the PostToolUse
/// hook, so it fails if the call site is removed and the helper is merely left
/// behind — the same lesson, third time.
#[test]
fn the_bash_hook_names_a_host_cut_it_cannot_prevent() {
    let home = crate::TempHome::new("host-cut-advisory");
    let payload = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": {"command": "strings -n 8 /usr/bin/python3"},
        "tool_response": {"stdout": "q".repeat(40_000), "stderr": ""}
    })
    .to_string();
    let (out, err, rc) =
        crate::run_hook_as(Some("claude-code"), &["hookwrap"], home.path(), &payload);
    assert_eq!(rc, 0, "{err}");
    assert!(
        out.contains("cuts a command result at 30000 characters"),
        "the advisory must name the host's own number, got: {out}"
    );
    assert!(
        out.contains("additionalContext"),
        "it must ride the hook's own channel: {out}"
    );
}

/// The mirror: a result that fits must cost nothing at all.
#[test]
fn the_bash_hook_says_nothing_about_a_result_that_fits() {
    let home = crate::TempHome::new("host-cut-quiet");
    let payload = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": {"command": "echo hi"},
        "tool_response": {"stdout": "q".repeat(1_000), "stderr": ""}
    })
    .to_string();
    let (out, _err, _rc) =
        crate::run_hook_as(Some("claude-code"), &["hookwrap"], home.path(), &payload);
    assert!(!out.contains("cuts a command result"), "{out}");
}

/// #287, the OTHER terminal exit. The passthrough guard above only reaches the
/// below-threshold return; a payload the fold DOES improve travels all the way
/// to the log-only tail, and the note has to be there too. Without this the
/// second call site can be deleted with every test still green.
#[test]
fn the_bash_hook_names_a_host_cut_on_output_it_could_fold() {
    let home = crate::TempHome::new("host-cut-folded");
    let body: String = (0..900)
        .map(|i| format!("2026-08-26T10:00:00Z INFO worker did the thing number {i}\n"))
        .collect();
    assert!(body.chars().count() > 30_000, "{}", body.chars().count());
    let payload = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": {"command": "journalctl -u worker"},
        "tool_response": {"stdout": body, "stderr": ""}
    })
    .to_string();
    let (out, err, rc) =
        crate::run_hook_as(Some("claude-code"), &["hookwrap"], home.path(), &payload);
    assert_eq!(rc, 0, "{err}");
    assert!(
        out.contains("cuts a command result at 30000 characters"),
        "the foldable path must carry the note too, got: {out}"
    );
}

/// No surface hands the agent more than a context window.
///
/// Three separate defects in three days, all the same shape — a delivery path
/// that compacts but never asks whether what it produced can be RECEIVED:
///
///   * `tokenade read` on a big source file: 293 904 bytes, because the
///     comment-strip cleared its 10 % bar and pre-empted the disclose path;
///   * `tokenade exec` on output the compactors could not fold: 1 048 678, the
///     raw byte cap, on the command that promises the opposite;
///   * `tokenade web <a> <b>`: 122 050 for two pages and 586 101 for six,
///     because the batch's HTML branch never stashed — on the call this
///     product's own guidance tells agents to prefer.
///
/// Each was found by hand, one per sweep. This is the sweep as a test: every
/// surface that takes a large input, fed one, and held to a bound. The bound is
/// deliberately loose — ~150 KB is well over the 25 000-token disclose
/// threshold and well under all three failures — because its job is to catch a
/// path with NO ceiling, not to police the ones that have one.
///
/// The fixtures are unfoldable ON PURPOSE: distinct words, not numbers. A
/// value-folding compactor collapses `req={i}` into one line with a count,
/// which is a correct fold and leaves nothing for the ceiling to do — that is
/// exactly what hid the `exec` defect until the fixture changed.
/// No surface that shows you code shows you a secret in it.
///
/// Redaction is applied per call site — nine of them in `code_intel` alone —
/// and per-site handling of one rule is how a new surface arrives uncovered.
/// The existing probe checks exactly ONE path (the PostToolUse hook, per
/// agent, in `compat_matrix`), so the commands an agent is TOLD to run instead
/// of reading files had no sweep at all. They pass today; this is what keeps
/// them passing, and what covers the surface added tomorrow.
#[test]
fn e2e_no_code_surface_reveals_a_secret() {
    let h = TempHome::new("secret-sweep");
    // A key-shaped literal that is not the canonical AWS example — an
    // allow-listed sample would make every probe pass for the wrong reason.
    const SECRET: &str = "AKIA7QZ3MPLW2XKD9RTV";
    let sub = h.path().join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(
        sub.join("creds.rs"),
        format!(
            "// helper for the deploy path\n\
             pub const AWS_ACCESS_KEY_ID: &str = \"{SECRET}\";\n\
             pub fn build_client() -> String {{ AWS_ACCESS_KEY_ID.to_string() }}\n"
        ),
    )
    .unwrap();
    std::fs::write(
        h.path().join("main.rs"),
        "pub fn main() { let _ = build_client(); }\n",
    )
    .unwrap();

    // Warm the index so `query`/`map`/`impact` answer from it.
    let _ = run(&["query", "build_client"], h.path());

    let probes: Vec<(&str, Vec<&str>)> = vec![
        (
            "query with context",
            vec!["query", "AWS_ACCESS_KEY_ID", "--context", "2"],
        ),
        (
            "query --json",
            vec!["query", "AWS_ACCESS_KEY_ID", "--context", "2", "--json"],
        ),
        ("skeleton", vec!["skeleton", "sub/creds.rs"]),
        ("impact", vec!["impact", "sub/creds.rs"]),
        ("map", vec!["map"]),
        ("read", vec!["read", "sub/creds.rs"]),
    ];
    let mut saw_the_line = false;
    for (name, args) in probes {
        let (out, err, _code) = run(&args, h.path());
        assert!(
            !out.contains(SECRET) && !err.contains(SECRET),
            "{name} printed the secret verbatim"
        );
        // At least one probe must actually REACH the line, or the sweep is
        // asserting that six commands printed nothing.
        if out.contains("AWS_ACCESS_KEY_ID") && out.contains("redacted") {
            saw_the_line = true;
        }
    }
    assert!(
        saw_the_line,
        "no probe rendered the declaring line at all — the sweep proves nothing"
    );

    // `--context` in JSON mode used to be accepted, echoed back as
    // `"context_lines": 2`, and silently ignored — a payload naming a setting
    // its own output did not honour. Asserted here so the surface stays
    // secret-bearing: without it, a future change could drop `context` and
    // this sweep would keep passing for want of anything to leak.
    let (out, _e, _c) = run(
        &["query", "AWS_ACCESS_KEY_ID", "--context", "1", "--json"],
        h.path(),
    );
    let v: serde_json::Value = serde_json::from_str(&out).expect("json");
    let ctx = v["hits"][0]["context"]
        .as_str()
        .expect("--context must reach the JSON payload it already advertises");
    assert!(ctx.contains("helper for the deploy path"), "{ctx}");
    assert!(!ctx.contains(SECRET));
}

#[test]
fn e2e_no_surface_delivers_more_than_the_ceiling() {
    const CEILING: usize = 150_000;
    let h = TempHome::new("delivery-ceiling");

    // base-26 words: deterministic, and no two alike.
    let w = |mut n: usize| -> String {
        let a = b"abcdefghijklmnopqrstuvwxyz";
        let mut s = String::new();
        for _ in 0..6 {
            s.push(a[n % 26] as char);
            n /= 26;
        }
        s
    };
    let log: String = (0..60_000)
        .map(|i| {
            format!(
                "2026-08-28T10:00:00Z INFO worker={} req={} path=/api/{}\n",
                w(i),
                w(i * 7),
                w(i * 13)
            )
        })
        .collect();
    // Headings AND long prose under each. Both halves are load-bearing: a page
    // of 12 000 one-line sections distils to 1 853 tokens — under the stash
    // threshold, so the batch defect does not show — and a page of long prose
    // with few headings gives the outline mode nothing to collapse. Measured on
    // this shape with the batch-stash fix reverted: one page 2 089 bytes, two pages
    // 28 754. Real Wikipedia articles have both, which is why they showed it.
    let page = |tag: usize| -> String {
        let mut parts = String::new();
        for i in 0..1_500 {
            parts.push_str(&format!("<h2>{} {}</h2>", w(i * 3 + tag), w(i * 5 + tag)));
            for p in 0..3 {
                parts.push_str("<p>");
                for j in 0..45 {
                    parts.push_str(&w(i * 97 + p * 31 + j + tag));
                    parts.push(' ');
                }
                parts.push_str("</p>");
            }
        }
        format!("<html><body>{parts}</body></html>")
    };
    let log_path = h.path().join("varied.log");
    let a = h.path().join("a.html");
    let b = h.path().join("b.html");
    std::fs::write(&log_path, &log).unwrap();
    std::fs::write(&a, page(0)).unwrap();
    std::fs::write(&b, page(7)).unwrap();
    for p in [&log_path, &a, &b] {
        let n = std::fs::metadata(p).unwrap().len();
        assert!(
            n as usize > CEILING * 2,
            "{p:?} is {n} bytes — the fixture must be far over the bound or the \
             probe proves nothing"
        );
    }

    // A big SOURCE file too: the comment-strip ladder is its own delivery
    // path, and the `read` defect was a rung of it clearing a percentage bar while handing
    // back 293 904 bytes.
    let rs_path = h.path().join("big.rs");
    let rs: String = (0..12_000)
        .map(|i| {
            format!(
                "pub fn {}(x: u32) -> u32 {{\n    // helper {i} {}\n    let y = x + {i};\n    y * 2\n}}\n",
                w(i),
                w(i * 3)
            )
        })
        .collect();
    std::fs::write(&rs_path, &rs).unwrap();
    let rp = rs_path.to_str().unwrap().to_string();
    let lp = log_path.to_str().unwrap().to_string();
    let ap = a.to_str().unwrap().to_string();
    let bp = b.to_str().unwrap().to_string();
    let script = "\nimport string\nA = string.ascii_lowercase\ndef w(n):\n    s = ''\n    for _ in range(6):\n        s += A[n % 26]\n        n //= 26\n    return s\nfor i in range(60000):\n    print(f'2026-08-28T10:00:00Z INFO worker={w(i)} req={w(i*7)} path=/api/{w(i*13)}')\n";
    let probes: Vec<(&str, Vec<&str>)> = vec![
        ("read a log", vec!["read", &lp]),
        ("read a source file", vec!["read", &rp]),
        ("read an html page", vec!["read", &ap]),
        ("wrap cat", vec!["wrap", "cat"]),
        ("exec", vec!["exec", "--lang", "python", "--script", script]),
        ("web, batched", vec!["web", &ap, &bp]),
    ];
    for (name, args) in probes {
        // `wrap` takes its command as ONE argument.
        let joined;
        let args: Vec<&str> = if name == "wrap cat" {
            joined = format!("cat {lp}");
            vec!["wrap", &joined]
        } else {
            args
        };
        let (out, err, _code) = run(&args, h.path());
        assert!(
            out.len() < CEILING,
            "{name} delivered {} bytes (bound {CEILING}): a surface with no \
             ceiling. stderr: {}",
            out.len(),
            &err[..err.len().min(200)]
        );
    }

    // The web BATCH is not held to a byte bound here. Its cost scales with the
    // number of pages, which is the caller's, so any honest bound passes for
    // two pages and fails for twenty. It is held to the right property — one
    // summary per page, exactly as a single page gets — in
    // `web::e2e_web_batch_summarises_each_page`.
}

// A recorded dead end must reach the agent at session boot.
//
// `mistakes` opens by saying the point is that the agent "doesn't cheerfully
// re-propose it next session", and `load` had exactly one caller: the
// `tokenade mistake list` subcommand a human has to type. Written, and never
// read where it mattered.
//
// Both halves are asserted, and the second is the one that makes the wire
// safe: this file's own history removed an additionalContext block for
// costing 560-710 cache-creation tokens a session with no behavioural gain,
// so a user who has recorded nothing must pay nothing.

/// `tokenade style <preset>` must name the file it wrote. It printed
/// `<repo>/.tokenade/style.json` while `style::save` writes a HOME-global path,
/// so the command reported success and pointed at a file — and a directory —
/// that did not exist. It must also say the setting is machine-wide, because
/// naming a path inside the repository implies it is not.
#[test]
fn style_reports_the_file_it_actually_wrote() {
    let home = crate::TempHome::new("style-path");
    let target = home.path().join("style-here.json");
    let bin = binary();
    let mut c = Command::new(&bin);
    c.args(["style", "chat"]);
    let out = e2e_env(&mut c, home.path())
        .env("TOKENADE_STYLE_PATH", &target)
        .output()
        .expect("spawn tokenade");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        target.exists(),
        "the setting was not persisted at all:\n{stdout}"
    );
    assert!(
        stdout.contains(&target.display().to_string()),
        "the message names a path other than the one written ({}):\n{stdout}",
        target.display()
    );
    assert!(
        stdout.contains("every project on this machine"),
        "a machine-wide setting must say so:\n{stdout}"
    );
}

/// Refusing a re-read has to be cheaper than re-serving. The gate upstream
/// measures the FILE's byte size, which for a document is the wrong quantity:
/// a container is large on disk and small once extracted. `pptx.pptx` is
/// 33 467 bytes holding 62 tokens of text, and the deny message costs 102 — so
/// saying "you already have it" made the context BIGGER than serving the deck
/// again.
#[test]
fn doc_dedup_refuses_only_when_refusing_is_cheaper() {
    let home = crate::TempHome::new("doc-dedup-floor");
    let fx = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/docs-bench/fixture/docs-audit");
    let decision_on_second_read = |file: &str| -> String {
        let path = fx.join(file);
        assert!(path.exists(), "fixture missing: {}", path.display());
        let payload = format!(
            "{{\"session_id\":\"dd-{file}\",\"tool_name\":\"Read\",\"tool_input\":{{\"file_path\":\"{}\"}}}}",
            path.display()
        );
        let once = |p: &str| -> String {
            let bin = binary();
            let mut c = Command::new(&bin);
            c.args(["hookwrap-pretool"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut child = e2e_env(&mut c, home.path())
                .env_remove("TOKENADE_HOOK_DISABLED")
                .spawn()
                .expect("spawn tokenade");
            use std::io::Write;
            child
                .stdin
                .as_mut()
                .unwrap()
                .write_all(p.as_bytes())
                .unwrap();
            drop(child.stdin.take());
            let out = child.wait_with_output().expect("wait");
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        once(&payload);
        once(&payload)
    };
    // A deck whose whole extracted text is smaller than the refusal.
    let tiny = decision_on_second_read("pptx.pptx");
    // `deny` is also how the hook SERVES extracted text, so the absence of a
    // deny is the wrong thing to assert: what must be absent is the dedup
    // refusal, and what must be present is the text served again.
    assert!(
        !tiny.contains("doc-dedup unchanged"),
        "a 62-token deck was refused by a 102-token message:\n{tiny}"
    );
    assert!(
        tiny.contains("BEGIN EXTRACTED TEXT"),
        "the deck should simply be served again, which is cheaper:\n{tiny}"
    );
    // A document where the refusal genuinely saves something.
    let big = decision_on_second_read("pdf_large.pdf");
    assert!(
        big.contains("doc-dedup unchanged"),
        "a real document must still be deduped:\n{big}"
    );
}

/// `filter --help` has to name the layer it does NOT run. Its only stated
/// difference from `wrap` was who runs the command, and an agent reading that
/// takes the two numbers as interchangeable. They are not: on output no
/// per-command rule shrinks, `wrap` still folds it into a stash and `filter`
/// returns it whole. Measured 2026-09-02 on `cat /proc/interrupts` — 81 rows
/// of 306 bytes — `filter` saved 0% and `wrap` 83%.
#[test]
fn filter_help_names_the_fold_it_does_not_run() {
    let home = crate::TempHome::new("filter-help");
    let (stdout, _err, code) = run(&["filter", "--help"], home.path());
    assert_eq!(code, 0, "filter --help must exit 0: {stdout}");
    // Assert the CLAIM, not the word: `stash` also appears in "filter does not
    // stash", so a help that dropped the half about `wrap` still contained it.
    assert!(
        stdout.contains("recoverable stash"),
        "the help does not say `wrap` folds into a recoverable stash:\n{stdout}"
    );
    assert!(
        stdout.contains("per-command compactor alone"),
        "the help does not say what its own number covers:\n{stdout}"
    );
    // And it still says the older difference, which remains true.
    assert!(
        stdout.contains("RUNS the command for you"),
        "the help lost the difference it always stated:\n{stdout}"
    );
}

/// The bypass nudge reaches the agent. Its own channel is the hook's stderr,
/// which a PostToolUse handler does not surface — 885 recorded fires on the
/// install this was found on and not one ever seen. It must travel in
/// `hookSpecificOutput.additionalContext`, like every nudge that does arrive,
/// and it must do so ONCE per session so it cannot flood the context it is
/// there to protect.
#[test]
fn the_bypass_nudge_travels_in_additional_context_once_per_session() {
    let home = crate::TempHome::new("bypass-nudge");
    let proj = home.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let payload = |sess: &str| {
        format!(
            "{{\"session_id\":\"{sess}\",\"tool_name\":\"Bash\",\"tool_input\":{{\"command\":\"TOKENADE_HOOK_DISABLED=1 echo x\"}},\"tool_response\":{{\"stdout\":\"x\"}}}}"
        )
    };
    let run = |sess: &str| -> String {
        let bin = binary();
        let mut c = Command::new(&bin);
        c.args(["hookwrap"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = e2e_env(&mut c, home.path())
            .env_remove("TOKENADE_HOOK_DISABLED")
            .current_dir(&proj)
            .spawn()
            .expect("spawn tokenade");
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(payload(sess).as_bytes())
            .unwrap();
        drop(child.stdin.take());
        let out = child.wait_with_output().expect("wait");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    // The counter has to pass its threshold before the nudge fires at all.
    let mut delivered = 0usize;
    let mut first_hit = 0usize;
    for i in 1..=14 {
        if run("sess-a").contains("additionalContext") {
            delivered += 1;
            if first_hit == 0 {
                first_hit = i;
            }
        }
    }
    assert!(
        first_hit > 0,
        "the nudge never reached the agent in 14 bypasses — stderr is not a delivery channel here"
    );
    assert_eq!(
        delivered, 1,
        "delivered {delivered} times; a nudge that repeats floods the context it protects"
    );
    // The redirection has to have taken, or this test measured the real store.
    assert!(
        home.path().join(".tokenade").exists(),
        "HOME was not redirected: the probe wrote into the real store"
    );
}

#[test]
fn e2e_a_recorded_mistake_reaches_the_agent_at_session_boot() {
    let h = TempHome::new("mistake-boot");
    let proj = h.path().join("proj");
    std::fs::create_dir_all(proj.join(".git")).unwrap();
    std::fs::write(proj.join("a.rs"), "pub fn alpha() {}\n").unwrap();
    let payload = format!(
        "{{\"session_id\":\"s1\",\"source\":\"startup\",\"cwd\":\"{}\"}}",
        proj.display()
    );
    // The hook is a no-op under `TOKENADE_HOOK_DISABLED`, which `e2e_env` sets
    // for every other spawn — this is the one test that must not have it.
    let boot = |home: &std::path::Path| -> String {
        let bin = binary();
        let mut c = Command::new(&bin);
        c.args(["hookwrap-sessionstart"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = e2e_env(&mut c, home)
            .env_remove("TOKENADE_HOOK_DISABLED")
            .current_dir(&proj)
            .spawn()
            .expect("spawn tokenade");
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
        drop(child.stdin.take());
        let out = child.wait_with_output().unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    };

    // Nothing recorded: the boot says nothing about mistakes.
    let quiet = boot(h.path());
    assert!(
        !quiet.contains("tokenade:mistakes"),
        "an empty ledger cost the session tokens: {quiet}"
    );

    let (_o, err, code) = run(
        &["mistake", "add", "tried a global cache, caused races"],
        h.path(),
    );
    assert_eq!(code, 0, "{err}");

    let loud = boot(h.path());
    assert!(
        loud.contains("tokenade:mistakes"),
        "a recorded dead end never reached the agent: {loud}"
    );
    assert!(
        loud.contains("tried a global cache, caused races"),
        "the reminder does not carry what was recorded: {loud}"
    );
}

/// SessionStart writes project files via `ensure` and must not re-inject
/// the full rules lecture as `additionalContext` on a normal (non-quota)
/// session. The file is the sole channel.
#[test]
fn e2e_sessionstart_does_not_put_scaffold_in_additional_context() {
    let h = TempHome::new("sessionstart-no-scaffold-ac");
    let proj = h.path().join("proj");
    std::fs::create_dir_all(proj.join(".git")).unwrap();
    std::fs::write(proj.join("a.rs"), "pub fn alpha() {}\n").unwrap();
    let payload = format!(
        "{{\"session_id\":\"s-no-ac\",\"source\":\"startup\",\"cwd\":\"{}\"}}",
        proj.display()
    );
    let bin = binary();
    let mut c = Command::new(&bin);
    c.args(["hookwrap-sessionstart"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = e2e_env(&mut c, h.path())
        .env_remove("TOKENADE_HOOK_DISABLED")
        .current_dir(&proj)
        .spawn()
        .expect("spawn tokenade");
    use std::io::Write;
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert_eq!(out.status.code(), Some(0), "SessionStart failed: {stdout}");
    assert!(
        !stdout.contains("Explore code with the tokenade CLI"),
        "SessionStart additionalContext carries the rules lecture: {stdout}"
    );
    assert!(
        !stdout.contains("Keep output lean"),
        "SessionStart additionalContext carries the brevity dump: {stdout}"
    );
    assert!(
        !stdout.contains("scaffold_block"),
        "SessionStart additionalContext carries a scaffold dump: {stdout}"
    );
}

/// The wiring, not just the wording. `explicit_timeout` is one binding
/// feeding one sentence; replacing it with a constant left every unit test on
/// the message green, because those pass the flag in by hand. Only a real run
/// tells the two apart, and the sentence is what the agent reads when its
/// script is killed.
#[test]
fn the_exec_timeout_notice_names_the_flag_and_knows_whose_ceiling_it_is() {
    let home = TempHome::new("exec-timeout");
    let run = |args: &[&str]| -> String {
        let bin = binary();
        let mut c = Command::new(&bin);
        c.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
        let out = e2e_env(&mut c, home.path())
            .output()
            .expect("spawn tokenade");
        String::from_utf8_lossy(&out.stderr).to_string()
    };

    // Only the explicit case is spawned: the implicit one would have to wait
    // out the 30 s default to prove anything, and the mutation this guard
    // exists for (`explicit_timeout = false`) is caught from this side —
    // a run that DID pass the flag must never be told it took the default.
    let explicit = run(&[
        "exec",
        "--lang",
        "bash",
        "--script",
        "sleep 3",
        "--timeout",
        "1",
    ]);
    assert!(
        explicit.contains("timed out"),
        "the script was not killed: {explicit}"
    );
    assert!(
        explicit.contains("--timeout"),
        "the remedy is not named: {explicit}"
    );
    assert!(
        !explicit.contains("the default"),
        "an explicit --timeout was reported as the default: {explicit}"
    );
}

/// A build/test run whose failure the exit code swallowed must be NAMED.
///
/// Through the binary on purpose. The unit guards next to
/// `disclose_success_with_failure_evidence` pin the marker set and the
/// allowlist, but neither exercises the WIRING — the notice is emitted from
/// `run_child`, and a call deleted there leaves every unit test green. That is
/// the same shape: only a real run kills that mutation.
#[test]
fn a_pipeline_that_hid_a_failure_says_so_through_the_binary() {
    let home = TempHome::new("swallowed-failure");
    let run = |cmd: &str| -> (String, Option<i32>) {
        let bin = binary();
        let mut c = Command::new(&bin);
        c.args(["wrap", cmd])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let out = e2e_env(&mut c, home.path())
            .output()
            .expect("spawn tokenade");
        (
            String::from_utf8_lossy(&out.stderr).to_string(),
            out.status.code(),
        )
    };

    // `cargo` is a real stage leader and `--version` is cheap; the pipe makes
    // the whole thing exit 0 while the output reports a failure. This is the
    // measured shape (`cargo test … 2>&1 | tail -40`) without the build.
    let (err, code) = run(
        "cargo --version >/dev/null; printf 'test result: FAILED. 2 passed; 1 failed\\n' | tail -1",
    );
    assert_eq!(code, Some(0), "the premise is an exit-0 command");
    assert!(
        err.contains("exited 0") && err.contains("reports a failure"),
        "the swallowed failure was not disclosed: {err:?}"
    );
    assert!(
        err.contains("LAST stage"),
        "a piped command must be told WHY its status is 0: {err:?}"
    );

    // In a session the lesson is said ONCE (523 copies, ≈ 122 KB, in one
    // working session); every later swallow still names its evidence.
    let in_session = |cmd: &str| -> String {
        let mut c = Command::new(binary());
        c.args(["wrap", cmd])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let out = e2e_env(&mut c, home.path())
            .env("TOKENADE_SESSION", "swallow-once")
            .output()
            .expect("spawn tokenade");
        String::from_utf8_lossy(&out.stderr).to_string()
    };
    let failing =
        "cargo --version >/dev/null; printf 'test result: FAILED. 2 passed; 1 failed\\n' | tail -1";
    let first = in_session(failing);
    assert!(
        first.contains("LAST stage"),
        "the first time explains: {first:?}"
    );
    let again = in_session(failing);
    assert!(
        !again.contains("LAST stage") && again.contains("\"test result: FAILED\""),
        "later, the evidence alone: {again:?}"
    );

    // The exit code is disclosed, never rewritten: changing it would break
    // `cmd | head`, where the producer dies on SIGPIPE every single time.
    let (green, gcode) = run("cargo --version | tail -1");
    assert_eq!(gcode, Some(0));
    assert!(
        !green.contains("reports a failure"),
        "a genuinely green run must stay silent: {green:?}"
    );

    // A command that ALREADY reports its failure needs no notice: the status
    // is not lying, and saying "exited 0" about exit 1 would be false.
    let (loud, lcode) = run("cargo --version >/dev/null; printf 'error[E0308]: nope\\n'; exit 1");
    assert_eq!(lcode, Some(1));
    assert!(
        !loud.contains("exited 0"),
        "a command that reports its own failure must stay silent: {loud:?}"
    );

    // No pipe, no pipeline explanation. `cmd; true` swallows just as well, and
    // blaming a pipeline that is not there would send the reader looking for
    // a stage that does not exist.
    let (nopipe, ncode) = run("cargo --version >/dev/null; printf 'error[E0308]: nope\\n'; true");
    assert_eq!(ncode, Some(0));
    assert!(
        nopipe.contains("reports a failure") && !nopipe.contains("LAST stage"),
        "an unpiped swallow must be named without inventing a pipeline: {nopipe:?}"
    );

    // `exec` swallows it identically and is the busiest command there is
    // (41% of recorded tokenade invocations), so the notice must reach it too.
    // Its own call site: deleting it leaves every wrap assertion above green.
    let bin = binary();
    let mut c = Command::new(&bin);
    c.args([
        "exec",
        "--lang",
        "bash",
        "--script",
        "cargo --version >/dev/null; printf 'test result: FAILED. 1 failed\\n' | tail -1",
    ])
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let e = e2e_env(&mut c, home.path())
        .output()
        .expect("spawn tokenade");
    let etxt = String::from_utf8_lossy(&e.stderr).to_string();
    assert_eq!(e.status.code(), Some(0), "the premise is an exit-0 script");
    assert!(
        etxt.contains("exited 0") && etxt.contains("reports a failure"),
        "exec must disclose the same swallow as wrap: {etxt:?}"
    );

    // Searching FOR the marker is not running the tool that emits it.
    let (grep, _) = run("printf 'test result: FAILED\\n' | grep FAILED");
    assert!(
        !grep.contains("reports a failure"),
        "grep printing the marker is not a failed build: {grep:?}"
    );
}

/// Zero bytes on stdin is not input, and what tokenade answers depends on
/// whether stdin was ASKED for.
///
/// Before: `tokenade read` with no argument and an empty stdin returned
/// `§ref:e3b0c44298fc§ (~0 tokens / 0 bytes, identical to a prior call — ask
/// it what you need)` and exit 0 — a recovery pointer to the empty string,
/// inviting the agent to spend a turn questioning nothing. The `is_terminal()`
/// guard already covered a human at a prompt; an agent harness hands over a
/// non-tty stdin that is simply empty, which is the case that leaked.
#[test]
fn empty_stdin_is_not_input_and_says_which_kind_of_nothing_it_was() {
    let home = TempHome::new("read-empty-stdin");
    let run = |args: &[&str], stdin: Option<&str>| -> (String, String, Option<i32>) {
        let bin = binary();
        let mut c = Command::new(&bin);
        c.args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            });
        let mut child = e2e_env(&mut c, home.path())
            .spawn()
            .expect("spawn tokenade");
        if let Some(s) = stdin {
            use std::io::Write as _;
            child.stdin.take().unwrap().write_all(s.as_bytes()).unwrap();
        }
        let out = child.wait_with_output().expect("wait");
        (
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
            out.status.code(),
        )
    };

    // No argument, nothing on stdin → a forgotten path, not a document.
    let (so, se, code) = run(&["read"], None);
    assert_eq!(code, Some(2), "a forgotten path must not report success");
    assert!(
        se.contains("usage: tokenade read"),
        "no usage shown: {se:?}"
    );
    assert!(
        !so.contains("§ref:") && !so.contains("expand-ref"),
        "must not hand back a recovery pointer to nothing: {so:?}"
    );

    // Explicit `-`, and the producer legitimately emitted nothing. That is a
    // result, not a mistake: silence and success, still no pointer.
    let (so, _se, code) = run(&["read", "-"], Some(""));
    assert_eq!(code, Some(0), "an empty producer is not a usage error");
    assert!(
        !so.contains("§ref:") && !so.contains("expand-ref"),
        "empty input must not produce a pointer either: {so:?}"
    );

    // Real content on stdin is untouched by any of this.
    let (so, _se, code) = run(&["read", "-"], Some("alpha\nbeta\n"));
    assert_eq!(code, Some(0));
    assert!(
        so.contains("alpha") && so.contains("beta"),
        "lost content: {so:?}"
    );
}

/// `web` was the one command of fifteen that answered a bare invocation with
/// silence and success, and the only one that turned a mistyped flag into a
/// fetch target.
///
/// Both were measured by surveying every subcommand with no argument and an
/// empty stdin: thirteen answered usage and 2, `map` legitimately maps the
/// current repository, and `web` returned nothing at all with exit 0.
#[test]
fn web_refuses_an_empty_invocation_and_an_unknown_flag() {
    let home = TempHome::new("web-usage");
    let page = home.path().join("p.html");
    std::fs::write(
        &page,
        "<html><body><h1>Hi</h1><p>body text here</p></body></html>",
    )
    .unwrap();
    let run = |args: &[&str]| -> (String, String, Option<i32>) {
        let bin = binary();
        let mut c = Command::new(&bin);
        c.args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        let out = e2e_env(&mut c, home.path())
            .output()
            .expect("spawn tokenade");
        (
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
            out.status.code(),
        )
    };
    let p = page.to_str().unwrap();

    // Nothing to fetch is a forgotten argument, not a job well done.
    let (so, se, code) = run(&["web"]);
    assert_eq!(code, Some(2), "a bare `web` must not report success");
    assert!(se.contains("usage: tokenade web"), "no usage shown: {se:?}");
    assert!(so.trim().is_empty(), "usage belongs on stderr: {so:?}");

    // An unrecognised flag is a mistake, not a URL. It used to be fetched,
    // fail, and let the command exit 0 anyway.
    let (_so, se, code) = run(&["web", "--json", p]);
    assert_eq!(
        code,
        Some(2),
        "an unknown flag must not be fetched then ignored"
    );
    assert!(
        se.contains("unknown flag") && se.contains("--json"),
        "the bad flag must be named: {se:?}"
    );
    assert!(
        !se.contains("fetch/read failed"),
        "the flag must never reach the fetcher: {se:?}"
    );

    // The flags `web` really has, and a real target, are untouched.
    for args in [
        vec!["web", p],
        vec!["web", "--max-tokens", "500", p],
        vec!["web", "--help"],
    ] {
        let (_o, e, c) = run(&args);
        assert_eq!(c, Some(0), "{args:?} must still work: {e:?}");
    }
}

/// `map` was the last command that swallowed an unknown flag.
///
/// Surveyed after `web` was closed: passing `--zzbogus` alongside valid
/// arguments, `read`, `web`, `query`, `impact`, `skeleton`, `exec` and
/// `file-search` all name the bad flag and fail; `map` alone accepted it and
/// exited 0. The cost is not the ignored flag but the silent substitution —
/// `--jsn` is one letter from `--json`, and the caller that asked for JSON got
/// the human map with nothing to say why.
#[test]
fn map_names_an_unknown_flag_instead_of_ignoring_it() {
    let home = TempHome::new("map-unknown-flag");
    let run = |args: &[&str]| -> (String, String, Option<i32>) {
        let bin = binary();
        let mut c = Command::new(&bin);
        c.args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        let out = e2e_env(&mut c, home.path())
            .output()
            .expect("spawn tokenade");
        (
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
            out.status.code(),
        )
    };

    // The shape that motivated this: one letter short of `--json`.
    let (so, se, code) = run(&["map", "--jsn"]);
    assert_eq!(code, Some(2), "a typo'd flag must not report success");
    assert!(
        se.contains("unknown flag") && se.contains("--jsn"),
        "the bad flag must be named: {se:?}"
    );
    assert!(
        !so.contains("files"),
        "a refused invocation must not also print a map: {so:?}"
    );

    // Every flag `map` really has still works, including the one whose value
    // is consumed separately (`-o paths`) — the arm that skips it must not be
    // turned into a rejection.
    for args in [
        vec!["map"],
        vec!["map", "--json"],
        vec!["map", "-o", "paths"],
        vec!["map", "--format", "count"],
        vec!["map", "--help"],
    ] {
        let (_o, e, c) = run(&args);
        assert_eq!(c, Some(0), "{args:?} must still work: {e:?}");
    }

    // #671's positional is now the narrowing it always meant. What stays
    // refused is a SECOND one: answering about one of two paths with a zero
    // exit code is the same silent accept this test exists for.
    let (_o, _e, c) = run(&["map", "src"]);
    assert_eq!(c, Some(0), "a single path is the narrowing now: {_e:?}");
    let (_o, e, c) = run(&["map", "src", "docs"]);
    assert_eq!(c, Some(2), "a second path must be refused: {e:?}");
    assert!(
        e.contains("second positional"),
        "the refusal must name what is wrong: {e:?}"
    );
}

/// `discover` reports WHERE a channel's bytes came back, on both routes.
///
/// The command answers "where would tokenade help me most", and its biggest
/// row was two very different channels summed into one: on this machine `Read`
/// read "9 206 calls, 30 MB" while the split is 6 609 calls returning 2.6 MB in
/// the main session and 2 598 returning 27.3 MB inside subagents — 401 bytes
/// per read against 10 633. The row that should have carried the finding was
/// the row hiding it.
#[test]
fn e2e_discover_splits_a_channel_between_the_main_session_and_its_subagents() {
    let h = TempHome::new("discover-sidechain");
    let logs = h.dir.join(".claude").join("projects").join("p");
    let subs = logs.join("subagents");
    std::fs::create_dir_all(&subs).unwrap();
    let answer = "y".repeat(4 * 1024 * 1024);
    std::fs::write(
        logs.join("main.jsonl"),
        format!(
            "{}\n{}",
            r#"{"isSidechain":false,"type":"assistant","message":{"content":[{"type":"tool_use","id":"m1","name":"Read","input":{}}]}}"#,
            r#"{"isSidechain":false,"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"m1","content":"tiny"}]}}"#
        ),
    )
    .unwrap();
    std::fs::write(
        subs.join("agent-a.jsonl"),
        format!(
            "{}\n{}",
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"s1","name":"Read","input":{}}]}}"#,
            format!(
                r#"{{"isSidechain":true,"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"s1","content":"{answer}"}}]}}}}"#
            )
        ),
    )
    .unwrap();

    let (out, _e, code) = run(&["discover", "--json"], &h.dir);
    assert_eq!(code, 0, "discover --json must answer: {out:.400}");
    let v: serde_json::Value = serde_json::from_str(out.trim()).expect("json");
    let read = v["top_tools"]
        .as_array()
        .expect("top_tools")
        .iter()
        .find(|t| t["tool"] == "Read")
        .unwrap_or_else(|| panic!("no Read row in {out:.400}"));
    assert_eq!(read["calls"], 2, "both calls counted");
    assert_eq!(read["sidechain_calls"], 1, "one of them is the subagent's");
    let total = read["bytes"].as_u64().unwrap();
    let side = read["sidechain_bytes"].as_u64().unwrap();
    assert!(
        side > total / 2 && side < total,
        "the subagent share is a SUBSET of the total, and here the larger part: {side} of {total}"
    );

    let (text, _e2, _c2) = run(&["discover"], &h.dir);
    // The HEADER ROW, not merely the phrase: the sentence above the table
    // explains the column and contains the same words, so asserting the phrase
    // alone passes with the column gone.
    assert!(
        text.lines()
            .any(|l| l.starts_with("| tool") && l.contains("in subagents")),
        "the table must carry the column: {text:.600}"
    );
    assert!(
        text.contains("of the bytes came back inside subagents"),
        "a channel this lopsided must be NAMED, not left to the reader: {text:.900}"
    );
    assert!(
        text.contains("tokenade subagent-brief"),
        "and the finding must name the lever that applies there: {text:.900}"
    );
    // The split is a place to LOOK. A deliberate Read under the disclosure
    // threshold is served whole on purpose, and the amortisation that
    // justifies it was measured in sidechains too — a median of 27 turns
    // follow a big subagent read. A finding that reads as waste sends the
    // reader to tune a threshold a benchmark already set, so the caveat
    // travels with the number.
    assert!(
        text.contains("served whole on purpose"),
        "the finding must say what it is NOT, or it reads as a leak: {text:.900}"
    );
    assert!(
        text.contains("median of 27 more"),
        "and it must carry the measurement that settles it: {text:.900}"
    );
}

/// `tokenade exec` names the shell-quoting mistake instead of leaving the
/// agent with the interpreter's word for it.
///
/// `--script 'print(\"hi\")'` passes the backslashes through, Python answers
/// `unexpected character after line continuation character` — a line
/// continuation nobody wrote — and nothing runs. Measured over 90 days here,
/// 62 exec calls died that way, 59 with the backslash-quote, and the count
/// rose every month (13 / 30 / 45).
#[test]
fn e2e_exec_names_the_backslash_the_shell_should_have_removed() {
    let h = TempHome::new("exec-escape");
    let (out, err, _c) = run(
        &["exec", "--lang", "python", "--script", r#"print(\"hi\")"#],
        &h.dir,
    );
    let all = format!("{out}\n{err}");
    assert!(
        all.contains("line continuation"),
        "the interpreter's own words must still reach the caller: {all:.400}"
    );
    assert!(
        all.contains("backslash-escaped"),
        "and tokenade must name what actually happened: {all:.400}"
    );
    assert!(
        all.contains("--script-file"),
        "and point at a route with no shell quoting in it: {all:.400}"
    );

    // The working form is untouched: no note, and the script runs.
    let (out2, err2, c2) = run(
        &["exec", "--lang", "python", "--script", "print(\"ok\")"],
        &h.dir,
    );
    assert_eq!(c2, 0, "the ordinary form must still run: {out2}{err2}");
    assert!(out2.contains("ok"), "{out2}");
    assert!(
        !format!("{out2}{err2}").contains("backslash-escaped"),
        "a script that ran must not be lectured: {out2}{err2}"
    );
}

/// A destructive command names everything it destroys.
///
/// `evict-stale` deletes rows and unlinks files across every store tokenade
/// owns, and its help opened with "Runs five sweeps" and listed five. The body
/// runs eleven. The four it never mentioned were not the harmless ones: the
/// disclose stash holds the bytes `tokenade expand-ref <hash>` hands back, so
/// a sweep of it turns a recovery pointer into a dead one, and the transcript
/// cache, the per-session files and the image copies are all real state. The
/// same command had this defect one layer down and it was fixed there: its
/// VACUUM loop "named three of the five" stores it sweeps.
///
/// Tied to the SOURCE rather than to a list written beside it: every sweep the
/// function calls must be in the map, and every mapped keyword must appear in
/// the shipped help. Adding a twelfth sweep fails this test until the help
/// says so.
#[test]
fn e2e_evict_stale_help_names_every_sweep_it_runs() {
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lifecycle/maintenance.rs"),
    )
    .expect("maintenance.rs");
    let body = {
        let at = src
            .find("pub(crate) fn cmd_evict_stale")
            .expect("cmd_evict_stale");
        let rest = &src[at..];
        // The function ends at the next top-level `fn` — good enough, and it
        // over-reads rather than under-reads, which is the safe direction for
        // an exhaustiveness check.
        let end = rest[1..].find("\nfn ").map(|i| i + 1).unwrap_or(rest.len());
        &rest[..end]
    };

    // sweep name → what the help must say about it.
    let expected: &[(&str, &[&str])] = &[
        ("sweep_dedup_ref", &["dedup_ref"]),
        ("sweep_chunk_stores", &["file_search", "fold_store"]),
        ("sweep_semantic_db", &["semantic_db"]),
        ("sweep_index_db", &["index_db"]),
        ("sweep_projects_registry", &["projects-registry"]),
        ("sweep_sessions", &["sessions"]),
        ("sweep_image_reads", &["image reads"]),
        ("sweep_transcript_cache", &["transcript cache"]),
        ("sweep_bypass_counter", &["bypass-counter"]),
        ("sweep_disclose_stash", &["disclose stash", "expand-ref"]),
        ("sweep_models", &["--models"]),
        ("vacuum_databases", &["VACUUM"]),
        ("prune_noise", &["gain.jsonl"]),
    ];

    let mut called: Vec<String> = Vec::new();
    for line in body.lines() {
        for name in ["sweep_", "vacuum_databases", "prune_noise"] {
            let mut from = 0usize;
            while let Some(i) = line[from..].find(name) {
                let start = from + i;
                let ident: String = line[start..]
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if line[start + ident.len()..].starts_with('(') && !ident.ends_with("fn") {
                    called.push(ident.clone());
                }
                from = start + name.len();
            }
        }
    }
    called.sort();
    called.dedup();
    assert!(
        called.len() >= 10,
        "the extractor found only {called:?} — it is not reading the function"
    );

    let h = TempHome::new("evict-help");
    let (help, _e, _c) = run(&["evict-stale", "--help"], &h.dir);

    let unmapped: Vec<&String> = called
        .iter()
        .filter(|c| !expected.iter().any(|(n, _)| *n == c.as_str()))
        .collect();
    assert!(
        unmapped.is_empty(),
        "a sweep nobody mapped — say in the help what it deletes, then add it here: {unmapped:?}"
    );
    for (name, words) in expected {
        if !called.iter().any(|c| c == *name) {
            continue; // mapped but no longer called: nothing to promise
        }
        for w in *words {
            assert!(
                help.contains(w),
                "`evict-stale --help` never mentions {w:?}, and the command runs {name}"
            );
        }
    }
    assert!(
        !help.contains("Runs five sweeps"),
        "the help still counts five sweeps while the command runs {}",
        called.len()
    );
}

/// A machine with no hooks at all must be TOLD it has no hooks.
///
/// The scaffold tokenade writes into every project says: "`tokenade health`
/// says whether they are INSTALLED, and names the fix when they are not, so
/// you never have to guess." On a HOME with no agent config the report used to
/// answer that question three times, all green: `OK all agent-config hook
/// paths resolve`, `OK every hook runs the same tokenade binary` — both
/// vacuously true over an empty set — and, for the missing settings.json, an
/// INFO line saying only that it is "not present". Nothing said hooks were
/// missing and nothing named `tokenade install`, on the machine where tokenade
/// was compacting nothing at all.
///
/// The control below is the half that makes it a test of the rule rather than
/// of the emptiness: with hooks really installed, the green lines come back.
#[test]
fn e2e_healthcheck_says_when_no_hooks_are_installed_at_all() {
    let bare = TempHome::new("hc-no-hooks");
    let health = |home: &std::path::Path, agent: &str| -> String {
        let mut c = std::process::Command::new(binary());
        c.args(["healthcheck"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let out = e2e_env(&mut c, home)
            .env("TOKENADE_AGENT", agent)
            .output()
            .expect("run healthcheck");
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    };

    let empty = health(&bare.dir, "claude-code");
    assert!(
        empty.contains("NOT installed") && empty.contains("tokenade install"),
        "an uninstalled machine was not told so, and not given the fix:\n{empty:.1200}"
    );
    for vacuous in [
        "all agent-config hook paths resolve",
        "every hook runs the same tokenade binary",
    ] {
        assert!(
            !empty.contains(vacuous),
            "a green line about an EMPTY set: {vacuous:?}\n{empty:.1200}"
        );
    }

    // Control: the same report on a HOME that DOES carry hooks. Without this
    // the assertions above would pass on a build that had simply stopped
    // printing those lines.
    let wired = TempHome::new("hc-with-hooks");
    std::fs::create_dir_all(wired.dir.join(".claude")).unwrap();
    let exe = binary();
    let cmd = format!(
        "sh -c 'TOKENADE_DEBUG_LOG='\\''/tmp/d.log'\\'' '\\''{}'\\'' hookwrap-pretool 2>/dev/null; exit 0'",
        exe.display()
    );
    let settings = serde_json::json!({
        "hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": cmd}]}]}
    });
    std::fs::write(
        wired.dir.join(".claude/settings.json"),
        serde_json::to_string(&settings).unwrap(),
    )
    .unwrap();
    let installed = health(&wired.dir, "claude-code");
    assert!(
        installed.contains("all agent-config hook paths resolve"),
        "the green line must come back when there ARE hooks:\n{installed:.1200}"
    );
    // The binaries check answers OK or "…and it is not the one on your PATH"
    // depending on which tokenade a bare name resolves to on the machine
    // running the suite — both are answers ABOUT the hooks. What must not come
    // back is the empty-set line.
    assert!(
        !installed.contains("no tokenade hooks in any agent config"),
        "the binaries check saw no hooks on a wired HOME:\n{installed:.1200}"
    );
    assert!(
        !installed.contains("NOT installed"),
        "a wired machine was told it has no hooks:\n{installed:.1200}"
    );

    // The state that is far more common than an empty HOME: a settings.json
    // that exists, carries the user's OWN hooks, and no tokenade hook. Three
    // checks had an opinion there too — paths, agreement and command shape —
    // and all three were green over a set of zero tokenade hooks.
    let theirs = TempHome::new("hc-their-hooks");
    std::fs::create_dir_all(theirs.dir.join(".claude")).unwrap();
    std::fs::write(
        theirs.dir.join(".claude/settings.json"),
        serde_json::to_string(&serde_json::json!({
            "hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [
                {"type": "command", "command": "echo my own hook"}
            ]}]}
        }))
        .unwrap(),
    )
    .unwrap();
    let mine = health(&theirs.dir, "claude-code");
    assert!(
        mine.contains("hookwrap NOT in") && mine.contains("tokenade install"),
        "a machine with someone else's hooks and none of ours was not told:\n{mine:.1200}"
    );
    for vacuous in [
        "all agent-config hook paths resolve",
        "every hook runs the same tokenade binary",
        "installed hook commands match the expected tokenade shape",
    ] {
        assert!(
            !mine.contains(vacuous),
            "a green line about ZERO tokenade hooks: {vacuous:?}\n{mine:.1200}"
        );
    }
    // And what REPLACED them must not be green either. The message alone is
    // not the fix: a row saying "nothing to compare" rendered as `OK` reads as
    // a pass at a glance, which is the whole defect wearing better wording.
    // `--` is the informational marker, `OK` the passing one.
    for empty_row in [
        "none to resolve",
        "nothing to compare",
        "no shape to verify",
    ] {
        let line = mine
            .lines()
            .find(|l| l.contains(empty_row))
            .unwrap_or_else(|| panic!("missing the row for {empty_row:?}:\n{mine:.1200}"));
        assert!(
            line.trim_start().starts_with("--"),
            "an empty subject set must not be a PASS: {line:?}"
        );
    }

    // …and on an agent that does not keep hooks in ~/.claude, a missing
    // settings.json is not a failure at all.
    let other = health(&bare.dir, "codex");
    assert!(
        !other.contains("NOT installed for Claude Code"),
        "a codex machine was failed for a file it never had:\n{other:.1200}"
    );
}

/// One report, one answer about the hook binaries.
///
/// Two checks used to ask the same question: a Claude-only one that read the
/// `sh -c '…'` shape correctly, and a multi-agent one whose private extractor
/// took the first whitespace token — always `sh` — and therefore always
/// answered OK. So `tokenade healthcheck` printed
/// `OK  all agent-config hook paths resolve` three lines above
/// `!!  1 hook command(s) reference a non-existent binary`, about the same
/// binary. The multi-agent check now uses the shared extractor and the
/// duplicate is gone.
#[test]
fn e2e_healthcheck_does_not_answer_twice_about_a_dead_hook_binary() {
    let h = TempHome::new("hc-dead-binary");
    std::fs::create_dir_all(h.dir.join(".claude")).unwrap();
    // Written the way `tokenade install` writes it.
    let cmd = "sh -c 'TOKENADE_DEBUG_LOG='\\''/tmp/d.log'\\'' '\\''/nonexistent/vendor/tokenade'\\'' hookwrap-pretool 2>/dev/null; exit 0'";
    let settings = serde_json::json!({
        "hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": cmd}]}]}
    });
    std::fs::write(
        h.dir.join(".claude/settings.json"),
        serde_json::to_string(&settings).unwrap(),
    )
    .unwrap();

    let (out, err, _c) = run(&["healthcheck"], &h.dir);
    let all = format!("{out}\n{err}");
    assert!(
        all.contains("/nonexistent/vendor/tokenade"),
        "the dead path must be named: {all:.900}"
    );
    assert!(
        !all.contains("all agent-config hook paths resolve"),
        "and the report must not also say they resolve:\n{all:.900}"
    );
    let claims: Vec<&str> = all
        .lines()
        .filter(|l| l.contains("MISSING tokenade binary") || l.contains("non-existent binary"))
        .collect();
    assert_eq!(claims.len(), 1, "one question, one answer — got {claims:?}");
}

/// A flag nobody implemented used to be accepted in silence on these,
/// and each silence has a different price: `watch` dropped every argument it
/// was given, `trust-filters --lst` TRUSTED every project filter when the
/// caller had asked to only list them.
///
/// The assertion is on the FORM the refusal takes, not on one string: the
/// command names itself, names the token it did not know, and exits 2. That is
/// the wording every other tokenade command already uses.
#[test]
fn e2e_the_quiet_commands_refuse_a_flag_nobody_implemented() {
    let h = TempHome::new("quiet-flag-gate");
    // The floor: if this list ever empties, the test passes while proving
    // nothing.
    let commands = ["watch", "trust-filters"];
    assert!(commands.len() >= 2, "the sweep must keep its members");

    for cmd in commands {
        // A regression on `watch` shows up here as a HANG, not as a failure:
        // without the gate the unknown flag falls through to the watcher,
        // which runs until it is killed. Measured while proving this guard —
        // the mutated build sat on this line for twenty minutes. Read a
        // timeout on this test as the gate having gone.
        let (out, err, code) = run(&[cmd, "--zzbogus"], h.path());
        let all = format!("{out}\n{err}");
        assert_eq!(
            code, 2,
            "`{cmd} --zzbogus` must refuse, got {code}: {all:.400}"
        );
        assert!(
            all.contains(&format!("tokenade {cmd}:")) && all.contains("--zzbogus"),
            "`{cmd}` must name itself and the token it refused: {all:.400}"
        );
    }
}

/// The other half: the flags these commands DO implement still reach them, and
/// a value flag still takes its value. Without this, refusing everything would
/// pass the test above.
#[test]
fn e2e_the_quiet_commands_still_take_the_flags_they_implement() {
    let h = TempHome::new("quiet-flag-precision");

    let (out, err, code) = run(&["trust-filters", "--list"], h.path());
    let all = format!("{out}\n{err}");
    assert_eq!(code, 0, "`trust-filters --list` must still run: {all:.400}");
    assert!(
        !all.contains("unknown flag"),
        "and must not be refused: {all:.400}"
    );

    // `--message` takes the token after it: a validator that missed that would
    // report the note as a stray positional.
    let (out, err, _c) = run(
        &["report", "--dry-run", "--message", "a note about a bug"],
        h.path(),
    );
    let all = format!("{out}\n{err}");
    assert!(
        !all.contains("unknown flag") && !all.contains("unexpected positional"),
        "the note is the value of --message, not a positional: {all:.400}"
    );
}

/// One refusal, one wording, on BOTH paths. `validate_simple_bool_flags` named
/// the nearest real flag; `classify_arg` — the path every code-intel and
/// positional-taking command goes through — printed the bare token and a
/// pointer to `--help`. The half that helps was on the side fewer commands use.
#[test]
fn e2e_a_near_miss_flag_is_told_which_real_flag_it_missed() {
    let h = TempHome::new("near-miss-flag");

    let (out, err, code) = run(&["skeleton", "x.rs", "--budgt", "50"], h.path());
    let all = format!("{out}\n{err}");
    assert_eq!(code, 2, "a typo must refuse: {all:.300}");
    assert!(
        all.contains("--budgt") && all.contains("did you mean `--budget`?"),
        "the refusal must name the flag it meant: {all:.300}"
    );

    // And it must not invent one: a token near nothing gets no suggestion,
    // because a wrong guess is worse than none on a refusal.
    let (out, err, _c) = run(&["skeleton", "x.rs", "--zzzzzzzzbogus"], h.path());
    let all = format!("{out}\n{err}");
    assert!(
        all.contains("--zzzzzzzzbogus") && !all.contains("did you mean"),
        "no suggestion when nothing is close: {all:.300}"
    );
}

/// A Read banner that PROMISES the full bytes must name a command that
/// returns them.
///
/// `expand-ref <hash>` bare REFUSES above the disclose threshold (5 000
/// tokens) and prints the recovery menu instead — deliberately, so an
/// accidental dump cannot re-inflate the context. Ten banners in
/// `read_diffs.rs` nevertheless said "Fetch the FULL new bytes via
/// `tokenade expand-ref {hash}`", and a Read is compacted precisely
/// because the file is big: measured 2026-09-02 on this machine's own
/// ledger, 218 of 274 read compactions (80%) carry a blob past that
/// threshold, so four times in five the banner's own command answered a
/// refusal and cost a turn. Same shape as the `raw` footer defect.
///
/// Behavioural, not textual: the command is EXTRACTED from the banner the
/// hook actually emitted and then RUN.
#[test]
fn a_read_banner_naming_the_full_bytes_names_a_command_that_returns_them() {
    let h = TempHome::new("read-diff-pointer");
    let file = h.path().join("config_loader.rs");
    // Past the 5 000-token refusal threshold, which is the whole point:
    // below it the bare form works and the banner would be honest by luck.
    let body = |tweaked: bool| -> String {
        (0..300)
            .map(|i| {
                if tweaked && (i == 7 || i == 61 || i == 62 || i == 148) {
                    format!("pub fn setting_{i}(cfg: &Config) -> u32 {{ cfg.limit_{i}.saturating_mul(2) }}  // adjusted for the new cap\n")
                } else {
                    format!("pub fn setting_{i}(cfg: &Config) -> u32 {{ cfg.limit_{i} }}  // reads one field from the parsed config\n")
                }
            })
            .collect()
    };
    let payload = format!(
        r#"{{"hook_event_name":"PreToolUse","session_id":"diffptr","tool_name":"Read","tool_input":{{"file_path":{:?}}}}}"#,
        file.to_str().unwrap()
    );

    std::fs::write(&file, body(false)).unwrap();
    let _ = run_hook(&["hookwrap-pretool"], h.path(), &payload);
    std::fs::write(&file, body(true)).unwrap();
    let (out, _err, _rc) = run_hook(&["hookwrap-pretool"], h.path(), &payload);
    assert!(
        out.contains("read-diff"),
        "the second Read did not take the diff path, so this test proves nothing: {}",
        &out[..out.len().min(400)]
    );

    // Pull the command out of the banner rather than restating it here — a
    // test that hardcodes `--raw` would still pass if the banner dropped it.
    let at = out.find("expand-ref ").expect("banner names no expand-ref");
    let tail = &out[at + "expand-ref ".len()..];
    let end = tail.find('`').unwrap_or(tail.len());
    let mut argv: Vec<&str> = tail[..end].split_whitespace().collect();
    argv.insert(0, "expand-ref");
    let (recovered, _e, _r) = run_bin(&binary(), &argv, h.path());

    assert!(
        !recovered.contains("too large to dump whole"),
        "the banner's own command was refused; it promised the full bytes: {argv:?} -> {}",
        &recovered[..recovered.len().min(300)]
    );
    // BOTH ends of the file, not one line of it. A pointer that named
    // `--grep setting_148` would satisfy a single-needle assertion while
    // breaking the very promise the sentence makes ("the FULL new bytes").
    for needle in ["setting_0(cfg: &Config)", "setting_299(cfg: &Config)"] {
        assert!(
            recovered.contains(needle),
            "the banner's own command did not return the whole file (missing {needle}): \
             {argv:?} -> {}",
            &recovered[..recovered.len().min(300)]
        );
    }
}

/// And the same must hold for the nine other pointers in that file, which
/// one e2e round-trip cannot reach.
///
/// Exhaustiveness by source sweep: no `expand-ref {…}` in the Read banners
/// may be BARE. `--raw` when the sentence promises the whole thing,
/// `--grep` when it promises one line. Whitespace is squeezed first
/// because the pointer and its flag live in a `\`-continued Rust string
/// and a reformat may put them on different source lines.
#[test]
fn no_read_banner_points_at_a_bare_expand_ref() {
    fn bare_pointers(src: &str) -> Vec<String> {
        let flat = src.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut bare = Vec::new();
        let mut rest = flat.as_str();
        while let Some(i) = rest.find("expand-ref {") {
            let after = &rest[i..];
            let close = after.find('}').map(|c| c + 1).unwrap_or(after.len());
            let ptr = &after[..close];
            let follows = &after[close..after.len().min(close + 20)];
            if !follows.starts_with(" --raw") && !follows.starts_with(" --grep") {
                bare.push(format!("{ptr}{follows}"));
            }
            rest = &rest[i + 1..];
        }
        bare
    }

    // The scanner must SEE a bare pointer, or the clean verdict below is the
    // verdict of a scanner that never says no. A guard run only against a
    // tree that already passes cannot tell a live predicate from `if false`.
    assert_eq!(
        bare_pointers("via `tokenade expand-ref {hash}` for the whole file").len(),
        1,
        "the scanner does not recognise a bare pointer"
    );
    assert!(
        bare_pointers(
            "`tokenade expand-ref {hash} --raw`, or `tokenade expand-ref {h2} --grep <p>`"
        )
        .is_empty(),
        "the scanner flags a pointer that carries its flag"
    );

    // The FAMILY: the banners promising the full bytes moved down into
    // `read_diffs/passthrough.rs` and `read_diffs/strip.rs`.
    let root = tokenade_core::source_guard::crates_dir()
        .join("tokenade-cli")
        .join("src");
    let src = tokenade_core::source_guard::module_family(&root.join("hooks"), "read_diffs");
    let src = src.as_str();
    let bare = bare_pointers(src);
    assert!(
        bare.is_empty(),
        "bare expand-ref pointer(s) in a Read banner — the command refuses above 5k tokens: {bare:?}"
    );
    // And it must have something to sweep: an empty scan is not a pass.
    //
    // The floor was 10 and the file carried exactly 10, which makes it a
    // HEADCOUNT rather than a floor: a change removed one banner while merging two
    // copies of the skeleton view into one, and this guard went red on a
    // correct deletion. A number that tracks the file it guards fires on every
    // legitimate edit and teaches people to raise it, which is how a ratchet
    // stops meaning anything.
    //
    // The scanner's liveness is already proven above, by a positive and a
    // negative it must classify correctly. This clause only has to say "it ran
    // against material", so it says that.
    assert!(
        src.matches("expand-ref {").count() >= 5,
        "the sweep found (almost) no pointers — it is measuring nothing"
    );
}

/// `turn-summary --max N` may return more than N, and now says when it did.
/// Constraint lines are pinned before the budget is spent and never dropped,
/// so a turn made of rules comes back at the pinned floor however small N is —
/// including at the DEFAULT of five, which is how ten sentences were arriving
/// for a documented five with nothing marking it. The floor stays; the silence
/// does not.
#[test]
fn turn_summary_says_when_the_pinned_floor_beat_the_cap() {
    use std::io::Write as _;
    let home = crate::TempHome::new("turn-summary-floor");
    let bin = binary();
    let rules = (0..15)
        .map(|i| format!("Never force-push to branch number {i}."))
        .collect::<Vec<_>>()
        .join(" ");
    let run = |arg: &str, input: &str| -> String {
        let mut c = Command::new(&bin);
        c.args(["turn-summary", "--max", arg, "-"]);
        let mut child = e2e_env(&mut c, home.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn tokenade");
        child
            .stdin
            .as_mut()
            .expect("stdin")
            .write_all(input.as_bytes())
            .expect("write stdin");
        let out = child.wait_with_output().expect("wait");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };

    let over = run("1", &rules);
    let marker = "[tokenade:turn-summary]";
    assert!(
        over.contains(marker),
        "the cap was exceeded in silence:\n{over}"
    );
    assert!(
        over.contains("for --max 1"),
        "the note does not name the cap it exceeded:\n{over}"
    );
    // The number it reports must be the number of sentences above it, not the
    // cap and not the input: a note that restates the flag explains nothing.
    let reported: usize = over
        .split(marker)
        .nth(1)
        .and_then(|t| t.split_whitespace().next())
        .and_then(|n| n.parse().ok())
        .expect("the note leads with a count");
    assert!(
        reported > 1,
        "the note reports {reported} for a cap of 1, so nothing was disclosed"
    );
    let body: String = over
        .lines()
        .filter(|l| !l.starts_with(marker))
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(
        reported,
        body.matches('.').count(),
        "the note counts something other than the sentences it sits under:\n{over}"
    );

    // Plain prose, where the budget genuinely binds, must stay unmarked —
    // otherwise the note is decoration and stops carrying information.
    let prose = (0..30)
        .map(|i| format!("The subsystem emitted metric {i} during the run."))
        .collect::<Vec<_>>()
        .join(" ");
    let under = run("4", &prose);
    assert!(
        !under.contains(marker),
        "a cap that bound was announced as exceeded:\n{under}"
    );

    // And the help states the floor, since it is the only thing that can
    // exceed the flag the user just typed.
    let mut h = Command::new(&bin);
    h.args(["turn-summary", "--help"]);
    let help = String::from_utf8_lossy(
        &e2e_env(&mut h, home.path())
            .output()
            .expect("spawn tokenade")
            .stdout,
    )
    .into_owned();
    assert!(
        help.contains("pinned") && help.contains("10"),
        "the help does not name the floor:\n{help}"
    );
}

/// `pack --budget N --json` must answer to the BUDGET, not to the repository.
///
/// The omitted manifest was bounded in the markdown render and left unbounded
/// in `--json`, so the machine-readable surface listed every over-budget file:
/// measured 2026-09-03, `--budget 200 --json` over one crate returned 4 198
/// tokens against a 200-token cap, and grew with the number of files. The
/// totals still cover every file, so bounding the list loses nothing — the
/// remainder is `omitted_count - omitted_listed`.
#[test]
fn pack_json_scales_with_the_budget_not_the_repo() {
    let home = crate::TempHome::new("pack-json-budget");
    let bin = binary();
    let make = |name: &str, n: usize| -> std::path::PathBuf {
        let dir = home.path().join(name);
        std::fs::create_dir_all(&dir).expect("mkdir");
        for i in 0..n {
            std::fs::write(
                dir.join(format!("file_number_{i:03}.txt")),
                format!("{}\n{i}\n", "lorem ipsum dolor sit amet ".repeat(40)),
            )
            .expect("write fixture");
        }
        dir
    };
    let pack = |dir: &std::path::Path| -> (String, serde_json::Value) {
        let mut c = Command::new(&bin);
        c.args(["pack", "--budget", "300", "--json"]).arg(dir);
        let out = e2e_env(&mut c, home.path())
            .output()
            .expect("spawn tokenade");
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let json = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("--json did not produce JSON ({e}):\n{text}"));
        (text, json)
    };

    let (small_text, small) = pack(&make("small", 30));
    let (big_text, big) = pack(&make("big", 120));

    // The totals still describe every file — bounding the LIST is not
    // dropping the count.
    assert!(
        big["omitted_count"].as_u64().unwrap() > small["omitted_count"].as_u64().unwrap() + 50,
        "the fixture does not oppose: {} vs {}",
        small["omitted_count"],
        big["omitted_count"]
    );
    for v in [&small, &big] {
        let listed = v["omitted_listed"]
            .as_u64()
            .expect("omitted_listed present");
        let count = v["omitted_count"].as_u64().expect("omitted_count present");
        assert!(listed < count, "nothing was bounded: {listed} of {count}");
        assert_eq!(
            v["omitted"].as_array().expect("omitted array").len() as u64,
            listed,
            "omitted_listed does not describe the array it sits with"
        );
    }

    // A generous budget must actually be SPENT, or the fix degenerates into
    // "always the floor" — which is what over-subtracting the included file
    // bodies did while writing this, and which every assertion above still
    // passed. A budget-shaped answer has to move in BOTH directions.
    let mut c = Command::new(&bin);
    c.args(["pack", "--budget", "4000", "--json"])
        .arg(home.path().join("big"));
    let roomy = String::from_utf8_lossy(
        &e2e_env(&mut c, home.path())
            .output()
            .expect("spawn tokenade")
            .stdout,
    )
    .into_owned();
    let roomy: serde_json::Value = serde_json::from_str(&roomy).expect("json");
    assert!(
        roomy["omitted_listed"].as_u64().unwrap() > 10,
        "a 4000-token budget listed only {} — the room is not being spent",
        roomy["omitted_listed"]
    );

    // The decisive one: four times the files must not mean four times the
    // answer. Sizes are compared in bytes because that is what the caller
    // pays for, and the two documents differ only by their counts.
    let (a, b) = (small_text.len(), big_text.len());
    assert!(
        b < a + a / 4,
        "the JSON grew with the repository rather than the budget: {a} then {b} bytes"
    );
}

/// `scrub-secret` removes the secret, including from anything it writes itself.
///
/// End-to-end because the promise is end-to-end: the command reports how many
/// occurrences it removed, and that report was wrong. Measured 2026-09-03 —
/// "scrubbed 3002 occurrence(s)" with 3000 of them still on disk in a
/// `.tokenade-original` the scrub had just created beside the stashed blob.
/// The unit test in `paths.rs` covers the rule; this covers the promise a user
/// reads.
#[test]
fn e2e_scrub_secret_leaves_no_copy_of_what_it_removed() {
    let h = TempHome::new("scrub-secret-e2e");
    let marker = "ZZmarker42";
    let log = h.dir.join("app.log");
    let mut body = String::new();
    for i in 0..3000 {
        body.push_str(&format!(
            "2026-01-01 INFO worker {i} connected to prod using {marker} fine\n"
        ));
    }
    std::fs::write(&log, &body).unwrap();

    // Through `wrap`, so the blob lands in the disclosure stash the way a real
    // command's output does.
    let (_o, _e, code) = run(&["wrap", &format!("cat {}", log.display())], &h.dir);
    assert_eq!(code, 0);

    let count_marker = |skip_source: bool| -> usize {
        let mut n = 0;
        let mut stack = vec![h.dir.clone()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if !(skip_source && p == log) {
                    if let Ok(b) = std::fs::read(&p) {
                        n += String::from_utf8_lossy(&b).matches(marker).count();
                    }
                }
            }
        }
        n
    };
    // Anti-vacuity, and about the RIGHT store: the defect was a pristine copy
    // taken beside a rewritten STASH file, so a run where nothing was stashed
    // proves nothing about it. `dedup.db` also picks the secret up, which is
    // why "some store holds it" was too weak a precondition.
    let stash = h.dir.join(".cache").join("tokenade").join("stash");
    let stashed: usize = std::fs::read_dir(&stash)
        .map(|rd| {
            rd.flatten()
                .filter(|e| {
                    std::fs::read(e.path())
                        .map(|b| String::from_utf8_lossy(&b).contains(marker))
                        .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(0);
    assert!(
        stashed > 0,
        "nothing carrying the secret reached the stash at {} — this run cannot \
         see the defect",
        stash.display()
    );

    // `--yes`: the command asks before writing and there is no terminal here.
    let (out, err, code) = run(&["scrub-secret", marker, "--yes"], &h.dir);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("scrubbed"), "{out}");
    assert_eq!(
        count_marker(true),
        0,
        "the secret survives somewhere tokenade wrote it:\n{out}"
    );
}

/// A destructive, irreversible command asks before it writes.
///
/// `scrub-secret` rewrites the user's stores in place and, since the pristine
/// copy it used to leave beside each file was a copy of the secret itself,
/// keeps nothing to undo with. So it shows what it found and asks. Without a
/// terminal there is nobody to ask, and it refuses rather than assuming yes —
/// the same rule `report` applies before it uploads anything, for the same
/// reason: an agent must not do this silently.
#[test]
fn e2e_scrub_secret_asks_before_it_writes() {
    let h = TempHome::new("scrub-secret-consent");
    let marker = "ZZconsent42";
    let log = h.dir.join("app.log");
    let mut body = String::new();
    for i in 0..3000 {
        body.push_str(&format!("INFO worker {i} using {marker}\n"));
    }
    std::fs::write(&log, &body).unwrap();
    let (_o, _e, code) = run(&["wrap", &format!("cat {}", log.display())], &h.dir);
    assert_eq!(code, 0);

    let still_there = |h: &std::path::Path| -> usize {
        let mut n = 0;
        let mut stack = vec![h.to_path_buf()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p != log {
                    if let Ok(b) = std::fs::read(&p) {
                        n += String::from_utf8_lossy(&b).matches(marker).count();
                    }
                }
            }
        }
        n
    };
    assert!(still_there(&h.dir) > 0, "nothing to scrub — proves nothing");

    // No terminal, no `--yes`: refuse, name the flag, change nothing.
    let (out, err, code) = run(&["scrub-secret", marker], &h.dir);
    let all = format!("{out}{err}");
    assert_eq!(code, 2, "a refusal is a usage error: {all}");
    assert!(
        all.contains("--yes") && all.contains("nothing here can be undone"),
        "the refusal must name the flag and why it is asking:\n{all}"
    );
    assert!(
        all.contains("will be replaced") || all.contains("will replace"),
        "and it must first show WHAT it would change:\n{all}"
    );
    let before = still_there(&h.dir);
    assert!(before > 0, "the refusal modified something:\n{all}");

    // …and it does not describe itself as a --dry-run the user never typed.
    assert!(
        !all.contains("[scrub-secret --dry-run]"),
        "the preview announced a flag nobody passed:\n{all}"
    );

    // With the flag, it writes.
    let (out, err, code) = run(&["scrub-secret", marker, "--yes"], &h.dir);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("scrubbed"), "{out}");
    assert_eq!(still_there(&h.dir), 0, "the secret survived --yes:\n{out}");
}

/// `hooks tail` returns the number its help promises, and `--json` answers
/// with a document.
///
/// Two defects in one function, both measured 2026-09-03. The help said
/// "(default N = 20)" and a bare `hooks tail` returned 40 — both spellings
/// date from the first commit, so neither was a regression and neither was
/// checked. And `--json` was accepted and ignored, while the sibling
/// `hooks status --nope` refuses an argument it does not know: the output is
/// one JSON object per line, so the flag looked like it worked, and a caller
/// asking for a document could not tell a stream from a truncated one.
#[test]
fn e2e_hooks_tail_honours_its_documented_default_and_json() {
    let h = TempHome::new("hooks-tail");
    let log = h.dir.join(".tokenade").join("debug.log");
    std::fs::create_dir_all(log.parent().unwrap()).unwrap();
    let mut body = String::new();
    for i in 0..60 {
        body.push_str(&format!(
            "{{\"component\":\"hookwrap\",\"event\":\"probe\",\"ts\":{i}}}\n"
        ));
    }
    // …and one line that is not JSON, because the ledger has been found torn
    // before and a document must say so rather than drop it.
    body.push_str("{not json at all\n");
    std::fs::write(&log, &body).unwrap();

    let (out, _e, code) = run(&["hooks", "tail"], &h.dir);
    assert_eq!(code, 0);
    let promised: usize = {
        let (help, _e, _c) = run(&["hooks", "--help"], &h.dir);
        let at = help
            .find("default N = ")
            .expect("the help states a default");
        help[at + "default N = ".len()..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .expect("a number")
    };
    assert_eq!(
        out.lines().count(),
        promised,
        "the help promises {promised} records and the command returned {}",
        out.lines().count()
    );
    // Consistency alone would hold at any number — including the 40 this used
    // to return — because the help now derives from the same constant. The
    // VALUE was a decision: on a product that sells fewer tokens, the answer
    // you get for typing no number stays small. Ask for more with `tail N`.
    assert!(
        promised <= 20,
        "the default answer grew to {promised} records; that is what an agent \
         pays for typing `hooks tail` with no number"
    );

    let (js, _e, code) = run(&["hooks", "tail", "5", "--json"], &h.dir);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&js).expect("one JSON document, not a stream");
    assert_eq!(v["requested"], 5);
    assert_eq!(v["total_lines"], 61);
    assert_eq!(
        v["records"].as_array().map(|a| a.len()),
        Some(4),
        "four of the last five lines parse: {js:.400}"
    );
    assert_eq!(
        v["unparsable"].as_array().map(|a| a.len()),
        Some(1),
        "the torn line must be named, not dropped: {js:.400}"
    );
}

/// `tokenade redact` leaves no private key in the file it reports as done.
///
/// The command streams line by line — the help promises it is safe on
/// multi-GB files — and handed each line to a redactor whose key rule spans
/// lines. Measured 2026-09-03: the BEGIN line became `<redacted-private-key>`
/// and the key BODY and the END line were written through into the `.redacted`
/// file, while the same bytes through the model path came back with the whole
/// block gone. This is the verb whose only job is that the file has no secret
/// left in it.
#[test]
fn e2e_redact_removes_a_key_that_spans_lines() {
    let h = TempHome::new("redact-pem");
    let f = h.dir.join("leak.txt");
    let material = "MIIEowIBAAKCAQEA3Zx8kQ9wVsecretmaterialhere0123456789";
    std::fs::write(
        &f,
        format!(
            "service: prod\n\
             -----BEGIN RSA PRIVATE KEY-----\n\
             {material}\n\
             AAAAB3NzaC1yc2EAAAADAQABAAABgQDmoreKeyBytes9876543210\n\
             -----END RSA PRIVATE KEY-----\n\
             owner: platform team\n"
        ),
    )
    .expect("write fixture");

    let (_o, err, code) = run(&["redact", f.to_str().unwrap()], h.path());
    assert_eq!(code, 0, "{err}");
    let out = std::fs::read_to_string(f.with_extension("txt.redacted"))
        .or_else(|_| std::fs::read_to_string(format!("{}.redacted", f.display())))
        .expect("the redacted file");
    assert!(
        !out.contains(material),
        "the key material is still in the file the command reported as redacted:\n{out}"
    );
    assert!(
        out.contains("<redacted-private-key>"),
        "the key must be replaced by its marker, not merely dropped:\n{out}"
    );
    // It must not eat the document around the key either.
    assert!(
        out.contains("service: prod") && out.contains("owner: platform team"),
        "lines outside the key were lost:\n{out}"
    );
}

/// `tokenade read --lines A-B` hands back that region and says which one.
///
/// A file too big to read whole was previously all-or-nothing: an agent told
/// WHERE the answer is — by a search, a stack trace, a previous read — still
/// had to take the whole document to see forty lines of it. The banner names
/// the range and the file's length, so the caller can page without guessing
/// where it stopped.
#[test]
fn e2e_read_serves_a_named_line_range() {
    let h = TempHome::new("read-lines");
    let f = h.dir.join("big.log");
    let body: String = (1..=500)
        .map(|i| format!("line {i} of the log\n"))
        .collect();
    std::fs::write(&f, &body).expect("write");

    let (out, err, code) = run(
        &["read", f.to_str().unwrap(), "--lines", "120-124"],
        h.path(),
    );
    assert_eq!(code, 0, "{err}");
    assert!(
        out.contains("[tokenade: lines 120-124 of 500]"),
        "the slice must name itself:\n{out}"
    );
    // These 500 lines are one template, so `read` folds them — which is the
    // point of the command. What the fold SAYS is then the evidence: five
    // lines, and their values are exactly the range's (consecutive, so said
    // as `120…124` without loss since 2026-10-02 — an `e.g.` before).
    assert!(
        out.contains("[×5 lines") && out.contains("N= 120…124"),
        "the fold must report the five lines of the range, from its start:\n{out}"
    );

    // A second fixture whose lines share no template: there the literal
    // content must come back, and its neighbours must not.
    let plain = h.dir.join("notes.txt");
    let varied: String = (1..=40)
        .map(|i| {
            format!(
                "{}\n",
                "abcdefghij"
                    .chars()
                    .cycle()
                    .skip(i)
                    .take(10 + i % 7)
                    .collect::<String>()
            )
        })
        .collect();
    std::fs::write(&plain, &varied).expect("write");
    let want: Vec<&str> = varied.lines().collect();
    let (slice, _e, cp) = run(
        &["read", plain.to_str().unwrap(), "--lines", "10-12"],
        h.path(),
    );
    assert_eq!(cp, 0);
    for i in 9..12 {
        assert!(
            slice.contains(want[i]),
            "line {} is missing:\n{slice}",
            i + 1
        );
    }
    assert!(
        !slice.contains(want[8]) && !slice.contains(want[12]),
        "the range is inclusive on both ends and nothing else comes with it:\n{slice}"
    );

    // An open end stops at the last line and says so.
    let (tail, _e, c1) = run(&["read", f.to_str().unwrap(), "--lines", "498-"], h.path());
    assert_eq!(c1, 0);
    assert!(tail.contains("[tokenade: lines 498-500 of 500]"), "{tail}");

    // Past the end is refused, not answered with the last line.
    let (_o, err2, c2) = run(
        &["read", f.to_str().unwrap(), "--lines", "9000-9100"],
        h.path(),
    );
    assert_eq!(c2, 2, "a range past the end must be refused");
    assert!(
        err2.contains("past the end") && err2.contains("500"),
        "{err2}"
    );

    // And a range written backwards is refused rather than silently swapped.
    let (_o, err3, c3) = run(&["read", f.to_str().unwrap(), "--lines", "80-20"], h.path());
    assert_eq!(c3, 2);
    assert!(err3.contains("ends before it starts"), "{err3}");
}

/// The line-label notice is said once per context, and again after a compaction.
///
/// The labels are only useful if the reader knows the next move is
/// `read --lines A-B`. Repeating that on every answer bills the sentence on
/// every call; saying it once and never again loses it at the next compaction,
/// which is exactly where an agent's memory of it goes.
#[test]
fn e2e_the_line_range_notice_is_said_once_and_re_armed_by_compaction() {
    let h = TempHome::new("line-notice");
    let doc = h.dir.join("notes.md");
    // Distinct, quotable lines so passages can be located in the file.
    let body: String = (1..=80)
        .map(|i| {
            format!("## section {i}\nthe deploy budget for stage {i} is documented right here\n\n")
        })
        .collect();
    std::fs::write(&doc, &body).expect("write");

    let ask = |q: &str| -> String {
        let out = std::process::Command::new(binary())
            .args(["read", doc.to_str().unwrap(), "--prompt", q])
            .env("HOME", h.path())
            .env("TOKENADE_SESSION", "e2e-line-notice")
            .output()
            .expect("run read");
        String::from_utf8_lossy(&out.stderr).into_owned()
    };

    let first = ask("what is the deploy budget for stage 7");
    if !first.contains("--lines A-B") {
        // No retrieval on this machine (no embedder): the claim cannot be
        // measured, and a test that cannot fail is worse than no test.
        eprintln!("skipped: no labelled retrieval here");
        return;
    }
    let second = ask("what is the deploy budget for stage 9");
    assert!(
        !second.contains("--lines A-B"),
        "the notice was billed twice in one context:\n{second}"
    );

    // A compaction drops what was said; the notice is due again.
    let stamp = h
        .dir
        .join(".tokenade")
        .join("sessions")
        .join("e2elinenotice-compact-ts");
    std::fs::create_dir_all(stamp.parent().unwrap()).expect("mkdir");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::write(&stamp, b"1").expect("stamp");
    let third = ask("what is the deploy budget for stage 11");
    assert!(
        third.contains("--lines A-B"),
        "after a compaction the notice must be said again:\n{third}"
    );
}

/// `file-search` is gone, and what it left behind is not.
///
/// The verb searched one file semantically. It was named in none of the three
/// texts tokenade writes into an agent's context, so no agent ever reached for
/// it: 35 invocations across 204 session histories on the machine it was built
/// on, all inside this repository, none anywhere else, none over MCP. And what
/// it did is covered better by what remains — measured on the same file and
/// question, `read --prompt` answered in 1,326 tokens against 5,195, reads
/// documents `file-search` could not open at all, and now labels each passage
/// with the file lines to open next.
///
/// Its STORE stays, and this test is the reason to say so: the chunk table is
/// swept by `evict-stale` and scrubbed by `scrub-secret`, and the language
/// helper beside it is what `pack` uses to pick a fence. Removing a command is
/// not removing everything it touched.
#[test]
fn e2e_file_search_is_gone_but_its_store_is_still_tended() {
    let h = TempHome::new("file-search-gone");
    let (out, err, code) = run(&["file-search", "some.txt", "a query"], h.path());
    let all = format!("{out}{err}");
    assert_ne!(code, 0, "a removed command must not report success:\n{all}");
    assert!(
        all.contains("unknown command"),
        "it must be refused as unknown, not half-handled:\n{all}"
    );
    // The help must not advertise what no longer exists.
    let (help, _e, _c) = run(&["--help"], h.path());
    assert!(
        !help.contains("file-search"),
        "the help still offers the removed verb:\n{help}"
    );

    // The store it filled is still maintained by the verbs that own it.
    let (evict, _e, _c) = run(&["evict-stale", "--help"], h.path());
    assert!(
        evict.contains("file_search") || evict.contains("chunk"),
        "the chunk store must still be named by the sweep that empties it:\n{evict}"
    );
}

/// A green test run, through the binary, never reports a failure.
///
/// The library guards cover the two renderers; this one covers the contract an
/// agent actually gets. MEASURED 2026-09-04 on the real 8 884-test run of this
/// repository: `[tokenade:cargo-nextest] 8906 line(s)` followed by
/// `... (+8811 more, 10 kept below because they look like failures) ...` and
/// ten PASSING tests, on a run whose own summary says `8884 passed`.
#[test]
fn e2e_a_green_test_run_is_not_folded_into_a_failing_one() {
    let h = TempHome::new("nextest-verdicts");
    let names = [
        "a_pipeline_that_hid_a_failure_says_so",
        "read_prompt_without_a_value_is_a_clean_error",
        "early_pipe_close_does_not_panic",
        "the_exec_timeout_notice_names_the_flag",
    ];
    let mut run = String::from("    Starting 400 tests across 12 binaries\n");
    for i in 1..=400 {
        run.push_str(&format!(
            "        PASS [   0.0{}s] ({i}/400) tokenade-cli::e2e mod{}::{}_{i}\n",
            i % 9,
            i % 12,
            names[i % names.len()]
        ));
    }
    run.push_str("     Summary [  12.4s] 400 tests run: 400 passed, 0 failed, 0 skipped\n");

    let (out, _err, code) = run_with_stdin(
        &["filter", "cargo", "nextest", "run", "--no-fail-fast"],
        h.path(),
        &run,
    );
    assert_eq!(code, 0, "filter failed:\n{out}");
    assert!(
        !out.contains("look like failures"),
        "a run where nothing failed reports failures:\n{out}"
    );
    assert!(
        !out.contains("problem(s) first"),
        "a run where nothing failed announces problems:\n{}",
        out.lines().next().unwrap_or_default()
    );
    assert!(
        out.contains("400 passed, 0 failed"),
        "the summary line did not survive the fold:\n{out}"
    );
}

/// No content entry point adds a byte to output it handed back unchanged.
///
/// MEASURED 2026-09-04: `tokenade read <file>` returned 26 bytes for a
/// 25-byte file with no trailing newline, and 28 for a 27-byte file that HAD
/// one — a blank line on every ordinary read and a corrupted copy under
/// `> file`. `wrap` and `filter` had been fixed to ask one declarant; `read`,
/// the content entry point, still answered with `println!`. The sweep that
/// guards the rule looks for sites that DECIDE with `ends_with('\n')`, so a
/// site that terminates unconditionally was invisible to it — this one drives
/// the binary instead.
#[test]
fn e2e_no_content_entry_point_adds_a_byte_to_a_passthrough() {
    let h = TempHome::new("byte-fidelity");
    let dir = h.path();
    for (label, body) in [
        ("no-newline", "alpha beta gamma delta epsilon zeta"),
        ("with-newline", "alpha beta gamma delta epsilon zeta\n"),
        ("two-lines", "alpha beta gamma\ndelta epsilon zeta\n"),
    ] {
        // Unique per run: an identical read is answered from the dedup layer
        // with a pointer, and a pointer is not a passthrough.
        let body = format!("{body}{}", std::process::id());
        let f = dir.join(format!("fidelity-{label}.txt"));
        std::fs::write(&f, &body).unwrap();
        let (out, err, code) = run_in(&["read", f.to_str().unwrap()], dir, dir);
        assert_eq!(code, 0, "[{label}] read failed:\n{err}");
        assert_eq!(
            out.len(),
            body.len(),
            "[{label}] read returned {} bytes for {} — {out:?}",
            out.len(),
            body.len()
        );
        assert_eq!(out, body, "[{label}] the bytes changed");
    }

    // A VIEW still closes: the fold reshaped the text, so the terminator is
    // the thing that keeps it from running into whatever prints next.
    let big = dir.join("fidelity-big.log");
    let mut blob = String::new();
    for i in 0..4000 {
        blob.push_str(&format!(
            "2026-09-04T10:00:00Z INFO worker={} handled id={i}\n",
            i % 7
        ));
    }
    std::fs::write(&big, &blob).unwrap();
    let (view, _e, _c) = run_in(&["read", big.to_str().unwrap()], dir, dir);
    assert!(
        view.len() < blob.len() / 4,
        "the big file was not folded at all"
    );
    assert!(
        view.ends_with('\n'),
        "a view must close: {:?}",
        &view[view.len().saturating_sub(60)..]
    );
}

/// The read-mode status says when the knob was set to something it does not know.
///
/// `ReadMode::parse` folds a typo into the default deliberately — no command
/// should fail because a shell exports a misspelled variable — so the surface
/// that exists to make the knob discoverable is the one that has to say it.
/// MEASURED 2026-09-04: `TOKENADE_READ_MODE=agressive` answered
/// `active: task (TOKENADE_READ_MODE=agressive)`, the value echoed beside a
/// mode it had not chosen, which reads as agreement.
#[test]
fn e2e_read_mode_says_when_the_knob_is_not_a_rung() {
    let h = TempHome::new("read-mode-knob");
    let bin = binary();

    let run_with = |val: Option<&str>, args: &[&str]| -> String {
        let mut c = Command::new(&bin);
        c.arg("read-mode");
        c.args(args);
        match val {
            Some(v) => {
                c.env("TOKENADE_READ_MODE", v);
            }
            None => {
                c.env_remove("TOKENADE_READ_MODE");
            }
        }
        let out = e2e_env(&mut c, h.path()).output().expect("read-mode");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };

    let typo = run_with(Some("agressive"), &[]);
    assert!(
        typo.contains("is not a rung") && typo.contains("active: task"),
        "a typo was absorbed in silence:\n{typo}"
    );
    let alias = run_with(Some("terse"), &[]);
    assert!(
        alias.contains("active: aggressive") && !alias.contains("is not a rung"),
        "a real alias was called a typo:\n{alias}"
    );
    let unset = run_with(None, &[]);
    assert!(
        unset.contains("unset") && !unset.contains("is not a rung"),
        "an unset knob was called a typo:\n{unset}"
    );

    // A machine reader cannot see a sentence.
    let js: serde_json::Value =
        serde_json::from_str(&run_with(Some("agressive"), &["--json"])).expect("json");
    assert_eq!(js["env_recognised"], serde_json::Value::Bool(false), "{js}");
    let js: serde_json::Value =
        serde_json::from_str(&run_with(Some("dense"), &["--json"])).expect("json");
    assert_eq!(js["env_recognised"], serde_json::Value::Bool(true), "{js}");
    let js: serde_json::Value = serde_json::from_str(&run_with(None, &["--json"])).expect("json");
    assert_eq!(js["env_recognised"], serde_json::Value::Null, "{js}");
}

/// The ladder says how far it reaches, and the claim is checkable.
///
/// MEASURED 2026-09-04: a 188 KB kernel log came back byte-identical under
/// all four rungs — through `read` and through `filter` — while `git ls-files`
/// moved 355 / 499 / 2 524 bytes across aggressive / task / reference. The
/// status printed the ladder with no reach beside it, and its own rung
/// descriptions ("configs, schemas, full diffs") read as promises about every
/// fold. The reach is stated as something the reader can verify: a fold that
/// honoured the mode stamps it in its banner.
#[test]
fn e2e_the_read_mode_ladder_says_how_far_it_reaches() {
    let h = TempHome::new("read-mode-reach");
    let bin = binary();
    let run_mode = |val: &str, args: &[&str], stdin: &str| -> String {
        let mut c = Command::new(&bin);
        c.args(args)
            .env("TOKENADE_READ_MODE", val)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = e2e_env(&mut c, h.path()).spawn().expect("spawn");
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        drop(child.stdin.take());
        let out = child.wait_with_output().unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    };

    let status = run_mode("task", &["read-mode"], "");
    assert!(
        status.contains("reach:") && status.contains("stamps the mode in its banner"),
        "the ladder claims no reach:\n{status}"
    );

    // A compactor that IS wired stamps the mode and changes size with it.
    // A listing that does NOT fit, of NAMED files: since `git ls-files`
    // delivers a list that fits whole, and a numbered one folds without
    // loss to `file_<N>.rs [×400 lines · N= 0…399]`, neither reaches the mode
    // (2026-10-03).
    let (a, b, c) = (
        [
            "alpha", "bravo", "cedar", "delta", "ember", "fjord", "grove", "harbor", "iris",
            "juniper", "kestrel", "lumen", "maple", "nectar", "onyx", "pylon", "quartz", "raven",
            "sable", "tundra",
        ],
        [
            "sync", "relay", "cache", "index", "probe", "vault", "mirror", "audit", "beacon",
            "ledger",
        ],
        [
            "core", "wire", "store", "view", "model", "codec", "guard", "queue", "shard", "token",
            "parse", "route", "query", "frame", "patch",
        ],
    );
    let mut listing = String::new();
    for x in a {
        for y in b {
            for z in c {
                listing.push_str(&format!("crates/{x}/src/{y}/{z}_{y}_{x}.rs\n"));
            }
        }
    }
    let aggr = run_mode("aggressive", &["filter", "git", "ls-files"], &listing);
    let refr = run_mode("reference", &["filter", "git", "ls-files"], &listing);
    assert!(
        aggr.contains("·aggressive"),
        "a wired compactor did not stamp the mode:\n{aggr:.200}"
    );
    assert!(
        aggr.len() < refr.len(),
        "the ladder moved nothing where it is wired: {} vs {}",
        aggr.len(),
        refr.len()
    );

    // …and one that is NOT wired stamps nothing and is unchanged by the rung,
    // which is exactly what the reach sentence tells the reader to expect.
    let log: String = (0..400)
        .map(|i| {
            format!(
                "2026-09-04T10:00:0{}Z INFO worker={} handled id={i}\n",
                i % 10,
                i % 7
            )
        })
        .collect();
    let a2 = run_mode("aggressive", &["filter", "cat", "app.log"], &log);
    let r2 = run_mode("reference", &["filter", "cat", "app.log"], &log);
    assert!(
        !a2.contains("·aggressive"),
        "an unwired fold stamped a mode:\n{a2:.200}"
    );
    assert_eq!(a2, r2, "an unwired fold changed with the rung");
}

/// A two-stage fold reports the magnitude of what it dropped, through the binary.
///
/// MEASURED 2026-09-04: `lsof -n -P` — 370,367 lines — piped through
/// `tokenade filter` announced `folded 8147 line(s) total`. Every one of those
/// was a line the lsof compactor had already collapsed with `(×N)`, so the
/// number an agent judges "how much am I not seeing" by understated the loss
/// by two orders of magnitude.
#[test]
fn e2e_a_two_stage_fold_reports_what_it_really_dropped() {
    let h = TempHome::new("fold-magnitude");
    let mut input = String::from("COMMAND       PID     USER   FD      TYPE   NODE NAME\n");
    // Real lsof shape: long runs of same command/pid/fd-type, which the lsof
    // compactor collapses before anything else sees them.
    // Enough DISTINCT runs that the collapsed view is itself too long and a
    // second stage folds it — which is the shape this test exists for. The
    // rows have to VARY, too: a uniform table is claimed whole by the template
    // fold (`[×N lines]`), nothing is elided, and there is no footer to check.
    let cmds = [
        "systemd", "chrome", "postgres", "node", "python", "sshd", "dockerd", "nvim",
    ];
    let exts = [".so", ".dat", ".log", ".sock", ".conf"];
    let dirs = ["usr", "lib", "var", "home", "opt", "srv"];
    for p in 0..3000usize {
        let cmd = format!("{}{}", cmds[p % cmds.len()], p % 97);
        let fd = ["mem", "cwd", "txt", "rtd"][p % 4];
        let depth = 1 + p % 5;
        for i in 0..10usize {
            let path: String = (0..depth)
                .map(|d| {
                    format!(
                        "/{}{}",
                        dirs[(p + d + i) % dirs.len()],
                        (p * 7 + d * 13) % 900
                    )
                })
                .collect();
            input.push_str(&format!(
                "{cmd}  {}  user{}  {fd}  REG  {}  {path}/{cmd}_{i}{}\n",
                1000 + p,
                p % 13,
                100_000 + (p * 31 + i) % 9_000_000,
                exts[(p + i) % exts.len()]
            ));
        }
    }
    let (out, err, code) = run_with_stdin(&["filter", "lsof", "-n", "-P"], h.path(), &input);
    assert_eq!(code, 0, "filter failed:\n{err}");
    let footer = out
        .lines()
        .find(|l| l.starts_with("[tokenade folded "))
        .unwrap_or_else(|| panic!("no fold footer in:\n{out:.400}"));
    assert!(
        footer.contains("standing for"),
        "the footer counts collapsed lines as one each:\n{footer}"
    );
    let nums: Vec<usize> = footer
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().unwrap())
        .collect();
    assert!(
        nums.len() >= 2 && nums[1] >= nums[0] * 2,
        "the magnitude is not the original one: {footer}"
    );
}

/// `tokenade discover --days <huge>` scans everything, on the live path.
///
/// The unit tests next to `cutoff_for_days` pin the arithmetic; this one pins
/// that the command actually goes through it. Before the fix the debug build
/// panicked here and still exited 0 with an empty stdout — a `--json` caller
/// got a parse error and a success code together — while release wrapped the
/// multiplication and scanned an arbitrary window instead of the widest one.
///
/// The contrast is carried by the entries' own timestamps rather than by the
/// file's mtime, so both runs read the same file and only the WINDOW differs.
#[test]
fn e2e_discover_widest_window_reaches_an_old_session() {
    let home = TempHome::new("discover-window");
    let dir = home.path().join(".claude").join("projects").join("proj");
    std::fs::create_dir_all(&dir).unwrap();
    let session = [
        r#"{"timestamp":"2020-01-01T00:00:00.000Z","type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{}}]}}"#,
        r#"{"timestamp":"2020-01-01T00:00:01.000Z","type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"0123456789"}]}}"#,
    ]
    .join("\n");
    std::fs::write(dir.join("old.jsonl"), session).unwrap();

    let scan = |days: &str| -> serde_json::Value {
        let (out, err, code) = run(&["discover", "--days", days, "--json"], home.path());
        assert_eq!(code, 0, "discover --days {days} exited {code}:\n{err}");
        serde_json::from_str(&out)
            .unwrap_or_else(|e| panic!("--days {days} printed no JSON document ({e}):\n{out}"))
    };

    let huge = scan("999999999999999");
    assert_eq!(
        huge["sessions_scanned"].as_u64(),
        Some(1),
        "the widest window must reach a session from 2020: {huge}"
    );
    let bash = huge["top_tools"]
        .as_array()
        .and_then(|a| a.iter().find(|t| t["tool"] == "Bash"))
        .unwrap_or_else(|| panic!("Bash missing from the widest window: {huge}"));
    assert_eq!(bash["bytes"].as_u64(), Some(10), "charged its answer");

    let narrow = scan("1");
    assert_eq!(
        narrow["sessions_scanned"].as_u64(),
        Some(0),
        "a one-day window must not reach a session from 2020: {narrow}"
    );
}

/// `tokenade read <picture>` names the picture, on the whole path.
///
/// The dimensions line and the stash line are produced by two different halves,
/// and only one of them knew. Measured 2026-09-04 on a TGA and a Radiance HDR:
/// `[tokenade:image 120x80]` immediately above `kind=bin … <hash>.bin` and
/// "Inspect with: file, xxd, hexdump" — the advice for an opaque blob, about a
/// file that had just been decoded. The path hint was there to be passed and
/// was not (`SourceHint::File`: "files have authoritative extensions, we
/// shouldn't second-guess them").
#[test]
fn e2e_read_names_a_picture_it_decoded() {
    let home = TempHome::new("read-tga");
    let p = home.path().join("shot.tga");

    // Uncompressed 24-bit TGA, big enough to reach the stash.
    let (w, h) = (200u16, 150u16);
    let mut tga = vec![0u8, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    tga.extend_from_slice(&w.to_le_bytes());
    tga.extend_from_slice(&h.to_le_bytes());
    tga.extend_from_slice(&[24, 0]);
    for i in 0..(w as usize * h as usize) {
        tga.extend_from_slice(&[(i % 251) as u8, (i % 253) as u8, (i % 249) as u8]);
    }
    std::fs::write(&p, &tga).unwrap();

    let (out, err, code) = run(&["read", p.to_str().unwrap()], home.path());
    assert_eq!(code, 0, "read exited {code}:\n{err}");
    assert!(
        out.contains(&format!("image {w}x{h}")),
        "the decoder must say what it read:\n{out}"
    );
    assert!(out.contains("kind=tga"), "and the stash must agree:\n{out}");
    assert!(
        out.contains("ext=tga"),
        "the file keeps its extension:\n{out}"
    );
    assert!(
        out.contains("identify"),
        "a picture gets the advice for a picture:\n{out}"
    );
    assert!(
        !out.contains("xxd"),
        "and not the advice for an opaque blob:\n{out}"
    );
}

/// Sixteen questions in one `--prompt` come back as sixteen answers.
///
/// Each question used to be given the WHOLE answer budget, so a batch cost the
/// budget once per question: measured 2026-09-04 on this repository's
/// CHANGELOG, which `tokenade read` delivers whole in 4,699 bytes, twelve
/// questions returned **82,056** — seventeen times the document — and sixteen
/// assembled past the delivery bound, which cut the answer back to an outline
/// carrying **six** `### Q` headings with no mention of the ten that had
/// vanished. A question asked and silently dropped is the one failure this
/// path exists to prevent.
#[test]
fn e2e_a_long_batch_of_questions_keeps_every_answer() {
    let home = TempHome::new("scoped-batch");
    let doc: String = (0..600)
        .map(|i| {
            format!(
                "## Section {i}\nRecords of class {i} are kept for {} days, after which the \
                 collector archives them to cold storage and the dashboard stops counting \
                 them.\n\n",
                i * 3 + 30
            )
        })
        .collect();
    let path = home.path().join("handbook.md");
    std::fs::write(&path, &doc).unwrap();

    let questions: Vec<String> = (0..16)
        .map(|i| format!("how long are records of class {} kept", i * 7))
        .collect();
    let (out, _err, code) = run(
        &[
            "read",
            path.to_str().unwrap(),
            "--prompt",
            &questions.join(", "),
        ],
        home.path(),
    );
    assert_eq!(code, 0, "{out}");
    let missing: Vec<usize> = (1..=16)
        .filter(|i| !out.contains(&format!("### Q{i} ·")))
        .collect();
    assert!(
        missing.is_empty(),
        "questions {missing:?} of 16 were asked and never answered, in {} bytes:\n{}",
        out.len(),
        &out[..out.len().min(600)]
    );
    let (plain, _e, _c) = run(&["read", path.to_str().unwrap()], home.path());
    assert!(
        out.len() < plain.len() * 6,
        "asking 16 questions returned {} bytes where reading the whole document \
         returns {} — the flag exists to cost less than the document",
        out.len(),
        plain.len()
    );
}

/// `tokenade read` never hands back more than the file holds.
///
/// Measured 2026-09-04 on a 6,579-byte YAML of 400 `keyN: valueN` lines: the
/// answer was **6,821 bytes** — 400 lines folded to 289 plus a 242-byte
/// recovery footer that cost more than the fold saved. On the tokenizer the
/// model is billed by, 2,399 tokens in and 2,475 out: using tokenade cost more
/// than not using it. Nothing could see it, because every comparison in the
/// path weighs our own token ESTIMATE, which read the same file as 2,839 in and
/// 2,373 out.
#[test]
fn e2e_a_read_never_returns_more_than_the_file() {
    let h = TempHome::new("read-never-inflates");
    let home = h.path();
    let shapes: Vec<(String, String)> = vec![
        (
            "kv.yaml".into(),
            (0..400).map(|i| format!("key{i}: value{i}\n")).collect(),
        ),
        (
            "kv.txt".into(),
            (0..400).map(|i| format!("key{i}: value{i}\n")).collect(),
        ),
        (
            "rows.csv".into(),
            std::iter::once("a,b,c\n".to_string())
                .chain((0..400).map(|i| format!("{i},{i},{i}\n")))
                .collect(),
        ),
        (
            "notes.md".into(),
            (0..200)
                .map(|i| format!("## Section {i}\n\nSome prose about topic {i}.\n\n"))
                .collect(),
        ),
    ];
    for (name, body) in &shapes {
        let p = home.join(name);
        std::fs::write(&p, body).unwrap();
        let (out, _e, code) = run(&["read", p.to_str().unwrap()], home);
        assert_eq!(code, 0, "{name}");
        assert!(
            out.len() <= body.len(),
            "{name}: {} bytes came back for a {}-byte file — a compactor that \
             hands back more than it was given has compacted nothing",
            out.len(),
            body.len()
        );
    }

    // Anti-vacuity: the rule is a ceiling, not a passthrough. A file that
    // really folds still folds.
    let log: String = (0..800)
        .map(|i| format!("2026-09-04T10:00:00Z INFO svc handled req {i} ok\n"))
        .collect();
    let p = home.join("run.log");
    std::fs::write(&p, &log).unwrap();
    let (out, _e, _c) = run(&["read", p.to_str().unwrap()], home);
    assert!(
        out.len() < log.len() / 20,
        "a real fold must survive the ceiling: {} bytes of {}",
        out.len(),
        log.len()
    );
}

/// A file whose line endings are CRLF comes back with CRLF.
///
/// A compactor that REBUILDS its input re-emits normalised text, and when there
/// was nothing to fold that rebuild is the entire difference. Measured
/// 2026-09-04: a 29-byte CRLF markdown came back as 26 bytes of the same words
/// with every `\r\n` turned into `\n`, and a 15-byte CRLF csv the same way.
/// Three bytes "saved", and `tokenade read x.md > y.md` silently rewrites the
/// file — the harm `read`'s byte-fidelity rule already names, one layer up.
#[test]
fn e2e_a_file_that_is_not_folded_keeps_its_line_endings() {
    let h = TempHome::new("read-keeps-crlf");
    let home = h.path();
    let cases: [(&str, &[u8]); 5] = [
        ("notes.md", b"# Title\r\n\r\nSome prose here.\r\n"),
        ("rows.csv", b"a,b\r\n1,2\r\n3,4\r\n"),
        ("plain.txt", b"alpha\r\nbeta\r\ngamma\r\n"),
        ("mixed.txt", b"unix\nwin\r\nunix2\n"),
        ("nonl.txt", b"no trailing newline"),
    ];
    for (name, body) in cases {
        let p = home.join(name);
        std::fs::write(&p, body).unwrap();
        let (out, _e, _c) = run(&["read", p.to_str().unwrap()], home);
        assert_eq!(
            out.as_bytes(),
            body,
            "{name}: {} bytes came back for {} in, and the difference is only \
             line endings — a rewrite, not a compaction",
            out.len(),
            body.len()
        );
    }

    // Anti-vacuity: a file that really folds is a VIEW, and a view is allowed
    // to differ however it likes — CRLF included. The rule is only about a
    // rebuild that changed nothing ELSE.
    let big: String = (0..800)
        .map(|i| format!("2026-09-04T10:00:00Z INFO svc handled req {i} ok\r\n"))
        .collect();
    let p = home.join("run.log");
    std::fs::write(&p, &big).unwrap();
    let (out, _e, _c) = run(&["read", p.to_str().unwrap()], home);
    assert!(
        out.len() < big.len() / 20,
        "a real fold must still happen, CRLF or not: {} bytes of {}",
        out.len(),
        big.len()
    );
}

/// A bash script too large to travel as an argument is delivered ON stdin,
/// which is the same channel the caller's data came in on — so the script
/// runs against an empty stdin. `MAX_SCRIPT_AS_ARG`'s own comment says this
/// case "is stated rather than silently mishandled"; it was stated in
/// `--help` alone. Measured on a 72 121-byte script whose last line is `cat`:
/// empty stdout, empty stderr, exit 0.
///
/// Three halves, because a warning that fires on everything is one an agent
/// learns to skip: it must fire when data was really lost, stay quiet when
/// the script was small enough to travel as an argument (the data arrives),
/// and stay quiet on `< /dev/null`, where there was nothing to lose.
#[test]
fn e2e_exec_says_when_a_large_script_ate_the_piped_data() {
    let h = TempHome::new("exec-stdin-eaten");
    let big = h.path().join("big.sh");
    let mut src = String::new();
    while src.len() < 70_000 {
        src.push_str("# padding line that exists only to make this script large\n");
    }
    src.push_str("echo SCRIPT-RAN\ncat\n");
    std::fs::write(&big, &src).unwrap();
    let small = h.path().join("small.sh");
    std::fs::write(&small, "echo SCRIPT-RAN\ncat\n").unwrap();

    const NOTE: &str = "the data on stdin did NOT reach this script";

    // Large script + real data: the script runs, the data is gone, and it says so.
    let (out, err, code) = run_with_stdin(
        &[
            "exec",
            "--lang",
            "bash",
            "--script-file",
            big.to_str().unwrap(),
        ],
        h.path(),
        "PAYLOAD-MARKER\n",
    );
    assert_eq!(code, 0, "stderr:\n{err}");
    assert!(
        out.contains("SCRIPT-RAN"),
        "the script must still run:\n{out}"
    );
    assert!(
        !out.contains("PAYLOAD-MARKER"),
        "fixture proves nothing if the data DID arrive:\n{out}"
    );
    assert!(
        err.contains(NOTE),
        "silent loss:\nstdout:\n{out}\nstderr:\n{err}"
    );

    // Small script + the same data: it arrives, and nothing is said.
    let (out2, err2, _c) = run_with_stdin(
        &[
            "exec",
            "--lang",
            "bash",
            "--script-file",
            small.to_str().unwrap(),
        ],
        h.path(),
        "PAYLOAD-MARKER\n",
    );
    assert!(
        out2.contains("PAYLOAD-MARKER"),
        "data lost on a small script:\n{out2}"
    );
    assert!(!err2.contains(NOTE), "unasked-for warning:\n{err2}");

    // Large script with stdin on /dev/null: nothing could be lost, so nothing
    // is said. An empty PIPE still warns and that is deliberate — a pipe
    // cannot be inspected for emptiness without consuming it, and the writer
    // may not have written yet, so the rule errs toward the warning. A
    // redirect from a character device can be told apart, and is.
    let mut c = std::process::Command::new(binary());
    c.args([
        "exec",
        "--lang",
        "bash",
        "--script-file",
        big.to_str().unwrap(),
    ])
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped());
    let out3 = e2e_env(&mut c, h.path()).output().unwrap();
    let err3 = String::from_utf8_lossy(&out3.stderr);
    assert!(
        String::from_utf8_lossy(&out3.stdout).contains("SCRIPT-RAN"),
        "the script must still run with no stdin"
    );
    assert!(!err3.contains(NOTE), "cried wolf on /dev/null:\n{err3}");
}

/// `scrub-cache --dry-run` says "no writes" and must make none.
///
/// Opening a SQLite database CREATES it. The dry run called the same
/// `open()`/`open_conn()` its real sibling does, so on a machine with no
/// caches yet it printed "reporting what WOULD change; no writes" and then
/// left behind `dedup.db`, `semantic.db` and `file_search.db` — three empty
/// databases, from the one flag whose entire purpose is to touch nothing. A
/// database that does not exist has nothing to redact, so zero is both the
/// honest answer and the one that writes nothing.
///
/// Known limit, stated rather than faked: the COUNTING path over a database
/// holding a redactable row is not asserted here. It cannot be reached through
/// the binary — the pipeline redacts before it stores, so a current tokenade
/// cannot produce such a row on purpose; `scrub-cache` exists for rows written
/// BEFORE those fixes shipped. Reaching it from a test would need the process's
/// own HOME moved, which is global state this suite does not mutate. The stash
/// arm below covers "a dry run still reports what it found" on the one store
/// plain file I/O can populate.
#[test]
fn e2e_a_dry_run_scrub_creates_nothing() {
    let dbs = |h: &std::path::Path| -> usize {
        fn walk(p: &std::path::Path, n: &mut usize) {
            if let Ok(rd) = std::fs::read_dir(p) {
                for e in rd.flatten() {
                    let q = e.path();
                    if q.is_dir() {
                        walk(&q, n);
                    } else if q.extension().is_some_and(|x| x == "db") {
                        *n += 1;
                    }
                }
            }
        }
        let mut n = 0;
        walk(h, &mut n);
        n
    };

    // 1. A dry run on a machine with no caches creates no database.
    let h = TempHome::new("scrub-dry-creates-nothing");
    let (out, err, code) = run(&["scrub-cache", "--dry-run"], h.path());
    assert_eq!(code, 0, "scrub-cache --dry-run failed:\n{err}");
    assert!(
        out.contains("no writes"),
        "the banner this test is about is gone; re-read it:\n{out}"
    );
    assert_eq!(
        dbs(h.path()),
        0,
        "`--dry-run` promised no writes and created {} database(s):\n{out}",
        dbs(h.path())
    );

    // 2. Anti-vacuity, and it is load-bearing twice over: it proves the
    // command really would touch state (so assertion 1 is not vacuous), and it
    // fails if the existence guard is applied to the REAL path as well.
    let h2 = TempHome::new("scrub-real-does-write");
    let (_o, err2, code2) = run(&["scrub-cache"], h2.path());
    assert_eq!(code2, 0, "scrub-cache failed:\n{err2}");
    assert!(
        dbs(h2.path()) > 0,
        "the real scrub created no database, so the dry-run comparison proves \
         nothing — fixture or guard is wrong"
    );
    // By NAME, not just "some database": each store is guarded separately, and
    // "at least one .db exists" stays true when one guard leaks into the real
    // path while its two neighbours still create theirs. A mutation dropping
    // the `dry_run &&` from the dedup guard lived through this assertion until
    // it named the file.
    let mut found = Vec::new();
    fn names(p: &std::path::Path, out: &mut Vec<String>) {
        if let Ok(rd) = std::fs::read_dir(p) {
            for e in rd.flatten() {
                let q = e.path();
                if q.is_dir() {
                    names(&q, out);
                } else if q.extension().is_some_and(|x| x == "db") {
                    out.push(q.file_name().unwrap().to_string_lossy().into_owned());
                }
            }
        }
    }
    names(h2.path(), &mut found);
    for want in ["dedup.db", "semantic.db", "file_search.db"] {
        assert!(
            found.iter().any(|f| f == want),
            "the real scrub did not create {want} (found {found:?}) — a guard \
             meant for the dry run is suppressing the real path"
        );
    }

    // 3. A dry run still REPORTS what it found, and still writes nothing.
    let h3 = TempHome::new("scrub-dry-still-reports");
    let stash = h3.path().join(".cache/tokenade/stash");
    std::fs::create_dir_all(&stash).unwrap();
    let victim = stash.join("abcdef012345.txt");
    let body = "log line one\nexport AWS_SECRET_ACCESS_KEY=\
                wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY\nlog line two\n";
    std::fs::write(&victim, body).unwrap();
    let (out3, err3, code3) = run(&["scrub-cache", "--dry-run"], h3.path());
    assert_eq!(code3, 0, "scrub-cache --dry-run failed:\n{err3}");
    assert!(
        out3.contains("WOULD be redacted"),
        "a dry run that reports nothing is not a dry run, it is a no-op:\n{out3}"
    );
    assert_eq!(
        std::fs::read_to_string(&victim).unwrap(),
        body,
        "`--dry-run` modified the file it was only supposed to count"
    );
    assert_eq!(dbs(h3.path()), 0, "`--dry-run` created a database:\n{out3}");
}

/// BOTH code-stripping modes deliver the same thing around the text: the
/// banner, the note, and the shared tail that says how to recover the
/// original. They carried two copies of it, and the "lite" copy was covered
/// by no test — so half of this delivery could go away without anything
/// saying so.
///
/// The test drives the real binary through the Read hook, once per mode.
#[test]
fn both_code_strip_modes_deliver_the_same_frame() {
    let home = crate::TempHome::new("code-strip-modes");

    // COMMENTS mode: long comments, so the cut exceeds 10 %.
    let commented = home.path().join("commented.go");
    let body: String = (0..200)
        .map(|i| format!("// what handler {i} does, at some length so it counts\nfunc handler{i}() {{ return }}\n"))
        .collect();
    std::fs::write(&commented, &body).unwrap();

    // LITE mode: almost no comments, but runs of blank lines — it is the
    // blank-line collapse that must clear the 5 % bar.
    let blanks = home.path().join("blanks.go");
    // "comments" mode ALSO collapses blanks, so it always dominates "lite":
    // the latter only opens in the narrow window where the cut stays under
    // 10 % while exceeding 5 %. Ten lines of code per two blank lines fall
    // inside it.
    // The threshold counts TOKENS, not lines: a blank line is worth one, a
    // line of code about ten. One line of code per two blank lines puts the
    // cut around 8 %, inside the window.
    let body_lite: String = (0..200)
        .map(|i| {
            let blanks = if i % 4 != 0 { "\n\n" } else { "" };
            format!("func spaced{i}() {{ return }}\n{blanks}")
        })
        .collect();
    std::fs::write(&blanks, &body_lite).unwrap();

    let payload = |p: &std::path::Path| {
        format!(
            r#"{{"hook_event_name":"PreToolUse","session_id":"strip-modes","tool_name":"Read","tool_input":{{"file_path":{:?}}}}}"#,
            p.to_str().unwrap()
        )
    };

    for (mode, file, own_note) in [
        ("comments", &commented, "Dropped ordinary comments"),
        ("lite", &blanks, "Collapsed blank-line runs"),
    ] {
        let (answer, _err, _rc) =
            crate::run_hook(&["hookwrap-pretool"], home.path(), &payload(file));
        // The view is served by REDIRECTION: the hook rewrites the Read's path
        // to a cache-stable temporary file, and IT is what carries the
        // banner. Read it, rather than asserting on the envelope.
        let v: serde_json::Value = serde_json::from_str(&answer)
            .unwrap_or_else(|e| panic!("unreadable answer ({e}): {answer}"));
        let redirected = v["hookSpecificOutput"]["updatedInput"]["file_path"]
            .as_str()
            .unwrap_or_else(|| panic!("no redirection in mode {mode}: {answer}"));
        let out = std::fs::read_to_string(redirected)
            .unwrap_or_else(|e| panic!("unreadable view in mode {mode} ({e}): {redirected}"));
        assert!(
            out.contains(&format!("mode={mode}]")),
            "mode {mode} was not delivered: {}",
            out.chars().take(300).collect::<String>()
        );
        assert!(
            out.contains(own_note),
            "the note specific to mode {mode} is missing: {}",
            out.chars().take(300).collect::<String>()
        );
        // The SHARED tail: it is what merging the two copies must make
        // impossible to lose on one side only.
        for tail in [
            "Editing? Re-Read with offset/limit for exact line anchoring.",
            "TOKENADE_NO_CODE_STRIP=1",
        ] {
            assert!(
                out.contains(tail),
                "the shared tail is missing in mode {mode}: {tail}"
            );
        }
    }
}

/// A question is asked of the FILE, not of its fold: 2 000 same-shape records
/// fold to a schema and three examples, which cannot say what record 1234
/// holds — and that schema was the "answer".
#[test]
fn a_question_reaches_the_records_a_fold_left_out() {
    let home = crate::TempHome::new("read-json-scoped");
    let f = home.path().join("services.json");
    let items: Vec<String> = (0..2000)
        .map(|i| {
            format!(
                "{{\"id\":{i},\"name\":\"svc-{i}\",\"timeout_ms\":{}}}",
                100 + (i * 37) % 900
            )
        })
        .collect();
    std::fs::write(&f, format!("{{\"services\":[{}]}}", items.join(","))).unwrap();
    let path = f.to_str().unwrap();
    let (answer, _e, rc) = crate::run(
        &[
            "read",
            path,
            "--prompt",
            "what is the timeout of service svc-1234",
        ],
        home.path(),
    );
    assert_eq!(rc, 0);
    assert!(answer.contains("\"svc-1234\",\"timeout_ms\":"), "{answer}");
}

/// A question about one row of a big sheet is asked of the ROWS: the sheet's
/// profile keeps three samples, and the row asked about was never in it.
#[test]
fn a_question_reaches_a_row_the_sheet_profile_left_out() {
    let home = crate::TempHome::new("read-sheet-scoped");
    let f = home.path().join("clients.fods");
    let cell = |v: &str| format!("<table:table-cell><text:p>{v}</text:p></table:table-cell>");
    let mut rows = format!(
        "<table:table-row>{}{}</table:table-row>",
        cell("client"),
        cell("montant")
    );
    for i in 0..300 {
        let name = if i == 150 {
            "Atelier Nord".to_string()
        } else {
            format!("Client {i}")
        };
        rows.push_str(&format!(
            "<table:table-row>{}{}</table:table-row>",
            cell(&name),
            cell(&(100 + i).to_string())
        ));
    }
    std::fs::write(
        &f,
        format!(
            "<?xml version=\"1.0\"?><office:document xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" \
             xmlns:table=\"urn:oasis:names:tc:opendocument:xmlns:table:1.0\" xmlns:text=\"urn:oasis:names:tc:opendocument:xmlns:text:1.0\" \
             office:mimetype=\"application/vnd.oasis.opendocument.spreadsheet\"><office:body><office:spreadsheet>\
             <table:table table:name=\"F\">{rows}</table:table></office:spreadsheet></office:body></office:document>"
        ),
    )
    .unwrap();
    let path = f.to_str().unwrap();
    let (whole, _e, rc) = crate::run(&["read", path], home.path());
    assert_eq!(rc, 0);
    assert!(
        !whole.contains("Atelier Nord"),
        "the profile is the premise of this test: {whole}"
    );
    let (answer, _e, rc) = crate::run(
        &[
            "read",
            path,
            "--prompt",
            "what is the montant of client Atelier Nord",
        ],
        home.path(),
    );
    assert_eq!(rc, 0);
    assert!(answer.contains("| Atelier Nord | 250 |"), "{answer}");
}
