//! End-to-end tests driving `tokenade web`.

use super::*;

/// A compactor piped non-UTF-8 bytes (legacy charset: 0xE9 = `é` in Latin-1,
/// 0x92 = curly quote in cp1252 — both invalid as standalone UTF-8) must NOT
/// drop the whole stream with "failed to read stdin" + exit 1. It reads
/// lossily (like the hook path) and still emits its best-effort compaction.
/// Regression guard for the read_stdin_lossy migration.
#[test]
fn e2e_compactor_tolerates_non_utf8_stdin() {
    let h = TempHome::new("non_utf8_stdin");
    // 60 log lines carrying raw Latin-1/cp1252 bytes, enough to exercise the
    // logs compactor's folding rather than a trivial passthrough.
    let mut payload: Vec<u8> = Vec::new();
    for i in 0..60 {
        payload.extend_from_slice(format!("2026-06-13 INFO request {i} caf").as_bytes());
        payload.push(0xE9); // 'é' in ISO-8859-1, invalid lone byte in UTF-8
        payload.extend_from_slice(b" served \x92ok\x92\n"); // 0x92 = cp1252 quote
    }
    let (stdout, err, code) = run_with_stdin_bytes(&["read", "-"], h.path(), &payload);
    assert_eq!(code, 0, "non-UTF-8 stdin must not fail; stderr:\n{err}");
    assert!(
        !stdout.is_empty(),
        "compactor must still emit output, not drop the stream:\n{err}"
    );
    assert!(
        !err.contains("failed to read stdin"),
        "must not hit the hard UTF-8 read failure:\n{err}"
    );
}

/// File-path twin of the test above: a compactor handed a *file* in a legacy
/// charset (Excel-exported CSV in cp1252, Latin-1 server log) must read it
/// lossily — exactly like `cat file | tokenade <cmd> -` does — instead of
/// dying with a hard read error on a perfectly readable, merely-non-UTF-8
/// file. Regression guard for the read_file_lossy migration: the file branch
/// of `validate_stdin_or_single_file` must match its stdin branch.
#[test]
fn e2e_compactor_tolerates_non_utf8_file() {
    use std::io::Write;
    let h = TempHome::new("non_utf8_file");
    let mut bytes: Vec<u8> = Vec::new();
    for i in 0..60 {
        bytes.extend_from_slice(format!("2026-06-13 INFO request {i} caf").as_bytes());
        bytes.push(0xE9); // 'é' in ISO-8859-1, invalid lone byte in UTF-8
        bytes.extend_from_slice(b" served \x92ok\x92\n"); // 0x92 = cp1252 quote
    }
    let file = h.path().join("legacy.log");
    std::fs::File::create(&file)
        .unwrap()
        .write_all(&bytes)
        .unwrap();
    let (stdout, err, code) = run(&["read", file.to_str().unwrap()], h.path());
    assert_eq!(code, 0, "non-UTF-8 file must not fail; stderr:\n{err}");
    assert!(
        !stdout.is_empty(),
        "compactor must still emit output, not error out on the file:\n{err}"
    );
    // The path-vs-stdin asymmetry would surface as a read error mentioning the
    // file; lossily-read input never does.
    assert!(
        !err.to_lowercase().contains("invalid utf-8")
            && !err.to_lowercase().contains("stream did not contain valid"),
        "must not hit the hard UTF-8 read failure:\n{err}"
    );
}

/// `read` is the sole content entry point after the per-format commands were
/// removed: it must still auto-detect + compact every format via the core
/// compactors. Smoke-covers a spread of formats end-to-end through the binary.
#[test]
fn e2e_read_compacts_multiple_formats() {
    use std::io::Write;
    let h = TempHome::new("read-formats");
    let cases: &[(&str, &str)] = &[
        ("f.json", "{\"a\":1,\"b\":[1,2,3,4,5],\"c\":{\"x\":true}}"),
        ("f.csv", "a,b,c\n1,2,3\n4,5,6\n,,\n"),
        (
            "f.yaml",
            "kind: Pod\nmetadata:\n  name: x\n  managedFields: [a, b]\n",
        ),
        ("f.env", "API_KEY=supersecret\nHOME=/root\nPATH=/usr/bin\n"),
        (
            "f.diff",
            "--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n-old\n+new\n ctx\n",
        ),
        ("f.md", "# Title\n\nbody **text**\n\n```\ncode\n```\n"),
    ];
    for (name, body) in cases {
        let p = h.path().join(name);
        std::fs::File::create(&p)
            .unwrap()
            .write_all(body.as_bytes())
            .unwrap();
        let (out, err, code) = run(&["read", p.to_str().unwrap()], h.path());
        assert_eq!(code, 0, "{name}: read exit != 0; stderr:\n{err}");
        assert!(!out.is_empty(), "{name}: read produced no output");
    }
}

/// A hollow page must not send the agent after a value only tokenade holds.
/// The banner told it to "Check the HTTP status" — the reader has the body and
/// nothing else, and on 26 of 37 real deliveries no status appeared anywhere
/// in the message.
#[test]
fn e2e_a_hollow_page_resolves_the_status_question_itself() {
    let h = TempHome::new("hollow-status");
    let page = h.path().join("shell.html");
    let padding = "<!-- padding padding padding padding padding padding -->\n".repeat(60);
    std::fs::write(
        &page,
        format!(
            "<html><head><title>App</title></head><body><div id=\"root\"></div>\
             <script src=\"/app.js\"></script>{padding}</body></html>"
        ),
    )
    .unwrap();

    let (out, _e, c) = run(&["web", page.to_str().unwrap()], h.path());
    assert_eq!(c, 0, "stderr:\n{_e}");
    assert!(
        out.contains("[tokenade: challenge=") || out.contains("[tokenade: thin-result]"),
        "expected a hollow-page banner:\n{out}"
    );
    assert!(
        !out.contains("Check the HTTP status"),
        "the banner still delegates a question only tokenade can answer:\n{out}"
    );
    assert!(
        out.contains("status-level refusal"),
        "the banner must resolve it instead:\n{out}"
    );

    // The OTHER banner that carries the clause. A hollow shell takes the
    // `challenge=generic-block` path; a page that is nearly all navigation
    // takes `thin-result`, and a mutation restoring the old wording there
    // survived until this half existed.
    let navpage = h.path().join("nav.html");
    let nav: String = (0..60)
        .map(|i| format!("<a href=\"/p{i}\">Section number {i} of the site</a>"))
        .collect();
    std::fs::write(
        &navpage,
        format!(
            "<html><head><title>Docs</title></head><body><nav>{nav}</nav>\
             <main><p>Loading.</p></main><script src=\"/a.js\"></script>{}</body></html>",
            format!("<!-- {} -->\n", "x".repeat(300)).repeat(30)
        ),
    )
    .unwrap();
    let (thin, _e, c) = run(&["web", navpage.to_str().unwrap()], h.path());
    assert_eq!(c, 0, "stderr:\n{_e}");
    assert!(
        thin.contains("[tokenade: thin-result]"),
        "expected the thin-result banner:\n{thin}"
    );
    assert!(
        !thin.contains("Check the HTTP status"),
        "thin-result still delegates the question:\n{thin}"
    );
    assert!(
        thin.contains("status-level refusal"),
        "thin-result must resolve it too:\n{thin}"
    );
}

/// A comma inside brackets is not a question separator, and the notice that
/// declines a batch must quote the part that caused it. Both measured on the
/// real binary: the split decision and the notice have to come from the same
/// splitter, or the message names parts nobody asked about.
#[test]
fn a_bracketed_comma_is_not_a_question_and_the_decline_names_its_cause() {
    let h = TempHome::new("prompt-brackets");
    let file = h.path().join("facts.md");
    std::fs::write(
        &file,
        "The founder is Ada Lovelace.\n\n\
         Pricing is twelve euros per month for the standard plan.\n\n\
         The age bands reported are 25-34 and 18-24 for the survey.\n",
    )
    .unwrap();
    let p = file.to_str().unwrap();

    // The production shape: a real batch of TWO questions, the first of which
    // carries a parenthetical comma. Torn there it became three retrievals,
    // the middle one `with the bar chart)` — a fragment that closes a bracket
    // it never opened, at the full budget, under its own `### Q` heading.
    //
    // Every fragment here is three words or more on purpose. On a shorter
    // prompt the word rule declines the batch anyway and hides whether the
    // bracket rule did anything — which is exactly how the first version of
    // this test passed with the bracket handling removed.
    let (out, _e, c) = run(
        &[
            "read",
            p,
            "--prompt",
            "who founded the company (the person, with their full name), \
             what does the standard plan cost",
        ],
        h.path(),
    );
    assert_eq!(c, 0, "stderr:\n{_e}");
    assert_eq!(
        out.matches("### Q").count(),
        2,
        "a parenthetical comma was read as a separator:\n{out}"
    );
    assert!(
        !out.lines()
            .any(|l| l.starts_with("### Q") && l.contains("· with their full name)")),
        "a fragment closing a bracket it never opened became a question:\n{out}"
    );

    // Same for a quoted span. The items are three words each so the word
    // rule cannot decline the batch and mask whether quotes were honoured —
    // the trap the parenthetical case above walked into first.
    let (q, _e, c) = run(
        &[
            "read",
            p,
            "--prompt",
            "the phrase \"one two three, four five six\" verbatim, \
             what does the standard plan cost",
        ],
        h.path(),
    );
    assert_eq!(c, 0, "stderr:\n{_e}");
    assert_eq!(
        q.matches("### Q").count(),
        2,
        "a comma inside a quoted span was read as a separator:\n{q}"
    );

    // A fragment that is too short is GLUED back to its neighbour, and the
    // warning names it along with the number of questions actually asked.
    let (_o, err, c) = run(
        &[
            "read",
            p,
            "--prompt",
            "offer pricing details, product description, founder",
        ],
        h.path(),
    );
    assert_eq!(c, 0, "stderr:\n{err}");
    assert!(
        err.contains("\"product description\"")
            && err.contains("2 of the 3 comma-separated parts")
            && err.contains("folded into its neighbour")
            && err.contains("1 question(s)"),
        "the warning must name its cause AND what was done:\n{err}"
    );

    // A genuine batch still splits, and says nothing.
    let (batch, err, c) = run(
        &[
            "read",
            p,
            "--prompt",
            "who is the founder here, what does the standard plan cost",
        ],
        h.path(),
    );
    assert_eq!(c, 0, "stderr:\n{err}");
    assert!(
        batch.contains("### Q1"),
        "a real batch must split:\n{batch}"
    );
    assert!(
        !err.contains("read as ONE question"),
        "an honoured batch must stay silent:\n{err}"
    );
}

/// `tokenade read --prompt` must retrieve, not just fold. Without a question the
/// command can only truncate; with one it serves the passage that answers it.
#[test]
fn read_prompt_serves_a_scoped_extract() {
    let h = TempHome::new("read_prompt_serves_a");
    let file = h.path().join("handbook.md");
    std::fs::write(&file, haystack_doc()).unwrap();
    let p = file.to_str().unwrap();

    let (whole, _, c0) = run(&["read", p], h.path());
    assert_eq!(c0, 0);
    let (scoped, err, c1) = run(
        &[
            "read",
            p,
            "--prompt",
            "how often must the signing certificate be rotated",
        ],
        h.path(),
    );
    assert_eq!(c1, 0, "stderr:\n{err}");
    assert!(
        scoped.contains("ninety days"),
        "the answering passage must survive:\n{scoped}"
    );
    assert!(
        scoped.split_whitespace().count() * 2 < whole.split_whitespace().count(),
        "a scoped extract must be markedly smaller than the whole ({} vs {} words)",
        scoped.split_whitespace().count(),
        whole.split_whitespace().count()
    );
}

/// `-q` is the short form, as on `web`.
#[test]
fn read_prompt_short_flag_is_equivalent() {
    let h = TempHome::new("read_prompt_short_fl");
    let file = h.path().join("handbook.md");
    std::fs::write(&file, haystack_doc()).unwrap();
    let p = file.to_str().unwrap();
    let q = "how often must the signing certificate be rotated";
    let (long, _, _) = run(&["read", p, "--prompt", q], h.path());
    let (short, _, code) = run(&["read", p, "-q", q], h.path());
    assert_eq!(code, 0);
    assert_eq!(long, short, "--prompt and -q must be identical");
}

/// An empty question is not a question: fall back to the normal serve rather
/// than retrieving against nothing.
#[test]
fn read_empty_prompt_falls_back_to_the_full_serve() {
    let h = TempHome::new("read_empty_prompt_fa");
    let file = h.path().join("handbook.md");
    std::fs::write(&file, haystack_doc()).unwrap();
    let p = file.to_str().unwrap();
    let (plain, _, _) = run(&["read", p], h.path());
    let (empty, _, code) = run(&["read", p, "--prompt", "   "], h.path());
    assert_eq!(code, 0);
    assert_eq!(plain, empty, "an empty question must behave like none");
}

/// A flag that swallows the next argument must not silently eat the filename.
#[test]
fn read_prompt_without_a_value_is_a_clean_error() {
    let h = TempHome::new("read_prompt_without_");
    let file = h.path().join("handbook.md");
    std::fs::write(&file, haystack_doc()).unwrap();
    let (_, err, code) = run(&["read", file.to_str().unwrap(), "--prompt"], h.path());
    assert_eq!(code, 2, "must exit with 2, not consume the file");
    assert!(err.contains("--prompt requires a value"), "stderr:\n{err}");
}

