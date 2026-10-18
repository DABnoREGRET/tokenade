//! Three invariants about what a fold OWES its reader, checked on the real
//! delivery path.
//!
//! They came out of a run of transcript audits where each was
//! first measured by hand and then found something. They are guards here so
//! the measuring does not have to be repeated:
//!
//! 1. a fold that removes content DECLARES itself,
//! 2. every recovery route a banner names actually RUNS,
//! 3. every count a banner states is TRUE of the content it replaced.
//!
//! Two instrument rules are baked in, both paid for:
//!
//! * drive `tokenade wrap`, never `tokenade filter` — `filter` is the "I
//!   already have the output" utility, it writes no stash, so it has no
//!   `expand-ref` footer and reports six honest folds as silent;
//! * make every payload UNIQUE. The identity dedup is content-addressed, so a
//!   fixture reused between cases comes back as `§ref:`/`exec-dedup` and the
//!   dedup answer gets measured instead of the compactor under test.

use super::*;

/// What counts as declaring itself.
///
/// Three spellings, and the third was learned here: `[tokenade:` banners,
/// `[disclose:` summaries, and a shape/table header — `[×400 rows · toon]`,
/// `[×600 lines · distinct …]` — which states how many things the rendering
/// stands for. The invariant is about SILENT loss, and a re-encoding that
/// names its own row count is not silent. A JSON array rendered as a table is
/// the case that taught it: 11 781 → 8 206 bytes, nothing removed, and no
/// `[tokenade` marker to find.
fn declares(out: &str) -> bool {
    out.contains("[tokenade") || out.contains("[disclose:") || out.contains("[×")
}