/// The flag must not be mistaken for the file to read.
#[test]
fn read_prompt_does_not_consume_the_path() {
    let h = TempHome::new("read_prompt_does_not");
    let file = h.path().join("handbook.md");
    std::fs::write(&file, haystack_doc()).unwrap();
    let p = file.to_str().unwrap();
    // Flag BEFORE the path — the path must still be found.
    let (out, err, code) = run(
        &["read", "--prompt", "rotating the signing certificate", p],
        h.path(),
    );
    assert_eq!(code, 0, "stderr:\n{err}");
    assert!(
        out.contains("ninety days"),
        "path lost or empty excerpt:\n{out}"
    );
}

/// `read` is advertised as the entry point for a blob whose format you don't
/// know. It used to decode binary Office files as lossy UTF-8 before detection,
/// and served the raw deflate stream back — measured on a 10-document fixture:
/// 1 of 10 planted facts recoverable.
#[test]
fn read_extracts_a_binary_office_document() {
    let h = TempHome::new("read-docx");
    let file = h.path().join("spec.docx");
    std::fs::write(
        &file,
        tiny_docx("La puissance nominale est ZK-5560 exactement."),
    )
    .unwrap();
    let (out, err, code) = run(&["read", file.to_str().unwrap()], h.path());
    assert_eq!(code, 0, "stderr:\n{err}");
    assert!(
        out.contains("ZK-5560"),
        "the document text must be extracted, not the ZIP:\n{}",
        &out[..out.len().min(200)]
    );
    assert!(
        !out.contains("[Content_Types]"),
        "raw ZIP bytes leaked:\n{}",
        &out[..out.len().min(200)]
    );
}

/// The document route must still honour the question.
#[test]
fn read_prompt_works_on_a_binary_document() {
    let h = TempHome::new("read-docx-q");
    let file = h.path().join("spec.docx");
    std::fs::write(
        &file,
        tiny_docx("La puissance nominale est ZK-5560 exactement."),
    )
    .unwrap();
    let (out, err, code) = run(
        &[
            "read",
            file.to_str().unwrap(),
            "--prompt",
            "quelle est la puissance nominale",
        ],
        h.path(),
    );
    assert_eq!(code, 0, "stderr:\n{err}");
    assert!(out.contains("ZK-5560"), "targeted excerpt empty:\n{out}");
}

/// Plain text must NOT take the document route — that would swap compactors
/// under every existing caller.
#[test]
fn read_still_compacts_plain_text_itself() {
    let h = TempHome::new("read-plain");
    let file = h.path().join("notes.txt");
    std::fs::write(
        &file,
        "ordinary text ZK-0001 nothing binary here\n".repeat(50),
    )
    .unwrap();
    let (out, _, code) = run(&["read", file.to_str().unwrap()], h.path());
    assert_eq!(code, 0);
    assert!(
        out.contains("ZK-0001"),
        "plain text must pass through:\n{out}"
    );
}

/// SECURITY: a scoped extract must not surface a secret the whole-document
/// serve would have redacted. The `--prompt` path returns raw matching lines,
/// so a query like "aws credentials" on a document holding an access key used
/// to print it verbatim — while the same read without a prompt redacted it.
/// Redaction now happens at the extraction source, closing the asymmetry.
#[test]
fn read_prompt_does_not_leak_a_secret_from_a_document() {
    let h = TempHome::new("read-secret");
    let file = h.path().join("runbook.docx");
    // AKIA… is a canonical AWS access-key id the redactor recognises.
    std::fs::write(
        &file,
        tiny_docx("Production access: AKIAIOSFODNN7EXAMPLE is the key id for the prod account."),
    )
    .unwrap();
    let p = file.to_str().unwrap();
    for args in [
        vec!["read", p],
        vec!["read", p, "--prompt", "production aws access credentials"],
        vec!["web", p, "--prompt", "production aws access credentials"],
    ] {
        let (out, err, code) = run(&args, h.path());
        assert_eq!(code, 0, "args={args:?} stderr:\n{err}");
        assert!(
            !out.contains("AKIAIOSFODNN7EXAMPLE"),
            "SECRET LEAKED via {args:?}:\n{}",
            &out[..out.len().min(300)]
        );
    }
}

/// One `--prompt` value, several questions separated by commas → one call, each
/// answered under its own `### Q` heading. The win Paul asked for: N answers,
/// one tool round-trip.
#[test]
fn read_prompt_answers_several_comma_separated_questions() {
    let h = TempHome::new("read_prompt_multi");
    let file = h.path().join("ops.md");
    std::fs::write(
        &file,
        "# Deployment\n\nThe deploy failed because the database migration timed out.\n\n\
         # Backups\n\nNightly backups are written to the eu-west object store at 02:00.\n",
    )
    .unwrap();
    let p = file.to_str().unwrap();
    let (out, err, code) = run(
        &[
            "read",
            p,
            "--prompt",
            "why did the deploy fail, where do backups go",
        ],
        h.path(),
    );
    assert_eq!(code, 0, "stderr:\n{err}");
    assert!(
        out.contains("### Q1 · why did the deploy fail"),
        "no Q1 header:\n{out}"
    );
    assert!(
        out.contains("### Q2 · where do backups go"),
        "no Q2 header:\n{out}"
    );
    assert!(out.contains("migration"), "Q1 answer missing:\n{out}");
    assert!(out.contains("eu-west"), "Q2 answer missing:\n{out}");
}

/// A reader that stops early (`| head`, an early-quit `less`) must NOT make
/// tokenade panic. Rust ignores SIGPIPE before `main`, so `println!` raised
/// "failed printing to stdout: Broken pipe" on stderr and exited non-zero
/// until `crash_guard::restore_sigpipe_default` put SIG_DFL back.
#[test]
#[cfg(unix)]
fn early_pipe_close_does_not_panic() {
    use std::io::Read;
    use std::process::{Command, Stdio};

    // This one set no HOME at all, so it ran against the DEVELOPER's home and
    // the real debug log. `help` writes nothing today, which is why it never
    // showed — containment that depends on what a command happens not to do
    // is not containment.
    let h = TempHome::new("early-pipe-close");
    let mut child = Command::new(binary())
        .arg("help")
        .env("HOME", h.path())
        .env("USERPROFILE", h.path())
        .env_remove("TOKENADE_DEBUG_LOG")
        .current_dir(h.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    // Read a few bytes, then drop the pipe — this is what `head` does.
    let mut buf = [0u8; 32];
    let _ = child.stdout.take().expect("stdout").read(&mut buf);
    let out = child.wait_with_output().expect("wait");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        !err.contains("panicked") && !err.contains("Broken pipe"),
        "SIGPIPE leaked a panic to stderr:\n{err}"
    );
}

/// Piped input has no path to go back to. When a fold on `-` drops most of the
/// bytes, the pointer it prints has to actually resolve — otherwise the only
/// copy of what the agent piped in is gone.
#[test]
fn a_fold_of_piped_input_leaves_a_pointer_that_returns_the_bytes() {
    let home = TempHome::new("stdin-recovery");
    let payload: String = (0..20_000)
        .map(|i| format!("2026-08-02 INFO worker={i} status=ok path=/var/run/app/{i}\n"))
        .collect();
    let (out, _err, code) = run_with_stdin(&["read", "-"], home.path(), &payload);
    assert_eq!(code, 0, "read - exits clean");
    assert!(
        out.len() < payload.len() / 100,
        "the fold is the point: {} bytes out of {}",
        out.len(),
        payload.len()
    );
    let hash = out
        .split("expand-ref ")
        .nth(1)
        .and_then(|s| s.split_whitespace().next())
        .map(|s| s.trim_end_matches('`').to_string())
        .unwrap_or_default();
    assert_eq!(hash.len(), 12, "a recovery pointer was printed:\n{out}");
    // `--budget 0` is the escape hatch and must still hand over everything in
    // one call: for piped input the stash is the ONLY copy, so "the bytes come
    // back" has to stay literally true somewhere.
    let (all, _err, code) = run(
        &["expand-ref", &hash, "--raw", "--budget", "0"],
        home.path(),
    );
    assert_eq!(code, 0);
    assert!(
        all.contains("worker=19999"),
        "the dropped tail did not come back under --budget 0"
    );

    // …and the BOUNDED default reaches it too, by following the continuation
    // it prints. `--raw` stops at a budget now — an agent that types it
    // cannot know it is asking for 1.8 M tokens — so the guarantee is that the
    // bytes come back, not that they come back in a single call. A pointer
    // that stops without a way onward would be the same loss in a new shape.
    let mut from = 1usize;
    let mut saw_tail = false;
    for _ in 0..40 {
        let (page, _err, code) = run(
            &[
                "expand-ref",
                &hash,
                "--raw",
                "--from-line",
                &from.to_string(),
            ],
            home.path(),
        );
        assert_eq!(code, 0);
        if page.contains("worker=19999") {
            saw_tail = true;
            break;
        }
        let Some(next) = page
            .split("--from-line ")
            .nth(1)
            .and_then(|s| s.split(['`', ' ']).next())
            .and_then(|s| s.trim().parse::<usize>().ok())
        else {
            panic!("a bounded page stopped without naming a continuation:\n{page}");
        };
        assert!(
            next > from,
            "the continuation did not advance: {from} -> {next}"
        );
        from = next;
    }
    assert!(
        saw_tail,
        "following the continuations never reached the tail"
    );
}

/// A question whose words are on EVERY line is not a question with no answer.
///
/// `--prompt` is the documented primary way to use a ref. Against 4 000 lines
/// of one shape it used to answer "no passage matched — widen the question",
/// which is both false and unactionable: widening makes it worse, and the only
/// ways left were `--grep` or dumping a quarter of a megabyte — on the most
/// foldable input that exists.
#[test]
fn a_ref_whose_every_line_answers_the_question_still_answers_it() {
    let home = TempHome::new("ref-ubiquitous");
    let payload: String = (0..4_000)
        .map(|i| format!("2026-08-02 INFO worker={i} status=ok path=/var/run/app/{i}\n"))
        .collect();
    let (out, _err, code) = run_with_stdin(&["read", "-"], home.path(), &payload);
    assert_eq!(code, 0);
    let hash = out
        .split("expand-ref ")
        .nth(1)
        .and_then(|s| s.split_whitespace().next())
        .map(|s| s.trim_end_matches('`').to_string())
        .unwrap_or_default();
    assert_eq!(hash.len(), 12, "a recovery pointer was printed:\n{out}");

    let (answer, _err, code) = run(
        &["expand-ref", &hash, "--prompt", "worker status path"],
        home.path(),
    );
    assert_eq!(code, 0, "asking a ref must not fail:\n{answer}");
    assert!(
        answer.contains("worker"),
        "the fold has to carry the shape it folded:\n{answer}"
    );
    assert!(
        answer.len() < payload.len() / 100,
        "and it has to stay small: {} bytes of {}",
        answer.len(),
        payload.len()
    );
}

/// The mirror of the rule: reading a FILE must NOT stash. The path the agent
/// typed is already the recovery, and stashing every read would double the
/// bytes on disk for nothing.
///
/// This asserted the absence of the words "tokens folded away", which is a
/// PROXY for the rule and not the rule — and the proxy is what made the file
/// route silent. Measured 2026-09-04, 4.6 MB of log came back as 154 bytes
/// naming no next step at all. Saying "these bytes are still in the file you
/// named" writes nothing to disk and is what this test is about; what must
/// stay absent is the HASH, because there is none.
#[test]
fn a_fold_of_a_named_file_needs_no_pointer() {
    let home = TempHome::new("file-no-recovery");
    let payload: String = (0..20_000)
        .map(|i| format!("2026-08-02 INFO worker={i} status=ok path=/var/run/app/{i}\n"))
        .collect();
    let p = home.path().join("big.log");
    std::fs::write(&p, &payload).unwrap();
    let (out, _err, _code) = run(&["read", p.to_str().unwrap()], home.path());
    assert!(
        !out.contains("expand-ref"),
        "no stash is written for a file, so no hash may be promised:\n{out}"
    );
    assert!(
        out.contains(p.to_str().unwrap()),
        "and the file it names is the recovery:\n{out}"
    );
}

/// Whatever tokenade decides to do with a non-text input, it must not hand the
/// bytes back as text.
///
/// Three fixes in three days each closed ONE hole in this rule — images, then
/// every other family at small sizes, then piped input, which inflated a PNG
/// to 2.2x its own size. Each was found by measuring, none by reading the
/// code. This is that measurement, kept.
#[test]
fn no_input_ever_comes_back_as_its_own_bytes() {
    let home = TempHome::new("binary-never-leaks");
    let mut leaks: Vec<String> = Vec::new();
    for (name, bytes) in binary_samples() {
        let path = home.path().join(name);
        std::fs::write(&path, &bytes).expect("write sample");
        let p = path.to_string_lossy().to_string();

        for (channel, out) in [
            ("file", run(&["read", &p], home.path()).0),
            (
                "stdin",
                run_with_stdin_bytes(&["read", "-"], home.path(), &bytes).0,
            ),
        ] {
            if !out.starts_with("[tokenade:") && !out.starts_with("[disclose:") {
                leaks.push(format!(
                    "{name} via {channel}: {} bytes in, {} out, not named — starts {:?}",
                    bytes.len(),
                    out.len(),
                    out.chars().take(24).collect::<String>()
                ));
            }
        }
    }
    assert!(leaks.is_empty(), "{}", leaks.join("\n"));
}

/// The other direction, which matters just as much: text must keep reaching
/// its compactor. A rule that called everything binary would satisfy the test
/// above and destroy the product.
#[test]
fn text_is_never_mistaken_for_bytes() {
    let home = TempHome::new("text-stays-text");
    let samples: Vec<(&str, String)> = vec![
        ("log.txt", "2026-08-02 INFO worker ok\n".repeat(60)),
        ("data.csv", "a,b,c\n1,2,3\n4,5,6\n".to_string()),
        ("notes.md", "# Title\n\nSome ordinary prose.\n".to_string()),
        (
            "obj.json",
            "{\"a\":1,\"b\":[1,2,3],\"c\":\"text\"}\n".repeat(20),
        ),
    ];
    let mut misread: Vec<String> = Vec::new();
    for (name, text) in samples {
        let path = home.path().join(name);
        std::fs::write(&path, &text).expect("write sample");
        let p = path.to_string_lossy().to_string();
        for (channel, out) in [
            ("file", run(&["read", &p], home.path()).0),
            (
                "stdin",
                run_with_stdin(&["read", "-"], home.path(), &text).0,
            ),
        ] {
            if out.contains("is not text") || out.contains("[disclose:binary") {
                misread.push(format!("{name} via {channel} was called binary: {out:.80}"));
            }
        }
    }
    assert!(misread.is_empty(), "{}", misread.join("\n"));
}

/// A named path that does not exist is an ERROR; an empty directory is an
/// empty RESULT. Both used to report success with nothing on stdout, so a
/// caller could not tell a typo from a genuinely empty scope — and with
/// `--json` it got no parseable answer either way.
/// A directory is a REAL path, so an existence check waves it through — and
/// `impact` then answered "declares (0 symbols) / dependents (0)" with success,
/// which on a populated module directory reads as "nothing depends on this".
/// A confident wrong answer is worse than a refusal.
/// Detectors that sniff by REOPENING a path are free on a regular file and
/// destructive on a pipe: the prefix read consumes it and the real read finds
/// nothing. Three of them ran in a row, so handing a command's output over —
/// `tokenade read <(git diff)` — answered with a single newline. And an
/// endless source (a device file) was read until the kernel killed the
/// process: measured at ~3.2 GB of RSS per second.
#[cfg(unix)]
#[test]
fn a_stream_that_cannot_be_reopened_is_still_read_once_and_whole() {
    let home = TempHome::new("pipe-read");
    let fifo = home.path().join("fifo");
    let ok = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        return; // no mkfifo on this box — nothing to assert
    }
    let payload = "pub fn alpha() {}\npub fn beta() {}\n";
    let w = fifo.clone();
    std::thread::spawn(move || {
        let _ = std::fs::write(&w, payload);
    });
    let (out, _err, code) = run(&["read", &fifo.to_string_lossy()], home.path());
    assert_eq!(code, 0, "reading a pipe by path must succeed");
    assert!(
        out.contains("alpha") && out.contains("beta"),
        "the whole stream must come back, got {out:?}"
    );

    // And an endless one must come back as a sentence, not a dead process.
    let bin = binary();
    let out = std::process::Command::new(&bin)
        .args(["read", "/dev/zero"])
        .env("HOME", home.path())
        .current_dir(home.path())
        .env("USERPROFILE", home.path())
        .env("TOKENADE_MAX_INPUT_BYTES", "2000000")
        .output()
        .expect("spawn");
    assert_eq!(
        out.status.code(),
        Some(1),
        "an endless source must fail cleanly"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no end in sight"),
        "and say why: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A secret must not survive ANYWHERE in a large output, whatever the folding
/// or truncation did to the text around it.
///
/// This class has drawn blood twice: a line-by-line redaction pass that
/// disarmed the multi-line PEM rules, and an output cap that cut a credential
/// in half BEFORE redaction ran, leaving the first half in clear. Both were
/// fixed; neither was guarded. Placement matters because the failure is always
/// about ordering — where the cut falls relative to the secret.
#[test]
fn a_secret_survives_no_placement_in_a_large_output() {
    let home = TempHome::new("secret-placement");
    // Recognised by a provider rule, so this tests the PIPELINE, not the
    // pattern: if the credential reaches the output the ordering is wrong.
    const SECRET: &str = "AKIAIOSFODNN7EXAMPLE";
    let filler: Vec<String> = (0..9_000)
        .map(|i| format!("2026-08-02 INFO worker={i} status=ok path=/var/run/app/{i}\n"))
        .collect();

    let mut leaks: Vec<String> = Vec::new();
    for pct in [1usize, 50, 99] {
        let mut lines = filler.clone();
        let at = (lines.len() * pct / 100).max(1);
        lines.insert(at, format!("credential {SECRET}\n"));
        let payload: String = lines.concat();
        let path = home.path().join(format!("at{pct}.log"));
        std::fs::write(&path, &payload).expect("write");
        let p = path.to_string_lossy().to_string();

        for (channel, out) in [
            ("wrap", run(&["wrap", &format!("cat {p}")], home.path()).0),
            ("read", run(&["read", &p], home.path()).0),
            (
                "pipe",
                run_with_stdin(&["read", "-"], home.path(), &payload).0,
            ),
        ] {
            if out.contains(SECRET) {
                leaks.push(format!("{pct}% via {channel}: the whole credential"));
            } else if out.contains(&SECRET[..14]) {
                // Half a credential is still half a credential, and it is the
                // exact shape a cut-before-redact bug produces.
                leaks.push(format!("{pct}% via {channel}: a 14-character prefix"));
            }
        }
    }
    assert!(leaks.is_empty(), "{}", leaks.join("\n"));
}

/// `-` means stdin for `--script`, and the scaffold documents `--script-file`
/// on the same line as its alternative. It used to answer `-` with `cannot read
/// --script-file -: No such file or directory` — a message that reads as a
/// missing file, so the caller looks for the file instead of changing the flag.
#[test]
fn e2e_exec_script_file_dash_reads_stdin() {
    let h = TempHome::new("scriptfiledash");
    let (out, err, code) = run_with_stdin(
        &["exec", "--lang", "python", "--script-file", "-"],
        h.path(),
        "print(6*7)\n",
    );
    assert_eq!(code, 0, "exit {code}, stderr: {err}");
    assert!(
        out.contains("42"),
        "script did not run from stdin: {out:?} {err:?}"
    );

    // An empty stdin must say so, not blame a missing file.
    let (_o, err, code) = run_with_stdin(
        &["exec", "--lang", "python", "--script-file", "-"],
        h.path(),
        "",
    );
    assert_ne!(code, 0, "an empty script must fail");
    assert!(
        err.contains("stdin") && !err.contains("No such file"),
        "empty stdin is reported as a missing file: {err:?}"
    );

    // A real path that does not exist keeps its own, correct message.
    let (_o, err, code) = run_with_stdin(
        &[
            "exec",
            "--lang",
            "python",
            "--script-file",
            "/nonexistent-script",
        ],
        h.path(),
        "",
    );
    assert_ne!(code, 0);
    assert!(
        err.contains("/nonexistent-script"),
        "lost the path: {err:?}"
    );
}

/// The guard above uses `help`, which writes a few hundred bytes — far less
/// than a pipe buffer — so it can only ever catch a panic on the FIRST write.
/// The subcommands that were actually broken are the ones deliberately kept
/// OUT of `restore_sigpipe_default` because they also write to a CHILD's
/// stdin: that exclusion is about the pipe going DOWN to the interpreter and
/// was applied to the whole process, so the pipe going OUT to the user
/// inherited SIG_IGN too. Measured 2026-08-26:
/// `tokenade exec --lang bash --script 'seq 1 300000' | head -1` printed
/// `failed printing to stdout: Broken pipe (os error 32)` and a Rust
/// backtrace note, where `seq 1 300000 | head -1` exits silently.
///
/// This one writes ~570 KB, which is several pipe buffers, so the reader is
/// guaranteed to be gone long before the writer is done.
#[test]
#[cfg(unix)]
fn a_subcommand_that_feeds_a_child_still_dies_quietly_on_a_closed_reader() {
    use std::io::Read;
    use std::process::{Command, Stdio};

    let h = TempHome::new("early-pipe-close-exec");
    let mut child = Command::new(binary())
        .args([
            "exec",
            "--lang",
            "bash",
            "--script",
            "seq 1 100000",
            "--no-compact",
        ])
        .env("HOME", h.path())
        .env("USERPROFILE", h.path())
        .env_remove("TOKENADE_DEBUG_LOG")
        .current_dir(h.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    let mut buf = [0u8; 32];
    let _ = child.stdout.take().expect("stdout").read(&mut buf);
    let out = child.wait_with_output().expect("wait");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        !err.contains("panicked") && !err.contains("Broken pipe"),
        "SIGPIPE leaked a panic to stderr:\n{err}"
    );
    // …and it exits the way the signal would, so a shell cannot tell the
    // difference. 141 is 128 + SIGPIPE.
    assert_eq!(
        out.status.code(),
        Some(141),
        "expected the 141 a shell reports for a SIGPIPE death, got {:?}",
        out.status
    );
}

/// "Was the question applied?" is asked ONCE, downstream, by the code that ran
/// the retrieval. An advisory upstream used to re-decide it from the source's
/// FORMAT — `--prompt` on plain text declared a no-op — and the two answers
/// disagreed: the scoped-extract dispatch is gated on a non-empty prompt and
/// nothing else, so a markdown file WAS retrieved from while the agent was told
/// on stderr that its question had been ignored and that what followed was "not
/// an answer". An agent that believes it was not answered re-reads the file.
mod one_home_for_was_the_question_applied {
    use super::*;

    #[test]
    fn a_plain_text_source_is_retrieved_from_and_says_so() {
        let h = TempHome::new("prompt-plain-hit");
        let file = h.path().join("handbook.md");
        std::fs::write(&file, haystack_doc()).unwrap();
        let p = file.to_str().unwrap();

        let (whole, _, c0) = run(&["read", p], h.path());
        assert_eq!(c0, 0);
        let (answer, err, c1) = run(
            &["read", "--prompt", "rotating the signing certificate", p],
            h.path(),
        );
        assert_eq!(c1, 0, "stderr:\n{err}");

        assert!(
            answer.contains("ninety days"),
            "the passage that answers must survive:\n{answer}"
        );
        assert!(
            answer.len() < whole.len(),
            "retrieval must be smaller than the whole serve: {} vs {}",
            answer.len(),
            whole.len()
        );
        assert!(
            !err.contains("was NOT applied"),
            "the question WAS applied — telling the agent otherwise makes it \
             re-read the file whole:\n{err}"
        );
        assert!(
            !answer.contains("not an answer"),
            "this IS an answer:\n{answer}"
        );
    }

    #[test]
    fn a_question_the_source_cannot_answer_still_says_so() {
        let h = TempHome::new("prompt-plain-miss");
        let file = h.path().join("handbook.md");
        std::fs::write(&file, haystack_doc()).unwrap();
        let (out, err, code) = run(
            &[
                "read",
                "--prompt",
                "quelle est la pression des pneus du tracteur",
                file.to_str().unwrap(),
            ],
            h.path(),
        );
        assert_eq!(code, 0, "stderr:\n{err}");
        assert!(
            out.contains("nothing in this source answers that question"),
            "a miss must not read like a hit:\n{}",
            &out[..out.len().min(300)]
        );
    }
}

/// A secret in a file big enough to take the source-strip path reached the
/// agent in clear. That path prints and exits on its own, BEFORE `cmd_auto`'s
/// print-site scrub, so size alone decided whether a key was shown: the same
/// key in a small file, and in a `.txt`, came back `<redacted>`.
///
/// Asserted through the BINARY on purpose. The defect lived in the gap between
/// deciding what to deliver and printing it, which a unit test on either half
/// cannot see. And asserted on a FILE rather than by printing the check —
/// tokenade redacts this session's own terminal output, so a printed check
/// shows `<redacted>` whether or not the leak happened.
mod a_large_source_file_does_not_leak_its_secrets {
    use super::*;

    const KEY: &str = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";

    fn python_with_a_secret(handlers: usize) -> String {
        let mut s = format!("AWS_SECRET_ACCESS_KEY = \"{KEY}\"\n");
        for i in 0..handlers {
            s.push_str(&format!(
                "# what handler {i} does, at some length so the comment share counts\n\
                 def handler{i}():\n    return {i}\n"
            ));
        }
        s
    }

    #[test]
    fn read_scrubs_whatever_the_size() {
        let h = TempHome::new("srcstrip-leak");
        // Big enough for the strip path (>3 000 bytes), and the TempHome is
        // outside any git work tree so the editable-source exception is off.
        let big = h.path().join("big.py");
        std::fs::write(&big, python_with_a_secret(200)).unwrap();
        let small = h.path().join("small.py");
        std::fs::write(&small, python_with_a_secret(1)).unwrap();

        for f in [&big, &small] {
            let (out, err, code) = run(&["read", f.to_str().unwrap()], h.path());
            assert_eq!(code, 0, "stderr:\n{err}");
            assert!(
                !out.contains(KEY),
                "{} served the key in clear ({} bytes)",
                f.display(),
                out.len()
            );
            assert!(
                out.contains("<redacted>"),
                "{} must say what it removed",
                f.display()
            );
        }
    }
}

/// Containment redirects `XDG_CACHE_HOME` at the per-test temp dir, which is
/// right — a test must not write into a real `~/.cache`. But the embedding
/// model cache is keyed on exactly that, so every test reaching retrieval
/// re-downloaded **132 MB**. Measured 2026-08-27 while the CLI suite refused to
/// finish: one test sat at **541 seconds** on a single child with no CPU and
/// one socket open. With the host's model shared read-only, the same two tests
/// finish in **0.16 s**.
mod the_suite_does_not_redownload_the_model {
    #[test]
    fn a_host_model_is_handed_to_the_binary() {
        let Some(d) = crate::host_model_dir() else {
            eprintln!("host has no model cached — the suite downloads, as on a fresh CI box");
            return;
        };
        assert!(
            d.join("model.safetensors").is_file(),
            "the shared dir must be a COMPLETE model, or the binary downloads anyway"
        );
        assert!(
            d.ends_with(tokenade_core::model_fetch::MODEL_VERSION),
            "pinned to MODEL_VERSION, so a bump downloads the new model instead \
             of silently exercising the old one: {}",
            d.display()
        );

        // And the harness actually hands it over — the point is the wiring, not
        // the path.
        let h = crate::TempHome::new("model-share");
        let (out, err, code) = crate::run(&["--version"], h.path());
        assert_eq!(code, 0, "stderr:\n{err}");
        assert!(!out.is_empty());
    }

    /// The share must not defeat containment: nothing may be written into the
    /// developer's real cache.
    #[test]
    fn sharing_the_model_writes_nothing_outside_the_temp_home() {
        let Some(d) = crate::host_model_dir() else {
            return;
        };
        let before: Vec<_> = std::fs::read_dir(&d)
            .map(|rd| rd.flatten().map(|e| e.file_name()).collect())
            .unwrap_or_default();
        let h = crate::TempHome::new("model-readonly");
        let file = h.path().join("doc.md");
        std::fs::write(&file, crate::haystack_doc()).unwrap();
        let (_out, _err, code) = crate::run(
            &[
                "read",
                "--prompt",
                "rotating the signing certificate",
                file.to_str().unwrap(),
            ],
            h.path(),
        );
        assert_eq!(code, 0);
        let after: Vec<_> = std::fs::read_dir(&d)
            .map(|rd| rd.flatten().map(|e| e.file_name()).collect())
            .unwrap_or_default();
        assert_eq!(
            before, after,
            "the shared model dir must be read-only to tests"
        );
    }
}

/// A batch of pages must be summarised page by page, like a single one.
///
/// `web_batch_render_one` stashes on its document branch and on its binary
/// branch, and returned the distilled markdown raw on the HTML branch — every
/// ordinary web page. So `tokenade web <a> <b>` skipped the summary that
/// `tokenade web <a>` always applies, on the call this product's own guidance
/// tells agents to prefer. Measured 2026-08-28 against Wikipedia: one URL
/// delivered 3 976 bytes behind a `[disclose:summary kind=md …]`; two
/// delivered 122 050 whole, six delivered 586 101 — about 146 000 tokens in one
/// answer.
///
/// Local files, not URLs: `web` reads a path down the same branch, so the rule
/// is exercised without a network.
#[test]
fn e2e_web_batch_summarises_each_page() {
    let h = TempHome::new("web-batch-stash");
    let big = |n: usize, tag: &str| {
        let body: String = (0..n)
            .map(|i| {
                format!(
                    "<h2>{tag} section {i}</h2><p>Paragraph {i} of {tag} \
                     discussing {tag}{i} at some length so the page is not tiny.</p>"
                )
            })
            .collect();
        format!("<html><head><title>{tag}</title></head><body>{body}</body></html>")
    };
    let a = h.path().join("a.html");
    let b = h.path().join("b.html");
    std::fs::write(&a, big(900, "alpha")).unwrap();
    std::fs::write(&b, big(900, "beta")).unwrap();
    let raw = std::fs::metadata(&a).unwrap().len() + std::fs::metadata(&b).unwrap().len();
    assert!(raw > 200_000, "the fixture must be big enough to stash");

    let (out, err, code) = run(&["web", a.to_str().unwrap(), b.to_str().unwrap()], h.path());
    assert_eq!(code, 0, "{err}");
    assert!(
        out.len() < raw as usize / 4,
        "a batch must be summarised like a single page ({} bytes of {raw})",
        out.len()
    );
    assert_eq!(
        out.matches("disclose:summary").count(),
        2,
        "each page keeps its OWN pointer, or a batch of one big and five small \
         loses the five:\n{}",
        &out[..out.len().min(400)]
    );

    // …and a small page still arrives whole: the floor is what keeps a batch
    // of short pages from becoming two pointers to nothing.
    let s = h.path().join("s.html");
    std::fs::write(
        &s,
        "<html><body><h1>Tiny</h1><p>One short line.</p></body></html>",
    )
    .unwrap();
    let (out, err, code) = run(&["web", s.to_str().unwrap(), s.to_str().unwrap()], h.path());
    assert_eq!(code, 0, "{err}");
    assert!(
        out.contains("Tiny"),
        "a small page must arrive whole: {out}"
    );
    assert!(!out.contains("disclose:summary"), "{out}");
}

/// A batch failure must name its cause, exactly like a single-target one.
///
/// The single path already does so: it calls
/// `fetch_detailed`, keeps the transport's verdict, and prints a local io
/// error verbatim. The batch path called the Option-returning `fetch`, threw
/// the reason away, and printed `fetch/read failed` — four words for a closed
/// port, an unknown host, a timeout, a typo in a path and a directory alike.
/// Same command, same failure, two answers, and the mute one was the form this
/// product's own guidance tells agents to prefer ("several URLs in ONE call").
#[test]
fn e2e_a_batch_failure_names_the_same_cause_as_a_single_one() {
    let h = TempHome::new("web-batch-why");
    let good = h.path().join("good.html");
    std::fs::write(
        &good,
        "<html><body><h1>Alpha</h1><p>alpha body.</p></body></html>",
    )
    .unwrap();
    let missing = h.path().join("no-such-file.html");
    let missing = missing.to_str().unwrap();

    let (_o1, e1, c1) = run(&["web", missing], h.path());
    let (o2, e2, c2) = run(&["web", good.to_str().unwrap(), missing], h.path());

    assert_ne!(c1, 0, "a single missing target must fail: {e1}");
    assert_ne!(c2, 0, "a batch with a missing target must fail: {e2}");
    // The good page still comes back — a batch is not all-or-nothing.
    assert!(
        o2.contains("Alpha"),
        "the readable target must still be served:\n{o2}"
    );
    for (label, e) in [("single", &e1), ("batch", &e2)] {
        assert!(
            e.contains("No such file") || e.contains("os error 2"),
            "the {label} path must name the cause, got:\n{e}"
        );
        assert!(
            !e.contains("fetch/read failed"),
            "the {label} path must not answer with four words that fit every failure, got:\n{e}"
        );
    }
}

// A batch says which of its targets failed, in the same words as one alone.
//
// `cmd_web` was given the status and the landing URL earlier, with the
// measurement behind it: "23 % of real fetches are non-2xx and 11.6 % end on a
// different host, all of it invisible here." The BATCH — the call the shipped
// guidance tells agents to prefer — kept going without them. Measured
// 2026-09-03 against a real 404: alone, the answer opened with
// `[tokenade: HTTP 404 — the page does not exist. …]`; in a pair, the section
// was the two words `Not Found`, indistinguishable from a page that says so.
//
// **Why there is NO redirect test here.**
//
// The distiller is handed the address that ANSWERED rather than the one
// requested, so that the origin announced for relative links is true after
// a redirect. An end-to-end test would need two loopback hosts, one
// redirecting to the other — and `web_fetch` REFUSES to follow a redirect to
// an internal host: that is a deliberate SSRF guard ("a public URL that
// 30x-es to 169.254.169.254 / localhost / a LAN IP must not turn a web fetch
// into a reach into the user's infrastructure"). Written then removed after
// measurement: the probe returns `empty response`, `first_status: null`.
//
// So the wiring stays WITHOUT an end-to-end test, and saying so beats
// dressing it up. What is tested: `declared_base_href` and the announcement
// itself, in `tokenade-core`.

/// Loopback, so the guard does not depend on somebody else's uptime.
#[test]
fn e2e_web_batch_names_a_failed_target_like_the_single_path_does() {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();
    // Bounded on BOTH ends: it stops after the two requests it expects, and it
    // stops anyway at the deadline. A blocking `accept` in a loop that counts
    // higher than the client ever calls is a test that hangs forever — this one
    // did, and it took the suite with it.
    listener
        .set_nonblocking(true)
        .expect("nonblocking loopback listener");
    let server = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut served = 0;
        while served < 2 && std::time::Instant::now() < deadline {
            let (mut sock, _) = match listener.accept() {
                Ok(pair) => pair,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                Err(_) => return,
            };
            let _ = sock.set_nonblocking(false);
            served += 1;
            let mut line = String::new();
            let _ = BufReader::new(sock.try_clone().unwrap()).read_line(&mut line);
            let missing = line.contains("/missing");
            let body = if missing {
                "<html><body><p>Not Found</p></body></html>".to_string()
            } else {
                format!(
                    "<html><head><title>Ok Page</title></head><body>{}</body></html>",
                    "<p>a paragraph of ordinary content</p>".repeat(20)
                )
            };
            let head = if missing {
                "HTTP/1.1 404 Not Found"
            } else {
                "HTTP/1.1 200 OK"
            };
            let _ = write!(
                sock,
                "{head}\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.flush();
        }
    });

    let h = TempHome::new("web-batch-status");
    let ok = format!("http://127.0.0.1:{port}/ok");
    let missing = format!("http://127.0.0.1:{port}/missing");
    let (out, _e, _c) = run(&["web", &ok, &missing], h.path());
    let _ = server.join();

    // A section is what follows its `# <url>` line, not the chunk that CONTAINS
    // the url — the url lives in the separator, so splitting on it and matching
    // the url finds the header and none of the body. The separator has been a
    // markdown HEADING for a while: that is what makes it survive into the
    // outline when the batch is summarised.
    let after = |url: &str| -> String {
        let head = format!("# {url}");
        // The cut happens at the boundary of the NEXT TARGET, not at the first
        // heading that comes along: the page serves its own `# Title` right
        // under the boundary, and cutting there would yield an empty section.
        match out.find(&head) {
            Some(at) => {
                let rest = &out[at + head.len()..];
                rest[..rest.find("\n# http").unwrap_or(rest.len())].to_string()
            }
            None => String::new(),
        }
    };
    let failed = after(&missing);
    assert!(
        failed.contains("HTTP 404"),
        "the failing target's section does not say it failed:\n{out:.900}"
    );
    // …and a target that worked is not decorated with a status it does not have.
    let good = after(&ok);
    assert!(
        good.contains("Ok Page"),
        "the CONTROL section is missing, so this proves nothing:\n{out:.900}"
    );
    assert!(
        !good.contains("tokenade: HTTP"),
        "a 200 acquired a status banner:\n{out:.900}"
    );
}