/// Payload builders, each seeded with `tag` so no two runs of the suite — and
/// no two cases within a run — share content.
fn corpus(dir: &std::path::Path, tag: &str) -> Vec<(String, String, usize)> {
    let w = |name: &str, text: String| {
        let p = dir.join(name);
        std::fs::write(&p, &text).unwrap();
        let lines = text.lines().count();
        (format!("cat {}", p.display()), text, lines)
    };
    vec![
        // Strip-only on purpose: heavily commented but under the truncation
        // bar, so the STRIP banner is the only declaration that can appear. A
        // bigger file also trips `cat-large`, whose banner would mask a silent
        // strip — which is exactly how the first version of this guard passed
        // with that fix reverted.
        w(
            "s.py",
            (0..60)
                .map(|i| format!("# {tag} explanation {i} that a reader wants\ndef s{i}(a):\n    return a+{i}\n"))
                .collect::<String>(),
        ),
        w(
            "a.py",
            (0..200)
                .map(|i| format!("# {tag} note {i}\ndef f{i}(a):\n    return a+{i}\n"))
                .collect::<String>(),
        ),
        w(
            "b.log",
            (0..600)
                .map(|i| format!("2026-09-05T10:00:{:02} INFO {tag} handler {i} in {i}ms\n", i % 60))
                .collect::<String>(),
        ),
        w(
            "c.csv",
            format!("a,b,c\n{}", (0..500).map(|i| format!("{tag}r{i},{i},{}\n", i * 2)).collect::<String>()),
        ),
        w(
            "d.ini",
            (0..200)
                .map(|i| format!("; {tag} c{i}\n[s{i}]\nk{i} = v{i}\n"))
                .collect::<String>(),
        ),
        w(
            "e.json",
            format!(
                "[{}]",
                (0..400)
                    .map(|i| format!("{{\"id\":{i},\"n\":\"{tag}-{i}\"}}"))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        ),
        // Four shapes this repository actually got wrong. MEASURED against
        // mutations, they do NOT all guard their own defect, and saying which
        // is which is the difference between a guard and a decoration:
        //
        //   g-table.tsv    KILLS its mutation — reverting the du fix makes
        //                  this corpus fail. The one with guard value here.
        //   f-progress.log does not, and should not: the padding defect was a
        //                  QUALITY fault (twice the rows), never an honesty
        //                  one — the fold declared itself throughout. Its test
        //                  belongs in `logs.rs`, and lives there.
        //   h-blame.txt    reach their compactors by COMMAND NAME, so `cat`ing
        //   i-install.log  their bytes never routes there. They stay because
        //                  they exercise the GENERIC path on unusual shapes,
        //                  which is a real if weaker guard — not because they
        //                  cover the blame/cargo-install fixes, whose mutation-killed tests are
        //                  unit tests next to the code.
        //
        // A progress log whose numbers are right-aligned.
        w(
            "f-progress.log",
            (0..300)
                .map(|i| {
                    format!(
                        "[ {:3}%] Building CXX object src/{tag}/mod{}.cpp.o\n",
                        i * 100 / 300,
                        i % 7
                    )
                })
                .collect::<String>(),
        ),
        // A headerless tab-separated table whose first column is numeric and
        // whose second looks like a path — `gh pr list` in a pipe. It
        // was read as disk usage, and rendered a total that did not exist.
        w(
            "g-table.tsv",
            (1..=90)
                .map(|i| format!("{}\t{tag} record {i}\tfeat/br-{i}\tOPEN\n", 1000 + i))
                .collect::<String>(),
        ),
        // `git blame` output: the author/date block is removed from
        // every line that IS kept, which no banner about folded LINES covers.
        w(
            "h-blame.txt",
            (1..=120)
                .map(|i| {
                    format!(
                        "3f8a4b2c1d ({tag} Author 2026-09-0{} 10:30:00 +0000 {i:4}) let v{i} = f({i});\n",
                        i % 9 + 1
                    )
                })
                .collect::<String>(),
        ),
        // `cargo install`: five whitelisted buckets, everything else
        // gone — including the `note:` line that explains a failure.
        w(
            "i-install.log",
            format!(
                "    Updating crates.io index\n{}  Installing /home/u/.cargo/bin/{tag}\n   Installed package `{tag} v1.0.0` (executable `{tag}`)\nnote: the linker `cc` failed with signal 9\n",
                (0..120)
                    .map(|i| format!("   Compiling dep{i} v0.{i}.0\n"))
                    .collect::<String>()
            ),
        ),
    ]
}

/// A fold that removes content says so.
///
/// The source path was the one that did not: `cat mod.py` / `sed -n 'A,Bp'`
/// returned the stripped text bare, and a removed comment line leaves an EMPTY
/// line behind, so the file read as one that simply has no comments.
#[test]
fn e2e_every_fold_declares_itself() {
    let h = TempHome::new("inv-declare");
    let tag = format!("t{}", std::process::id());
    let mut checked = 0;
    for (cmd, raw, _) in corpus(&h.dir, &tag) {
        let (out, err, _) = run(&["wrap", &cmd], &h.dir);
        let all = format!("{out}{err}");
        if all.len() >= raw.len() {
            continue; // nothing was folded; nothing to declare
        }
        checked += 1;
        assert!(
            declares(&all),
            "{cmd} shrank {} → {} bytes and said nothing:\n{}",
            raw.len(),
            all.len(),
            &all[..all.len().min(300)]
        );
    }
    assert!(
        checked >= 4,
        "only {checked} cases folded — the corpus stopped exercising this"
    );
}

/// The way back a banner names has to run.
///
/// A banner once advertised `skeleton path="<file>"`, which the CLI answers
/// with `No such file or directory`. This runs what the banners actually print.
#[test]
fn e2e_every_recovery_route_a_banner_names_runs() {
    let h = TempHome::new("inv-routes");
    let tag = format!("r{}", std::process::id());
    let bin = binary();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut checked = 0;
    for (cmd, _, _) in corpus(&h.dir, &tag) {
        let (out, err, _) = run(&["wrap", &cmd], &h.dir);
        for piece in format!("{out}{err}").split('`') {
            let s = piece.trim();
            // `tokenade …` invocations only; prose between backticks
            // (`xmllint --xpath`) is not a command. A `<…>` placeholder has no
            // sensible filling and is skipped, but `"q1, q2"` is a TEMPLATE
            // for a real question and gets one — skipping it left the whole
            // `--prompt` family unchecked, and a typo planted in that banner
            // went unnoticed.
            if !s.starts_with("tokenade ") || s.contains('<') {
                continue;
            }
            let s = s.replace("\"q1, q2\"", "\"what does it define, which names appear\"");
            if !seen.insert(s.clone()) {
                continue;
            }
            checked += 1;
            let argv = split_argv(&s);
            let args: Vec<&str> = argv.iter().skip(1).map(String::as_str).collect();
            let (o, e, rc) = run_bin(&bin, &args, &h.dir);
            assert_eq!(rc, 0, "`{s}` exited {rc}:\n{e}");
            assert!(
                !format!("{o}{e}").trim().is_empty(),
                "`{s}` ran but answered nothing"
            );
        }
    }
    assert!(
        checked >= 4,
        "only {checked} routes named — the banners stopped offering one"
    );
}

/// A banner may not claim more lines than the content it replaced.
///
/// Found a strip banner being counted as a line of the file: 450 lines
/// delivered as `451 lines`, with the "N line(s) elided" arithmetic computed
/// against the inflated total.
#[test]
fn e2e_no_banner_claims_more_than_the_input_holds() {
    let h = TempHome::new("inv-counts");
    let tag = format!("c{}", std::process::id());
    let mut checked = 0;
    for (cmd, _raw, lines) in corpus(&h.dir, &tag) {
        let (out, err, _) = run(&["wrap", &cmd], &h.dir);
        let all = format!("{out}{err}");
        for claim in line_counts_claimed(&all) {
            checked += 1;
            assert!(
                claim <= lines,
                "{cmd} claims {claim} lines of a {lines}-line input:\n{}",
                &all[..all.len().min(300)]
            );
        }
    }
    assert!(
        checked >= 3,
        "no countable claim found — the sweep proves nothing"
    );
}

/// Split a printed command into argv, keeping a double-quoted argument whole.
///
/// `split_whitespace` tore `--prompt "what does it define, …"` into four
/// arguments and the command answered `unexpected positional 'does'` — a
/// failure of the test, reported as a failure of the banner.
fn split_argv(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in cmd.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Every "N lines"-shaped number a banner states: `[×N lines]`, `N line(s)`,
/// `· N total`. Deliberately a plain scan rather than per-compactor parsing —
/// a new compactor is covered the day it prints a count.
fn line_counts_claimed(text: &str) -> Vec<usize> {
    let mut out = Vec::new();
    for (i, _) in text.match_indices("line") {
        // Walk back over "s"/"(s)" and spaces to the digits before the word.
        let before = &text[..i];
        let digits: String = before
            .trim_end()
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if digits.is_empty() {
            continue;
        }
        if let Ok(n) = digits.chars().rev().collect::<String>().parse::<usize>() {
            out.push(n);
        }
    }
    for (i, _) in text.match_indices("total") {
        let before = text[..i].trim_end();
        let digits: String = before
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if let Ok(n) = digits.chars().rev().collect::<String>().parse::<usize>() {
            out.push(n);
        }
    }
    out
}

/// When two folds land in one answer, the reader must be able to do the
/// arithmetic.
///
/// The sibling guard above asks whether any SINGLE claim exceeds the input.
/// Two claims can each be smaller than the input and still be nonsense
/// together. Measured on a 400-line log: the compactor collapsed 222 lines
/// into 111 markers and said `folded 222 line(s) total`; the summary then
/// elided 260 lines OF THAT VIEW and said `elided 260 lines`. Both true of
/// their own stage; 482 removed from 400 for anyone adding them, because the
/// 222 are inside the 260.
///
/// So the demand is not "each number is true" but "the answer states the total
/// a reader needs": one of its numbers must be exactly the count of original
/// lines that did not come back.
#[test]
fn e2e_a_two_stage_fold_states_the_original_magnitude() {
    let h = TempHome::new("two-stage-arithmetic");
    let dir = h.dir.join("w");
    std::fs::create_dir_all(&dir).unwrap();
    let tag = format!(
        "{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    // Distinct-but-similar lines: enough shape for the compactor to collapse
    // runs into markers, enough length for the summary to then cut a middle.
    // A repeated payload would be answered by exec-dedup instead.
    // A constant prefix makes every line ONE template, which the shape folder
    // collapses to a single `[×500 lines · distinct …]` row — correct, and a
    // one-stage fold that proves nothing here. Hash noise no longer varies the
    // shape either: a digit-leading hash is one `<HEX>` slot now, which
    // is what this fixture used to lean on. So the shape is varied in WORDS:
    // two adjacent lines per shape (the run pass folds them), 250 shapes that
    // never come back (nothing for the regroup), and enough bytes for the
    // summary to cut a middle.
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let (wa, wb) = (
        [
            "alpha", "bravo", "cedar", "delta", "ember", "fjord", "grove", "harbor", "iris",
            "juniper", "kestrel", "lumen", "maple", "nectar", "onyx", "pylon", "quartz", "raven",
            "sable", "tundra", "umber", "vesper", "willow", "xenon", "yarrow",
        ],
        [
            "sync", "relay", "cache", "index", "probe", "vault", "mirror", "audit", "beacon",
            "ledger",
        ],
    );
    let body: String = (0..500)
        .map(|i| {
            let k = i / 2;
            format!(
                "{:016x}{:016x} feat({}-{}): {tag} change number {i}\n",
                rnd(),
                rnd(),
                wa[k % 25],
                wb[k / 25]
            )
        })
        .collect();
    let path = dir.join("two-stage.log");
    std::fs::write(&path, &body).unwrap();
    let input_lines = body.lines().count();

    let cmd = format!("cat {}", path.display());
    let (out, _err, _rc) = run(&["wrap", &cmd], &h.dir);
    assert!(
        out.contains("elided") && out.contains("folded"),
        "this fixture no longer produces the two-stage shape the guard is about \
         — it proves nothing as written:\n{}",
        &out[..out.len().min(600)]
    );

    // What the answer RETURNS, weighted by what each line represents.
    //
    // Counting them as one each is the exact confusion the twin guard in
    // `disclose` describes — "a marker kept in the tail is DELIVERED and still
    // represents its lines; counting it as one, or as nothing, is the
    // confusion this guard is about". This test made it: as long as no marker
    // survived the cut, 500 − 29 gave the right answer by accident. Regular
    // gaps are now folded into ONE banner which does survive — it is
    // delivered, it announces 324 lines, and a line weighted as one makes it
    // disappear from the count.
    let weigh = |l: &str| tokenade_core::elision::weight_of(l).unwrap_or(1);
    let delivered: usize = out
        .lines()
        .filter(|l| !l.trim_start().starts_with('['))
        .map(|l| {
            let t = l.trim_start();
            // The summary's own marker is not delivered content: it DECLARES
            // what is missing, and counting it would amount to subtracting it
            // from what it announces.
            if t.starts_with("… (elided") {
                0
            } else {
                weigh(t)
            }
        })
        .sum();
    let missing = input_lines - delivered;
    let claims = line_counts_claimed(&out);
    assert!(
        claims.contains(&missing),
        "the answer folds twice and never states how many of the {input_lines} \
         original lines are gone ({missing}). Its numbers are {claims:?} — each \
         true of its own stage, and none of them the one a reader needs:\n{}",
        &out[..out.len().min(900)]
    );
}

/// A fold must not hide the failure — including a failure that names itself in
/// words the keyword list did not know.
///
/// A failure at the END of an output survives easily; the fold keeps a tail.
/// A failure in a middle the template folder can collapse survives too — the
/// odd line out IS the distinct shape. The case that loses it is a
/// HETEROGENEOUS output, where the middle is elided positionally and only a
/// keyword can rescue the line.
///
/// Measured 2026-09-06 on the published 1.1.8, six real failure sentences at
/// line 401 of 800: the English `error: build failed` came back and FIVE did
/// not, among them a checksum mismatch, a French verification failure, a
/// `terminated with status 137`, and a terraform plan that would destroy
/// production.
///
/// One of the six is deliberately NOT here: `0 of 42 migrations applied;
/// database left at revision 3` names no failure at all, and no keyword list
/// reaches it. Saying so is the honest bound of this guard.
#[test]
fn e2e_a_fold_does_not_hide_a_failure_it_has_no_keyword_for() {
    let h = TempHome::new("fold-keeps-the-failure");
    let dir = h.dir.join("logs");
    std::fs::create_dir_all(&dir).unwrap();
    let tag = format!(
        "{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    let cases: Vec<(&str, String)> = vec![
        (
            "checksum",
            format!("checksum mismatch for artifact bundle-{tag}.tar.gz"),
        ),
        (
            "french",
            format!("échec de la vérification de signature du paquet {tag}"),
        ),
        (
            "killed",
            format!("child process terminated with status 137 for run {tag}"),
        ),
        (
            "destroy",
            format!("plan would DESTROY 3 resources in production-{tag}"),
        ),
        // The control: the shape that already worked. If it stops working the
        // fixture, not the vocabulary, is what broke.
        ("classic", format!("error: build failed for target {tag}")),
    ];

    let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for (name, needle) in &cases {
        // Heterogeneous on purpose: commit-hash noise the shape folder cannot
        // collapse, so the middle is cut positionally.
        let mut lines: Vec<String> = (0..400)
            .map(|i| {
                format!(
                    "{:016x}{:016x} commit {i} by dev{} touching src/mod{}/f{i}.rs {tag}",
                    rnd(),
                    rnd(),
                    i % 29,
                    i % 41
                )
            })
            .collect();
        lines.push(format!("{needle} UNIQ{tag}{name}"));
        lines.extend((0..399).map(|i| {
            format!(
                "{:016x}{:016x} commit {} by dev{} touching lib/p{}/g{i}.ts {tag}",
                rnd(),
                rnd(),
                i + 401,
                i % 23,
                i % 37
            )
        }));
        let p = dir.join(format!("{name}.log"));
        std::fs::write(&p, lines.join("\n") + "\n").unwrap();

        let cmd = format!("cat {}", p.display());
        let (out, err, _) = run(&["wrap", &cmd], &h.dir);
        let all = format!("{out}{err}");
        // Anti-vacuity: if nothing was folded there is no middle to lose.
        assert!(
            all.len() < lines.join("\n").len(),
            "{name}: nothing was folded, so the guard proves nothing"
        );
        assert!(
            all.contains(&format!("UNIQ{tag}{name}")),
            "{name}: the fold dropped the one line that says the run failed \
             ({needle:?}) out of {} lines. What came back:\n{}",
            lines.len(),
            &all[..all.len().min(700)]
        );
    }
}

/// Run the binary and give it a deadline of its own, so a command that never
/// returns fails this test instead of hanging the suite.
///
/// This is not belt-and-braces: the defect under test IS an unbounded read, so
/// a plain `.output()` here would block `cargo test` forever the moment a
/// mutation put the defect back. Measured before the fix: `search --html
/// /dev/zero` on the published binary was still running at 25 s, and
/// `--script-file /dev/zero` was OOM-killed at 20 s.
fn run_bounded(args: &[&str], home: &Path) -> Option<(i32, String)> {
    run_bounded_cap(args, home, "2000000")
}

fn run_bounded_cap(args: &[&str], home: &Path, cap: &str) -> Option<(i32, String)> {
    let mut child = Command::new(binary());
    e2e_env(&mut child, home);
    let mut child = child
        .args(args)
        .env("TOKENADE_MAX_INPUT_BYTES", cap)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        match child.try_wait().expect("try_wait") {
            Some(_) => break,
            None if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            None => std::thread::sleep(std::time::Duration::from_millis(50)),
        }
    }
    let out = child.wait_with_output().expect("wait");
    Some((
        // A real exit code. `unwrap_or(-1)` collapsed a SIGNAL death into a
        // plain non-zero, and the caller's `assert_ne!(code, 0)` then read an
        // OOM kill — the very runaway this guards — as a clean refusal.
        // Measured on the stdin twin: the unbounded mutant dies rc=137 at
        // ~20 s, inside the deadline, and the test passed.
        out.status.code().unwrap_or_else(|| {
            panic!(
                "`tokenade {}` was killed by a signal instead of refusing — \
                 that is the runaway read, not a bound",
                args.join(" ")
            )
        }),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    ))
}

/// Every command that slurps a path the AGENT chose bounds that read.
///
/// `read` was fixed alone and the hole stayed open two commands over:
/// `tokenade search --html /dev/zero` and `tokenade exec --script-file
/// /dev/zero` both ran until the kernel or the operator stopped them, at
/// 2.6 GB resident after five seconds. A caller cannot tell an endless source
/// from a slow one, so the command has to.
///
/// The two arms with a REAL file are the anti-vacuity half: a bound that also
/// refuses ordinary input would pass the first half and be useless.
#[test]
fn every_path_a_caller_names_is_read_with_an_end_in_sight() {
    let home = TempHome::new("bounded-path-reads");
    if !Path::new("/dev/zero").exists() {
        return; // no character devices on this box — nothing to assert
    }

    for args in [
        vec!["search", "--html", "/dev/zero"],
        vec!["exec", "--lang", "bash", "--script-file", "/dev/zero"],
    ] {
        let got = run_bounded(&args, home.path());
        let (code, out) = match got {
            Some(v) => v,
            None => panic!("`tokenade {}` never returned", args.join(" ")),
        };
        assert_ne!(code, 0, "an endless source must not read as success");
        assert!(
            out.contains("no end in sight"),
            "`tokenade {}` must say WHY, got {out:?}",
            args.join(" ")
        );
    }

    // And an ordinary file still reads whole, through both readers.
    let page = home.path().join("serp.html");
    std::fs::write(
        &page,
        "<html><body><a href=\"https://example.com/x\">Example result title</a></body></html>",
    )
    .unwrap();
    let (code, out) = run_bounded(&["search", "--html", &page.to_string_lossy()], home.path())
        .expect("saved page must return");
    assert_eq!(code, 0, "a saved results page still reads: {out:?}");
    assert!(
        out.contains("Example result title"),
        "and its hits come back: {out:?}"
    );

    let script = home.path().join("s.sh");
    std::fs::write(&script, "echo ok-from-script\n").unwrap();
    let (code, out) = run_bounded(
        &[
            "exec",
            "--lang",
            "bash",
            "--script-file",
            &script.to_string_lossy(),
        ],
        home.path(),
    )
    .expect("script file must return");
    assert_eq!(code, 0, "a real script still runs: {out:?}");
    assert!(
        out.contains("ok-from-script"),
        "and its stdout comes back: {out:?}"
    );

    // A FINITE file bigger than the cap is not the thing being refused. The
    // cap exists because an endless source has no size to check, so applying
    // it to a regular file would refuse a large log that reads perfectly well
    // — 3 KB against a 1 KB ceiling here, the same shape as a 2 GB one.
    let big = home.path().join("big.sh");
    std::fs::write(
        &big,
        format!("# {}\necho ok-from-script\n", "x".repeat(3000)),
    )
    .unwrap();
    let (code, out) = run_bounded_cap(
        &[
            "exec",
            "--lang",
            "bash",
            "--script-file",
            &big.to_string_lossy(),
        ],
        home.path(),
        "1000",
    )
    .expect("a regular file must return");
    assert_eq!(
        code, 0,
        "a finite file over the cap still reads whole: {out:?}"
    );
    assert!(
        out.contains("ok-from-script"),
        "and runs to its last line: {out:?}"
    );
}

/// Every command that slurps STDIN bounds that read too.
///
/// The path twin above was fixed first, and the stdin side kept the hole in
/// four more commands. Measured 2026-09-06 on the published binary with
/// `yes |`: `tokenade read -` reached **50 189 MB** and was OOM-killed after
/// 28 s, `search --html -` 48 943 MB after 19 s — while `filter` and
/// `exec --script -`, which already went through the capped reader, refused in
/// under half a second. Same helper, same ceiling, four consumers that skipped
/// it.
///
/// A 49 GB process does not merely fail: the OOM killer picks a victim, and it
/// need not be this one.
#[test]
fn every_stdin_a_caller_pipes_is_read_with_an_end_in_sight() {
    let home = TempHome::new("bounded-stdin-reads");

    // `yes` is the endless source; the cap is lowered so the test spends
    // megabytes, not the full ceiling.
    for args in [
        vec!["read", "-"],
        vec!["search", "--html", "-"],
        vec!["web", "-"],
        vec!["filter", "cat"],
    ] {
        let mut child = Command::new(binary());
        e2e_env(&mut child, home.path());
        let mut child = child
            .args(&args)
            .env("TOKENADE_MAX_INPUT_BYTES", "4000000")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn");

        // Write until the child gives up. `write_all` fails once it does, and
        // that failure is the pass condition, not an error.
        let mut sink = child.stdin.take().expect("stdin");
        let writer = std::thread::spawn(move || {
            let chunk = "endless line of output that never stops coming\n".repeat(2048);
            loop {
                use std::io::Write;
                if sink.write_all(chunk.as_bytes()).is_err() {
                    return;
                }
            }
        });

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            match child.try_wait().expect("try_wait") {
                Some(_) => break,
                None if std::time::Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = writer.join();
                    panic!("`tokenade {}` never stopped reading stdin", args.join(" "));
                }
                None => std::thread::sleep(std::time::Duration::from_millis(50)),
            }
        }
        let out = child.wait_with_output().expect("wait");
        let _ = writer.join();
        // A real EXIT CODE, not a signal. "did not exit 0" is satisfied by
        // being OOM-killed, which is the very outcome this guards against:
        // measured, the unbounded mutant dies with SIGKILL at ~20 s and a
        // `code() != Some(0)` assertion reads that as a clean refusal.
        let code = out.status.code().unwrap_or_else(|| {
            panic!(
                "`tokenade {}` was killed by a signal instead of refusing — \
                 that is the runaway read, not a bound",
                args.join(" ")
            )
        });
        assert_ne!(
            code,
            0,
            "`tokenade {}` must not read an endless pipe as success",
            args.join(" ")
        );
    }

    // Anti-vacuity: ordinary piped input still arrives whole. A bound that
    // refused every pipe would pass everything above and break the product.
    let page = "<html><body><a href=\"https://e.com/x\">Result title</a></body></html>";
    for (args, needle) in [
        (vec!["search", "--html", "-"], "Result title"),
        (vec!["web", "-"], "Result title"),
    ] {
        let mut child = Command::new(binary());
        e2e_env(&mut child, home.path());
        let mut child = child
            .args(&args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn");
        {
            use std::io::Write;
            let mut sink = child.stdin.take().expect("stdin");
            sink.write_all(page.as_bytes()).expect("write");
        }
        let out = child.wait_with_output().expect("wait");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            out.status.code(),
            Some(0),
            "`tokenade {}` must still read a real pipe: {text:.200}",
            args.join(" ")
        );
        assert!(
            text.contains(needle),
            "`tokenade {}` must still deliver it: {text:.200}",
            args.join(" ")
        );
    }
}