/// One place builds a rendered batch target, so a new return path cannot drop
/// the provenance: the constructor takes it as its FIRST argument.
#[test]
fn no_batch_path_builds_a_rendered_target_by_hand() {
    // The FAMILY: `Rendered` and its constructor moved down into
    // `web_batch/render.rs`, and reading only the trunk yielded ZERO literals
    // — that is, a verdict on a file that no longer carries the subject.
    let root = tokenade_core::source_guard::crates_dir()
        .join("tokenade-cli")
        .join("src")
        .join("compactors");
    let src = tokenade_core::source_guard::module_family(&root, "web_batch");
    let production: std::collections::HashSet<usize> =
        tokenade_core::srcsweep::production_lines(&src)
            .into_iter()
            .map(|(n, _)| n)
            .collect();
    let offenders: Vec<String> = src
        .lines()
        .enumerate()
        // The definition, the constructor's own signature and the inherent
        // impl block all end in `Rendered {` and none is a construction.
        .filter(|(i, l)| {
            l.contains("Rendered {")
                && !l.contains("struct Rendered")
                && !l.contains("-> Rendered {")
                && !l.contains("impl Rendered {")
                && production.contains(i)
        })
        .map(|(i, l)| format!("{}: {}", i + 1, l.trim()))
        .collect();
    assert_eq!(
        offenders.len(),
        1,
        "exactly one struct literal, inside `rendered()`; the others must call it: {offenders:?}"
    );
    assert!(
        src.contains("fn rendered(provenance: &str"),
        "the constructor must still take the provenance first"
    );
}

/// The batch asks, for every target, whether a dedicated reader exists.
///
/// Its sibling below sweeps the sources, and a sweep cannot tell a live call
/// from a dead one: wrapping the reader block in `if false` left it green.
/// This one watches the run instead — the decision is logged per target on the
/// product's own diagnostic channel, so a batch that stops asking stops saying
/// so. Loopback, because what is being checked is that the QUESTION is asked,
/// not that a reader matches.
#[test]
fn e2e_web_batch_asks_the_readers_for_every_target() {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).expect("nonblocking");
    let server = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut served = 0;
        while served < 2 && std::time::Instant::now() < deadline {
            let (mut sock, _) = match listener.accept() {
                Ok(p) => p,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                Err(_) => return,
            };
            let _ = sock.set_nonblocking(false);
            served += 1;
            let mut line = String::new();
            let _ = BufReader::new(sock.try_clone().unwrap()).read_line(&mut line);
            let body = format!(
                "<html><head><title>P</title></head><body>{}</body></html>",
                "<p>ordinary content</p>".repeat(20)
            );
            let _ = write!(
                sock,
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.flush();
        }
    });

    let h = TempHome::new("web-batch-asks");
    let log = h.dir.join("debug.log");
    let a = format!("http://127.0.0.1:{port}/one");
    let b = format!("http://127.0.0.1:{port}/two");
    let bin = binary();
    let out = std::process::Command::new(&bin)
        .args(["web", &a, &b])
        .env("HOME", h.path())
        .env("TOKENADE_DEBUG_LOG", &log)
        .output()
        .expect("run web batch");
    let _ = server.join();
    assert!(
        out.status.success(),
        "{:?}",
        String::from_utf8_lossy(&out.stderr)
    );

    let recorded = std::fs::read_to_string(&log).unwrap_or_default();
    for url in [&a, &b] {
        assert!(
            recorded
                .lines()
                .any(|l| l.contains("site_reader_considered") && l.contains(url)),
            "the batch never asked whether a reader exists for {url}:\n{recorded:.600}"
        );
    }

    // Asking is half of it. A target whose plan MATCHES must actually reach the
    // reader — asking and then dropping the answer looked identical above,
    // since no loopback URL has a reader. The reader says so on either outcome:
    // it logs its own failure, and on success the answer opens `> source:`. So
    // this holds with or without a network, and fails when the plan is
    // consulted and then ignored.
    let h2 = TempHome::new("web-batch-reaches");
    let log2 = h2.dir.join("debug.log");
    let known = "https://github.com/BurntSushi/ripgrep/issues/1109";
    let out2 = std::process::Command::new(&bin)
        .args(["web", known, &a])
        .env("HOME", h2.path())
        .env("TOKENADE_DEBUG_LOG", &log2)
        .output()
        .expect("run web batch");
    let logged = std::fs::read_to_string(&log2).unwrap_or_default();
    let served = String::from_utf8_lossy(&out2.stdout).contains("> source:");
    assert!(
        served || logged.contains("webfetch_site_reader_"),
        "a target with a dedicated reader neither reached it nor said why:\n{logged:.600}"
    );
}

/// Every FUNCTION that fetches a page for the user tries the site readers first.
///
/// Seventeen readers turn a URL into its structured source — the GitHub API for
/// an issue, old.reddit for a thread, the registry for a crate. The single
/// `tokenade web` path was given them first, the batch a little later, and
/// `answer_from_results` — the top three results of a web search — later
/// still.
///
/// It is that third one this sweep failed to see. It asked whether the MODULE
/// mentioned a reader, and `answer_from_results` sits in the same file as the
/// batch path that had just been fixed, so the guard was green over a function
/// that scraped every page it read. Measured 2026-09-03 on the first hit of a
/// live search, `github.com/BurntSushi/ripgrep/issues/176`: 18,965 bytes of
/// thread through `tokenade web`, 832 bytes of `Description` and a `References`
/// list of login links through `search --answer`. A guard whose grain is
/// coarser than the defect cannot see the defect.
///
/// Tied to the source because the readers key on real hosts over https: a
/// loopback server cannot be github.com, so there is no offline way to watch
/// the choice being made. What CAN be checked is that no page-fetching
/// function exists without one. Its siblings watch the runs.
#[test]
fn every_page_fetching_path_consults_the_site_readers() {
    /// Top-level items only: a `fn` at column 0 opens a new one, and anything
    /// indented belongs to the one it is written inside.
    fn functions(src: &str) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for line in src.lines() {
            let head = line
                .strip_prefix("pub ")
                .or_else(|| {
                    line.starts_with("pub(")
                        .then(|| line.split_once(") ").map(|(_, r)| r))
                        .flatten()
                })
                .unwrap_or(line);
            let head = head.strip_prefix("async ").unwrap_or(head);
            if let Some(name) = head.strip_prefix("fn ") {
                let name = name
                    .split(['(', '<', ' '])
                    .next()
                    .unwrap_or_default()
                    .to_string();
                out.push((name, String::new()));
            }
            if let Some((_, body)) = out.last_mut() {
                body.push_str(line);
                body.push('\n');
            }
        }
        out
    }

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut fetchers: Vec<(String, bool)> = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).expect("readable").flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            if p.extension().is_some_and(|x| x == "rs") {
                let src = std::fs::read_to_string(&p).unwrap_or_default();
                if !src.contains("FetchOpts::page") {
                    continue;
                }
                let file = p.strip_prefix(&root).unwrap_or(&p).display().to_string();
                for (name, body) in functions(&src) {
                    if !body.contains("FetchOpts::page") {
                        continue;
                    }
                    // The ask itself, or a reader handed in — the answer path
                    // takes one as a parameter so a test can watch it compose.
                    let asks = body.contains("site_reader(")
                        || body.contains("site_reader_read(")
                        || body.contains("via_reader(");
                    fetchers.push((format!("{file}::{name}"), asks));
                }
            }
        }
    }
    assert!(
        fetchers.len() >= 4,
        "the sweep found only {fetchers:?} — it is not reading the sources"
    );
    let blind: Vec<&String> = fetchers
        .iter()
        .filter(|(_, ok)| !ok)
        .map(|(f, _)| f)
        .collect();
    assert!(
        blind.is_empty(),
        "these fetch a page for the user without ever asking whether a dedicated \
         reader exists for it: {blind:?}"
    );
}

/// A saved page that carries no results does not blame the network.
///
/// `tokenade search <file|->` queries nobody: it parses a page you already
/// have. Measured 2026-09-04, an ordinary HTML page came back
/// `no usable results for "saved page" — no engine was reachable`.
#[test]
fn e2e_a_saved_page_with_no_results_does_not_blame_the_network() {
    let h = TempHome::new("saved-page-empty");
    let page = "<html><head><title>T</title></head><body><h1>Titre</h1>\
                <p>Un paragraphe sans le moindre resultat.</p></body></html>";
    let (out, err, code) = run_with_stdin(&["search", "-"], h.path(), page);
    assert_eq!(code, 0, "search - failed:\n{out}\n{err}");
    let both = format!("{out}\n{err}");
    assert!(
        !both.contains("reachable"),
        "a page handed to us on stdin blamed the network:\n{both}"
    );
    assert!(
        both.contains("no engine was queried"),
        "the answer does not say that nobody was asked:\n{both}"
    );
}

/// A local file whose lines ARE its meaning keeps them — on both routes.
///
/// The filename is the label for a local file, and the map of "certainly
/// prose" extensions left everything else on the HTML pipeline, which reads
/// the file as one giant text node and collapses its newlines. Measured
/// 2026-09-09: forty CSV rows came back as ONE line. Checked on the single
/// target AND in a batch, because a `web` behaviour has now lived on one of
/// the two routes only four times.
#[test]
fn e2e_a_local_csv_keeps_its_rows_on_both_routes() {
    let h = TempHome::new("web-local-csv");
    let csv = h.path().join("donnees.csv");
    let mut body = String::from(
        "id,name,qty
",
    );
    for i in 0..40 {
        body.push_str(&format!(
            "{i},item-{i},{}
",
            i * 3
        ));
    }
    std::fs::write(&csv, &body).expect("write the csv");
    let other = h.path().join("notes.md");
    std::fs::write(
        &other,
        "# Titre

un paragraphe
",
    )
    .expect("write the md");
    for args in [
        vec!["web", csv.to_str().unwrap()],
        vec!["web", csv.to_str().unwrap(), other.to_str().unwrap()],
    ] {
        let (out, err, code) = run(&args, h.path());
        let both = format!("{out}\n{err}");
        assert_eq!(code, 0, "{args:?}:\n{both}");
        assert!(
            both.contains("0,item-0,0\n1,item-1,3"),
            "the rows are glued together, the file was read as HTML ({args:?}):\n{both:.400}"
        );
    }
}

/// A page no engine produced is not served as a result list.
///
/// `extract_saved_page` falls back to a generic anchor walk when it cannot
/// tell whose results page this is, and its output is numbered exactly like an
/// engine's. Measured 2026-09-09: `tokenade search docsrs.html` answered
/// « 1. Apache-2.0 … 2. Privacy policy … », exit 0, with the ABSENCE of the
/// « parsed as a X results page » line as the only signal.
#[test]
fn e2e_a_page_no_engine_produced_is_named_as_such() {
    let h = TempHome::new("saved-page-anchors");
    let mut page = String::from("<html><head><title>Doc</title></head><body>");
    for i in 0..12 {
        page.push_str(&format!(
            "<p><a href=\"https://example.com/page{i}\">Un titre de lien {i}</a></p>"
        ));
    }
    page.push_str("</body></html>");
    let (out, err, code) = run_with_stdin(&["search", "-"], h.path(), &page);
    let both = format!("{out}\n{err}");
    assert_eq!(code, 0, "search - failed:\n{both}");
    assert!(
        both.contains("no engine we know produced this page"),
        "a list of anchors was served as results:\n{both}"
    );
    assert!(
        both.contains("the page on stdin"),
        "the note does not name what we are holding:\n{both}"
    );
}

/// A mistyped path never becomes a web search.
///
/// `search` reads a readable file as a saved results page and treats anything
/// else as a query, so a missing path went to the engines as the literal query
/// string — exit 0, and the path off the machine. Checked on both routes: one
/// argument, and a batch where only one of them is the typo.
#[test]
fn e2e_a_missing_path_is_not_searched_for_on_the_web() {
    let h = TempHome::new("search-missing-path");
    for args in [
        vec!["search", "/nope/missing.html"],
        vec!["search", "rust tokio cancellation", "/nope/missing.html"],
    ] {
        let (out, err, code) = run(&args, h.path());
        let both = format!("{out}\n{err}");
        assert_eq!(code, 2, "a missing path was accepted ({args:?}):\n{both}");
        assert!(
            both.contains("/nope/missing.html") && both.contains("No such file"),
            "the refusal does not name the file:\n{both}"
        );
        assert!(
            !both.contains("via ") && !both.contains("no usable results"),
            "an engine was reached anyway:\n{both}"
        );
    }
}

/// A page with no readable text says so; it does not answer with a newline.
///
/// Measured 2026-09-04 on five shapes — an empty body, whitespace only, a
/// comment only, empty tags only, a `<style>` only — `tokenade web <file>`
/// returned a single newline with exit 0 and an empty stderr. One byte is
/// indistinguishable from a crash, a truncated fetch, and a page that really is
/// blank. The BATCH route already named the empty one
/// (`the body decoded to no readable text`), so the two spellings of one
/// command disagreed about whether this is worth saying.
#[test]
fn a_page_with_no_readable_text_says_so() {
    let home = TempHome::new("web-empty-page");
    let cases = [
        ("empty.html", ""),
        ("blank.html", "<html><body>   \n\t  </body></html>"),
        ("comment.html", "<html><body><!-- nothing --></body></html>"),
        (
            "tags.html",
            "<html><body><div></div><span></span></body></html>",
        ),
        (
            "style.html",
            "<html><head><style>.a{color:red}</style></head><body></body></html>",
        ),
    ];
    for (name, body) in cases {
        let p = home.path().join(name);
        std::fs::write(&p, body).unwrap();
        let (out, _err, code) = run(&["web", p.to_str().unwrap()], home.path());
        assert_eq!(code, 0, "{name}");
        assert!(
            out.contains("carried no readable text"),
            "{name} came back as {:?} — an agent cannot tell that from a crash",
            out
        );
        assert!(
            out.contains(name),
            "and it has to name the source it is talking about: {out}"
        );
    }

    // The other half, or the rule would be "always say the page is empty": a
    // page with words in it is delivered, and says nothing of the sort.
    let real = home.path().join("real.html");
    std::fs::write(
        &real,
        "<html><body><h1>Titre</h1><p>du contenu lisible ici.</p></body></html>",
    )
    .unwrap();
    let (out, _err, code) = run(&["web", real.to_str().unwrap()], home.path());
    assert_eq!(code, 0);
    assert!(out.contains("du contenu lisible ici"), "{out}");
    assert!(!out.contains("carried no readable text"), "{out}");
}

/// An accessibility snapshot keeps every ref, whatever route it arrives by.
///
/// A ref is what `browser_click` targets; the a11y compactor keeps all of them
/// on purpose. Measured 2026-09-04 on a 53,084-byte tree of 500 list items with
/// 500 refs and no `generic` node: content detection declined it (its rule
/// required a `generic` wrapper), the repetition fold claimed it, and the
/// answer came back as `[ref=e<N2>]  [×500 lines]` — **zero refs**, 113x
/// smaller and impossible to act on. That is the route every MCP tool result
/// and every plugin-bridge result takes, which is how a browser snapshot
/// always arrives.
#[test]
fn an_accessibility_snapshot_keeps_its_refs_on_every_route() {
    let home = TempHome::new("a11y-refs");
    let tree: String = std::iter::once("- document \"Dashboard\":\n".to_string())
        .chain((0..500).map(|i| {
            format!(
                "  - listitem:\n    - link \"Item {i}\" [ref=e{i}]:\n      - /url: /items/{i}\n\
                     - text: \"row {i} description\"\n"
            )
        }))
        .collect();
    assert!(!tree.contains("- generic"), "the fixture's whole point");
    let path = home.path().join("snapshot.txt");
    std::fs::write(&path, &tree).unwrap();

    let count = |s: &str| s.matches("[ref=e").count();
    for (route, out) in [
        (
            "snapshot",
            run(&["snapshot", path.to_str().unwrap()], home.path()).0,
        ),
        (
            "read",
            run(&["read", path.to_str().unwrap()], home.path()).0,
        ),
        (
            "content (no command name)",
            run_with_stdin(&["filter", "zzz"], home.path(), &tree).0,
        ),
    ] {
        assert_eq!(
            count(&out),
            500,
            "{route} returned {} of 500 refs in {} bytes — the agent cannot click what it \
             cannot name",
            count(&out),
            out.len()
        );
    }

    // Detection has to CLAIM it, too — the rule required a `- generic`
    // wrapper, which a page built from semantic elements never has, and an
    // unclaimed tree comes back whole. Refs alone cannot see that: raw bytes
    // carry all 500 of them.
    let (read_out, _e, _c) = run(&["read", path.to_str().unwrap()], home.path());
    assert!(
        read_out.len() < tree.len() * 9 / 10,
        "the tree was passed through at {} bytes of {} — nothing claimed it",
        read_out.len(),
        tree.len()
    );

    // The other half: the repetition fold is not disabled, it is withheld from
    // handles. Receipts still collapse.
    let receipts: String = (0..400)
        .map(|i| format!("Setting up libpkg{i} (1.0.{i}-1) ...\n"))
        .collect();
    let (folded, _e, _c) = run_with_stdin(&["filter", "apt"], home.path(), &receipts);
    assert!(
        folded.len() * 10 < receipts.len(),
        "a run of receipts must still fold: {} bytes of {}",
        folded.len(),
        receipts.len()
    );
}