/// The MCP pipe is bounded too, in both directions.
///
/// A JSON-RPC line has no declared length, so `read_line` grows until the peer
/// sends a newline — and the peer is a THIRD-PARTY MCP server the user wired
/// in, on the transport this product's own guidance tells them to wrap.
/// Measured 2026-09-06 against a server that answers one line and never ends
/// it: `tokenade mcp-proxy` reached **50 972 MB** on the published binary and
/// was still climbing at 30 s. Under a 2 GB address-space limit the same run
/// dies with "memory allocation of 2147483648 bytes failed".
///
/// Driven at a small ceiling through the knob that already exists, so the test
/// spends a few hundred kilobytes.
#[test]
fn the_mcp_pipe_is_read_with_an_end_in_sight() {
    let home = TempHome::new("mcp-endless-line");

    // A server that answers one endless line: bytes, never a newline.
    let endless = home.path().join("endless_server.py");
    std::fs::write(
        &endless,
        "import sys\nsys.stdin.readline()\nchunk='x'*8192\nwhile True:\n    sys.stdout.write(chunk)\n    sys.stdout.flush()\n",
    )
    .unwrap();

    let mut child = Command::new(binary());
    e2e_env(&mut child, home.path());
    let mut child = child
        .args(["mcp-proxy", "--", "python3", &endless.to_string_lossy()])
        .env("TOKENADE_MAX_INPUT_BYTES", "200000")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    {
        use std::io::Write;
        let mut sink = child.stdin.take().expect("stdin");
        let _ = sink.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n");
        let _ = sink.flush();
    }

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        match child.try_wait().expect("try_wait") {
            Some(_) => break,
            None if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("mcp-proxy never stopped reading an endless response line");
            }
            None => std::thread::sleep(std::time::Duration::from_millis(50)),
        }
    }
    let out = child.wait_with_output().expect("wait");
    // A real exit code, not a signal: "did not exit 0" accepts the OOM
    // kill this exists to prevent.
    let code = out
        .status
        .code()
        .expect("mcp-proxy was killed by a signal instead of refusing");
    assert_ne!(code, 0, "an endless response line must not read as success");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("with no end") && err.contains("resynchronised"),
        "and it must say why: {err:.300}"
    );

    // Anti-vacuity: an ordinary exchange still passes through. A proxy that
    // refused every line would satisfy the arm above and serve nobody.
    let good = home.path().join("good_server.py");
    std::fs::write(
        &good,
        "import sys, json\nsys.stdin.readline()\nprint(json.dumps({\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"serverInfo\":{\"name\":\"probe\"}}}), flush=True)\n",
    )
    .unwrap();
    let mut child = Command::new(binary());
    e2e_env(&mut child, home.path());
    let mut child = child
        .args(["mcp-proxy", "--", "python3", &good.to_string_lossy()])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    {
        use std::io::Write;
        let mut sink = child.stdin.take().expect("stdin");
        let _ = sink.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n");
        let _ = sink.flush();
    }
    let out = child.wait_with_output().expect("wait");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("\"serverInfo\""),
        "an ordinary response must still reach the client: {text:.300}"
    );
}