/// `tokenade web --prompt "…"` on a page big enough to be stashed must name a
/// way back to the bytes it did not show. The scoped extract REPLACES a body
/// that already carried its `[stashed: …]` pointer, and keeping only the
/// answer threw that pointer away: measured on a 59 552-byte page, the same
/// bytes and the same questions ended on `expand-ref <hash>` through `read
/// --prompt` and on the last line of the answer through `web --prompt`. Worst
/// on the documented pipe form, where the bytes came off the network and are
/// gone from the shell. Both halves are asserted: the pointer is printed, AND
/// the hash it names resolves to content the answer did not show.
#[test]
fn e2e_web_prompt_names_a_way_back() {
    let h = TempHome::new("web_prompt_way_back");
    // The page OPENS on a line that looks like a pointer of ours, and the fold
    // keeps its head — so a search that takes the FIRST match would name that
    // stale hash instead of the one just written. A dead way back reads exactly
    // like a live one and is worse than none.
    let mut html = String::from(
        "<html><head><title>Handbook</title></head><body>\
         <p>[stashed: /tmp/quoted.md \u{b7} ~9 tokens \u{b7} `tokenade expand-ref deadbeefdead`]</p>",
    );
    for (i, topic) in ["key rotation", "session timeout", "vulnerability scanning"]
        .iter()
        .enumerate()
    {
        html.push_str(&format!("<h2>{topic}</h2>"));
        for j in 0..120 {
            html.push_str(&format!(
                "<p>The {topic} section states rule {i}.{j}: operators must review the {topic} \
                 configuration every {} days and record the outcome in ticket SEC-{i}{j:03}.</p>",
                (i + 1) * 3
            ));
        }
    }
    html.push_str("</body></html>");
    let file = h.path().join("handbook.html");
    std::fs::write(&file, &html).unwrap();

    let (out, err, code) = run(
        &[
            "web",
            file.to_str().unwrap(),
            "--prompt",
            "what is the key rotation rule",
        ],
        h.path(),
    );
    assert_eq!(code, 0, "stderr:\n{err}");
    assert!(
        out.len() < html.len() / 2,
        "fixture must actually fold, else the guard asserts nothing: {} vs {}",
        out.len(),
        html.len()
    );
    let pointer = out
        .lines()
        .rev()
        .find(|l| l.starts_with("[stashed: "))
        .unwrap_or_else(|| panic!("no way back named on a folded --prompt answer:\n{out}"));
    assert!(
        !pointer.contains("deadbeefdead"),
        "named the pointer QUOTED BY THE PAGE, not the one just written: {pointer}"
    );

    // The hash it names must resolve, and must hold what the answer left out.
    let hash = pointer
        .split("expand-ref ")
        .nth(1)
        .and_then(|r| r.split_whitespace().next())
        .unwrap_or_else(|| panic!("pointer names no hash: {pointer}"))
        .to_string();
    let needle = "SEC-2119";
    assert!(!out.contains(needle), "fixture too small to have folded");
    let (back, err, code) = run(&["expand-ref", &hash, "--grep", needle], h.path());
    assert_eq!(code, 0, "expand-ref failed; stderr:\n{err}");
    assert!(
        back.contains(needle),
        "the named way back does not hold the folded bytes:\n{back}"
    );
}

/// A batch answer meets the same size bar as a single answer.
///
/// The stash gate ran per PAGE and the answer is the BATCH. Measured
/// 2026-09-06 on ONE body of content split two ways, byte for byte the same:
/// fetched as a single page it came back 4 641 bytes — folded, stashed, and
/// carrying the pointer back. Fetched as eight pages it came back 23 152 bytes
/// with no fold, no stash and no pointer: five times the tokens for the same
/// information, decided by nothing but how the agent packaged the request —
/// and the packaging this product's own guidance tells agents to prefer.
#[test]
fn e2e_a_web_batch_meets_the_same_size_bar_as_one_page() {
    let h = TempHome::new("web-batch-size-bar");
    let dir = h.path().join("pages");
    std::fs::create_dir_all(&dir).unwrap();
    let tag = format!(
        "{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    let mut parts: Vec<String> = Vec::new();
    let mut paths: Vec<String> = Vec::new();
    for k in 1..=8 {
        let body: String = (0..28)
            .map(|i| {
                format!(
                    "<h2>S{i} p{k} {tag}</h2><p>Para {i} page {k} {tag}, a moderately long \
                 sentence so the page is a real one.</p>"
                )
            })
            .collect();
        let p = dir.join(format!("p{k}.html"));
        std::fs::write(
            &p,
            format!("<html><head><title>Small {k} {tag}</title></head><body>{body}</body></html>"),
        )
        .unwrap();
        paths.push(p.display().to_string());
        parts.push(body);
    }
    let all = dir.join("all.html");
    std::fs::write(
        &all,
        format!(
            "<html><head><title>All {tag}</title></head><body>{}</body></html>",
            parts.concat()
        ),
    )
    .unwrap();

    let pointed = |s: &str| s.contains("[stashed:") || s.contains("[disclose:");

    let (one, _e, _c) = run(&["web", all.to_str().unwrap()], h.path());
    // Anti-vacuity: if the single page does not clear the bar either, the
    // comparison below is between two passthroughs and proves nothing.
    assert!(
        pointed(&one),
        "the single-page control was not folded, so this fixture is under the \
         gate and the guard is vacuous ({} bytes)",
        one.len()
    );

    let mut args: Vec<&str> = vec!["web"];
    args.extend(paths.iter().map(|s| s.as_str()));
    let (many, _e, _c) = run(&args, h.path());
    assert!(
        pointed(&many),
        "the same content as {} pages came back with no fold and no pointer \
         ({} bytes, against {} for one page): the size gate is per page and \
         the answer is the batch",
        paths.len(),
        many.len(),
        one.len()
    );
    // Same information, so the two answers must be of the same ORDER — not
    // five times apart.
    assert!(
        many.len() <= one.len() * 2,
        "the batch answer is {} bytes against {} for the identical content in \
         one page",
        many.len(),
        one.len()
    );
}

/// A question asked of several pages is answered, not discarded.
///
/// `--prompt` was announced as ignored for a batch — "applies to a single
/// target" — while the single-target path scopes the page to the question and
/// carries its stash pointer over. Measured 2026-09-06 on two local pages each
/// hiding one figure: prompted, one page answered in 1 038 bytes and the pair
/// returned 9 644 with the flag thrown away. The caller paid 9.3x and did the
/// finding themselves, on the call shape this product's own guidance tells
/// agents to prefer.
#[test]
fn e2e_a_prompt_is_answered_for_every_page_of_a_batch() {
    let h = TempHome::new("web-batch-prompt");
    let dir = h.path().join("pages");
    std::fs::create_dir_all(&dir).unwrap();
    let tag = format!(
        "{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    let mut paths: Vec<String> = Vec::new();
    for k in 1..=2u32 {
        let body: String = (0..150)
            .map(|i| {
                format!(
                    "<h2>Topic {i} on page {k}</h2><p>Page {k} paragraph {i} of {tag} with \
                 filler prose so the page is substantial enough to matter.</p>"
                )
            })
            .collect();
        let fact = format!(
            "<h2>Pricing</h2><p>The plan for page {k} costs {} euros per seat per \
             month in {tag}.</p>",
            k * 37
        );
        let p = dir.join(format!("p{k}.html"));
        std::fs::write(
            &p,
            format!(
                "<html><head><title>Doc {k} {tag}</title></head><body>{body}{fact}</body></html>"
            ),
        )
        .unwrap();
        paths.push(p.display().to_string());
    }
    let q = "how much does the plan cost per seat";

    let (plain, _e, _c) = run(&["web", &paths[0], &paths[1]], h.path());
    let (asked, _e, _c) = run(&["web", &paths[0], &paths[1], "--prompt", q], h.path());

    assert!(
        !asked.contains("ignoring it for a batch"),
        "the batch still throws the question away:\n{}",
        &asked[..asked.len().min(300)]
    );
    // Both pages answered, each with its own figure.
    for k in 1..=2u32 {
        assert!(
            asked.contains(&format!("costs {} euros", k * 37)),
            "page {k}'s figure is missing from the scoped answer:\n{}",
            &asked[..asked.len().min(800)]
        );
    }
    // Anti-vacuity: if the un-prompted batch were already this small there
    // would be nothing to save and the comparison would prove nothing.
    assert!(
        plain.len() > asked.len() * 2,
        "the un-prompted batch is {} bytes and the scoped one {} — this fixture \
         no longer shows the difference the guard is about",
        plain.len(),
        asked.len()
    );
    // The way back survives the replacement — one pointer per page.
    assert!(
        asked.matches("expand-ref").count() >= 2 || asked.matches("[stashed:").count() >= 2,
        "the scoped answer replaced bodies that carried a pointer and kept none \
         of them:\n{}",
        &asked[..asked.len().min(800)]
    );
}

/// `tokenade web` on a LOCAL prose file must keep the file's line structure.
///
/// `serves_plain_text` defaults an absent content-type to the HTML path, which
/// is right for a fetch (an unlabelled web response overwhelmingly IS HTML) and
/// wrong for a file: nobody omitted a header, there was never a server, and the
/// name carries the label. Left on that default the HTML parser saw the whole
/// document as one text node and collapsed every newline in it — MEASURED on
/// 1.1.8, a three-heading markdown file went in as 11 lines and came out as
/// **1**, headings fused into the paragraphs around them. Downstream that is
/// what `--prompt` reads, so its answers came back cut mid-sentence and with
/// passages repeated: on this repo's PATCHNOTES.md, 2 headings and 2 duplicated
/// sentences, against 7 and 0 once the lines survive.
///
/// The `.html` control is load-bearing in the other direction: it must STILL
/// take the HTML path, or the fix has simply turned the conversion off.
#[test]
fn e2e_a_local_prose_file_keeps_its_line_structure() {
    let h = TempHome::new("web-local-prose-lines");
    let dir = h.path().join("docs");
    std::fs::create_dir_all(&dir).unwrap();

    let md = "# Alpha\n\nFirst paragraph about a fox crossing the river bank.\n\n\
              ## Beta\n\nSecond paragraph, distinct, mentioning the lazy dog.\n\n\
              ## Gamma\n\nThird paragraph concerning mountains and nothing else.\n";

    // Every prose extension the local-path label covers, plus the HTML control.
    for ext in ["md", "markdown", "txt", "rst"] {
        let p = dir.join(format!("doc.{ext}"));
        std::fs::write(&p, md).unwrap();
        let (stdout, err, code) = run(&["web", p.to_str().unwrap()], h.path());
        assert_eq!(code, 0, "web on .{ext} failed; stderr:\n{err}");
        let body: Vec<&str> = stdout
            .lines()
            .filter(|l| !l.trim_start().starts_with('['))
            .collect();
        // The defect collapsed the document onto ONE line. Anti-vacuity: the
        // fixture really does have three headings and three paragraphs.
        assert!(
            body.iter().filter(|l| !l.trim().is_empty()).count() >= 6,
            ".{ext} came back collapsed ({} non-empty line(s)); the file's own \
             line structure must survive:\n{stdout}",
            body.iter().filter(|l| !l.trim().is_empty()).count()
        );
        for head in ["# Alpha", "## Beta", "## Gamma"] {
            assert!(
                body.iter().any(|l| l.trim() == head),
                ".{ext}: heading {head:?} must stand on its own line, not be \
                 fused into the prose around it:\n{stdout}"
            );
        }
    }

    // The BATCH route is a second implementation of the same fetch, and it
    // had the same `None`. Every `web` behaviour so far has had to be given to
    // both routes separately (site readers, provenance notes, the size gate,
    // `--prompt`), so a fix proven on one target is not proven at two.
    let p1 = dir.join("one.md");
    let p2 = dir.join("two.md");
    std::fs::write(&p1, md).unwrap();
    std::fs::write(&p2, "# Delta\n\nFourth paragraph about the open sea.\n").unwrap();
    let (stdout, err, code) = run(
        &["web", p1.to_str().unwrap(), p2.to_str().unwrap()],
        h.path(),
    );
    assert_eq!(code, 0, "web batch failed; stderr:\n{err}");
    for head in ["# Alpha", "## Beta", "## Gamma", "# Delta"] {
        assert!(
            stdout.lines().any(|l| l.trim() == head),
            "batch: heading {head:?} must stand on its own line:\n{stdout}"
        );
    }

    // Control: HTML must still be CONVERTED, not passed through. If this file
    // came back with its tags intact the fix has disabled the pipeline.
    //
    // The fixture is a FRAGMENT, with no `<html>` or doctype opening it. A full
    // document is the weaker test: `serves_plain_text` overrides a plain label
    // when the body announces itself as HTML, so a full document survives even
    // a mapping that wrongly calls `.html` prose — a mutation that did exactly
    // that lived through this control until the fixture stopped opening with a
    // tag the override recognises. A saved page fragment is also the realistic
    // shape on disk.
    let hp = dir.join("doc.html");
    std::fs::write(
        &hp,
        "<h1>Alpha</h1><p>First paragraph about a fox crossing the river \
         bank.</p><h2>Beta</h2><p>Second paragraph, distinct, mentioning the \
         lazy dog.</p>",
    )
    .unwrap();
    let (stdout, err, code) = run(&["web", hp.to_str().unwrap()], h.path());
    assert_eq!(code, 0, "web on .html failed; stderr:\n{err}");
    assert!(
        !stdout.contains("<h1>") && !stdout.contains("<p>"),
        "an .html file must still go through html_to_markdown:\n{stdout}"
    );
    assert!(
        stdout.lines().any(|l| l.trim() == "# Alpha"),
        "the HTML control must still be converted to markdown headings:\n{stdout}"
    );
}

/// A page already delivered in this session folds to a pointer on the BATCH
/// route too, and a batch RECORDS what it delivered.
///
/// `web_replay` lived on the single-target path alone. Measured 2026-09-06 on
/// one loopback page: fetched twice on its own it came back 9 259 bytes then
/// 526; fetched twice as a pair it came back 18 601 bytes then 18 601, byte for
/// byte. The second half was worse than the first — a batch recorded nothing,
/// so a single fetch of the same URL *after* a batch also came back whole. The
/// packaging this product's own guidance tells agents to prefer ("several URLs
/// in ONE call") threw the saving away and took the other route's with it.
/// Third divergence between these two paths, after the size gate and
/// `--prompt`.
///
/// Four arms, because four different things can break: the fold on a repeated
/// batch, the fold for EVERY page of it rather than the first, the record a
/// batch leaves for a later single fetch, and the single path's own fold, which
/// now goes through the same shared spelling.
///
/// Loopback, so the guard does not depend on somebody else's uptime.
#[test]
fn e2e_web_replay_folds_on_the_batch_route_too() {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();
    // Bounded on both ends, like the status guard above: it serves the seven
    // requests the four arms make, and it stops at the deadline regardless.
    listener
        .set_nonblocking(true)
        .expect("nonblocking loopback listener");
    let server = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let mut served = 0;
        while served < 7 && std::time::Instant::now() < deadline {
            let (mut sock, _) = match listener.accept() {
                Ok(pair) => pair,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                Err(_) => return,
            };
            let _ = sock.set_nonblocking(false);
            served += 1;
            let mut line = String::new();
            let _ = BufReader::new(sock.try_clone().unwrap()).read_line(&mut line);
            // Byte-identical on every request: replay folds identical bodies
            // and only identical bodies, so a server that varied would be
            // testing the freshness guard instead of this one.
            let which = if line.contains("/two") { "two" } else { "one" };
            // DISTINCT paragraphs. Forty identical ones collapse to a single
            // line in the ordinary page pipeline, and the 187-byte page left
            // over is below the size replay bothers with — the fixture would
            // have measured the compactor and called it a replay failure.
            let paras: String = (0..60)
                .map(|i| {
                    format!("<p>paragraph {i} of page {which}, ordinary prose about compaction</p>")
                })
                .collect();
            let body = format!(
                "<html><head><title>Page {which}</title></head><body>{paras}</body></html>"
            );
            let _ = write!(
                sock,
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.flush();
        }
    });

    let h = TempHome::new("web-replay-batch");
    let one = format!("http://127.0.0.1:{port}/one");
    let two = format!("http://127.0.0.1:{port}/two");

    let (first, _e, _c) = run_with_session(&["web", &one, &two], h.path(), "replaybatch");
    let (again, _e, _c) = run_with_session(&["web", &one, &two], h.path(), "replaybatch");
    // The record a batch leaves: the SAME session then asks for one of those
    // pages on its own.
    let (after_batch, _e, _c) = run_with_session(&["web", &one], h.path(), "replaybatch");
    // The single path's own fold, through the shared helper, in its own
    // session so the batch above cannot be what folds it.
    let (solo_first, _e, _c) = run_with_session(&["web", &one], h.path(), "replaysolo");
    let (solo_again, _e, _c) = run_with_session(&["web", &one], h.path(), "replaysolo");
    let _ = server.join();

    // Arm 1 — a repeated batch does not re-send what it already sent.
    assert!(
        again.len() * 4 < first.len(),
        "a repeated batch should fold to a pointer, not re-send the pages: \
         first {} bytes, again {} bytes\n--- again ---\n{again}",
        first.len(),
        again.len()
    );
    // Arm 2 — EVERY page of the batch, not just the first one emitted.
    for url in [&one, &two] {
        // The pointer must be in THAT page's section, not merely somewhere in
        // the blob: folding only the first target would satisfy a loose
        // `contains` on both passes of this loop.
        let section = again
            .split("\n# ")
            .find(|s| s.starts_with(url.as_str()))
            .unwrap_or_else(|| panic!("no section for {url}\n{again}"));
        assert!(
            section.contains("not re-sent") && section.contains("expand-ref"),
            "every page of the batch owes a replay pointer naming its recovery \
             path; {url} did not carry one\n--- its section ---\n{section}"
        );
    }
    // Arm 3 — what the batch delivered is remembered for a later single fetch.
    assert!(
        after_batch.contains("not re-sent"),
        "a single fetch after a batch should fold: the batch has to RECORD what \
         it delivered\n{after_batch}"
    );
    // Arm 4 — the single path still folds on its own, through the shared
    // spelling this fix moved it to.
    assert!(
        solo_again.contains("not re-sent") && solo_again.len() * 4 < solo_first.len(),
        "the single path's own fold regressed: first {} bytes, again {} bytes\n{solo_again}",
        solo_first.len(),
        solo_again.len()
    );
}

/// A batch whose pages ALL folded to replay pointers keeps those pointers.
///
/// The whole-answer size gate summarises the assembly only when no part
/// already carries a pointer — otherwise it cuts those pointers out of the
/// middle, which is what that condition exists to prevent. It tested the text
/// for `[stashed:` and `[tokenade:`, and a replay note opens `[tokenade]`. So a
/// batch of pages this session had already delivered read as "nothing is
/// pointed at" and was eligible to have every one of its per-URL pointers
/// replaced by a single pointer to the assembly — losing which URL was which,
/// and each page's own recovery hash. The gate now asks whether a fold
/// HAPPENED rather than looking for its spelling.
///
/// The fixture is sized, not guessed. Three constraints have to hold at once:
/// a page must stay ABOVE the 400-token floor below which replay does not
/// bother folding, must stay BELOW the disclosure threshold so the per-page
/// gate does not summarise it first (a summarised page is small, drops under
/// that floor, and then nothing folds at all — measured: pages at a threshold
/// of 40 folded zero times), and the assembled pointers must stand ABOVE the
/// threshold or the gate under test never fires. MEASURED on this fixture: a
/// page is 800 tokens and a pointer 59, so twenty-four pages at a threshold of
/// 1000 clears all three — 1 154 bytes on the first pass, 6 677 with
/// twenty-four pointers on the second.
#[test]
fn e2e_a_fully_folded_batch_keeps_its_per_page_pointers() {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    const PAGES: usize = 24;
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();
    listener
        .set_nonblocking(true)
        .expect("nonblocking loopback listener");
    let server = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
        let mut served = 0;
        while served < PAGES * 2 && std::time::Instant::now() < deadline {
            let (mut sock, _) = match listener.accept() {
                Ok(pair) => pair,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                Err(_) => return,
            };
            let _ = sock.set_nonblocking(false);
            served += 1;
            let mut line = String::new();
            let _ = BufReader::new(sock.try_clone().unwrap()).read_line(&mut line);
            let which = line
                .split_whitespace()
                .nth(1)
                .unwrap_or("/p0")
                .trim_start_matches('/')
                .to_string();
            let paras: String = (0..60)
                .map(|i| format!("<p>paragraph {i} of page {which}, ordinary prose</p>"))
                .collect();
            let body = format!(
                "<html><head><title>Page {which}</title></head><body>{paras}</body></html>"
            );
            let _ = write!(
                sock,
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.flush();
        }
    });

    let h = TempHome::new("web-replay-allfolded");
    let urls: Vec<String> = (0..PAGES)
        .map(|i| format!("http://127.0.0.1:{port}/p{i}"))
        .collect();
    let mut args = vec!["web"];
    args.extend(urls.iter().map(String::as_str));
    let env = [
        ("TOKENADE_SESSION", "replayallfolded"),
        ("TOKENADE_DISCLOSE_THRESHOLD", "1000"),
    ];
    let (first, _e, _c) = run_with_env(&args, h.path(), &env);
    let (again, _e, _c) = run_with_env(&args, h.path(), &env);
    let _ = server.join();

    // Anti-vacuity: the first pass folded NOTHING, so the second pass's folds
    // are real work and not a fixture that was already pointing at itself.
    //
    // Not "the first pass is bigger": at this threshold it is SMALLER, because
    // nothing in it is pointed at and the whole assembly collapses to one
    // stash pointer, while the second pass keeps twenty-four. That is the
    // behaviour under test, so sizing the guard on it would have inverted it.
    assert!(
        !first.contains("not re-sent"),
        "a cold session cannot have folded anything\n--- first ---\n{first}"
    );
    // The OTHER side of the same gate, and it has to be here: a fix that
    // simply declared every batch "already pointed at" would keep all
    // twenty-four pointers below and satisfy every other assertion in this
    // test, while quietly disabling the whole-answer size gate for every batch
    // that folded nothing — which is most of them. So the cold pass, with
    // nothing pointed at, owes the caller the summary.
    assert!(
        first.contains("[disclose:summary") || first.contains("[stashed:"),
        "a batch that folded nothing must still meet the whole-answer size \
         gate\n--- first ---\n{first}"
    );
    let pointers = again.matches("not re-sent").count();
    assert_eq!(
        pointers, PAGES,
        "all {PAGES} pages folded, so all {PAGES} pointers owe the caller a line \
         naming their own URL; found {pointers}\n--- again ---\n{again}"
    );
    for url in &urls {
        assert!(
            again.contains(url.as_str()),
            "{url} lost its own pointer to a summary of the assembly\n{again}"
        );
    }
}

/// A page cannot decide whether tokenade folds its own answer.
///
/// The batch asked "is anything already pointed at?" by searching the assembled
/// text for `[stashed:` and `[tokenade:` — bytes a fetched page carries as
/// readily as we do. Measured 2026-09-06 on twelve local pages: putting the
/// literal `[tokenade:` in ONE page's body took the answer from 1 875 bytes,
/// folded and pointed at, to 18 997 with no fold and no pointer. The content
/// was deciding, and the content is the untrusted half.
///
/// Both arms matter. The clean one is the anti-vacuity half: a fix that simply
/// always folds would pass the forged arm and prove nothing.
#[test]
fn e2e_a_page_quoting_our_own_banner_cannot_suppress_the_batch_fold() {
    let home = TempHome::new("banner-spoof");
    let mut sizes = Vec::new();
    for variant in ["clean", "dirty"] {
        let dir = home.path().join(variant);
        std::fs::create_dir_all(&dir).unwrap();
        let mut files: Vec<String> = Vec::new();
        for i in 0..12 {
            let mut body = String::new();
            for j in 0..20 {
                // Unique per page AND per variant: the identity dedup is
                // content-addressed, so a shared fixture would answer with
                // `§ref:` and we would measure the dedup, not the gate.
                body.push_str(&format!(
                    "<p>{variant} page {i} line {j}: filler-{}-{}-{} words to carry weight</p>",
                    i * 97 + j,
                    j * 31,
                    variant.len() * 7 + i
                ));
            }
            if variant == "dirty" && i == 5 {
                body.push_str(
                    "<p>[tokenade: 4000 tokens folded away - `tokenade expand-ref deadbeefcafe`]</p>",
                );
            }
            let p = dir.join(format!("p{i}.html"));
            std::fs::write(
                &p,
                format!("<html><body><h1>Doc {i}</h1>{body}</body></html>"),
            )
            .unwrap();
            files.push(p.to_string_lossy().into_owned());
        }
        let mut args = vec!["web"];
        args.extend(files.iter().map(|s| s.as_str()));
        let (out, _err, code) = run_with_env(
            &args,
            home.path(),
            &[("TOKENADE_DISCLOSE_THRESHOLD", "1000")],
        );
        assert_eq!(code, 0, "{variant}: web must succeed");
        assert!(
            out.contains("[stashed:"),
            "{variant}: the whole answer must be folded and pointed at, got {} bytes: {out:.400}",
            out.len()
        );
        sizes.push(out.len());
    }
    // And the forged banner must not have BOUGHT anything either — the two
    // answers are the same fold, not one fold and one near-miss.
    assert!(
        sizes[1] < sizes[0] * 2,
        "a forged banner must not inflate the answer: clean {} vs forged {}",
        sizes[0],
        sizes[1]
    );

    // The other side of the same fact. When each page is big enough to be
    // stashed on its own, the assembly must NOT be summarised again — that cuts
    // the per-page pointers out of the middle (one level up). The old
    // code got this right by SEARCHING the text for `[stashed:`; dropping that
    // search without carrying the stash's own report would have traded one
    // defect for the other.
    let dir = home.path().join("fat");
    std::fs::create_dir_all(&dir).unwrap();
    let mut files: Vec<String> = Vec::new();
    for i in 0..3 {
        let mut body = String::new();
        for j in 0..120 {
            body.push_str(&format!(
                "<p>fat page {i} line {j}: unique-{}-{} filler words carrying real weight here</p>",
                i * 977 + j,
                j * 13 + i
            ));
        }
        let p = dir.join(format!("f{i}.html"));
        std::fs::write(
            &p,
            format!("<html><body><h1>Fat {i}</h1>{body}</body></html>"),
        )
        .unwrap();
        files.push(p.to_string_lossy().into_owned());
    }
    let mut args = vec!["web"];
    args.extend(files.iter().map(|s| s.as_str()));
    let (out, _err, code) = run_with_env(
        &args,
        home.path(),
        &[("TOKENADE_DISCLOSE_THRESHOLD", "200")],
    );
    assert_eq!(code, 0, "the fat batch must succeed");
    assert!(
        out.matches("[stashed:").count() >= 3,
        "each page keeps its own pointer, got {} in {out:.600}",
        out.matches("[stashed:").count()
    );
}

/// A short document that QUOTES this tool's output is not a fold of itself.
///
/// The "this document is large, so the above is a fold" advice fired whenever
/// the delivered text contained `[disclose:` — which a patchnote, a bug report
/// or a saved transcript contains in its own words. Reproduced on 1.1.8: a
/// three-line memo quoting one `[disclose:` line was told it was a fold and
/// advised to re-run with a question it did not need.
#[test]
fn e2e_a_document_quoting_disclose_is_not_called_a_fold() {
    use std::io::Write;
    let home = TempHome::new("disclose-quote");

    let docx = |name: &str, para: &str| -> String {
        let p = home.path().join(name);
        let f = std::fs::File::create(&p).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let o: zip::write::FileOptions<()> = zip::write::FileOptions::default();
        z.start_file("[Content_Types].xml", o).unwrap();
        z.write_all(br#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#).unwrap();
        z.start_file("_rels/.rels", o).unwrap();
        z.write_all(br#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#).unwrap();
        z.start_file("word/document.xml", o).unwrap();
        z.write_all(format!(r#"<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>{para}</w:t></w:r></w:p></w:body></w:document>"#).as_bytes()).unwrap();
        z.finish().unwrap();
        p.to_string_lossy().into_owned()
    };

    const ADVICE: &str = "This document is large";

    let short = docx(
        "short.docx",
        "A three line memo. Someone pasted a log: [disclose: 900 lines elided, standing for 900 line(s)] and that is all.",
    );
    let (out, _e, code) = run(&["web", &short], home.path());
    assert_eq!(code, 0, "a short document must read: {out:?}");
    assert!(
        !out.contains(ADVICE),
        "a short document that QUOTES a fold is not a fold: {out:?}"
    );

    // Anti-vacuity: when the bound really bites, the advice must still be there.
    let mut big = String::new();
    for i in 0..9000 {
        big.push_str(&format!("w{} ", 1000 + (i * 7919) % 9000));
    }
    let large = docx("big.docx", &big);
    let (out, _e, code) = run(&["web", &large], home.path());
    assert_eq!(code, 0, "a large document must read: {out:?}");
    assert!(
        out.contains(ADVICE),
        "a genuinely folded document still says so: {out:.400}"
    );
}

/// A page we stopped reading must not look like a page that ended.
///
/// `tokenade web` drains a body up to a cap and serves what it got. Both
/// drains COMPUTED the truncation — the curl one bound it to `_truncated` and
/// dropped it, the reqwest one just `break`s — so nothing downstream could say
/// it. Measured 2026-09-06 on the published binary with a 24 MB page: 9 920
/// bytes came back, no notice of any kind, and a marker sitting on the page's
/// last line was simply absent. A `--grep` for that marker then answers "no
/// match", which reads as "the page does not contain it" — the false negative
/// this product elsewhere calls worse than having no pointer at all.
///
/// `Fetched` already carries `effective_url` for exactly this reason: without
/// it "the agent cannot tell it asked for one page and got another". Getting
/// PART of one is the same class of surprise.
#[test]
fn e2e_a_body_cut_at_the_cap_says_so() {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    let home = TempHome::new("body-cut");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();

    // ~600 KB, with a marker on the FIRST and the LAST line. The cap is driven
    // by env below, so the fixture stays small and the test stays fast.
    let mut body = String::from("<html><body><h1>MARKER-START</h1>");
    for i in 0..9_000 {
        body.push_str(&format!(
            "<p>ordinary sentence {i} of page text a reader might care about</p>"
        ));
    }
    body.push_str("<p>MARKER-END-9f3a1c</p></body></html>");

    let served = body.clone();
    let t = std::thread::spawn(move || {
        for stream in listener.incoming().take(2) {
            let Ok(mut s) = stream else { continue };
            {
                let mut r = BufReader::new(&s);
                let mut line = String::new();
                while r.read_line(&mut line).unwrap_or(0) > 0 {
                    if line == "\r\n" || line == "\n" {
                        break;
                    }
                    line.clear();
                }
            }
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                served.len()
            );
            let _ = s.write_all(served.as_bytes());
            let _ = s.flush();
        }
    });

    const CUT: &str = "BEGINNING of it";

    // Cap well under the page: the body is cut and the answer must say so.
    let url = format!("http://127.0.0.1:{port}/big");
    let (out, _err, code) = run_with_env(
        &["web", &url],
        home.path(),
        &[("TOKENADE_WEB_FETCH_CAP", "100000")],
    );
    assert_eq!(code, 0, "the fetch must succeed: {out:.200}");
    assert!(
        !out.contains("MARKER-END-9f3a1c"),
        "the fixture must actually be cut for this to test anything"
    );
    assert!(
        out.contains(CUT),
        "a cut body must say it is only the beginning: {out:.400}"
    );

    // Anti-vacuity: the same page under a cap that fits arrives whole and says
    // nothing. A notice printed unconditionally would pass the arm above and
    // teach the reader to ignore it.
    let (out, _err, code) = run_with_env(
        &["web", &format!("{url}?whole=1")],
        home.path(),
        &[("TOKENADE_WEB_FETCH_CAP", "99999999")],
    );
    assert_eq!(code, 0, "the fetch must succeed: {out:.200}");
    assert!(
        out.contains("MARKER-END-9f3a1c"),
        "the whole page must arrive under a cap that fits: {out:.400}"
    );
    assert!(
        !out.contains(CUT),
        "nothing was cut, so nothing may be claimed: {out:.400}"
    );

    let _ = t.join();
}

/// **An oversized page is delivered as its OUTLINE, never as its body.**
///
/// A change stashed the whole page — which was right — by passing the capped
/// view as the BODY to `disclose`. But a caller-supplied body short-circuits
/// the mode choice: the delivery is then the body itself. MEASURED on
/// `pages/zhwiki`, the only corpus page that reaches the hard cap: **2 783
/// characters delivered before, 18 523 after** — six times the outline, for
/// the very page the product exists not to dump. Green across the 111
/// binaries: no test looked at the SIZE of what is delivered.
#[test]
fn e2e_a_capped_page_is_delivered_as_its_outline_not_its_body() {
    let h = TempHome::new("capped_outline");
    let page = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tokenade-core/tests/fixtures/pages/zhwiki.html");
    let (out, err, code) = run(&["web", page.to_str().unwrap()], &h.dir);
    assert_eq!(code, 0, "{err}");
    let delivered = out.chars().count();
    // The rule is REACHED: this page does go through the stash.
    let stash = out
        .lines()
        .find(|l| l.contains("[stashed:"))
        .unwrap_or_else(|| panic!("the page was not stashed:\n{out}"));
    let path = stash
        .split_once("[stashed: ")
        .and_then(|(_, r)| r.split_once(' '))
        .map(|(p, _)| p.to_string())
        .unwrap_or_else(|| panic!("no path in {stash}"));
    let stashed = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    // The stash holds the PAGE — at least ten times what is delivered — and
    // the delivery stays an outline.
    assert!(
        stashed.chars().count() > delivered * 10,
        "stash of {} chars for {delivered} delivered: it is not the page that is stashed",
        stashed.chars().count()
    );
    assert!(
        delivered < 6_000,
        "{delivered} characters delivered: that is the body, not the outline"
    );
}

/// **A question asked of a capped page is asked of what remains after the
/// answer was thrown away.**
///
/// `--prompt` received `d.md` — the view AFTER the hard cap, a sample of
/// lines that keeps the headings and throws away the body. MEASURED on the
/// Claude Code hooks page (2.8 MB of HTML, 233 795 characters of markdown)
/// with `--prompt updatedToolOutput`: the term appears SEVEN times in the
/// page's markdown and ZERO times in the capped view, so the extractor found
/// no anchor and delivered "nothing in this source answers that question"
/// followed by the whole outline — **5 089 bytes without the answer, versus
/// 6 032 with it after the fix**. `full_md` already carried the pre-cut
/// markdown, for the stash; it now carries the answer too.
#[test]
fn e2e_a_question_is_answered_from_the_page_not_from_the_cap() {
    let h = TempHome::new("prompt_reads_the_page");
    // Big enough to REACH the hard cap (50 000 tokens), and the answer lives
    // in a paragraph, not a heading: the cap's salvage keeps headings, so a
    // word in a heading would survive the cut and the test would be green
    // without proving anything.
    let mut html = String::from("<html><head><title>Manual</title></head><body>");
    for s in 0..40 {
        html.push_str(&format!("<h2>Section {s}</h2>"));
        for p in 0..100 {
            if s == 35 && p == 50 {
                html.push_str(
                    "<p>The <code>zephyrCalibrationHandle</code> field carries the \
                     calibration token returned by the probe.</p>",
                );
            } else {
                html.push_str(&format!(
                    "<p>Paragraph {p} of section {s} describes the ordinary operation of \
                     the subsystem and repeats the same wording as its neighbours.</p>"
                ));
            }
        }
    }
    html.push_str("</body></html>");
    let file = h.path().join("manual.html");
    std::fs::write(&file, &html).unwrap();
    let p = file.to_str().unwrap();

    // The reachability floor: without a question, the page is capped AND the
    // term is indeed gone from what is delivered. If that stops being true,
    // the test below measures nothing.
    let (plain, err, c) = run(&["web", p], h.path());
    assert_eq!(c, 0, "stderr:\n{err}");
    assert!(
        !plain.contains("zephyrCalibrationHandle"),
        "the page is not capped: the test proves nothing\n{plain:.400}"
    );

    let (out, err, c) = run(&["web", p, "--prompt", "zephyrCalibrationHandle"], h.path());
    assert_eq!(c, 0, "stderr:\n{err}");
    assert!(
        out.contains("zephyrCalibrationHandle"),
        "the question was asked of the cut, not of the page:\n{out:.600}"
    );
    assert!(
        !out.contains("nothing in this source answers that question"),
        "the answer is in the page; the refusal comes from the cut:\n{out:.600}"
    );
}

/// **Only a question that was asked can go unanswered.**
///
/// Splitting the extraction in two (pure computation before the stash,
/// re-gluing after), `attach_scoped(delivered, None)` stopped distinguishing
/// "no question" from "question asked, nothing anchored". Result measured on
/// the 27 real pages from the transcripts: **27 out of 27** received "nothing
/// in this source answers that question — what follows is the source itself,
/// not an answer", 108 characters each, without any question being asked.
///
/// No test looked at what a page WITHOUT `--prompt` receives: the corpus
/// assertions are about the delivered content, and one extra line at the top
/// does not break them. It was the boilerplate census that caught it.
#[test]
fn e2e_a_page_without_a_question_is_never_told_it_has_no_answer() {
    let h = TempHome::new("no_question_no_note");
    let file = h.path().join("page.html");
    // Above `UNANSWERED_NOTE_MIN_CHARS` (900): below that floor the note is
    // not written at all, and the second half of the test would pass for a
    // reason that is not its own.
    let mut html = String::from("<html><head><title>Guide</title></head><body>");
    for i in 0..12 {
        html.push_str(&format!(
            "<h2>Section {i}</h2><p>Run the installer for step {i} and follow the prompts \
             through to completion, then invoke the command with a path to a file.</p>"
        ));
    }
    html.push_str("</body></html>");
    std::fs::write(&file, &html).unwrap();
    let p = file.to_str().unwrap();

    let (out, err, c) = run(&["web", p], h.path());
    assert_eq!(c, 0, "stderr:\n{err}");
    assert!(
        !out.contains("nothing in this source answers that question"),
        "no question was asked:\n{out}"
    );

    // And the reachability floor: the note still EXISTS when a question is
    // asked and nothing answers it, otherwise this test would also pass on a
    // product that had simply removed it.
    let (asked, err, c) = run(
        &[
            "web",
            p,
            "--prompt",
            "quelle est la politique de remboursement",
        ],
        h.path(),
    );
    assert_eq!(c, 0, "stderr:\n{err}");
    assert!(
        asked.contains("nothing in this source answers that question"),
        "an unanswered question must be announced as such:\n{asked}"
    );
}
