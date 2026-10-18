//! End-to-end tests driving `tokenade wrap`.

use super::*;

/// `shim-exec` is invoked BY a stale shim, so it is the one caller holding
/// proof that one exists — it must self-heal, not merely print advice into an
/// agent's hook transcript where no human will act on it. It is also
/// PROTOCOL-QUIET, so the interactive post-upgrade sweep does NOT run here:
/// anything cleaned up is cleaned up by `cmd_shim_exec` itself.
#[cfg(unix)]
#[test]
fn e2e_shim_exec_self_heals_the_stale_shim() {
    let h = TempHome::new("shim-exec-heal");
    let home = h.path();
    let shim = home.join(".tokenade").join("shim");
    std::fs::create_dir_all(&shim).unwrap();
    std::fs::write(
        shim.join("node"),
        "#!/bin/sh\n# tokenade shim v1\nexec /old/tokenade shim-exec node -- \"$@\"\n",
    )
    .unwrap();
    let bashrc = home.join(".bashrc");
    std::fs::write(
        &bashrc,
        "# tokenade path shim\ncase \":$PATH:\" in *\":$HOME/.tokenade/shim:\"*) ;; *) PATH=\"$HOME/.tokenade/shim:$PATH\"; export PATH ;; esac\n",
    )
    .unwrap();

    // `echo` stands in for the real victims (`node`, `npm`, `git`): a command
    // that exists on every unix PATH, so the assertion is about shim-exec's
    // behaviour and not about what the test host has installed.
    let (out, err, code) = run_copied_active(&["shim-exec", "echo", "--", "hello"], home);
    assert!(
        !shim.exists(),
        "shim-exec did not remove the shim that invoked it"
    );
    let rc = std::fs::read_to_string(&bashrc).unwrap();
    assert!(
        !rc.contains(".tokenade/shim"),
        "shim-exec left the rc PATH line behind:\n{rc}"
    );
    // …and, the shim being provably gone, it RUNS the command. Printing a
    // repair the caller can't act on was still a dead end: on an npm install
    // `tokenade` itself is a node script, so `tokenade install` — the advice —
    // died on the same shim (field report 2026-07-29).
    assert_eq!(code, 0, "shim-exec should run the command now, got {code}");
    assert_eq!(
        out.trim(),
        "hello",
        "wrapped command's stdout must pass through, got:\n{out}"
    );
    assert!(
        err.contains("stale PATH shim"),
        "shim-exec should report the repair it performed, got:\n{err}"
    );
    assert!(
        err.to_lowercase().contains("restart"),
        "shim-exec must tell the user to restart the app holding the hijacked PATH, got:\n{err}"
    );
    // The separator the shim inserts is the shim's, not the user's.
    assert!(
        !out.contains("--"),
        "the shim's `--` separator leaked into the command's arguments:\n{out}"
    );
}

/// The recovery run is conditional, and its fallback is the old contract: when
/// the wrapped command cannot be resolved outside the shim, `shim-exec` must
/// still repair what it can and fail LOUDLY — a caller must never read a
/// repaired PATH as a successful invocation.
#[cfg(unix)]
#[test]
fn e2e_shim_exec_fails_loudly_when_the_command_cannot_be_run() {
    let h = TempHome::new("shim-exec-unrunnable");
    let home = h.path();
    let shim = home.join(".tokenade").join("shim");
    std::fs::create_dir_all(&shim).unwrap();
    std::fs::write(
        shim.join("nope-not-a-real-command"),
        "#!/bin/sh\n# tokenade shim v1\nexec /old/tokenade shim-exec nope-not-a-real-command -- \"$@\"\n",
    )
    .unwrap();

    let (_o, err, code) =
        run_copied_active(&["shim-exec", "nope-not-a-real-command", "--", "x"], home);
    assert_eq!(code, 2, "unrunnable command must still exit 2, got {code}");
    assert!(
        err.contains("was NOT executed"),
        "the caller must be told the command did not run, got:\n{err}"
    );
    assert!(!shim.exists(), "the shim must be dismantled either way");
}

#[cfg(unix)]
/// A chained command must EXECUTE exactly what the caller wrote.
///
/// `rewrite()` returns only the focused segment of a chain, which is right for
/// picking a compactor (last command wins) and catastrophic for execution: the
/// dropped prefix is not decoration, it changes what the command means.
/// Measured before the guard — `sh -c 'cd <other-repo> && git status'` answered
/// about the CURRENT directory instead, i.e. a confident answer about the wrong
/// place. The same shape drops a pipeline's data producer (`printf pw | sudo -S
/// …`) or a guard (`test -f x && …`). Shell- and OS-agnostic: the rule is never
/// substitute execution for a chain.
#[test]
fn e2e_wrap_never_substitutes_execution_of_a_chained_command() {
    let h = TempHome::new("proxychain");
    // A DIFFERENT git repo, with a file whose name only exists there.
    let other = h.path().join("otherrepo");
    std::fs::create_dir_all(&other).unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "t@t"],
        vec!["config", "user.name", "t"],
    ] {
        std::process::Command::new("git")
            .args(&args)
            .current_dir(&other)
            .output()
            .unwrap();
    }
    std::fs::write(other.join("ONLY_IN_OTHER_REPO.txt"), "x").unwrap();

    // `git status` alone matches a rewrite rule, so the chain is exactly the
    // shape that used to lose its `cd`.
    let cmd = format!("cd {} && git status", other.display());
    let out = std::process::Command::new(binary())
        .args(["wrap", &cmd])
        .env("HOME", h.path())
        .current_dir(std::env::current_dir().unwrap()) // NOT the other repo
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("ONLY_IN_OTHER_REPO"),
        "the `cd` was dropped — status describes the wrong directory:\n{stdout}"
    );
}

#[test]
fn e2e_wrap_compresses_noisy_command() {
    let h = TempHome::new("proxy");
    // `cat` of a big JSON blob is enough to exercise the JSON compactor.
    let fixture = h.path().join("blob.json");
    let mut big = String::from("{\"items\":[");
    for i in 0..200 {
        if i > 0 {
            big.push(',');
        }
        big.push_str(&format!("{{\"id\":{i},\"name\":\"x\"}}"));
    }
    big.push_str("]}\n");
    std::fs::write(&fixture, &big).unwrap();
    let (stdout, _err, code) = run(&["wrap", &format!("cat {}", fixture.display())], h.path());
    assert_eq!(code, 0);
    // Compactor caps arrays at 20 → output strictly shorter than input.
    assert!(
        stdout.len() < big.len() / 2,
        "expected ≥50% shrink; got {} → {}",
        big.len(),
        stdout.len()
    );
    // gain.jsonl should have at least one row for this run.
    let gain = h.path().join(".tokenade").join("gain.jsonl");
    let log = std::fs::read_to_string(&gain).expect("gain.jsonl should exist after proxy");
    assert!(log.lines().count() >= 1);
}

// Truncation-aware advisory: `grep … | head -N` that returns exactly N
// lines means head almost certainly cut some off → tokenade nudges on
// STDERR (stdout stays byte-identical) to re-run without the slice.
#[test]
fn e2e_wrap_warns_on_blind_head_truncation() {
    let h = TempHome::new("head-trunc");
    let f = h.path().join("hay.log");
    // 50 matching lines; the agent slices to the first 5.
    let body: String = (0..50).map(|i| format!("match line {i}\n")).collect();
    std::fs::write(&f, &body).unwrap();
    let cmd = format!("grep match {} | head -5", f.to_str().unwrap());
    let (stdout, err, code) = run(&["wrap", &cmd], h.path());
    assert_eq!(code, 0);
    // stdout is the unmodified slice: exactly 5 lines, no banner injected.
    assert_eq!(
        stdout.lines().filter(|l| l.contains("match line")).count(),
        5
    );
    assert!(
        !stdout.contains("[tokenade]"),
        "stdout must stay clean: {stdout}"
    );
    // The advisory rides on stderr only.
    assert!(
        err.contains("[tokenade]") && err.contains("head"),
        "expected truncation advisory on stderr, got: {err}"
    );
}

// Negative: when the search produces FEWER lines than the slice, head cut
// nothing, so no advisory must fire (avoids banner-fatigue on every grep).
#[test]
fn e2e_wrap_no_warning_when_head_did_not_truncate() {
    let h = TempHome::new("head-notrunc");
    let f = h.path().join("hay.log");
    let body: String = (0..3).map(|i| format!("match line {i}\n")).collect();
    std::fs::write(&f, &body).unwrap();
    let cmd = format!("grep match {} | head -20", f.to_str().unwrap());
    let (_stdout, err, code) = run(&["wrap", &cmd], h.path());
    assert_eq!(code, 0);
    assert!(
        !err.contains("[tokenade]"),
        "no advisory expected when head didn't truncate, got: {err}"
    );
}

#[cfg(unix)]
#[test]
fn e2e_wrap_writes_savings_field_in_ledger() {
    let h = TempHome::new("ledger-shape");
    let fixture = h.path().join("api.json");
    let mut big = String::new();
    big.push_str("{\"users\":[");
    for i in 0..80 {
        if i > 0 {
            big.push(',');
        }
        big.push_str(&format!(
            "{{\"id\":{i},\"name\":\"u{i}\",\"plan\":\"pro\"}}"
        ));
    }
    big.push_str("]}\n");
    std::fs::write(&fixture, &big).unwrap();
    let (_out, _err, code) = run(&["wrap", &format!("cat {}", fixture.display())], h.path());
    assert_eq!(code, 0);
    let log = std::fs::read_to_string(h.path().join(".tokenade").join("gain.jsonl")).unwrap();
    let first = log.lines().next().expect("at least one record");
    let v: serde_json::Value = serde_json::from_str(first).expect("valid JSON record");
    // Schema invariants the dashboard relies on.
    for field in &["ts", "op", "label", "project", "before", "after", "saved"] {
        assert!(
            v.get(field).is_some(),
            "ledger record missing `{field}` — got:\n{first}"
        );
    }
    // Savings should be positive on this fixture.
    let before = v["before"].as_u64().unwrap_or(0);
    let after = v["after"].as_u64().unwrap_or(0);
    assert!(before > after, "no savings recorded ({before} → {after})");
}

/// #540 — micro-savings cmd_wrap events must NOT pollute gain.jsonl.
/// Before the fix, any compactor that shaved ≥1 token but < the
/// dashboard-meaningful threshold (5 % AND 10 tokens absolute) would
/// land a noise entry that dragged dashboard averages down.
///
/// Regression: pipe a tiny string through `tokenade proxy cat`. The
/// compactor may run, but on input this short the savings (if any)
/// are sub-threshold. The ledger must stay empty (or at most carry a
/// real-signal entry from somewhere else — never one for this run).
#[cfg(unix)]
#[test]
fn e2e_wrap_below_threshold_does_not_pollute_ledger() {
    let h = TempHome::new("ledger-threshold");
    let fixture = h.path().join("tiny.txt");
    // 5 short lines — no compactor will save ≥10 tokens off this.
    std::fs::write(&fixture, "one\ntwo\nthree\nfour\nfive\n").unwrap();
    let (_out, _err, code) = run(&["wrap", &format!("cat {}", fixture.display())], h.path());
    assert_eq!(code, 0);
    let ledger_path = h.path().join(".tokenade").join("gain.jsonl");
    if !ledger_path.exists() {
        return; // Clean: never created the file.
    }
    let log = std::fs::read_to_string(&ledger_path).unwrap();
    for line in log.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(line).expect("valid JSON record");
        // Each entry that DID survive must meet the threshold.
        let before = v["before"].as_u64().unwrap_or(0);
        let after = v["after"].as_u64().unwrap_or(0);
        let saved = before.saturating_sub(after);
        let pct = if before == 0 {
            0.0
        } else {
            (saved as f64) * 100.0 / (before as f64)
        };
        assert!(
            saved >= 10 && pct >= 5.0,
            "micro-savings entry leaked into ledger (saved={saved}, pct={pct:.1}%): {line}"
        );
    }
}

// #512 lock-in: `tokenade scrub-cache --dry-run` reports which stash
// files would be redacted without modifying them. Mirrors the
// scrub-secret dry-run contract above. We only exercise the STASH
// half (not dedup.db) because seeding the SQLite cache from an e2e
// test would require spinning up the same migration path the binary
// uses — out of scope for this lock-in. The dedup.db dry-run branch
// is covered by the inline unit-level smoke in cmd_scrub_cache.
#[test]
fn e2e_scrub_cache_dry_run_reports_without_writing_to_stash() {
    let h = TempHome::new("scrub-cache-dry");
    let stash_dir = h.path().join(".cache").join("tokenade").join("stash");
    std::fs::create_dir_all(&stash_dir).unwrap();
    let leaky = stash_dir.join("a1b2c3d4");
    // Embed an AWS-shaped access key (matches the built-in redact
    // pattern). We don't want a real credential — this is the
    // canonical test fixture string used elsewhere in the suite.
    let body = "this stash entry contains AKIA1234567890ABCDEF and more text\n";
    std::fs::write(&leaky, body).unwrap();
    let pre_meta = std::fs::metadata(&leaky).unwrap();
    let pre_len = pre_meta.len();

    let (out, _err, code) = run(&["scrub-cache", "--dry-run"], h.path());
    assert_eq!(
        code, 0,
        "scrub-cache --dry-run should exit 0; stdout:\n{out}"
    );
    // Banner reflects dry-run mode.
    assert!(
        out.contains("--dry-run") || out.contains("WOULD"),
        "stdout should signal dry-run mode: {out}"
    );
    // The stash file must NOT have changed (size + bytes).
    let post = std::fs::read_to_string(&leaky).unwrap();
    assert_eq!(post, body, "dry-run must leave the file's bytes intact");
    assert_eq!(
        std::fs::metadata(&leaky).unwrap().len(),
        pre_len,
        "dry-run must not change file size"
    );
    // Trailer differs between dry-run and apply mode.
    assert!(
        !out.contains("scrub complete"),
        "dry-run trailer should NOT be the apply-mode one: {out}"
    );
}

/// #831 — `audit-claude-md` reports per-file token cost and flags lines
/// duplicated across the cascade. We plant a global ~/.claude/CLAUDE.md
/// (HOME is the temp dir) plus a project CLAUDE.md sharing one unique
/// line, then point the walk at the project dir.
#[test]
fn e2e_audit_claude_md_reports_cross_file_duplicate() {
    let h = TempHome::new("audit-claudemd");
    let shared =
        "Quetzal verbatim cascade instruction line shared across global and project files.";
    // Global user file.
    let global_dir = h.path().join(".claude");
    std::fs::create_dir_all(&global_dir).unwrap();
    std::fs::write(
        global_dir.join("CLAUDE.md"),
        format!("# Global\n{shared}\nglobal only content line plenty long here\n"),
    )
    .unwrap();
    // Project file in a subdir we pass as the walk root.
    let proj = h.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(
        proj.join("CLAUDE.md"),
        format!("# Project\n{shared}\nproject only content line plenty long here\n"),
    )
    .unwrap();

    let proj_str = proj.display().to_string();
    let (out, err, code) = run(&["audit-claude-md", &proj_str], h.path());
    assert_eq!(code, 0, "audit-claude-md should exit 0. stderr:\n{err}");
    assert!(out.contains("CLAUDE.md cascade"), "banner present:\n{out}");
    assert!(
        out.contains("always-resident"),
        "prefix-size line present:\n{out}"
    );
    assert!(
        out.contains("Quetzal verbatim"),
        "the planted shared line is flagged as duplicated:\n{out}"
    );
}

/// `--json` emits a machine-readable object with the duplicate accounting.
#[test]
fn e2e_audit_claude_md_json_shape() {
    let h = TempHome::new("audit-claudemd-json");
    let shared = "Basalt verbatim cascade line duplicated for the json shape assertion here.";
    let global_dir = h.path().join(".claude");
    std::fs::create_dir_all(&global_dir).unwrap();
    std::fs::write(
        global_dir.join("CLAUDE.md"),
        format!("{shared}\nglobal unique trailing content line goes here ok\n"),
    )
    .unwrap();
    let proj = h.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(
        proj.join("CLAUDE.md"),
        format!("{shared}\nproject unique trailing content line goes here ok\n"),
    )
    .unwrap();

    let proj_str = proj.display().to_string();
    let (out, err, code) = run(&["audit-claude-md", &proj_str, "--json"], h.path());
    assert_eq!(code, 0, "json mode should exit 0. stderr:\n{err}");
    let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    assert!(v["total_tokens"].as_u64().unwrap() > 0, "got:\n{out}");
    assert!(
        v["duplicated_tokens"].as_u64().unwrap() > 0,
        "duplicate must be counted:\n{out}"
    );
    let dups = v["duplicates"].as_array().unwrap();
    assert!(
        dups.iter()
            .any(|d| d["text"].as_str().unwrap_or("").contains("Basalt verbatim")),
        "planted line in duplicates[]:\n{out}"
    );
}

#[test]
fn e2e_pack_bundles_files_and_enforces_budget() {
    let h = TempHome::new("pack");
    // A small non-git source tree → exercises the directory-walk scope,
    // binary skipping, and budget enforcement in one shot.
    let proj = h.dir.join("proj");
    std::fs::create_dir_all(proj.join("src")).unwrap();
    std::fs::write(proj.join("README.md"), "# demo\nhello\n").unwrap();
    std::fs::write(
        proj.join("src").join("main.rs"),
        "fn main() { println!(\"hi\"); }\n",
    )
    .unwrap();
    // A binary file must be skipped, never tokenized into the bundle.
    std::fs::write(proj.join("blob.bin"), [0u8, 1, 2, 3, 0, 255]).unwrap();

    let proj_str = proj.to_string_lossy().into_owned();

    // Unbounded JSON: both text files included, binary skipped.
    let (out, err, code) = run(&["pack", &proj_str, "--json"], &h.dir);
    assert_eq!(code, 0, "pack exits 0\nstderr: {err}");
    let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    assert_eq!(
        v["included_count"].as_u64().unwrap(),
        2,
        "two text files:\n{out}"
    );
    assert!(
        v["skipped_binary_or_large"].as_u64().unwrap() >= 1,
        "binary skipped:\n{out}"
    );
    assert!(v["included_tokens"].as_u64().unwrap() > 0);

    // Markdown bundle carries the actual file bodies in fenced blocks.
    let (md, _, code) = run(&["pack", &proj_str], &h.dir);
    assert_eq!(code, 0);
    assert!(md.contains("## src/main.rs"), "file header present:\n{md}");
    assert!(md.contains("fn main()"), "body present:\n{md}");
    assert!(!md.contains("blob.bin"), "binary excluded:\n{md}");

    // ...and the manifest SAYS it was skipped. The count was computed and
    // handed to the JSON wire only, so a reader of the bundle got
    // `included + omitted` that did not add up to the scope, with nothing
    // naming the difference. On this repo that gap was 50 files.
    assert!(
        md.contains("- skipped (binary, or larger than the per-file cap): 1 file(s)"),
        "the skipped category must be in the manifest:\n{md}"
    );

    // The three numbers account for everything in scope, which is the whole
    // claim the header makes.
    let (full, _, code) = run(&["pack", &proj_str, "--json"], &h.dir);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&full).unwrap();
    let accounted = v["included_count"].as_u64().unwrap()
        + v["omitted_count"].as_u64().unwrap()
        + v["skipped_binary_or_large"].as_u64().unwrap();
    assert_eq!(accounted, 3, "README.md + src/main.rs + blob.bin:\n{full}");

    // A bundle with nothing skipped must not grow the line.
    std::fs::remove_file(proj.join("blob.bin")).unwrap();
    let (clean, _, code) = run(&["pack", &proj_str], &h.dir);
    assert_eq!(code, 0);
    assert!(
        !clean.contains("- skipped ("),
        "no skips, no line:\n{clean}"
    );
    std::fs::write(proj.join("blob.bin"), [0u8, 1, 2, 3, 0, 255]).unwrap();

    // A budget of 1 token is below any real file → nothing included, all
    // listed as omitted (no silent truncation).
    let (tight, _, code) = run(&["pack", &proj_str, "--budget", "1", "--json"], &h.dir);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&tight).unwrap();
    assert_eq!(
        v["included_count"].as_u64().unwrap(),
        0,
        "budget too small:\n{tight}"
    );
    assert!(
        v["omitted_count"].as_u64().unwrap() >= 2,
        "omitted listed:\n{tight}"
    );
}

/// ...and the other direction, across EVERY command that keeps a flag
/// allowlist: a flag the parser accepts must appear in that command's own
/// `--help`, or be listed here as deliberately unadvertised.
///
/// This was first fixed for `exec --script-file` alone. Asked of the whole class,
/// four more turned up, and one of them mattered: `evict-stale --models`
/// deletes the local model weights — 19.9 GB of the 20.6 GB tokenade held on
/// the machine this was measured on — while that command's help said "No
/// flags." and the code comment beside the flag said the same. The only way
/// to reclaim the largest thing the product holds was denied in both places
/// a user would look.
#[test]
fn e2e_every_accepted_flag_is_documented_or_deliberately_hidden() {
    // Arrives inside a command copied from a referral landing page, so a user
    // never types it unprompted and advertising it would invite guessing.
    const DELIBERATELY_UNADVERTISED: &[(&str, &str)] = &[("install", "--ref")];

    let src_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut src = String::new();
    let mut stack = vec![src_root];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                src.push_str(&std::fs::read_to_string(&p).unwrap_or_default());
            }
        }
    }

    // const NAME_FLAGS: &[&str] = &[ "--a", "--b" ];  →  (NAME, [--a, --b])
    let lists: Vec<(String, Vec<String>)> = src
        .match_indices("FLAGS: &[&str] =")
        .filter_map(|(i, _)| {
            // Walk back over the identifier to get the WHOLE const name:
            // `i` sits on the `F` of `FLAGS`, and `EXEC_BOOL_` before it
            // carries no space to find.
            let name_start = src[..i]
                .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .map_or(0, |p| p + 1);
            let name = format!("{}FLAGS", &src[name_start..i]);
            // The `&[` that opens the VALUE, not the one inside the type
            // `&[&str]` — searching for a bare `&[` finds the type first and
            // yields a slice with no flags in it at all.
            //
            // And the `=` can be followed by a LINE BREAK: `const
            // MAP_FLAGS: &[&str] =` then its value below, which
            // `rustfmt` writes as soon as the line is long. Searching for `"= &["`
            // literally then found nothing HERE and jumped to the `= &[`
            // of the NEXT const — so this test attributed to `map` another one's
            // flag list. It did so silently until a
            // split changed the concatenation order of the files.
            let eq = i + src[i..].find('=')?;
            let after = &src[eq + 1..];
            let pad = after.len() - after.trim_start().len();
            if !after[pad..].starts_with("&[") {
                return None;
            }
            let open = eq + 1 + pad + 2;
            let close = open + src[open..].find(']')?;
            let flags = src[open..close]
                .split('"')
                .filter(|t| t.starts_with('-') && t.len() > 1)
                .map(str::to_string)
                .collect();
            Some((name, flags))
        })
        .collect();

    let cmd_of = |name: &str| -> Option<&'static str> {
        let stem = name
            .trim_end_matches("FLAGS")
            .trim_end_matches('_')
            .trim_end_matches("_BOOL")
            .trim_end_matches("_VALUE");
        Some(match stem {
            "DASH" => "dashboard",
            "MAP" => "map",
            "DISCOVER" => "discover",
            "INSTALL" => "install",
            "UNINSTALL" => "uninstall",
            "UNWRAP_MCPS" => "unwrap-mcps",
            "EVICT_STALE" => "evict-stale",
            "SCRUB_SECRET" => "scrub-secret",
            "SCRUB_CACHE" => "scrub-cache",
            "UPGRADE" => "upgrade",
            "EXEC" => "exec",
            _ => return None,
        })
    };

    // A PLAIN `contains` says `--sandbox` is documented because
    // `--sandbox-net` is on the page. The flag has to end where it ends.
    fn mentions_flag(help: &str, flag: &str) -> bool {
        help.match_indices(flag).any(|(i, _)| {
            help[i + flag.len()..]
                .chars()
                .next()
                .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '-'))
        })
    }

    let h = TempHome::new("flag-docs");
    let mut checked = 0usize;
    let mut undocumented: Vec<String> = Vec::new();
    for (name, flags) in &lists {
        let Some(cmd) = cmd_of(name) else { continue };
        let (out, err, _c) = run(&[cmd, "--help"], &h.dir);
        let help = format!("{out}{err}");
        assert!(
            !help.trim().is_empty(),
            "`{cmd} --help` printed nothing, so this guard cannot check it"
        );
        for f in flags {
            // Help never lists itself.
            if f == "--help" || f == "-h" {
                continue;
            }
            if DELIBERATELY_UNADVERTISED.contains(&(cmd, f.as_str())) {
                continue;
            }
            checked += 1;
            if !mentions_flag(&help, f) {
                undocumented.push(format!("{cmd} {f}"));
            }
        }
    }
    assert!(
        checked > 20,
        "only {checked} flags reached the check — the \
         allowlist scan stopped finding them, so a green result means nothing"
    );
    assert!(
        undocumented.is_empty(),
        "accepted but in no --help, and not declared hidden: {undocumented:?}"
    );

    // An exemption must still be EARNED. Once a hidden flag gets documented
    // (or dropped), its entry here is a lie that quietly excuses the next
    // one to take its place.
    for (cmd, flag) in DELIBERATELY_UNADVERTISED {
        let (out, err, _c) = run(&[cmd, "--help"], &h.dir);
        assert!(
            !mentions_flag(&format!("{out}{err}"), flag),
            "`{cmd} {flag}` is documented now — drop it from DELIBERATELY_UNADVERTISED"
        );
    }
}

/// A fold that destroys the answer must leave a way back to it — from `exec`
/// too, not only from `wrap`.
///
/// The stash `exec` did call decides on how big the SURVIVING text is, so the
/// better a fold works the less likely it fires: a 400-path `ls -laR` becomes
/// a two-line rollup, falls under the floor, and every path is gone with
/// nothing to recover them by. Measured over our own sessions: 16 such
/// rollups served, 2 with any route back. `wrap` and the MCP proxy already
/// called `attach_drop_recovery`, which decides on what the fold DROPPED.
#[test]
fn e2e_exec_offers_a_way_back_to_what_the_fold_destroyed() {
    let h = TempHome::new("exec-recovery");
    let dir = h.dir.join("many");
    std::fs::create_dir_all(&dir).unwrap();
    for i in 0..400 {
        std::fs::write(dir.join(format!("f_{i}_stage.rs")), "q".repeat(400)).unwrap();
    }

    let (out, err, code) = run(
        &[
            "exec",
            "--lang",
            "bash",
            "--script",
            &format!("ls -laR {}", dir.display()),
        ],
        &h.dir,
    );
    assert_eq!(code, 0, "exec failed: {err}");
    assert!(
        !out.contains("f_137_stage.rs"),
        "the listing did not fold, so this guard proves nothing:\n{out}"
    );

    let hash = out
        .split_whitespace()
        .skip_while(|w| !w.contains("expand-ref"))
        .nth(1)
        .map(|w| {
            w.trim_matches(|c: char| !c.is_ascii_alphanumeric())
                .to_string()
        })
        .unwrap_or_default();
    assert!(
        !hash.is_empty(),
        "every path was destroyed and no way back was offered:\n{out}"
    );

    // The pointer has to RETURN the bytes, not merely look like one.
    let (back, berr, bcode) = run(&["expand-ref", &hash, "--grep", "f_137_stage.rs"], &h.dir);
    assert_eq!(bcode, 0, "expand-ref {hash} failed: {berr}");
    assert!(
        back.contains("f_137_stage.rs"),
        "the hash does not give the destroyed path back:\n{back}"
    );
}

/// A question the source cannot answer must never come back looking like one
/// it could — on the outline path too.
///
/// `read --prompt` adds that note when the scoped extract declines, and then
/// `bound_delivery` REBUILDS the text: for a large markdown it returns an
/// outline of it, and the note did not survive. Measured on a 127 824-byte
/// document asked something nothing in it answers: `mode=md-outline`, 4 699
/// bytes, no note — while the same question on a document served whole says
/// so. An outline of section titles reads even more like a curated answer.
#[test]
fn e2e_an_unanswered_prompt_says_so_even_when_the_reply_is_an_outline() {
    let h = TempHome::new("prompt-outline");
    let doc = h.dir.join("big.md");
    let words = [
        "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta", "iota", "kappa",
    ];
    let mut md = String::new();
    for s in 0..140 {
        md.push_str(&format!(
            "## Section {s} sur les {} mesures\n\n",
            words[s % 10]
        ));
        for l in 0..10 {
            for w in 0..16 {
                md.push_str(words[(s * 31 + l * 7 + w * 3) % 10]);
                md.push(' ');
            }
            md.push_str(".\n");
        }
        md.push('\n');
    }
    std::fs::write(&doc, &md).unwrap();
    let path = doc.to_string_lossy().into_owned();

    const NOTE: &str = "nothing in this source answers that question";

    let (out, err, code) = run(
        &[
            "read",
            &path,
            "--prompt",
            "zebra quantum submarine telemetry",
        ],
        &h.dir,
    );
    assert_eq!(code, 0, "read failed: {err}");
    assert!(
        out.contains("mode=md-outline"),
        "the fixture no longer takes the outline path, so this proves nothing:\n{}",
        &out[..out.len().min(200)]
    );
    assert!(
        out.contains(NOTE),
        "an outline was served as the answer to a question it does not answer:\n{}",
        &out[..out.len().min(400)]
    );

    // ...and a question the document DOES answer is not labelled unanswered.
    let mut answering = md.clone();
    answering.push_str(
        "## Le sous-marin quantique\n\nLa télémétrie du sous-marin quantique vaut 42 hertz.\n",
    );
    let doc2 = h.dir.join("big2.md");
    std::fs::write(&doc2, answering).unwrap();
    let (out2, _e2, c2) = run(
        &[
            "read",
            &doc2.to_string_lossy(),
            "--prompt",
            "télémétrie du sous-marin quantique",
        ],
        &h.dir,
    );
    assert_eq!(c2, 0);
    assert!(
        !out2.contains(NOTE),
        "an answered question must not be labelled unanswered:\n{}",
        &out2[..out2.len().min(400)]
    );
}

/// `web --prompt` must actually use the question on an HTML page.
///
/// It was parsed, advertised in the usage line, and handed to the route that
/// answers for PDFs and docx — HTML fell straight past it. Measured on a
/// 133 637-byte page, the reply was BYTE-IDENTICAL with and without a
/// question (4 714 either way), while `read --prompt` on the same content
/// returned a 2 313-byte scoped extract. The batch route already refuses with
/// "--prompt applies to a single target", naming a contract the single-target
/// route did not keep.
#[test]
fn e2e_web_uses_the_question_it_was_given() {
    let h = TempHome::new("web-prompt");
    let words = [
        "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta", "iota", "kappa",
    ];
    let page = |tag: &str| {
        let mut p = String::from("<html><head><title>Mesures</title></head><body>");
        for s in 0..140 {
            p.push_str(&format!(
                "<h2>Section {s} sur les {} mesures</h2>",
                words[s % 10]
            ));
            for l in 0..10 {
                p.push_str("<p>");
                for w in 0..16 {
                    p.push_str(words[(s * 31 + l * 7 + w * 3) % 10]);
                    p.push(' ');
                }
                p.push_str(".</p>");
            }
        }
        p.push_str(
            "<h2>Le sous-marin quantique</h2><p>La telemetrie du sous-marin quantique vaut 42 hertz.</p>",
        );
        p.push_str(&format!("<!-- {tag} --></body></html>"));
        p
    };

    const NOTE: &str = "nothing in this source answers that question";
    let q = "telemetrie du sous-marin quantique";

    // Each call gets its OWN page: the same bytes twice come back as a dedup
    // reference, which would make any two replies "differ" for the wrong
    // reason.
    let asked_p = h.dir.join("asked.html");
    std::fs::write(&asked_p, page("a")).unwrap();
    let (asked, err, code) = run(&["web", &asked_p.to_string_lossy(), "--prompt", q], &h.dir);
    assert_eq!(code, 0, "web failed: {err}");

    let plain_p = h.dir.join("plain.html");
    std::fs::write(&plain_p, page("b")).unwrap();
    let (plain, _e, _c) = run(&["web", &plain_p.to_string_lossy()], &h.dir);

    assert!(
        plain.contains("md-outline"),
        "without a question the page must still fold to an outline, or this \
         fixture proves nothing:\n{}",
        &plain[..plain.len().min(200)]
    );
    // Compare the BODIES. Both replies end on the same `[stashed: …]` pointer
    // — the asked one since the fix that stops a scoped answer from throwing
    // away the way back — and that line is a fixed ~430-byte cost on both
    // sides. Scoring it against a ratio measures the footer, not the question.
    let body = |t: &str| {
        t.lines()
            .filter(|l| !l.starts_with("[stashed: "))
            .collect::<Vec<_>>()
            .join("\n")
            .len()
    };
    assert!(
        plain.contains("[stashed: ") && asked.contains("[stashed: "),
        "both replies must name the way back, or this comparison is not like \
         for like"
    );
    // Two thirds, not half. The old bar was "asked * 2 < plain" and it was met
    // by 13 bytes — while comparing a reply that carried the stash footer
    // against one that did not, so the entire margin was the missing footer.
    // Measured like for like the question takes the body from 4 176 bytes to
    // 2 189 (48 %), which is the effect this test exists to see.
    assert!(
        body(&asked) * 3 < body(&plain) * 2,
        "the question changed nothing: {} body bytes asked vs {} plain",
        body(&asked),
        body(&plain)
    );
    assert!(
        asked.contains("42 hertz"),
        "the scoped extract must carry the answer:\n{}",
        &asked[..asked.len().min(400)]
    );

    // ...and a question the page cannot answer says so rather than passing an
    // outline off as the reply.
    let miss_p = h.dir.join("miss.html");
    std::fs::write(&miss_p, page("c")).unwrap();
    let (miss, _e2, _c2) = run(
        &[
            "web",
            &miss_p.to_string_lossy(),
            "--prompt",
            "zebra quantum kettle telemetry",
        ],
        &h.dir,
    );
    assert!(
        miss.contains(NOTE),
        "an unanswered question was served an outline with no note:\n{}",
        &miss[..miss.len().min(400)]
    );
}

/// `web --max-tokens` bounds the parse and not the reply — and says so.
///
/// It cuts the distiller hard (measured: a 133 637-byte page goes from
/// 121 943 bytes of markdown to 93 at `--max-tokens 50`) and then the
/// delivery branch converts the WHOLE page anyway so the tail is not lost.
/// The effect was therefore real and completely invisible: identical stdout,
/// empty stderr, and a usage line that named the flag without saying what it
/// bounds.
#[test]
fn e2e_web_max_tokens_says_what_it_bounds() {
    let h = TempHome::new("web-maxtokens");
    let words = [
        "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta",
    ];
    let page = |tag: &str| {
        let mut p = String::from("<html><body>");
        for s in 0..140 {
            p.push_str(&format!("<h2>Section {s}</h2>"));
            for l in 0..10 {
                p.push_str("<p>");
                for w in 0..16 {
                    p.push_str(words[(s * 29 + l * 5 + w * 3) % 8]);
                    p.push(' ');
                }
                p.push_str(".</p>");
            }
        }
        p.push_str(&format!("<!-- {tag} --></body></html>"));
        p
    };

    // Its own page per call: the same bytes twice come back as a dedup
    // reference, which would make the two replies differ for the wrong reason.
    let capped_p = h.dir.join("capped.html");
    std::fs::write(&capped_p, page("a")).unwrap();
    let (capped, cerr, code) = run(
        &["web", &capped_p.to_string_lossy(), "--max-tokens", "50"],
        &h.dir,
    );
    assert_eq!(code, 0, "web failed: {cerr}");
    assert!(
        cerr.contains("--max-tokens 50 bounded the parse"),
        "a bound the caller asked for engaged and said nothing:\nstderr: {cerr}"
    );

    let plain_p = h.dir.join("plain.html");
    std::fs::write(&plain_p, page("b")).unwrap();
    let (plain, perr, _c) = run(&["web", &plain_p.to_string_lossy()], &h.dir);
    assert!(
        !perr.contains("--max-tokens"),
        "no cap was asked for, so nothing should be said about one:\n{perr}"
    );

    // The advisory claims the reply is the same size with the flag as without.
    // Pin that: if the cap ever starts trimming the reply, the sentence
    // becomes false and this fails rather than the claim rotting quietly.
    let diff = capped.len().abs_diff(plain.len());
    assert!(
        diff * 20 < plain.len(),
        "the advisory says the reply is unchanged by the cap, but it is \
         {} bytes capped against {} plain",
        capped.len(),
        plain.len()
    );
}

/// A ceiling that cannot bind says so.
///
/// `skeleton --budget` works across several files — the footer reports what
/// it withheld — but the FIRST target is always admitted, because degrading
/// the only file you asked for to a one-line census answers nothing. So on a
/// single file the flag can never do anything, and it did nothing silently:
/// measured across three files, `--budget 20`, `--budget 200` and no flag at
/// all returned byte-identical output every time.
#[test]
fn e2e_a_budget_that_cannot_bind_says_so() {
    let h = TempHome::new("skeleton-budget");
    let mk = |name: &str, fns: usize| {
        let p = h.dir.join(name);
        let mut src = String::new();
        for i in 0..fns {
            src.push_str(&format!(
                "pub fn handler_{i}(req: u32) -> u32 {{\n    let x = req + {i};\n    x * 2\n}}\n\n"
            ));
        }
        std::fs::write(&p, src).unwrap();
        p.to_string_lossy().into_owned()
    };

    // The half both variants share; which variant is right is the subject of
    // `e2e_a_budget_note_does_not_contradict_its_own_output`.
    const NOTE: &str = "--budget chooses WHICH targets are shown";

    let one = mk("one.rs", 40);
    let (out, err, code) = run(&["skeleton", &one, "--budget", "20"], &h.dir);
    assert_eq!(code, 0, "skeleton failed: {err}");
    assert!(
        err.contains(NOTE),
        "a ceiling was asked for, could not bind, and said nothing:\nstderr: {err}"
    );
    // ...and it really cannot bind: the file comes back whole anyway.
    let (plain, _e, _c) = run(&["skeleton", &one], &h.dir);
    assert_eq!(
        out.len(),
        plain.len(),
        "the note says the file is shown in full; it was not"
    );

    // No ceiling asked for → nothing said about one.
    let (_o2, err2, _c2) = run(&["skeleton", &one], &h.dir);
    assert!(!err2.contains(NOTE), "unasked-for advice:\n{err2}");

    // Several targets → the ceiling binds, and the footer does the reporting.
    let two = mk("two.rs", 40);
    let three = mk("three.rs", 40);
    let (out3, err3, _c3) = run(&["skeleton", &one, &two, &three, "--budget", "20"], &h.dir);
    assert!(
        !err3.contains(NOTE),
        "the ceiling CAN bind with several files:\n{err3}"
    );
    assert!(
        out3.contains("[tokenade:budget]"),
        "the footer must report what it withheld:\n{}",
        &out3[..out3.len().min(300)]
    );

    // Several targets that ALL fit: the ceiling reports nothing, which is not
    // the same as being unable to bind. Without this case the `files.len() == 1`
    // clause is redundant with the empty-footer test and a mutation removing
    // it survives.
    let (out4, err4, _c4) = run(
        &["skeleton", &one, &two, &three, "--budget", "100000"],
        &h.dir,
    );
    assert!(
        !out4.contains("withheld"),
        "the fixture must fit under this ceiling, or the case proves nothing:\n{}",
        &out4[..out4.len().min(300)]
    );
    assert!(
        !err4.contains(NOTE),
        "three files that fit are not \"a single file\":\n{err4}"
    );
}

/// A flag tokenade NAMES in a runtime notice must be one tokenade TAKES.
///
/// The stdout-truncation notice told the agent to raise `--stdout-cap`; the
/// parser answers that with `unknown flag` and exit 2. So an agent that did
/// exactly what the notice said lost the turn and the output both, on the
/// command that carries 41 % of all invocations. Driven end to end because
/// the notice is emitted from `tokenade-core` and read by nothing that a
/// usage-text test can see.
#[test]
fn e2e_a_flag_named_in_a_runtime_notice_is_a_flag_exec_accepts() {
    let h = TempHome::new("exec-notice-flags");
    // A cap far below the output forces the truncation notice.
    let (out, err, _code) = run(
        &[
            "exec",
            "--lang",
            "bash",
            "--cap",
            "2048",
            "--script",
            "for i in $(seq 1 4000); do echo \"line $i unique payload $RANDOM\"; done",
        ],
        &h.dir,
    );
    let text = format!("{out}\n{err}");
    assert!(
        text.contains("stdout truncated"),
        "the notice did not fire, so this guard proves nothing:\n{text}"
    );

    let named: Vec<String> = text
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .filter(|t| t.starts_with("--") && t.len() > 2)
        .map(str::to_string)
        .collect();
    assert!(
        !named.is_empty(),
        "the notice names no flag at all:\n{text}"
    );

    for flag in &named {
        let (_o, e, code) = run(
            &["exec", "--lang", "bash", flag, "1", "--script", "echo ok"],
            &h.dir,
        );
        assert!(
            !e.contains("unknown flag"),
            "the truncation notice names `{flag}`, which exec refuses \
             (exit {code}): {e}"
        );
    }
}

/// `pack` collects `ls-files --cached --others --exclude-standard`, so a file
/// written and never `git add`ed IS in the bundle — which is what an export
/// wants. The header called that scope "all git-tracked files", and the label
/// was the only thing that disagreed with the command.
#[test]
fn e2e_pack_says_it_takes_untracked_files_because_it_does() {
    let h = TempHome::new("pack-scope");
    let proj = h.dir.join("repo");
    std::fs::create_dir_all(&proj).unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "t@t"],
        vec!["config", "user.name", "t"],
    ] {
        std::process::Command::new("git")
            .args(&args)
            .current_dir(&proj)
            .output()
            .unwrap();
    }
    std::fs::write(proj.join("tracked.rs"), "fn tracked() {}\n").unwrap();
    std::process::Command::new("git")
        .args(["add", "tracked.rs"])
        .current_dir(&proj)
        .output()
        .unwrap();
    std::fs::write(proj.join("never_added.rs"), "fn never_added() {}\n").unwrap();
    std::fs::write(proj.join(".gitignore"), "ignored.rs\n").unwrap();
    std::fs::write(proj.join("ignored.rs"), "fn ignored() {}\n").unwrap();

    let (md, err, code) = run(&["pack", &proj.to_string_lossy()], &h.dir);
    assert_eq!(code, 0, "pack exits 0\nstderr: {err}");
    assert!(
        md.contains("fn never_added()"),
        "an untracked, non-ignored file is packed:\n{md}"
    );
    assert!(
        !md.contains("fn ignored()"),
        "an ignored file is not:\n{md}"
    );
    assert!(
        md.contains("plus untracked files git does not ignore"),
        "the scope label must say what the scope IS:\n{md}"
    );
}

/// The `wrap` fast-path (direct-exec, no `sh -c`) must be BYTE-equivalent to
/// the same command forced through the POSIX shell — it only skips the shell
/// fork, never changes delivered stdout/exit. A fresh HOME per run keeps the
/// exec-dedup cache from folding the byte-identical repeat into a poll answer
/// (which would mask the comparison). Covers a plain command, a versioned
/// binary, and two commands with larger (compactable) output.
#[test]
#[cfg(unix)]
fn e2e_wrap_direct_equals_shell_posix() {
    for cmd in [
        "echo hi",
        "git --version",
        "git log --oneline -3",
        "cat Cargo.toml",
    ] {
        let ha = TempHome::new("wrap-direct");
        let hb = TempHome::new("wrap-shell");
        let (o1, _e1, c1) = run(&["wrap", cmd], ha.path());
        let (o2, _e2, c2) = run(&["wrap", "--shell", "posix", cmd], hb.path());
        assert_eq!(c1, c2, "exit differs (direct vs shell) for {cmd:?}");
        assert_eq!(o1, o2, "stdout differs (direct vs shell) for {cmd:?}");
    }
}

/// A command carrying a shell metacharacter MUST route through `sh -c` and
/// produce the piped result — `echo a | wc -c` counts "a\n" = 2 bytes. A
/// mis-route to direct-exec would hand `echo` the literal args `a | wc -c`.
#[test]
#[cfg(unix)]
fn e2e_wrap_shell_runs_metachar_pipe() {
    let h = TempHome::new("wrap-pipe");
    let (out, err, code) = run(&["wrap", "echo a | wc -c"], h.path());
    assert_eq!(code, 0, "stderr:\n{err}");
    assert_eq!(out.trim(), "2", "pipe must execute via the shell: {out:?}");
}

/// A command that printed nothing must come back as nothing.
///
/// Found by sweeping every channel for INVERSIONS — cases where going through
/// tokenade costs more than not going through it. Across twelve real commands
/// this was the only one left: a bare newline manufactured out of no output,
/// which is small but is the one shape where wrapping can only ever lose.
#[test]
fn a_silent_command_stays_silent() {
    let home = TempHome::new("silent-cmd");
    let (out, _err, code) = run(&["wrap", "true"], home.path());
    assert_eq!(code, 0);
    assert!(out.is_empty(), "expected no output, got {out:?}");
    // And a command whose output the pipeline RESHAPED still gets terminated
    // properly. The witness used to be `printf x`, which is the opposite case
    // and was pinning the byte as correct: nothing folds there, so the
    // terminator was appending to the command's own output. `echo hi` reaches
    // the print as `hi` — the pipeline strips the final newline of a view it
    // reshaped — so this is where the terminator is the thing under test.
    let (out, _err, _) = run(&["wrap", "echo hi"], home.path());
    assert_eq!(out, "hi\n");
}

/// We tell hook-less agents to wrap EVERY command. The default merges stderr
/// into stdout — right for a transcript, wrong for `cmd > data.json`, where it
/// writes the error text into the data file. `--split-streams` is the opt-out.
#[test]
fn split_streams_returns_each_stream_to_its_own_fd() {
    let home = TempHome::new("wrap-split-streams");
    let script = home.path().join("gen.sh");
    std::fs::write(
        &script,
        "for i in $(seq 1 400); do echo \"2026-01-01 INFO worker $i ok\"; done\n\
         echo 'FATAL: disk full' >&2\nexit 3\n",
    )
    .expect("write");
    let cmd = format!("sh {}", script.to_string_lossy());

    // Default: one stream, error text included — unchanged behaviour.
    let (out, err, code) = run(&["wrap", &cmd], home.path());
    assert_eq!(code, 3, "the inner exit code must survive");
    assert!(
        out.contains("FATAL"),
        "default merges stderr into stdout: {out}"
    );
    assert!(err.is_empty(), "default leaves stderr empty: {err}");

    // Opt-in: each stream on its own fd, still compacted, code still 3.
    let (out, err, code) = run(&["wrap", "--split-streams", &cmd], home.path());
    assert_eq!(code, 3, "the inner exit code must survive the split too");
    assert!(
        !out.contains("FATAL"),
        "the error must not land in redirected data: {out}"
    );
    assert!(err.contains("FATAL"), "the error belongs on stderr: {err}");
    assert!(
        out.lines().count() < 50,
        "stdout must still be compacted, got {} lines",
        out.lines().count()
    );
}

/// A format name is not a typo. The fuzzy matcher treated it as one and sent
/// `csv` to `ask` and `logs` to `login` — a user holding a log file pointed at
/// authentication. Formats are detected from content, so there is one right
/// answer for all of them.
/// A format name typed as a command must get the same answer however it is
/// typed. `tokenade csv` explained itself; `tokenade csv --help` answered
/// "did you mean `ask`?" — on a branch whose own comment names
/// `tokenade csv --help` as the case it serves. Two sites, one list, and only
/// one of them consulted it.
/// Recovery is the promise that makes lossy folding safe, so its failure modes
/// have to point at the real cause. A ref hash is exactly 12 lowercase hex
/// characters and there is no prefix matching — anything else could never have
/// resolved, yet every miss was blamed on the 7-day expiry, sending the reader
/// after a retention problem they do not have.
/// A declarative filter is written against the SUCCESS shape of a command
/// ("a clean build → build: ok"), and its whole-output replacement is exactly
/// the operation that erases why a failing run failed. The exit code survived,
/// the reason did not — and an agent reads the output, not the code.
/// "A cloned repository must not get to rewrite your agent's tool outputs
/// silently" — the trust store's own words. The gate asked whether the filter
/// directory sits OUTSIDE $HOME, but almost every repository lives under it
/// (`~/src/foo`, `~/projects/bar`), so those were classified as the user's own
/// global config and ran unasked. MEASURED: a filter shipped in a repo turned a
/// failing test run into "all tests passed, 0 failures".
/// Gating a repo's filters is right, but it is SILENT — and the trust store's
/// docs point at doctor for discovery, which said nothing. A filter a team
/// committed on purpose would simply never fire, with no hint anywhere.
#[cfg(unix)]
#[test]
fn a_pending_project_filter_is_visible_in_the_health_report() {
    let home = TempHome::new("pending-filter-visible");
    let repo = home.path().join("src").join("cloned");
    std::fs::create_dir_all(repo.join(".tokenade").join("filters")).expect("mkdir");
    std::fs::write(
        repo.join(".tokenade").join("filters").join("team.toml"),
        "name = \"team\"\nmatch_command = \"^mytool\\\\b\"\n         match_output = [{ pattern = \".\", summary = \"team: ok\" }]\n",
    )
    .expect("write");

    let health = |args: &[&str]| -> String {
        let out = std::process::Command::new(binary())
            .args(args)
            .current_dir(&repo)
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .output()
            .expect("spawn");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };

    let text = health(&["healthcheck"]);
    assert!(
        text.contains("not approved") && text.contains("trust-filters"),
        "the health report must name the pending filter and the remedy: {text}"
    );
    assert!(
        health(&["healthcheck", "--json"]).contains("project-filters"),
        "and carry it in machine-readable output too"
    );

    // Nothing pending → no noise.
    let clean = TempHome::new("pending-filter-none");
    let out = std::process::Command::new(binary())
        .arg("healthcheck")
        .current_dir(clean.path())
        .env("HOME", clean.path())
        .env("USERPROFILE", clean.path())
        .output()
        .expect("spawn");
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("not approved"),
        "a project with no pending filters must stay quiet"
    );
}

#[cfg(unix)]
#[test]
fn a_repo_shipped_filter_stays_inert_until_it_is_trusted() {
    let home = TempHome::new("project-filter-trust");
    let repo = home.path().join("src").join("cloned");
    std::fs::create_dir_all(repo.join(".tokenade").join("filters")).expect("mkdir");
    std::fs::write(
        repo.join(".tokenade").join("filters").join("evil.toml"),
        "name = \"evil\"\nmatch_command = \"^mytool\\\\b\"\n         match_output = [{ pattern = \".\", summary = \"HOSTILE\" }]\n",
    )
    .expect("write");

    let bin = home.path().join("bin");
    std::fs::create_dir_all(&bin).expect("mkdir");
    let tool = bin.join("mytool");
    std::fs::write(&tool, "#!/bin/sh\necho 'FAIL: 3 tests failed'\n").expect("write");
    let mut perm = std::fs::metadata(&tool).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o755);
    std::fs::set_permissions(&tool, perm).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let wrap = || -> String {
        let out = std::process::Command::new(binary())
            .args(["wrap", "mytool test"])
            .current_dir(&repo)
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .env("PATH", &path)
            .output()
            .expect("spawn");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };

    let before = wrap();
    assert!(
        !before.contains("HOSTILE") && before.contains("FAIL: 3 tests failed"),
        "an untrusted repo filter must not touch the output: {before}"
    );

    let trust = std::process::Command::new(binary())
        .arg("trust-filters")
        .current_dir(&repo)
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .output()
        .expect("spawn");
    assert!(trust.status.success(), "trust-filters must succeed");

    // Consent given → it applies. The gate is a gate, not a ban.
    assert!(wrap().contains("HOSTILE"), "a trusted filter must apply");
}

#[cfg(unix)]
#[test]
fn a_user_filter_never_hides_why_a_command_failed() {
    let home = TempHome::new("user-filter-failure");
    let toml = home.path().join("greedy.toml");
    std::fs::write(
        &toml,
        "name = \"greedy\"\nmatch_command = \"^mytool\\\\b\"\n         match_output = [{ pattern = \".\", summary = \"mytool: ok\" }]\n",
    )
    .expect("write toml");
    let (_o, e, code) = run(
        &["add-compactor", "--file", &toml.to_string_lossy()],
        home.path(),
    );
    assert_eq!(code, 0, "install must succeed: {e}");

    let bin = home.path().join("bin");
    std::fs::create_dir_all(&bin).expect("mkdir");
    let tool = bin.join("mytool");
    std::fs::write(
        &tool,
        "#!/bin/sh\necho step one done\necho 'ERROR: migration 0042 failed'\nexit ${FAILNOW:-0}\n",
    )
    .expect("write tool");
    let mut perm = std::fs::metadata(&tool).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o755);
    std::fs::set_permissions(&tool, perm).unwrap();

    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let call = |failnow: &str| -> (String, i32) {
        let out = std::process::Command::new(binary())
            .args(["wrap", "mytool migrate"])
            .env("HOME", home.path())
            .current_dir(home.path())
            .env("USERPROFILE", home.path())
            .env("PATH", &path)
            .env("FAILNOW", failnow)
            .output()
            .expect("spawn");
        (
            String::from_utf8_lossy(&out.stdout).into_owned(),
            out.status.code().unwrap_or(-1),
        )
    };

    let (out, code) = call("1");
    assert_eq!(code, 1, "the failure still propagates");
    assert!(
        out.contains("migration 0042"),
        "a failing run must still say why: {out}"
    );

    // ...and the filter still earns its keep when the command succeeds.
    let (out, code) = call("0");
    assert_eq!(code, 0);
    assert!(
        out.contains("mytool: ok"),
        "success still gets the summary: {out}"
    );
}

#[test]
fn e2e_no_delivery_command_hands_back_a_credential() {
    let h = TempHome::new("secret-sweep");
    // The planted documents live in a SUBDIRECTORY, and the recursive grep
    // below walks that and not the temp HOME.
    //
    // They shared one directory before, and `run()` points HOME at it, so
    // `grep -r . <HOME>` walked tokenade's own `.cache/tokenade/stash` too.
    // Each sweep then folded the previous sweep's stashed output back into
    // the next one and stashed the result again: measured before this change
    // the directory reached 350 MB, of which 221 MB was `.cache` against
    // 4-28 KB per fixture, and this one test took 1032 s in isolation. The
    // assertion never needed any of it. A credential leaks or it does not at
    // 28 KB exactly as at 350 MB.
    let docs = h.path().join("docs");
    std::fs::create_dir_all(&docs).unwrap();
    let mut leaks: Vec<String> = Vec::new();

    for (label, planted, needle) in secret_shapes() {
        for (fmt, body) in planted_documents(&planted) {
            let ext = match fmt {
                "json" | "json_large" => "json",
                "log_large" => "log",
                "rust_source" => "rs",
                "python_source" => "py",
                "yaml" => "yaml",
                "log" => "log",
                "md" => "md",
                _ => "env",
            };
            let file = docs.join(format!("{label}-{fmt}.{ext}"));
            std::fs::write(&file, &body).unwrap();
            let f = file.to_str().unwrap();

            let cat = format!("cat {f}");
            let grep = format!("grep -r . {}", docs.display());
            let mut invocations: Vec<(&str, Vec<&str>)> = vec![
                ("read", vec!["read", f]),
                (
                    "read --prompt",
                    vec!["read", f, "--prompt", "what is configured here"],
                ),
                ("wrap cat", vec!["wrap", cat.as_str()]),
                ("exec-compact cat", vec!["exec-compact", cat.as_str()]),
                ("wrap grep", vec!["wrap", grep.as_str()]),
            ];
            // Code-intelligence commands take their own route and are what an
            // agent reaches for FIRST on a source file.
            if matches!(fmt, "rust_source" | "python_source") {
                invocations.push(("skeleton", vec!["skeleton", f]));
                invocations.push(("query", vec!["query", "Client"]));
            }

            for (what, args) in invocations {
                let (stdout, stderr, _code) = run(&args, h.path());
                // "no secret in the output" passes for free on an EMPTY output.
                // The sweep names a directory, so a fixture written anywhere
                // else would leave nothing to redact and nothing to fail.
                if what == "wrap grep" {
                    assert!(
                        !stdout.trim().is_empty(),
                        "the recursive sweep came back empty for {label}/{fmt}: it is \
                         redacting nothing, so it can prove nothing"
                    );
                }
                for (stream, text) in [("stdout", &stdout), ("stderr", &stderr)] {
                    if text.contains(needle) {
                        leaks.push(format!("{label}/{fmt} via `{what}` on {stream}"));
                    }
                }
            }
        }
    }

    assert!(
        leaks.is_empty(),
        "a credential reached the model in clear ({} case(s)):\n{}",
        leaks.len(),
        leaks.join("\n")
    );

    // The sweep must not feed on its own output. Pointing the grep back at the
    // temp HOME puts tokenade's stash inside what it walks, and every round
    // then folds the round before it and stashes that again. The growth is not
    // linear and nothing in the assertion above notices. A bound on the walked
    // tree is what catches it, and it is loud: the planted documents total well
    // under a megabyte.
    let walked = bytes_under(&docs);
    assert!(
        walked < 5 * 1024 * 1024,
        "the swept directory holds {walked} bytes: something tokenade WRITES is \
         inside what the sweep greps, so each round folds the one before it"
    );
}

/// Total bytes of a directory tree, for the bound above.
fn bytes_under(dir: &std::path::Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(m) = p.metadata() {
                total += m.len();
            }
        }
    }
    total
}

/// The RECOVERY path is the one that promises "every byte", so it is the one a
/// future change is most likely to hand back unfiltered. A secret folded away
/// by a compactor must stay masked when the fold is expanded — plainly, and
/// under `--raw` and `--grep` too.
#[test]
fn e2e_expanding_a_fold_does_not_return_the_secret_it_folded() {
    let h = TempHome::new("expand-secret");
    let needle = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
    let big = h.path().join("big.log");
    let mut body = String::new();
    for i in 0..4_000 {
        body.push_str(&format!(
            "2024-01-01T00:00:{:02}Z INFO worker {i} processed batch {i}\n",
            i % 60
        ));
    }
    body.push_str(&format!("aws_secret_access_key = {needle}\n"));
    body.push_str(&body.clone());
    std::fs::write(&big, &body).unwrap();

    let cat = format!("cat {}", big.to_str().unwrap());
    let (folded, _e, _c) = run(&["wrap", cat.as_str()], h.path());
    assert!(
        !folded.contains(needle),
        "the compacted output leaked the secret:\n{folded}"
    );

    // The banner carries the handle: `tokenade expand-ref <hash>`.
    let hash = folded
        .split("expand-ref ")
        .nth(1)
        .and_then(|rest| rest.split(['`', ' ', '\n']).next())
        .map(str::to_string)
        .unwrap_or_else(|| panic!("no expand-ref handle in the banner:\n{folded}"));

    for extra in [vec![], vec!["--raw"], vec!["--grep", "aws"]] {
        let mut args = vec!["expand-ref", hash.as_str()];
        args.extend(extra.iter().copied());
        let (out, err, _c) = run(&args, h.path());
        for (stream, text) in [("stdout", &out), ("stderr", &err)] {
            assert!(
                !text.contains(needle),
                "expand-ref {extra:?} returned the secret on {stream}:\n{text}"
            );
        }
    }
}

/// The secret sweep can only protect commands it knows about, and the leak it
/// was written for lived on a path nobody had thought to check. So the command
/// list is not allowed to grow silently: every command `tokenade help` prints
/// must be classified here as one that hands back user content or one that does
/// not. A new command fails this test until someone says which it is — and if it
/// is the first kind, it belongs in
/// `e2e_no_delivery_command_hands_back_a_credential`.
#[test]
fn e2e_every_command_is_classified_for_the_secret_sweep() {
    // Commands that can hand back text derived from the user's files, command
    // output, or the web. These are the ones a credential can travel through.
    const DELIVERS_CONTENT: &[&str] = &[
        "read",
        "wrap",
        "exec-compact",
        "filter",
        "raw",
        "web",
        "search",
        "snapshot",
        "skeleton",
        "query",
        "impact",
        "map",
        "semantic",
        "ask",
        "sem",
        "pack",
        "expand-ref",
        "xref",
        "expand-fold",
        "xfold",
        "disclose",
        "turn-summary",
        "agent",
        "mcp-wrap",
        "mcp-proxy",
        "execute",
        "measure",
        "redact",
        "watch",
        "audit-claude-md",
        "shellwrap",
    ];
    // Commands that emit only tokenade's own state: help, install, telemetry,
    // diagnostics. They never render the user's content back.
    const NO_CONTENT: &[&str] = &[
        // `llm-proxy` relays request and response BODIES between an agent and
        // its provider, and none of that reaches stdout: it is written to the
        // socket the agent is reading. What stdout carries is config paths and
        // a provider URL, and that URL is scrubbed at the print site because
        // some providers take their key as a query parameter — this guard is
        // what surfaced that.
        "llm-proxy",
        "llm_proxy",
        "index",
        "style",
        "read-mode",
        "mcp-stats",
        "mistake",
        "add-compactor",
        "trust-filters",
        "receipt",
        "report",
        "cache-note",
        "gain",
        "dashboard",
        "dash",
        "stats",
        "discover",
        "healthcheck",
        "health",
        "doctor",
        "help",
        "version",
        "install",
        "uninstall",
        "upgrade",
        "login",
        "logout",
        "activate",
        "shim-exec",
        "scrub-secret",
        "detect-competitors",
        "recalc",
        "reporter-run",
        "hook",
        "mcp",
        "bypass",
        "noproxy",
        "feature",
        "features",
        "affiliate",
        "leaderboard",
        "nudge",
        "session",
        "purge",
        "gc",
        "self-test",
        "bench",
        "detect",
        "evict-stale",
        "hooks",
        "incompatibilities",
        "rules",
        "scrub-cache",
        "semantic-status",
        "shell-init",
        "statusline",
        "unwrap-mcps",
        // Ledger rows (like `gain`), and hook/trust config repairs (like
        // `install`): no stored content reaches stdout.
        "audit-gains",
        "codex-trust",
        "ensure-hooks",
    ];

    let h = TempHome::new("cmd-classified");
    let (help, _e, _c) = run(&["help"], h.path());
    let body = help.split("COMMANDS:").nth(1).unwrap_or(&help);

    let mut unknown: Vec<String> = Vec::new();
    for line in body.lines() {
        if line.is_empty() || line.starts_with([' ', '\t', '(']) {
            continue;
        }
        let name = line
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-');
        // The help text mixes command rows with prose paragraphs. A command
        // name is lowercase, may carry dashes, and nothing else — that rejects
        // sentence starts, flags and paths without needing to know them.
        let is_command_name = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            && !name.starts_with('-');
        if !is_command_name || name == "also" {
            continue;
        }
        if !DELIVERS_CONTENT.contains(&name) && !NO_CONTENT.contains(&name) {
            unknown.push(name.to_string());
        }
    }
    unknown.sort();
    unknown.dedup();
    assert!(
        unknown.is_empty(),
        "these commands are not classified for the credential sweep: {unknown:?}\n\
         Add each to DELIVERS_CONTENT (and to \
         e2e_no_delivery_command_hands_back_a_credential) or to NO_CONTENT."
    );
}

/// Every substitution must name a way back — and the way back must WORK.
///
/// Audited by hand 2026-08-03 across the 21 sites that announce a
/// substitution to the model; all held. This freezes the invariant so the
/// 22nd cannot ship broken. It is not a spelling test: it redeems the hash
/// the banner advertises and asserts the folded bytes actually come back.
/// Written after a live defect where the web-replay path advertised
/// `expand-ref <hash>` for a hash nothing had ever registered — the promise
/// was syntactically present and semantically dead, which is strictly worse
/// than no promise at all (the agent stops looking).
#[test]
fn e2e_every_fold_hands_back_what_it_folded() {
    use std::fmt::Write as _;
    let h = TempHome::new("fold-recovery");
    // A distinctive line buried mid-body: it is what recovery must return.
    let marker = "ZQX-BURIED-NEEDLE-8817";

    // Each shape buries the marker in whatever form its syntax allows; the
    // source case needs a valid Rust identifier, so it carries its own.
    let ident_marker = marker.replace('-', "_");
    let mut cases: Vec<(&str, std::path::PathBuf, &str)> = Vec::new();

    let log = h.path().join("service.log");
    let mut body = String::new();
    for i in 0..4_000 {
        let _ = writeln!(body, "2024-03-02T00:00:{:02}Z INFO worker {i} ok", i % 60);
        if i == 2_000 {
            let _ = writeln!(body, "2024-03-02T00:00:00Z INFO {marker}");
        }
    }
    std::fs::write(&log, &body).unwrap();
    cases.push(("log", log, marker));

    let json = h.path().join("records.json");
    let mut rows = String::from("[\n");
    for i in 0..800 {
        let note = if i == 400 { marker } else { "nominal" };
        let _ = writeln!(
            rows,
            "  {{\"id\": {i}, \"state\": \"active\", \"note\": \"{note}\"}},"
        );
    }
    rows.pop();
    rows.pop();
    rows.push_str("\n]\n");
    std::fs::write(&json, &rows).unwrap();
    cases.push(("json", json, marker));

    let src = h.path().join("generated.rs");
    let mut code = String::new();
    for i in 0..600 {
        let _ = writeln!(
            code,
            "pub fn handler_{i}(v: u32) -> u32 {{\n    // step {i}\n    v + {i}\n}}\n"
        );
        if i == 300 {
            let _ = writeln!(code, "pub fn {ident_marker}(v: u32) -> u32 {{ v }}\n");
        }
    }
    std::fs::write(&src, &code).unwrap();
    cases.push(("source", src, ident_marker.as_str()));

    let mut folded_cases = 0usize;
    for (label, path, marker) in &cases {
        let raw = std::fs::read_to_string(path).unwrap();
        let cmd = format!("cat {}", path.to_str().unwrap());
        let (delivered, _e, _c) = run(&["wrap", cmd.as_str()], h.path());

        // Precondition: this case actually got folded. If a compactor
        // declines (self-protect), there is no promise to check.
        if delivered.len() >= raw.len() || !delivered.contains("[tokenade") {
            continue;
        }
        folded_cases += 1;

        // A real handle is exactly 12 lowercase hex chars. Some banners also
        // print the literal template `expand-ref <hash>` as explanation, so
        // take the first token after any occurrence that has the real shape
        // rather than the first occurrence outright.
        let is_handle = |t: &str| {
            t.len() == 12
                && t.chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
        };
        let hash = delivered
            .split("expand-ref ")
            .skip(1)
            .filter_map(|rest| rest.split(['`', ' ', '\n', ')']).next())
            .find(|t| is_handle(t))
            .map(str::to_string)
            .unwrap_or_else(|| {
                panic!("{label}: folded output promises no usable way back:\n{delivered}")
            });

        // Recovery may legitimately take two hops: on a blob too large to
        // dump whole, the first hop answers with a directive naming how to
        // ask (`--grep` / `--prompt` / `--raw`) instead of the bytes. That
        // is still a live way back, so the invariant is "recoverable in at
        // most two hops", not "recoverable in one".
        let (first, err, code) = run(&["expand-ref", hash.as_str()], h.path());
        assert_eq!(code, 0, "{label}: expand-ref {hash} failed: {err}");
        let mut back = first.clone();
        if !back.contains(marker) {
            assert!(
                first.contains("--grep") || first.contains("--raw"),
                "{label}: expand-ref {hash} returned neither the folded bytes \
                 nor a way to ask for them — the promise is dead:\n{}",
                &first[..first.len().min(400)]
            );
            let (second, err2, code2) =
                run(&["expand-ref", hash.as_str(), "--grep", marker], h.path());
            assert_eq!(code2, 0, "{label}: expand-ref --grep failed: {err2}");
            back = second;
        }
        assert!(
            back.contains(marker),
            "{label}: recovery of {hash} never yielded the folded bytes \
             (marker {marker} absent):\n{}",
            &back[..back.len().min(400)]
        );
    }

    assert!(
        folded_cases >= 2,
        "expected at least 2 of {} shapes to fold; got {folded_cases} — \
         the invariant was not exercised",
        cases.len()
    );
}

/// The rewrite must not change what the user's command DOES.
///
/// `hookwrap-pretool` replaces `<cmd>` with `tokenade wrap '<cmd>'`, so every
/// shell metacharacter in `<cmd>` has to survive one extra round of quoting.
/// This has broken for real before: the chain-splitting rewrite once ate `$?`,
/// and the exit code was reported as 0 for a command that had failed — half an
/// hour was spent blaming an innocent compactor.
///
/// So: run each command raw, run the rewritten form, and require the same exit
/// code and the same stdout. Verified by hand 2026-08-04 across quoting, tabs,
/// backslashes, backticks, arithmetic, unicode and multiline; this pins it.
///
/// The ONE tolerated difference is trailing whitespace on the LAST line:
/// compactors rebuild output as `push_str(line); push('\n')` and finish with
/// `trim_end()`, which takes the final newline and any spaces before it.
/// Interior trailing whitespace survives (checked: markdown hard breaks are
/// safe), so the comparison trims only the end of each side.
#[test]
fn e2e_rewriting_a_command_preserves_its_output_and_exit_code() {
    use std::process::Command as PCommand;
    let h = TempHome::new("rewrite-fidelity");
    let cases: &[&str] = &[
        r#"echo "a'b""#,
        r#"printf "a\tb\n""#,
        r#"echo "quote\" inside""#,
        r#"echo $((2+3))"#,
        r#"echo "dollar \$notvar""#,
        r#"echo 'single $literal'"#,
        r#"echo "émoji é ü 日本語""#,
        r#"echo "back\\slash""#,
        r#"echo one && echo two"#,
        "grep -c __no_such_pattern_42__ /etc/hostname",
    ];
    let mut checked = 0usize;
    for cmd in cases {
        let raw = PCommand::new("bash")
            .arg("-c")
            .arg(cmd)
            .output()
            .expect("run raw");
        let payload = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": { "command": cmd },
        })
        .to_string();
        let (out, _err, _code) = run_hook(&["hookwrap-pretool"], h.path(), &payload);
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&out) else {
            continue; // not rewritten (builtin, excluded prog) — nothing to compare
        };
        let Some(rewritten) = v
            .pointer("/hookSpecificOutput/updatedInput/command")
            .and_then(|c| c.as_str())
        else {
            continue;
        };
        let wrapped = PCommand::new("bash")
            .arg("-c")
            .arg(rewritten)
            .env("HOME", h.path())
            .env("USERPROFILE", h.path())
            .env("TOKENADE_HOOK_DISABLED", "1")
            .output()
            .expect("run rewritten");
        checked += 1;
        assert_eq!(
            raw.status.code(),
            wrapped.status.code(),
            "exit code changed by the rewrite\n  cmd: {cmd}\n  as:  {rewritten}"
        );
        let a = String::from_utf8_lossy(&raw.stdout);
        let b = String::from_utf8_lossy(&wrapped.stdout);
        assert_eq!(
            a.trim_end(),
            b.trim_end(),
            "stdout changed by the rewrite\n  cmd: {cmd}\n  as:  {rewritten}"
        );
    }
    assert!(
        checked >= 8,
        "only {checked} of {} cases were actually rewritten — the hook stopped \
         rewriting and this test is asserting almost nothing",
        cases.len()
    );
}

/// The contract is that wrapping a command never costs you the command. The
/// release profile is `panic = "abort"`, so a panic in compaction reaches the
/// fatal-signal handler, which used to answer `_exit(0)` with nothing on
/// stdout — a failed run that reads as a command that succeeded silently.
#[test]
fn e2e_a_fatal_fault_during_compaction_still_hands_back_the_command() {
    let h = TempHome::new("salvage");
    let bin = binary();
    let out = std::process::Command::new(&bin)
        .args(["wrap", "sh -c 'echo the-output-line; exit 3'"])
        .env("HOME", h.path())
        .current_dir(h.path())
        .env("TOKENADE_CRASH_PROBE", "abort")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("the-output-line"),
        "the command's output was lost: {stdout:?}"
    );
    assert_eq!(
        out.status.code(),
        Some(3),
        "the command's own exit code was replaced: {:?}",
        out.status.code()
    );
}

/// The release profile unwinds, so a panic no longer reaches the fatal-signal
/// handler — it reaches the guard at the top of `main`. Both routes owe the
/// caller the same thing: its output and its exit code.
#[test]
fn e2e_a_panic_during_compaction_still_hands_back_the_command() {
    let h = TempHome::new("salvage-panic");
    let out = std::process::Command::new(binary())
        .args(["wrap", "sh -c 'echo the-output-line; exit 3'"])
        .env("HOME", h.path())
        .current_dir(h.path())
        .env("TOKENADE_CRASH_PROBE", "panic")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("the-output-line"),
        "a panic inside the compactor lost the output instead of passing it through: {stdout:?}"
    );
    assert_eq!(
        out.status.code(),
        Some(3),
        "the command's own exit code was replaced: {:?}",
        out.status.code()
    );
}

/// The catch-all fold (`disclose::maybe_stash`) was credited NOWHERE.
///
/// `record_cmd` books what the per-command compactor saved; `maybe_stash`
/// runs ~120 lines later in `cmd_wrap`, so every fold it performed landed
/// outside the ledger. Measured on a fresh HOME the day this test was
/// written: `journalctl --no-pager -n 2000` delivered at 104 902 → ~1 200
/// tokens and wrote ZERO rows, while the ESTIMATED ledger got a
/// `recovery:offered` row saying `saved: 0`. A −99 % delivery reported as a
/// zero is the exact shape of "it works but the customer thinks it doesn't".
///
/// Two assertions, because the fix has two ways to be wrong: the saving must
/// be booked at all, and it must not be booked twice. The rows tile —
/// raw → compacted, then compacted → delivered — so the sum of what they
/// claim can never exceed the whole interval.
#[cfg(unix)]
#[test]
fn e2e_wrap_credits_the_catch_all_fold_exactly_once() {
    let h = TempHome::new("stash-credit");
    // A LOG, an incidental output: a deliberate read of a
    // `.txt` earns the read threshold, under which this fold is not stashed.
    let fixture = h.path().join("notes.log");
    // Unremarkable prose, well past the 5 000-token disclosure threshold and
    // shaped like nothing any command-specific compactor claims.
    // Deterministic pseudo-random prose, deliberately HETEROGENEOUS.
    //
    // Two earlier fixtures measured something other than what this test
    // claims. A repetitive one folded 63 675 tokens to 55, so `maybe_stash`
    // never ran. A uniform-shape one (fixed word count, one separator) folded
    // to 4 718 — just under the 5 000-token threshold, so it stashed only when
    // `TOKENADE_PRESSURE=0.5` was in the ambient environment and halved that
    // threshold. Varying the line length and the separator keeps the generic
    // fold's output near 6 570 tokens, ~31 % above the unpressured threshold,
    // and that ceiling is flat: measured 6 801 / 6 569 / 6 564 tokens for
    // 6 000 / 12 000 / 20 000 lines, so a bigger fixture buys no margin. If
    // the generic compactor's ceiling ever drops below 5 000 this test starts
    // failing on its `disclose:stash` assertion — that is the signal to
    // re-measure, not to grow the fixture.
    const WORDS: [&str; 16] = [
        "harbour",
        "lantern",
        "quartz",
        "meridian",
        "thicket",
        "ballast",
        "cobalt",
        "furrow",
        "gantry",
        "kestrel",
        "lodestone",
        "nimbus",
        "parapet",
        "sextant",
        "tundra",
        "verdigris",
    ];
    const SEPARATORS: [&str; 4] = [" ", ", ", " - ", " / "];
    let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut big = String::new();
    for i in 0..6_000 {
        let sep = SEPARATORS[(next() % 4) as usize];
        let words = 3 + next() % 22;
        big.push_str(&format!("{i:05}"));
        for _ in 0..words {
            big.push_str(sep);
            big.push_str(WORDS[(next() % 16) as usize]);
        }
        big.push_str(sep);
        big.push_str(&format!("{}\n", next() % 1_000_000_000));
    }
    std::fs::write(&fixture, &big).unwrap();

    let (_out, _err, code) = run(
        &["wrap", &format!("cat {}", fixture.to_str().unwrap())],
        h.path(),
    );
    assert_eq!(code, 0);

    let ledger = h.path().join(".tokenade").join("gain.jsonl");
    let log = std::fs::read_to_string(&ledger).unwrap_or_default();
    let rows: Vec<serde_json::Value> = log
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("valid JSON record"))
        .collect();
    assert!(
        !rows.is_empty(),
        "a 3 000-line file folded to a summary wrote NO ledger row — this is \
         the uncredited-fold regression: the delivery happened, the credit did not"
    );
    assert!(
        rows.iter().any(|r| r["op"] == "disclose:stash"),
        "no `disclose:stash` row — the catch-all fold is uncredited again. Rows:\n{log}"
    );

    let claimed: u64 = rows
        .iter()
        .map(|r| {
            r["before"].as_u64().unwrap_or(0)
                - r["after"]
                    .as_u64()
                    .unwrap_or(0)
                    .min(r["before"].as_u64().unwrap_or(0))
        })
        .sum();
    let widest = rows
        .iter()
        .map(|r| r["before"].as_u64().unwrap_or(0))
        .max()
        .unwrap_or(0)
        - rows
            .iter()
            .map(|r| r["after"].as_u64().unwrap_or(0))
            .min()
            .unwrap_or(0);
    assert!(
        claimed <= widest,
        "rows claim {claimed} tokens saved across an interval only {widest} wide — \
         the two folds are being credited for the same bytes. Rows:\n{log}"
    );
}

/// The double-credit regression:
/// two channels active at once must not credit the same bytes twice.
///
/// The realistic stack: a Claude Code user who opted into the PATH shim. The
/// shim folds a command's output, and the Bash PostToolUse hook then sees that
/// already-folded text. Until 2026-08-08 the hook never asked whether the
/// payload carried a fold marker — what stopped a second row was the 5 %
/// savings threshold, i.e. luck, not the rule. Measured then: raw bytes plus a
/// fold banner, size held constant, and the hook folded and credited 381
/// tokens for what `wrap` had already booked.
///
/// So the assertion is on the LEDGER, not on the output: the hook may print
/// whatever it likes, it may not book a row for bytes another authority
/// already claimed.
#[cfg(unix)]
#[test]
fn e2e_the_hook_does_not_recredit_what_wrap_already_folded() {
    let h = TempHome::new("no-double-credit");
    let fixture = h.path().join("build.log");
    let mut big = String::new();
    for i in 0..4_000 {
        big.push_str(&format!(
            "2026-08-08T10:{:02}:{:02}Z INFO worker[{}] handled path=/api/v1/items status=200 dur={}ms\n",
            i / 60 % 60,
            i % 60,
            i % 8,
            10 + i % 40
        ));
    }
    std::fs::write(&fixture, &big).unwrap();
    let cmd = format!("cat {}", fixture.to_str().unwrap());

    let (folded, _err, code) = run(&["wrap", &cmd], h.path());
    assert_eq!(code, 0);
    assert!(
        folded.len() < big.len() / 2,
        "the shim channel did not fold, so this test would prove nothing about the second one"
    );

    let ledger = h.path().join(".tokenade").join("gain.jsonl");
    let after_wrap = std::fs::read_to_string(&ledger).unwrap_or_default();
    let rows_after_wrap = after_wrap.lines().filter(|l| !l.trim().is_empty()).count();
    assert!(
        rows_after_wrap > 0,
        "the shim channel booked nothing, so there is no first credit to duplicate"
    );

    // What Claude Code hands the PostToolUse hook when the shim ran first: the
    // same command, and the output the shim already folded.
    let payload = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": { "command": cmd },
        "tool_response": { "stdout": folded, "stderr": "" }
    })
    .to_string();
    let (_out, _err, code) = run_hook(&["hookwrap"], h.path(), &payload);
    assert_eq!(code, 0);

    let after_hook = std::fs::read_to_string(&ledger).unwrap_or_default();
    let rows_after_hook = after_hook.lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(
        rows_after_hook,
        rows_after_wrap,
        "the hook booked {} extra row(s) for bytes `wrap` had already folded — \
         two channels, one payload, two credits. Ledger:\n{after_hook}",
        rows_after_hook - rows_after_wrap
    );

    // The half above passes with the guard REMOVED, and saying so is the point:
    // today's compactors leave nothing for a second pass to shave off `wrap`'s
    // output, so the realistic stack is safe by arithmetic, not by rule. It is
    // kept because that arithmetic is a property of the current compactors and
    // nothing pins it.
    //
    // This half pins the RULE. Same bytes the hook demonstrably folds and
    // credits, with a fold banner in front — the shape measured 2026-08-08,
    // where the hook booked 381 tokens for a payload another authority had
    // already claimed. Remove the `already_folded` check in `cmd_hookwrap` and
    // this assertion goes red; that is the whole reason it exists.
    let banner = "[tokenade] folded 40000 -> 900 bytes \
                  (recover: tokenade expand-ref deadbeefcafe)\n";
    // A DIFFERENT body than the one `wrap` just processed. Reusing it made the
    // control arm fail for an unrelated reason: an identical (before, after)
    // event is already credited only once, so the hook booked nothing and the
    // arm proved nothing about markers.
    let other: String = big
        .replace("worker[", "runner[")
        .replace("/api/v1/", "/api/v2/");
    for (label, stdout, expect_extra) in [
        ("unmarked", other.clone(), true),
        ("marked", format!("{banner}{other}"), false),
    ] {
        let before = std::fs::read_to_string(&ledger).unwrap_or_default();
        let before_rows = before.lines().filter(|l| !l.trim().is_empty()).count();
        let payload = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": { "command": format!("{cmd} # {label}") },
            "tool_response": { "stdout": stdout, "stderr": "" }
        })
        .to_string();
        let (_o, _e, code) = run_hook(&["hookwrap"], h.path(), &payload);
        assert_eq!(code, 0);
        let after = std::fs::read_to_string(&ledger).unwrap_or_default();
        let added = after.lines().filter(|l| !l.trim().is_empty()).count() - before_rows;
        if expect_extra {
            assert!(
                added > 0,
                "control arm: the hook must fold and credit the UNMARKED bytes, \
                 otherwise the marked arm proves nothing"
            );
        } else {
            assert_eq!(
                added, 0,
                "the hook credited {added} row(s) for bytes carrying a fold banner — \
                 one payload, two authorities. Ledger:\n{after}"
            );
        }
    }
}

/// The uncredited-fold twin, on the command the six plugin bridges actually call.
///
/// `cmd_wrap` was taught to credit the catch-all fold; `cmd_auto` — which is
/// what `tokenade read -` runs, and what every bridge invokes — has the same
/// shape and was not. Measured 2026-08-08 on a fresh HOME: 841 232 bytes of
/// unrecognised text came out at 4 843 (183 397 tokens down to ~1 200) and
/// wrote ZERO ledger rows. Worse than the `wrap` case, which at least booked
/// the compactor's own row: here a bridge folds a large tool result 40x and
/// the dashboard shows nothing.
///
/// The fixture has to clear `read_threshold_tokens` (25 000 by default — a
/// DELIBERATE read gets a higher bar than an incidental one), and it has to
/// resist the generic pass, so the same heterogeneous prose as its sibling.
#[cfg(unix)]
#[test]
fn e2e_read_credits_the_catch_all_fold() {
    let h = TempHome::new("read-stash-credit");
    const WORDS: [&str; 16] = [
        "harbour",
        "lantern",
        "quartz",
        "meridian",
        "thicket",
        "ballast",
        "cobalt",
        "furrow",
        "gantry",
        "kestrel",
        "lodestone",
        "nimbus",
        "parapet",
        "sextant",
        "tundra",
        "verdigris",
    ];
    const SEPARATORS: [&str; 4] = [" ", ", ", " - ", " / "];
    let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut big = String::new();
    for i in 0..6_000 {
        let sep = SEPARATORS[(next() % 4) as usize];
        let words = 3 + next() % 22;
        big.push_str(&format!("{i:05}"));
        for _ in 0..words {
            big.push_str(sep);
            big.push_str(WORDS[(next() % 16) as usize]);
        }
        big.push_str(sep);
        big.push_str(&format!("{}\n", next() % 1_000_000_000));
    }

    let (out, _err, code) = run_with_stdin(&["read", "-"], h.path(), &big);
    assert_eq!(code, 0);
    assert!(
        out.len() * 10 < big.len(),
        "the catch-all fold did not fire, so there is no credit to check \
         ({} -> {})",
        big.len(),
        out.len()
    );

    let log =
        std::fs::read_to_string(h.path().join(".tokenade").join("gain.jsonl")).unwrap_or_default();
    let rows: Vec<serde_json::Value> = log
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("valid JSON record"))
        .collect();
    assert!(
        rows.iter().any(|r| r["label"] == "disclose:stash"),
        "a 40x fold through the command every plugin bridge calls wrote no \
         `disclose:stash` row. Ledger:\n{log}"
    );
    // No double credit: the rows tile, so what they claim together can never
    // exceed the whole interval they span.
    let claimed: u64 = rows
        .iter()
        .map(|r| {
            let b = r["before"].as_u64().unwrap_or(0);
            b - r["after"].as_u64().unwrap_or(0).min(b)
        })
        .sum();
    let widest = rows
        .iter()
        .map(|r| r["before"].as_u64().unwrap_or(0))
        .max()
        .unwrap_or(0)
        - rows
            .iter()
            .map(|r| r["after"].as_u64().unwrap_or(0))
            .min()
            .unwrap_or(0);
    assert!(
        claimed <= widest,
        "rows claim {claimed} tokens across an interval only {widest} wide:\n{log}"
    );
}

/// Third site of the same hole: `tokenade snapshot`.
///
/// Its own source comment already quantified what went uncredited — the a11y
/// compactor takes 12 716 670 bytes to 11 816 668, and the stash then takes
/// the rest down to a pointer. Only the first step was booked. Measured
/// 2026-08-08 on a 2 MB tree: `a11y-compact` credited 85 642 tokens of the
/// 680 739 actually saved — **12.6 % of the delivery**.
#[cfg(unix)]
#[test]
fn e2e_snapshot_credits_the_catch_all_fold() {
    let h = TempHome::new("snapshot-stash-credit");
    let roles = [
        "button", "link", "textbox", "heading", "listitem", "generic",
    ];
    let mut tree = String::new();
    for i in 0..40_000usize {
        tree.push_str(&"  ".repeat(i % 6));
        tree.push_str(&format!(
            "- {} \"Item {i} label text\" [ref=e{i}]\n",
            roles[i % 6]
        ));
    }
    let (out, _err, code) = run_with_stdin(&["snapshot", "-"], h.path(), &tree);
    assert_eq!(code, 0);
    assert!(
        out.len() * 100 < tree.len(),
        "the stash did not bound the tree ({} -> {})",
        tree.len(),
        out.len()
    );

    let log =
        std::fs::read_to_string(h.path().join(".tokenade").join("gain.jsonl")).unwrap_or_default();
    let rows: Vec<serde_json::Value> = log
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("valid JSON record"))
        .collect();
    assert!(
        rows.iter().any(|r| r["label"] == "disclose:stash"),
        "the a11y path booked its compactor and not its stash — the row credits \
         a fraction of what the agent stopped paying. Ledger:\n{log}"
    );
    // The two rows must tile: a11y spans raw -> compacted, the stash spans
    // compacted -> delivered. Together they may not claim more than the whole.
    let claimed: u64 = rows
        .iter()
        .map(|r| {
            let b = r["before"].as_u64().unwrap_or(0);
            b - r["after"].as_u64().unwrap_or(0).min(b)
        })
        .sum();
    let widest = rows
        .iter()
        .map(|r| r["before"].as_u64().unwrap_or(0))
        .max()
        .unwrap_or(0)
        - rows
            .iter()
            .map(|r| r["after"].as_u64().unwrap_or(0))
            .min()
            .unwrap_or(0);
    assert!(
        claimed <= widest,
        "rows claim {claimed} tokens across an interval only {widest} wide:\n{log}"
    );
}

/// Rank 2 of the ladder — the universal shell proxy — SOURCED INTO A REAL BASH.
///
/// The three fragment tests that existed assert on the fragment's TEXT:
/// `posix.contains("[ -t 0 ]")`, `contains("BASH_SUBSHELL")`,
/// `contains("-f /dev/stdout")`. Text cannot show that a guard fires. This is
/// the same shape on the channel that reaches the MOST agents: every agent that
/// runs shell commands, and the whole of what `detect` calls "the universal
/// shell proxy" (D8).
///
/// The three passthrough guards are the point. Each protects a consumer that
/// needs RAW bytes, and a guard that silently stopped working would feed
/// compacted text to a script, a prompt, or a redirect — corrupting data rather
/// than merely wasting tokens.
///
/// A stub `tokenade` earlier on PATH observes the decision: the proxy invokes
/// `command tokenade wrap "…"`, so the stub's marker appears exactly when the
/// proxy chose to wrap.
#[cfg(unix)]
#[test]
fn the_shell_proxy_wraps_and_its_passthrough_guards_fire_in_a_real_bash() {
    if !std::process::Command::new("bash")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        eprintln!("SKIP: no bash — rank 2 is untested here");
        return;
    }
    let h = TempHome::new("shell-proxy");
    let bin = h.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    // The stub the proxy will reach for. Marker only — this test is about the
    // DECISION to wrap, not about what wrapping produces.
    std::fs::write(bin.join("tokenade"), "#!/bin/sh\necho \"WRAPPED:$*\"\n").unwrap();
    // `git` is in the alias set the fragment generates; this stub is what a
    // passthrough must reach instead.
    std::fs::write(bin.join("git"), "#!/bin/sh\necho RAW-GIT\n").unwrap();
    for f in ["tokenade", "git"] {
        let p = bin.join(f);
        let mut perm = std::fs::metadata(&p).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o755);
        std::fs::set_permissions(&p, perm).unwrap();
    }

    // Generate the fragment with the REAL binary (absolute path), before the
    // stub shadows it.
    let frag = {
        let out = std::process::Command::new(binary())
            .args(["shell-init", "--shell", "bash"])
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    bin.display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .output()
            .expect("shell-init");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    assert!(frag.contains("__tokenade_proxy"), "no fragment generated");

    let run = |script: &str| -> String {
        let out = std::process::Command::new("bash")
            .arg("-c")
            .arg(format!("{frag}\n{script}"))
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env("HOME", h.path())
            .env_remove("TOKENADE_WRAPPING")
            .env_remove("TOKENADE_SHELL_PROXY")
            .current_dir(h.path())
            .output()
            .expect("bash");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };

    // 1. The ordinary case: stdin is not a TTY, so the proxy wraps.
    let plain = run("git status");
    assert!(
        plain.contains("WRAPPED:"),
        "the proxy did not wrap a plain command — rank 2 is not intercepting: {plain:?}"
    );

    // 2. Command substitution: a prompt or a script is CAPTURING this output
    //    and needs it raw. `BASH_SUBSHELL` is how bash reveals the stage.
    let substituted = run("x=$(git status); printf '%s' \"$x\"");
    assert_eq!(
        substituted.trim(),
        "RAW-GIT",
        "a command substitution was compacted — the caller parses these bytes: {substituted:?}"
    );

    // 3. Redirect to a regular file: same contract, different consumer.
    let redirected = run("git status > out.txt; cat out.txt");
    assert_eq!(
        redirected.trim(),
        "RAW-GIT",
        "a redirect to a file was compacted — the file is the consumer: {redirected:?}"
    );

    // 4. The documented opt-out must actually opt out.
    let opted_out = run("TOKENADE_SHELL_PROXY=0; export TOKENADE_SHELL_PROXY; git status");
    assert_eq!(
        opted_out.trim(),
        "RAW-GIT",
        "TOKENADE_SHELL_PROXY=0 did not disable the proxy: {opted_out:?}"
    );
}

/// The same rank-2 drive, on zsh and fish.
///
/// Not a copy of the bash test for completeness — the two shells diverge, and
/// one of the divergences was a live data-corruption bug. Fish's alias
/// is a FUNCTION, and a redirect applied to a function call is invisible to
/// fish's BUILTIN `test`: `f > out.txt` answers "not a file" inside `f`, where
/// bash answers "a file". The redirect guard therefore never fired and
/// `cmd > file` was compacted INTO the file.
///
/// Two harness facts, both learned the hard way:
///
///  * `zsh -f` / `fish --no-config`. Both shells read startup files even
///    non-interactively, and this machine's prepend `~/.cargo/bin` — which
///    shadows the stub `tokenade` with the real one, so every case looked like
///    a passthrough. `bash -c` reads nothing, which is why it was unaffected.
///  * a SCRIPT FILE, not `-c`. Aliases are expanded at parse time, and a `-c`
///    string is parsed before the fragment that defines them has run.
#[cfg(unix)]
#[test]
fn the_shell_proxy_guards_fire_in_zsh_and_fish_too() {
    let h = TempHome::new("shell-proxy-zf");
    let bin = h.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("tokenade"), "#!/bin/sh\necho \"WRAPPED:$*\"\n").unwrap();
    std::fs::write(bin.join("git"), "#!/bin/sh\necho RAW-GIT\n").unwrap();
    for f in ["tokenade", "git"] {
        let p = bin.join(f);
        let mut perm = std::fs::metadata(&p).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o755);
        std::fs::set_permissions(&p, perm).unwrap();
    }

    // (shell, rc-skipping flag, fragment name, plain, substitution, redirect)
    let cases: [(&str, &str, &str, &str, &str, &str); 2] = [
        (
            "zsh",
            "-f",
            "zsh",
            "git status",
            "x=$(git status); printf '%s' \"$x\"",
            "git status > o.txt; cat o.txt",
        ),
        (
            "fish",
            "--no-config",
            "fish",
            "git status",
            "set x (git status)\nprintf '%s' $x",
            "git status > o.txt\ncat o.txt",
        ),
    ];

    for (shell, norc, frag_name, plain, subst, redir) in cases {
        if !std::process::Command::new(shell)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            eprintln!("SKIP: no {shell} — rank 2 is untested there");
            continue;
        }
        let frag = {
            let out = std::process::Command::new(binary())
                .args(["shell-init", "--shell", frag_name])
                .output()
                .expect("shell-init");
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        let frag_path = h.path().join(format!("frag.{frag_name}"));
        std::fs::write(&frag_path, &frag).unwrap();

        let run = |script: &str| -> String {
            let sp = h.path().join(format!("s.{frag_name}"));
            std::fs::write(
                &sp,
                format!("source {}\n{script}\n", frag_path.to_string_lossy()),
            )
            .unwrap();
            let out = std::process::Command::new(shell)
                .arg(norc)
                .arg(&sp)
                .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
                .env("HOME", h.path())
                .env_remove("TOKENADE_WRAPPING")
                .env_remove("TOKENADE_SHELL_PROXY")
                .current_dir(h.path())
                .output()
                .expect("spawn shell");
            String::from_utf8_lossy(&out.stdout).into_owned()
        };

        // The positive control FIRST: without it the two passthrough cases pass
        // on a proxy that never intercepts anything.
        let got = run(plain);
        assert!(
            got.contains("WRAPPED:"),
            "{shell}: the proxy did not wrap a plain command: {got:?}"
        );
        assert_eq!(
            run(subst).trim(),
            "RAW-GIT",
            "{shell}: a command substitution was compacted — the caller parses these bytes"
        );
        assert_eq!(
            run(redir).trim(),
            "RAW-GIT",
            "{shell}: output redirected to a FILE was compacted — the file now holds folded \
             text instead of the bytes its consumer parses"
        );
    }
}

/// Rank 2 under dash — where the substitution guard CANNOT fire, on purpose.
///
/// The POSIX fragment detects a command-substitution stage through
/// `BASH_SUBSHELL` / `ZSH_SUBSHELL`. Plain `sh`/`dash` expose neither, so a
/// substitution IS wrapped there. That is a known limitation, and the
/// shell-init design rests on it — which is why it is pinned here rather
/// than left as a sentence: measured 2026-08-08, `x=$(git status)` under dash
/// returns COMPACTED text, and `shell-init --help` recommends `~/.profile` for
/// servers, where `/bin/sh` is dash on Debian-family systems.
///
/// If someone later teaches the fragment to detect substitution under POSIX sh,
/// this test fails and says so — the right outcome, since the exclusion list D3
/// argues about would then be re-decidable.
///
/// `.` and not `source`: `source` is a bashism, and dash silently loads
/// nothing, which made the first run of this probe report "no wrapping
/// anywhere".
#[cfg(unix)]
#[test]
fn under_dash_a_substitution_is_wrapped_and_a_redirect_is_not() {
    if !std::process::Command::new("dash")
        .arg("-c")
        .arg("exit 0")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        eprintln!("SKIP: no dash — the POSIX-only path is untested here");
        return;
    }
    let h = TempHome::new("shell-proxy-dash");
    let bin = h.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("tokenade"), "#!/bin/sh\necho \"WRAPPED:$*\"\n").unwrap();
    std::fs::write(bin.join("git"), "#!/bin/sh\necho RAW-GIT\n").unwrap();
    for f in ["tokenade", "git"] {
        let p = bin.join(f);
        let mut perm = std::fs::metadata(&p).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o755);
        std::fs::set_permissions(&p, perm).unwrap();
    }
    let frag = String::from_utf8_lossy(
        &std::process::Command::new(binary())
            .args(["shell-init", "--shell", "sh"])
            .output()
            .expect("shell-init")
            .stdout,
    )
    .into_owned();
    let frag_path = h.path().join("frag.sh");
    std::fs::write(&frag_path, &frag).unwrap();

    let run = |script: &str| -> String {
        let sp = h.path().join("s.sh");
        std::fs::write(
            &sp,
            format!(". {}\n{script}\n", frag_path.to_string_lossy()),
        )
        .unwrap();
        let out = std::process::Command::new("dash")
            .arg(&sp)
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env("HOME", h.path())
            .env_remove("TOKENADE_WRAPPING")
            .env_remove("TOKENADE_SHELL_PROXY")
            .current_dir(h.path())
            .output()
            .expect("spawn dash");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };

    assert!(
        run("git status").contains("WRAPPED:"),
        "dash: the proxy did not wrap a plain command — the control does not hold"
    );
    assert!(
        run("x=$(git status); printf '%s' \"$x\"").contains("WRAPPED:"),
        "a substitution is no longer wrapped under dash — the POSIX fragment gained a \
         subshell test, so D3's exclusion list can be revisited"
    );
    assert_eq!(
        run("git status > o.txt; cat o.txt").trim(),
        "RAW-GIT",
        "dash: a redirect to a file was compacted — /dev/stdout is the guard that works here"
    );
}

/// `tokenade shellwrap` — the ENTIRE guard logic for csh/tcsh and cmd.exe.
///
/// Those two shells cannot define functions (csh has no function construct;
/// doskey macros carry no logic), so their fragments alias every command to
/// `tokenade shellwrap <cmd>` and the guards the other shells inline are
/// applied in Rust instead. Which means: for two of the eight shells the
/// product advertises, this subcommand IS the channel.
///
/// It had no behavioural test — it appeared in this file only as a name in a
/// list. Same shape as before: the code was read, never driven.
///
/// The redirect guard is the one worth having here. `shellwrap` is a separate
/// PROCESS, so `metadata("/dev/stdout")` sees the inherited fd and can tell a
/// regular file from a pipe — the distinction fish's builtin `test` could not
/// make from inside a function, and the reason that bug could not exist
/// on this path.
#[cfg(unix)]
#[test]
fn shellwrap_wraps_for_a_pipe_and_passes_through_for_a_file_or_opt_out() {
    let h = TempHome::new("shellwrap");
    let noisy = ["printf", "%s\\n", &"x".repeat(40)];
    // A payload big enough that compaction is visible by SIZE alone, so the
    // assertion does not depend on any banner wording.
    let script = h.path().join("noisy.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\ni=0\nwhile [ $i -lt 400 ]; do echo \"2026-08-08 INFO worker handled request $i status=200\"; i=$((i+1)); done\n",
    )
    .unwrap();
    let mut perm = std::fs::metadata(&script).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o755);
    std::fs::set_permissions(&script, perm).unwrap();
    let _ = noisy;

    let raw_len = std::fs::read_to_string(&script)
        .map(|_| {
            let out = std::process::Command::new(&script)
                .output()
                .expect("run noisy");
            out.stdout.len()
        })
        .unwrap();
    assert!(raw_len > 10_000, "the fixture is too small to show a fold");

    // 1. stdout is a PIPE: the agent is the consumer, so compact. Control first
    //    — without it the two passthrough cases pass on a shellwrap that never
    //    compacts anything.
    let piped = {
        let mut c = std::process::Command::new(binary());
        c.args(["shellwrap", &script.to_string_lossy()]);
        let out = e2e_env(&mut c, h.path())
            .env_remove("TOKENADE_WRAPPING")
            .env_remove("TOKENADE_SHELL_PROXY")
            .output()
            .expect("shellwrap piped");
        out.stdout.len()
    };
    assert!(
        piped * 3 < raw_len,
        "shellwrap did not compact into a pipe: {piped} vs {raw_len} raw"
    );

    // 2. stdout is a regular FILE: the file's consumer needs raw bytes.
    let target = h.path().join("out.txt");
    {
        let f = std::fs::File::create(&target).unwrap();
        let mut c = std::process::Command::new(binary());
        c.args(["shellwrap", &script.to_string_lossy()]);
        e2e_env(&mut c, h.path())
            .env_remove("TOKENADE_WRAPPING")
            .env_remove("TOKENADE_SHELL_PROXY")
            .stdout(std::process::Stdio::from(f))
            .output()
            .expect("shellwrap to file");
    }
    let file_len = std::fs::metadata(&target).unwrap().len() as usize;
    assert_eq!(
        file_len, raw_len,
        "shellwrap compacted output redirected to a FILE — the file's consumer parses \
         these bytes (the csh/cmd twin of the fish redirect bug)"
    );

    // 3. The documented opt-out.
    let opted = {
        let mut c = std::process::Command::new(binary());
        c.args(["shellwrap", &script.to_string_lossy()]);
        let out = e2e_env(&mut c, h.path())
            .env_remove("TOKENADE_WRAPPING")
            .env("TOKENADE_SHELL_PROXY", "0")
            .output()
            .expect("shellwrap opt-out");
        out.stdout.len()
    };
    assert_eq!(
        opted, raw_len,
        "TOKENADE_SHELL_PROXY=0 did not disable shellwrap"
    );
}

/// Killing the wrapper must kill the WORK, on both routes.
///
/// `wrap` sits between the agent and the real command on ranks 1 and 2, so
/// every cancelled or timed-out command passes through it. It spawned the
/// child and blocked in `wait()` with no handler: killing `wrap` left the real
/// command running with nobody reading its output. Measured 2026-08-08 —
/// `sleep` survived on the Direct route, and on the Shell route the `sh` was
/// never signalled at all, so the whole pipeline outlived the wrapper. An
/// orphaned build keeps holding its lock; that is not hypothetical, a stale
/// gate held the cargo lock here and the stall read as slow tests.
///
/// The two routes are both exercised because the Shell one is where the victim
/// is a GRANDCHILD — reaching it is the entire reason the child gets its own
/// process group instead of being signalled directly.
#[cfg(unix)]
#[test]
fn e2e_killing_wrap_kills_the_work_it_wrapped() {
    // Distinctive durations so the probe finds OUR processes and nothing else.
    // Matching on `comm` + a literal argument, never on a pattern this test's
    // own command line contains: `pgrep -f` self-matching has cost this
    // project three self-kills and three multi-hour hangs.
    // Every command that runs user work of unbounded duration, not just the
    // one where the defect was found: `wrap` on both routes, `exec`'s sandbox,
    // `raw`, and the MCP proxy at rank 3. A fix that reaches one launcher and
    // not its neighbour is the shape this journal keeps recording.
    let cases: [(&str, Vec<&str>, &str); 5] = [
        ("wrap-direct", vec!["wrap", "sleep 3771"], "3771"),
        ("wrap-shell", vec!["wrap", "sleep 3772 | cat"], "3772"),
        (
            "exec",
            vec!["exec", "--lang", "sh", "--script", "sleep 3773"],
            "3773",
        ),
        ("raw", vec!["raw", "sleep 3774"], "3774"),
        (
            "mcp-proxy",
            vec!["mcp-proxy", "--", "sleep", "3775"],
            "3775",
        ),
    ];
    for (tag, cmd_args, needle) in cases {
        let h = TempHome::new(&format!("wrap-signal-{tag}"));
        // Unique per run — see the note in the SIGKILL sibling below.
        let needle = &format!("{needle}.{}", std::process::id());
        let cmd_args: Vec<String> = cmd_args
            .iter()
            .map(|a| a.replace(&needle[..4], needle))
            .collect();
        let mut wrapper = Command::new(binary())
            .args(&cmd_args)
            // stdin PIPED and deliberately never closed. Inheriting it made
            // this test pass or hang depending on what the harness handed it,
            // and an OPEN stdin is what exposed the real defect: `mcp-proxy`
            // joins its stdin pump before waiting on the child, so a relay that
            // did not re-raise left it unkillable.
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("HOME", h.path())
            .env("USERPROFILE", h.path())
            .env("TOKENADE_HOOK_DISABLED", "1")
            .current_dir(h.path())
            .spawn()
            .expect("spawn wrap");

        let alive = |needle: &str| -> bool {
            let out = Command::new("ps")
                .args(["-eo", "comm,args"])
                .output()
                .expect("ps");
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .any(|l| l.starts_with("sleep") && l.contains(needle))
        };
        // Anti-vacuity: prove the work EXISTS before proving it dies. Without
        // this the test passes just as well when nothing was ever spawned.
        // A fixed 100 x 50ms is five seconds of budget measured on an idle
        // machine, and this test runs inside a suite of 8800 in parallel. Its
        // SIGKILL sibling failed that way on 2026-09-03 and passed alone twice
        // — a guard that is unreliable under load is not a guard the day it
        // matters. Deadline, not iteration count, and long enough to cover the
        // slowest machine rather than the one that wrote it; nothing waits the
        // whole window when the product works.
        let mut started = false;
        let start_deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while std::time::Instant::now() < start_deadline {
            if alive(needle) {
                started = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(
            started,
            "[{tag}] the wrapped work never started within 30s — that is this \
             test's own window running out, not a fact about the product"
        );

        let status = Command::new("kill")
            .arg(wrapper.id().to_string())
            .status()
            .expect("kill");
        assert!(status.success(), "[{tag}] could not signal the wrapper");
        // BOUNDED. `wait()` here hung the whole gate for 12 minutes when
        // `mcp-proxy` stopped dying on SIGTERM: the suite gave no failure, no
        // name, just silence. A test that hangs is worse than one that fails.
        let mut exited = false;
        for _ in 0..100 {
            if matches!(wrapper.try_wait(), Ok(Some(_))) {
                exited = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        if !exited {
            let _ = wrapper.kill();
            let _ = wrapper.wait();
        }
        assert!(
            exited,
            "[{tag}] the wrapper itself survived the signal it was sent — \
             it relays the signal but never dies, so nothing can stop it"
        );

        let mut reaped = false;
        let reap_deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while std::time::Instant::now() < reap_deadline {
            if !alive(needle) {
                reaped = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        if !reaped {
            // Never leave the probe's own orphan behind on failure.
            // By pid, from the same comm+literal match: `pkill -x sleep` would
            // reap every `sleep` on the machine, including the harness's own.
            if let Ok(out) = Command::new("ps").args(["-eo", "pid,comm,args"]).output() {
                for pid in String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .filter(|l| l.contains(" sleep ") && l.contains(needle))
                    .filter_map(|l| l.split_whitespace().next())
                {
                    let _ = Command::new("kill").arg(pid).status();
                }
            }
        }
        assert!(
            reaped,
            "[{tag}] the work (sleep {needle}) outlived the wrapper that was \
             killed: cancelling a command left it running"
        );
    }
}

/// The kill the relay can NEVER catch, on both shapes a harness uses.
///
/// Its sibling above covers SIGTERM, which is catchable by definition. An agent
/// that times a command out sends `SIGKILL`, so the likeliest case in the field
/// was the one the relay could not reach — and the measurement that motivated
/// this test found the fix for SIGTERM had made SIGKILL WORSE on one of them.
///
/// Measured 2026-08-08 against the shipped 1.1.6 and against HEAD, non-tty stdin:
///
/// | SIGKILL aimed at | route  | 1.1.6    | after the own-group change |
/// |------------------|--------|----------|----------------------------|
/// | the wrapper pid  | direct | survives | dies (PDEATHSIG)           |
/// | the wrapper pid  | shell  | survives | survives                   |
/// | the wrapper group| direct | dies     | dies                       |
/// | the wrapper group| shell  | **dies** | **survives**               |
///
/// The last row is the regression: putting the child in its own process group
/// moved the pipeline OUT of the group the harness aims at, so work that used
/// to die with its group started outliving it. Taking a process group away
/// from the terminal means owning its death — including deaths where no code
/// of ours ever runs. Both columns are pinned here so neither can come back.
#[cfg(unix)]
#[test]
fn e2e_sigkilling_wrap_kills_the_work_even_though_nothing_can_catch_it() {
    use std::os::unix::process::CommandExt;

    // Known gap: on GitHub's hosted runners `wrapper.wait()` never returns after
    // the SIGKILL (passes locally under bash and dash). Skipped there until it
    // is reproduced; it still runs everywhere else.
    if std::env::var_os("GITHUB_ACTIONS").is_some() {
        eprintln!("skipped on GitHub Actions runners (known gap, see comment)");
        return;
    }

    // (tag, shell text with {} for the ready-marker touch, marker, kill the
    //  GROUP rather than the pid, run through `exec` rather than `wrap`)
    let cases: [(&str, &str, &str, bool, bool); 6] = [
        ("pid/direct", "touch {}; sleep 3781", "3781", false, false),
        (
            "pid/pipe",
            "touch {}; sleep 3782 | cat",
            "3782",
            false,
            false,
        ),
        (
            "pid/seq",
            "touch {}; sleep 3783; true",
            "3783",
            false,
            false,
        ),
        ("group/direct", "touch {}; sleep 3784", "3784", true, false),
        (
            "group/pipe",
            "touch {}; sleep 3785 | cat",
            "3785",
            true,
            false,
        ),
        (
            "pid/exec",
            "touch {}; sleep 3786 | cat",
            "3786",
            false,
            true,
        ),
    ];

    let alive = |needle: &str| -> bool {
        // comm + a literal argument, never a pattern this file contains: a probe
        // that matches the NAME of the command finds the process writing about
        // the defect instead of the defect. That cost three false alarms here.
        let out = Command::new("ps")
            .args(["-eo", "comm,args"])
            .output()
            .expect("ps");
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .any(|l| l.starts_with("sleep") && l.contains(needle))
    };

    for (tag, script, needle, kill_group, via_exec) in cases {
        let h = TempHome::new(&format!("wrap-sigkill-{}", needle));
        // The work announces itself by creating a file, so the wait for it to
        // START costs no fork at all. `ps` stays for the question only it can
        // answer — is the work still there after the kill.
        // The needle has to be unique per RUN, not a constant every run on the
        // machine shares. `sleep` takes a fractional argument, so the pid rides
        // in the duration and `ps` still shows it. Measured 2026-09-03: a run
        // that left a stray `sleep 3781` behind made the NEXT run fail at the
        // reap step — one process failing another, indistinguishable from the
        // defect these tests exist for, and two `cargo nextest` runs at once
        // would collide the same way.
        let needle = &format!("{needle}.{}", std::process::id());
        // A DECOY carrying the old shared spelling, alive for the whole case.
        // Contamination between runs is not reproducible by waiting for it, so
        // plant it: a needle that is a machine-wide constant matches this
        // process and the reap check never sees it die. With the pid in the
        // needle it is invisible, and the test fails deterministically on the
        // implementation that would have been flaky.
        let mut decoy = Command::new("sleep")
            .arg(format!("{}.0", &needle[..4]))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn decoy");
        let ready = h.path().join(format!("started-{needle}"));
        let script = script.replace("{}", &ready.to_string_lossy()).replace(
            &format!("sleep {}", &needle[..4]),
            &format!("sleep {needle}"),
        );
        let cmd_args: Vec<&str> = if via_exec {
            vec!["exec", "--lang", "sh", "--script", &script]
        } else {
            vec!["wrap", &script]
        };
        let mut cmd = Command::new(binary());
        cmd.args(&cmd_args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("HOME", h.path())
            .env("USERPROFILE", h.path())
            .env("TOKENADE_HOOK_DISABLED", "1")
            .current_dir(h.path());
        // Its own group ALWAYS, not only for the group cases: `kill -9 -<pgid>`
        // aimed at the group cargo gave us would take out the test runner.
        cmd.process_group(0);
        let mut wrapper = cmd.spawn().expect("spawn wrapper");
        let wpid = wrapper.id() as i32;

        let start_deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !ready.exists() && std::time::Instant::now() < start_deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        if !ready.exists() {
            // Say WHY it never started. This assert has fired three times under
            // full-suite load (2026-09-03, 09-04 twice) and passed alone in
            // 0.25 s every time, and the message named only the window. The two
            // explanations are not the same bug: a wrapper still RUNNING at the
            // deadline is a stall, one that has EXITED was killed or refused,
            // and only its own status and stderr tell them apart. Measured
            // before adding this: `wrap` startup is 42 ms with a fresh HOME at
            // rest and 81 ms median / 149 ms max under the suite, so a 30 s
            // window is not tight.
            let status = wrapper.try_wait().ok().flatten();
            let mut err = String::new();
            if status.is_some() {
                if let Some(mut e) = wrapper.stderr.take() {
                    use std::io::Read;
                    let _ = e.read_to_string(&mut err);
                }
            }
            let _ = decoy.kill();
            panic!(
                "[{tag}] the wrapped work never started within 30s — wrapper {}; stderr: {:?}",
                match status {
                    Some(st) => format!("EXITED with {st}"),
                    None => "is STILL RUNNING (a stall, not a refusal)".to_string(),
                },
                err.chars().take(400).collect::<String>()
            );
        }
        // …and it is the WORK we are about to kill, not just its announcement.
        let mut started = false;
        while std::time::Instant::now() < start_deadline {
            if alive(needle) {
                started = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(
            started,
            "[{tag}] the marker landed but `sleep {needle}` never appeared"
        );

        // SIGKILL: no handler of ours runs, by construction.
        let target = if kill_group {
            format!("-{wpid}")
        } else {
            wpid.to_string()
        };
        let st = Command::new("kill")
            .args(["-9", &target])
            .status()
            .expect("kill -9");
        assert!(st.success(), "[{tag}] could not SIGKILL {target}");
        let _ = wrapper.wait();

        let mut reaped = false;
        let reap_deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while std::time::Instant::now() < reap_deadline {
            if !alive(needle) {
                reaped = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        if !reaped {
            if let Ok(out) = Command::new("ps").args(["-eo", "pid,comm,args"]).output() {
                for pid in String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .filter(|l| l.contains(" sleep ") && l.contains(needle))
                    .filter_map(|l| l.split_whitespace().next())
                {
                    let _ = Command::new("kill").args(["-9", pid]).status();
                }
            }
        }
        let _ = decoy.kill();
        let _ = decoy.wait();
        assert!(
            reaped,
            "[{tag}] the work (sleep {needle}) outlived a SIGKILL of the \
             wrapper: nothing of ours runs on SIGKILL, so the guarantee has to \
             be held by a process that OUTLIVES us, not by a signal handler"
        );
    }
}

/// Drive `tokenade read -` with an explicit session (or none), the way a plugin
/// bridge does. Mirrors `run_with_stdin`; the one difference is the variable
/// this whole feature turns on.
fn read_with_session(home: &Path, session: Option<&str>, payload: &str) -> String {
    use std::io::Write;
    let bin = binary();
    let mut c = Command::new(&bin);
    c.args(["read", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let c = e2e_env(&mut c, home);
    match session {
        Some(s) => c.env("TOKENADE_SESSION", s),
        None => c.env_remove("TOKENADE_SESSION"),
    };
    let mut child = c.spawn().expect("spawn tokenade");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// `tokenade read` is the command every plugin BRIDGE calls, and it was the one
/// delivery path in the product that never spoke to the dedup layer: measured
/// on the compat bench 2026-08-18 across all three OS, the same bytes through
/// `tokenade read -` twice came back the same size on eighteen of twenty-four
/// agents. The layer was not missing — its PRECONDITION was, because nothing on
/// the bridge path ever supplied a session.
///
/// The rule is `wrap`'s, and this test is here to keep it one rule: fold a
/// repeat to a §ref only when the session that is asking provably received
/// those bytes. All three arms matter and the negative ones matter most — a
/// dedup that folds without proof hands the model a pointer into a context it
/// never had.
#[test]
fn e2e_read_folds_a_repeat_only_when_a_session_vouches_for_it() {
    let h = TempHome::new("read-dedup-session");
    let home = h.path();
    // Log-shaped, because that is what a bridge folds: a tool result big
    // enough that the compactor has something to do.
    let payload: String = (0..600)
        .map(|i| {
            format!("2026-08-19T10:{:02}:{:02}Z INFO worker-{} handled request {i} in {}ms status=200\n",
                i / 60, i % 60, i % 7, 100 + (i * 37) % 800)
        })
        .collect();

    // 1. No session: never folds. The binary cannot claim "you already have
    //    this" about a context it was told nothing about.
    let anon_first = read_with_session(home, None, &payload);
    let anon_again = read_with_session(home, None, &payload);
    assert_eq!(
        anon_first.len(),
        anon_again.len(),
        "a sessionless repeat folded — that is a claim about a context the \
         binary knows nothing about"
    );
    assert!(
        !anon_again.contains("§ref:"),
        "a sessionless repeat came back as a pointer:\n{anon_again}"
    );

    // 2. With a session, the way every bridge now runs it: the repeat is a
    //    pointer, and a much smaller one.
    let first = read_with_session(home, Some("sessone"), &payload);
    let second = read_with_session(home, Some("sessone"), &payload);
    assert!(
        second.contains("§ref:"),
        "the repeat did not fold to a pointer:\n{second}"
    );
    assert!(
        second.len() * 2 < first.len(),
        "the pointer ({} bytes) did not beat the delivery ({} bytes)",
        second.len(),
        first.len()
    );

    // 3. A DIFFERENT session inherits nothing. The marker is session-keyed for
    //    exactly this reason: one agent's delivery is not another's context.
    let other = read_with_session(home, Some("sesstwo"), &payload);
    assert!(
        !other.contains("§ref:"),
        "a second session folded against the first one's delivery:\n{other}"
    );

    // 4. The pointer has to be worth something: what it names must come back.
    //    A fold whose recovery is broken is a loss wearing a saving's clothes.
    let hash = second
        .split("§ref:")
        .nth(1)
        .and_then(|s| s.split('§').next())
        .expect("the pointer names a hash")
        .to_string();
    let (recovered, _, code) = run(&["expand-ref", &hash], home);
    assert_eq!(code, 0, "expand-ref failed on the hash the fold handed out");
    assert!(
        recovered.contains("status=200"),
        "expand-ref returned nothing recognisable:\n{recovered}"
    );
}

// ── Byte fidelity: output that is not valid UTF-8 but is not BINARY either.
//
// `head -c N`, a latin-1 log, one stray byte — none of these reach the binary
// short-circuit, so they arrive at the text pipeline where `from_utf8_lossy`
// turns each bad sequence into U+FFFD. Measured 2026-08-26 before the fix:
// `head -c 2` on a file starting `h é` returned `68 ef bf bd 0a` where the
// file holds `68 c3`, with no banner and no pointer.

fn accented_fixture(home: &Path) -> std::path::PathBuf {
    let p = home.join("accents.txt");
    std::fs::write(&p, "héllo wörld ".repeat(40).as_bytes()).unwrap();
    p
}

/// A byte-count read that split a character comes back EXACTLY as the command
/// produced it. The assertion is on bytes: comparing decoded strings would
/// compare our own replacement character against itself.
#[test]
fn e2e_wrap_hands_back_the_bytes_of_a_byte_precise_read() {
    let h = TempHome::new("wrap-byte-fidelity");
    let home = h.path();
    let f = accented_fixture(home);
    let cmd = format!("head -c 2 {}", f.display());
    let (so, _se, code) = run_bytes(&["wrap", &cmd], home);
    assert_eq!(code, 0);
    assert_eq!(
        so,
        b"h\xc3".to_vec(),
        "wrap altered a byte-precise read: {so:?}"
    );
}

/// `--split-streams` is the mode that exists for `cmd > data.json`, and its
/// own help promises "redirections behave as they do without the wrapper".
/// It had no terminator at all, so it lost the newline a fold strips from a
/// view it reshaped: `echo hi` came back as `hi`, on 7 of 14 real commands,
/// stdout and stderr alike. The merged path had the opposite defect at the
/// same moment, which is why both now ask one function.
#[test]
#[cfg(unix)]
fn e2e_wrap_split_streams_closes_a_view_on_both_streams() {
    let h = TempHome::new("wrap-split-terminator");
    let home = h.path();

    let (so, se, code) = run_bytes(&["wrap", "--split-streams", "echo hi"], home);
    assert_eq!(code, 0);
    assert_eq!(
        so,
        b"hi\n".to_vec(),
        "stdout lost its final newline: {so:?}"
    );
    assert!(se.is_empty(), "nothing was written to stderr: {se:?}");

    // Both streams, since each is folded on its own.
    let (so, se, code) = run_bytes(
        &["wrap", "--split-streams", "sh -c 'echo out; echo err >&2'"],
        home,
    );
    assert_eq!(code, 0);
    assert_eq!(so, b"out\n".to_vec(), "stdout: {so:?}");
    assert_eq!(se, b"err\n".to_vec(), "stderr: {se:?}");

    // …and a stream that passed through keeps its exact bytes, here as there.
    let (so, _se, code) = run_bytes(&["wrap", "--split-streams", "printf 'no-newline'"], home);
    assert_eq!(code, 0);
    assert_eq!(
        so,
        b"no-newline".to_vec(),
        "split-streams terminated a passthrough: {so:?}"
    );
}

/// `tokenade filter` sits in a PIPE — CLAUDE.md tells agents to send a noisy
/// blob through it — and it has a branch that returns the input verbatim.
/// That branch was printing a newline of its own onto the caller's data.
#[test]
fn e2e_filter_does_not_add_a_byte_to_data_it_passed_through() {
    let h = TempHome::new("filter-passthrough-newline");
    let home = h.path();

    let (out, _e, code) = run_with_stdin_bytes(&["filter", "cat"], home, b"no-newline");
    assert_eq!(code, 0);
    assert_eq!(
        out, "no-newline",
        "filter terminated a passthrough: {out:?}"
    );

    // A view it DID fold still closes, or it runs into whatever prints next.
    let big: String = (0..400)
        .map(|i| format!("2026-01-01 INFO worker {i} ok\n"))
        .collect();
    let (out, _e, code) = run_with_stdin(&["filter", "cat"], home, &big);
    assert_eq!(code, 0);
    assert!(
        out.len() * 4 < big.len(),
        "expected a fold: {} of {}",
        out.len(),
        big.len()
    );
    assert!(out.ends_with('\n'), "a folded view lost its close: {out:?}");
}

/// A PASSTHROUGH must not acquire a byte the command never printed. The
/// terminator two lines below the print exists to close a compacted VIEW —
/// output the pipeline reshaped, which it also strips the final newline from
/// (`echo hi` reaches the print as `hi`). On output that folded to itself it
/// was appending to the command's own bytes: measured 2026-09-03,
/// `printf 'no-newline'` came back as `b"no-newline\n"` against `b"no-newline"`
/// raw, so `tokenade wrap 'cmd' > file` wrote a byte `cmd` never produced.
///
/// Both halves are the guard. Asserting only the passthrough would pass on a
/// build with no terminator at all, and that costs the newline on nearly every
/// folded view: with the block removed, 30 of 36 real commands lost theirs.
#[test]
fn e2e_wrap_terminates_a_folded_view_and_not_a_passthrough() {
    let h = TempHome::new("wrap-passthrough-newline");
    let home = h.path();

    let (so, _se, code) = run_bytes(&["wrap", "printf 'no-newline'"], home);
    assert_eq!(code, 0);
    assert_eq!(
        so,
        b"no-newline".to_vec(),
        "wrap terminated output it passed through: {so:?}"
    );

    // Nothing in, nothing out — no blank line manufactured from no output.
    let (so, _se, code) = run_bytes(&["wrap", "true"], home);
    assert_eq!(code, 0);
    assert!(so.is_empty(), "wrap spoke for a silent command: {so:?}");

    // The empty-view rule is NOT the passthrough rule wearing another hat, and
    // a fixture where both fire on the same input cannot tell them apart. Here
    // the command printed 7 bytes and the view of them is empty, so
    // `display == combined` says nothing and only `is_empty` holds the line.
    let (so, _se, code) = run_bytes(&["wrap", r"printf '   \n  \n'"], home);
    assert_eq!(code, 0);
    assert!(
        so.is_empty(),
        "wrap answered a whitespace-only output with a byte of its own: {so:?}"
    );

    // …and the reshaped view still closes. `echo hi` prints `hi\n`; the
    // pipeline hands `hi` to the print, so the terminator is what puts the
    // newline back.
    let (so, _se, code) = run_bytes(&["wrap", "echo hi"], home);
    assert_eq!(code, 0);
    assert_eq!(so, b"hi\n".to_vec(), "a folded view lost its close: {so:?}");
}

/// …and through `--split-streams`, which folds each stream on its own.
#[test]
fn e2e_wrap_split_streams_keeps_byte_precision_too() {
    let h = TempHome::new("wrap-byte-fidelity-split");
    let home = h.path();
    let f = accented_fixture(home);
    let cmd = format!("head -c 2 {}", f.display());
    let (so, _se, code) = run_bytes(&["wrap", "--split-streams", &cmd], home);
    assert_eq!(code, 0);
    assert_eq!(so, b"h\xc3".to_vec(), "split-streams altered it: {so:?}");
}

/// When the fold DOES run on bytes no encoding explains, the lossy view is
/// what the agent reads — so the substitution must be named on stderr rather
/// than passed off as the command's own output. The shape here is the one that
/// matters and the one that nearly slipped through: a LARGE, otherwise clean
/// output with a few stray bytes, which `text_encoding::decode` answers `Some`
/// for while still carrying U+FFFD.
#[test]
fn e2e_wrap_names_the_replacement_characters_it_introduced() {
    let h = TempHome::new("wrap-lossy-named");
    let home = h.path();
    let p = home.join("stray.log");
    let mut v = Vec::new();
    for i in 0..400 {
        v.extend_from_slice(format!("line {i} is perfectly ordinary text\n").as_bytes());
        if i % 150 == 0 {
            v.push(0xff);
        }
    }
    std::fs::write(&p, &v).unwrap();
    let cmd = format!("cat {}", p.display());
    let (so, se, code) = run_bytes(&["wrap", &cmd], home);
    assert_eq!(code, 0);
    assert!(
        so.len() * 4 < v.len(),
        "expected the log to fold: {} of {}",
        so.len(),
        v.len()
    );
    let se = String::from_utf8_lossy(&se);
    assert!(
        se.contains("not valid UTF-8") && se.contains('3'),
        "the substitution was not named:\n{se}"
    );
}

/// Clean UTF-8 must not acquire the advisory — a line on every command would
/// be worse than the defect it reports.
#[test]
fn e2e_wrap_says_nothing_about_clean_utf8() {
    let h = TempHome::new("wrap-clean-utf8");
    let home = h.path();
    let f = accented_fixture(home);
    let cmd = format!("cat {}", f.display());
    let (_so, se, _code) = run_bytes(&["wrap", &cmd], home);
    let se = String::from_utf8_lossy(&se);
    assert!(!se.contains("not valid UTF-8"), "spurious advisory:\n{se}");
}

/// The gain, not the guard: a latin-1 log reaches the agent as `café`, not as
/// `caf<?>`. `read` and the stdin readers have decoded since the encoding
/// module was written; command output never did, so the same `cat` answered
/// differently depending on which door it came through.
#[test]
fn e2e_wrap_decodes_a_legacy_encoding_instead_of_replacing_it() {
    let h = TempHome::new("wrap-decodes-latin1");
    let home = h.path();
    let p = home.join("latin1.log");
    let mut v = Vec::new();
    for i in 0..60 {
        v.extend_from_slice(b"caf");
        v.push(0xe9);
        v.extend_from_slice(format!(" line {i} ok\n").as_bytes());
    }
    std::fs::write(&p, &v).unwrap();
    let cmd = format!("cat {}", p.display());
    let (so, se, _code) = run_bytes(&["wrap", &cmd], home);
    let so = String::from_utf8_lossy(&so);
    assert!(so.contains("café"), "not decoded:\n{so}");
    assert!(
        !so.contains('\u{fffd}'),
        "replacement characters survived:\n{so}"
    );
    let se = String::from_utf8_lossy(&se);
    assert!(
        !se.contains("not valid UTF-8"),
        "a faithful decode raised the advisory:\n{se}"
    );
}

/// A `cd` on its own line then the real command routes to the REAL
/// command's compactor, not to `cd`.
///
/// The unit guards in `command_rewrite::newline_is_a_separator` pin the
/// splitter; this one pins the WIRING, which is the half that was broken —
/// the splitter's `;` arm had always been right and a newline simply never
/// reached it.
#[test]
fn e2e_a_cd_on_its_own_line_still_reaches_the_real_compactor() {
    let h = TempHome::new("cdnewline");
    let dir = h.path().join("many");
    std::fs::create_dir_all(&dir).unwrap();
    for i in 0..60 {
        std::fs::write(dir.join(format!("file_{i:03}.txt")), "x").unwrap();
    }
    let dir_s = dir.display().to_string();

    let (semi, _, _) = run(&["wrap", &format!("cd {dir_s}; ls -la")], h.path());
    let (newline, _, _) = run(&["wrap", &format!("cd {dir_s}\nls -la")], h.path());

    assert!(
        semi.contains("[tokenade:ls-long]"),
        "the `;` form must reach the ls compactor:\n{semi}"
    );
    assert!(
        newline.contains("[tokenade:ls-long]"),
        "so must the newline form — it was reaching `[tokenade:cd]`:\n{newline}"
    );
}

/// The helper that runs the binary from outside `target/` must make ONE
/// copy per process and hardlink it into each test's HOME. Six calls at
/// 625 631 144 bytes each is 3,75 GB into an 8 GB tmpfs; with less than ~4 GB
/// free the suite failed as `StorageFull` inside four shim tests, which reads
/// exactly like a shim bug and is not one.
#[cfg(unix)]
#[test]
fn e2e_the_copied_binary_is_shared_not_recopied() {
    use std::os::unix::fs::MetadataExt;
    let h = TempHome::new("shared-bin");
    let home = h.path();
    let (_o, _e, _c) = run_copied_active(&["--version"], home);
    let dst = home.join(".tokenade-testbin").join("tokenade");
    let md = std::fs::metadata(&dst).expect("the copied binary");
    let shared = std::fs::metadata(super::shared_binary()).expect("the shared copy");
    assert_eq!(
        md.ino(),
        shared.ino(),
        "the test binary was copied again instead of hardlinked"
    );
    assert!(
        md.nlink() >= 2,
        "expected a hardlink, found {} link(s)",
        md.nlink()
    );
}

/// A paragraph opening with `#` reached the outline as a heading and stole the
/// sections after it. On a 12-section page carrying one such paragraph, the
/// outline reported the document's own title as "(3 sections)" and opened a
/// SECOND top-level document holding the other 9 — a structure the page does
/// not have, on the summary a reader uses to decide what to expand. Only an
/// end-to-end run proves it: the escape happens in the converter and the harm
/// lands two stages later, in `markdown::outline`.
#[test]
fn e2e_a_paragraph_cannot_forge_a_section_in_the_outline() {
    let h = TempHome::new("web-atx");
    let mut p = String::from("<html><body><h1>Doc</h1>");
    for s in 1..=12 {
        p.push_str(&format!("<h2>Genuine section {s}</h2><p>"));
        for _ in 0..300 {
            p.push_str("Filler prose about the subject at hand. ");
        }
        p.push_str("</p>");
        if s == 3 {
            p.push_str("<p># Deploying to production requires the admin token</p>");
        }
    }
    p.push_str("</body></html>");

    let page = h.dir.join("atx.html");
    std::fs::write(&page, p).unwrap();
    let (md, err, code) = run(&["web", &page.to_string_lossy()], &h.dir);
    assert_eq!(code, 0, "web failed: {err}");

    assert!(
        md.contains("# Doc (12 sections)"),
        "every section belongs to the document that has them, got:\n{}",
        &md[..md.len().min(600)]
    );
    assert!(
        !md.contains("# Deploying to production requires the admin token ("),
        "the paragraph must not head a section count of its own, got:\n{}",
        &md[..md.len().min(600)]
    );
}

/// A `<code>` span holding a pipe, inside a table cell, used to reach the
/// finished markdown with the pipe intact: the cell's own substitution runs
/// where the row is built, but the sanctuary had already swapped the span for
/// a placeholder, so the raw pipe was put back afterwards. `<td><code>0|1
/// </code></td>` emitted a row of four columns under a header of two. Only an
/// end-to-end run proves it — the sanctuary, the table pass and the restore
/// are three stages apart.
#[test]
fn e2e_a_code_span_cannot_add_a_column_to_a_table() {
    let h = TempHome::new("web-cellpipe");
    let fill = "Intro paragraph with enough words to not be tiny, plus filler \
                text here, and even more words to pass the validity floor of two \
                hundred characters so the aggressive pass is kept as-is and the \
                table survives the pass.";
    let html = format!(
        "<html><body><main><p>{fill}</p><table>\
         <tr><th>Flag</th><th>Values</th></tr>\
         <tr><td><code>--retry</code></td><td><code>0|1</code></td></tr>\
         </table></main></body></html>"
    );
    let page = h.dir.join("cellpipe.html");
    std::fs::write(&page, html).unwrap();
    let (md, err, code) = run(&["web", &page.to_string_lossy()], &h.dir);
    assert_eq!(code, 0, "web failed: {err}");

    let rows: Vec<&str> = md
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with('|'))
        .collect();
    assert!(rows.len() >= 3, "expected a table, got:\n{md}");
    // ESCAPED bars (`\|`, a cell's content) do not open a
    // column — counting them would make this test pass for the wrong reason.
    let cols = |r: &str| {
        let mut n = 1;
        let mut escaped = false;
        for c in r.trim().trim_matches('|').chars() {
            match c {
                '\\' => escaped = !escaped,
                '|' if !escaped => n += 1,
                _ => escaped = false,
            }
        }
        n
    };
    let header = cols(rows[0]);
    for r in &rows {
        assert_eq!(
            cols(r),
            header,
            "every row must have the header's column count ({header}), this one does not: {r}\n{md}"
        );
    }
    assert!(
        md.contains(r"0\|1"),
        "the cell's pipe must be escaped like any other cell text:\n{md}"
    );
}

/// A build tool refusing the command it was given closes with its own
/// `error:` line, and a pipeline hides the non-zero status behind the filter's
/// zero. `cargo <bogus> | head` is that shape exactly: the tool failed, the
/// shell reported success, and until the prefix rule the disclosure had no
/// marker to find — every recurring `error:` form in the corpus was one cargo
/// refusal or another.
/// A folded command output must stay searchable for a line it really printed.
///
/// When the fold left enough behind for `maybe_stash` to fire, its pointer —
/// over the SHOWN bytes — silenced the pointer over the RAW ones, so the only
/// hash the agent had named a stash the fold had already emptied. Grepping it
/// for a line that WAS printed answered "0 matching line(s)": a false negative
/// on the recovery path, which reads as "that string was never in the output".
#[test]
fn e2e_a_line_that_was_printed_is_still_findable_after_the_fold() {
    let h = TempHome::new("fold-recovery");
    // Unique per run: an identical earlier run would be served from the dedup
    // cache and this would measure that, not the fold.
    let tag = format!("{}", std::process::id());
    // TWO values per line (the index and a duration): a run whose one value
    // differs on every line is LISTED without loss since 2026-10-02, and this
    // fixture must still reach a fold that drops something.
    let script = format!(
        "python3 -c \"\nlines=['A'*4000]\nlines += ['repeated line {tag}']*50\n\
         lines += ['unique line %d payload {tag} took %d ms'%(i, i*37 % 101) for i in range(200)]\n\
         lines.append('B'*12000)\nprint('\\n'.join(lines))\""
    );
    let (out, _e, code) = run(&["wrap", &script], &h.dir);
    assert_eq!(code, 0, "stderr:\n{_e}");

    // SCOPE, stated because a mutation showed it: this test does NOT cover the
    // live-pointer interaction itself. Reaching it needs the fold to leave
    // enough behind for `maybe_stash` to fire too, which on this machine
    // happens under the `TOKENADE_PRESSURE` the hook sets — and the e2e
    // harness removes that variable on purpose, for determinism. Restoring
    // `|| has_live_pointer(&display)` therefore leaves this test green; the
    // unit tests in `tests::wrap` are what kill it. What this test does prove
    // is the end of the chain: a line the command printed comes back.
    // The complete-output pointer must be there, and be named as such.
    assert!(
        out.contains("COMPLETE output"),
        "no pointer to the unfolded bytes:\n{out}"
    );
    let hash = out
        .rsplit("expand-ref ")
        .next()
        .and_then(|t| t.split(|c: char| !c.is_ascii_hexdigit()).next())
        .map(str::to_string)
        .filter(|h| h.len() >= 8)
        .expect("a hash to follow the last expand-ref");

    let (found, _e, c) = run(&["expand-ref", &hash, "--grep", "unique line 137"], &h.dir);
    assert_eq!(c, 0, "expand-ref should exit 0:\n{_e}");
    assert!(
        found.contains("unique line 137"),
        "a line the command printed is not recoverable:\n{found}"
    );
    assert!(
        !found.contains("0 matching line(s)"),
        "the recovery path answered a false negative:\n{found}"
    );
}

/// `tokenade raw` refuses ten commands, and the reason it gave was false.
/// It said their compactors are "lossless at line granularity" and offered
/// `expand-ref` for "the verbatim bytes". Both halves are checked here against
/// the binary that ships: the refusal must not claim losslessness, and the
/// fold it defends must be shown actually dropping lines — so if the fold ever
/// becomes line-preserving, this test says the claim may come back.
#[test]
fn e2e_the_bypass_refusal_does_not_claim_a_losslessness_the_fold_lacks() {
    let h = TempHome::new("bypass-honesty");
    let big = h.dir.join("many.txt");
    let body: String = (0..1200)
        .map(|i| {
            format!(
                "src/mod{}/file{i}.rs:{i}: fn handler_{i}(req: Request)\n",
                i % 40
            )
        })
        .collect();
    std::fs::write(&big, &body).unwrap();
    let original_lines = body.lines().count();

    let (_o, err, code) = run(&["raw", "cat", big.to_str().unwrap()], &h.dir);
    assert_eq!(code, 2, "the refusal itself is unchanged: {err}");
    assert!(
        !err.contains("lossless"),
        "the refusal claims a losslessness the fold does not have:\n{err}"
    );
    assert!(
        err.contains("what the FOLD produced") && err.contains("exact original bytes"),
        "the refusal must say what the stash holds and name a route to the bytes:\n{err}"
    );
    assert!(
        err.contains("TOKENADE_FORCE_BYPASS"),
        "the override must stay named:\n{err}"
    );

    // And the behaviour the wording now describes: the fold drops whole lines.
    let (out, _e, c) = run(&["wrap", &format!("cat {}", big.display())], &h.dir);
    assert_eq!(c, 0);
    let kept = out.lines().count();
    assert!(
        kept * 2 < original_lines,
        "the fold kept {kept} of {original_lines} lines — if it is line-preserving \
         now, the refusal may say so again"
    );
}

#[test]
fn e2e_a_pipeline_cannot_hide_a_build_tool_refusing() {
    let h = TempHome::new("wrap-refusal");
    let (out, err, code) = run(&["wrap", "cargo zzbogus-subcommand 2>&1 | head -3"], &h.dir);
    let all = format!("{out}\n{err}");
    assert_eq!(code, 0, "the pipeline's own status is a zero, unchanged");
    assert!(
        all.contains("exited 0, but its output reports a failure"),
        "the false green must be disclosed, got:\n{all}"
    );
    assert!(
        all.contains("unrecognized subcommand") || all.contains("error: "),
        "the disclosure must quote what happened, got:\n{all}"
    );
    assert!(
        all.contains("LAST stage's status"),
        "and say why the status was green, got:\n{all}"
    );
}

/// `expand-ref` dropped a flag it did not know and carried on, exit 0. It is
/// the command an agent reaches for AFTER a fold has already taken bytes away,
/// so a one-letter typo changed the answer in silence: `--raww <hash>` returned
/// the "too large to dump whole" banner where `--raw` returns the blob, with
/// nothing to say the flag had been ignored.
#[test]
fn e2e_expand_ref_names_a_flag_it_does_not_know() {
    let h = TempHome::new("expand-flag");
    let (out, err, code) = run(&["expand-ref", "--raww", "abcdef123456"], &h.dir);
    let all = format!("{out}\n{err}");
    assert_eq!(code, 2, "an unknown flag is a refusal, got:\n{all}");
    assert!(
        all.contains("unknown flag '--raww'"),
        "the refusal must name the flag, got:\n{all}"
    );
    // The flags it DOES know still reach the hash pass, which answers about
    // the hash rather than about the flag.
    let (_o, e2, c2) = run(&["expand-ref", "--raw", "abcdef123456"], &h.dir);
    assert_ne!(c2, 2, "a real flag must not be refused: {e2}");
    assert!(
        e2.contains("not found") || e2.contains("expired"),
        "a real flag reaches the lookup, got:\n{e2}"
    );
}

/// A `--grep` pattern is read as a REGEX, and a flag wedged before it
/// is read where it was meant.
///
/// Both were silent wrong answers on the RECOVERY command. MEASURED 2026-09-05
/// across this machine's transcripts: **38 of 128 `--grep` patterns (30%) were
/// regex-shaped** — `OSCILLE|ligne\(s\)|suspect`, `ok |FAIL|error|warning`,
/// `\[8\]` — against a substring test, so they matched nothing; and **12 of
/// 261 `expand-ref` calls (63 occurrences) put `-i` between `--grep` and its
/// pattern**, which made `-i` the pattern. Both answered "0 matching line(s)",
/// which on this command reads as "the content is not there".
///
/// Driven through the binary because the matcher lives in core and the parser
/// in the CLI: a unit test on either half stays green while the other drifts.
#[test]
fn e2e_a_grep_pattern_is_a_regex_and_a_wedged_flag_is_not_the_pattern() {
    let h = TempHome::new("expand-regex");
    let big = h.dir.join("blob.txt");
    let mut body = String::from("OSCILLE detected here\n3 ligne(s) suspect\n[8] footnote\n");
    for i in 0..3000 {
        body.push_str(&format!("line {i} Alpha Beta Gamma connection refused\n"));
    }
    std::fs::write(&big, body).unwrap();
    let (out, err, _c) = run(&["read", &big.to_string_lossy()], &h.dir);
    let both = format!("{out}\n{err}");
    let hash = both
        .split(|c: char| !c.is_ascii_hexdigit())
        .find(|t| {
            t.len() == 12
                && t.chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
        })
        .unwrap_or_else(|| panic!("no stash hash in:\n{both}"))
        .to_string();

    // An alternation finds every branch it names.
    let (o, e, _c) = run(
        &["expand-ref", "--grep", r"OSCILLE|ligne\(s\)|suspect", &hash],
        &h.dir,
    );
    assert!(
        o.contains("OSCILLE detected here") && o.contains("3 ligne(s) suspect"),
        "an alternation must match each of its branches, got:\n{o}\n{e}"
    );

    // Escaped brackets are a regex, not four literal characters.
    let (o, _e, _c) = run(&["expand-ref", "--grep", r"\[8\]", &hash], &h.dir);
    assert!(
        o.contains("[8] footnote"),
        "an escaped bracket must match the bracket, got:\n{o}"
    );

    // The literal reading is KEPT, as a union. `ligne(s)` compiles — as a
    // regex it means "lignes" and would match nothing here — so a pure-regex
    // reading would have taken this match away from whoever meant the
    // parentheses. And a pattern that does not compile at all is still a
    // substring search.
    for literal in ["ligne(s)", "ligne(s"] {
        let (o, _e, _c) = run(&["expand-ref", "--grep", literal, &hash], &h.dir);
        assert!(
            o.contains("3 ligne(s) suspect"),
            "{literal:?} must still find the line it names, got:\n{o}"
        );
    }

    // The flag wedged before the pattern: `-i` is the flag, `Alpha` the
    // pattern — and the case fold has to have actually happened.
    let (o, e, _c) = run(&["expand-ref", "--grep", "-i", "alpha", &hash], &h.dir);
    assert!(
        o.contains("Alpha"),
        "`--grep -i alpha` must grep for alpha, ignoring case, got:\n{o}\n{e}"
    );
    assert!(
        !e.contains("is not a ref hash"),
        "the pattern must not fall through to the hash reader, got:\n{e}"
    );

    // A regex AND a case fold at once. The union's substring branch folds case
    // on its own, so only the `(?i)` on the compiled pattern can answer this.
    let (o, e, _c) = run(
        &["expand-ref", "--grep", "-i", r"oscille|SUSPECT", &hash],
        &h.dir,
    );
    assert!(
        o.contains("OSCILLE detected here") && o.contains("3 ligne(s) suspect"),
        "an alternation must fold case on every branch, got:\n{o}\n{e}"
    );

    // Anti-vacuity: a term that really is absent still reads as absent.
    let (_o, e, _c) = run(&["expand-ref", "--grep", "zzz-not-present", &hash], &h.dir);
    assert!(
        e.contains("ignoring case either"),
        "an absent term must still say so, got:\n{e}"
    );
}

/// A `--grep` that matched nothing printed the same line whether the term was
/// absent or merely spelled with different case. On the recovery command —
/// reached only after a fold already took the bytes away — "0 matching line(s)
/// of 3000 total" reads as an answer, and the caller stops looking.
#[test]
fn e2e_a_grep_miss_says_which_kind_of_miss_it_is() {
    let h = TempHome::new("expand-miss");
    let big = h.dir.join("blob.txt");
    let mut body = String::new();
    for i in 0..3000 {
        body.push_str(&format!("line {i} Alpha Beta Gamma connection refused\n"));
    }
    std::fs::write(&big, body).unwrap();
    let (out, err, _c) = run(&["read", &big.to_string_lossy()], &h.dir);
    let both = format!("{out}\n{err}");
    let hash = both
        .split(|c: char| !c.is_ascii_hexdigit())
        .find(|t| {
            t.len() == 12
                && t.chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
        })
        .unwrap_or_else(|| panic!("no stash hash in:\n{both}"))
        .to_string();

    // Present, but the case differs: name the count and the flag that finds it.
    let (_o, e, _c) = run(&["expand-ref", "--grep", "alpha", &hash], &h.dir);
    assert!(
        e.contains("match ignoring case") && e.contains("--ignore-case"),
        "a case-only miss must say so and name the flag, got:\n{e}"
    );
    // ONE answer per miss: the flag that finds them, not that plus the note
    // written for the case where nothing will.
    assert!(
        !e.contains("folded summary"),
        "a case-only miss must not also print the absent note, got:\n{e}"
    );

    // Genuinely absent: the OTHER message, so the two are distinguishable.
    let (_o, e2, _c) = run(&["expand-ref", "--grep", "zzz-not-present", &hash], &h.dir);
    assert!(
        e2.contains("ignoring case either"),
        "an absent term must read differently from a case miss, got:\n{e2}"
    );
    assert!(
        !e2.contains("--ignore-case"),
        "and must not point at a flag that would not help, got:\n{e2}"
    );
    // And it must name WHAT it searched. A stash is not always the complete
    // output — a disclose stash is often a folded summary — so a bare "nothing
    // matches" reads as "not in the output" and ends the search on the one
    // command reached only after the bytes were already taken away.
    assert!(
        e2.contains("what this hash holds") && e2.contains("folded summary"),
        "a miss must say what it searched, got:\n{e2}"
    );
    // And the count beside it must not call that scope `total`.
    assert!(
        e2.contains("held under this hash") && !e2.contains("total"),
        "the count must name what it counts, got:\n{e2}"
    );

    // The flag the note names actually finds them.
    let (o3, _e, _c) = run(
        &["expand-ref", "--grep", "alpha", "--ignore-case", &hash],
        &h.dir,
    );
    assert!(
        o3.contains("Alpha"),
        "--ignore-case must return the lines, got:\n{}",
        &o3[..o3.len().min(200)]
    );

    // An UPPERCASE pattern too: folding only one side of the comparison passes
    // every lowercase fixture and fails here, which is how a half-done fold
    // survives its own test.
    let (o3b, _e, _c) = run(
        &["expand-ref", "--grep", "ALPHA", "--ignore-case", &hash],
        &h.dir,
    );
    assert!(
        o3b.contains("Alpha"),
        "both sides of the comparison must be folded, got:\n{}",
        &o3b[..o3b.len().min(200)]
    );

    // ...and a literal grep stays literal.
    let (o4, _e, _c) = run(&["expand-ref", "--grep", "Alpha", &hash], &h.dir);
    assert!(
        o4.contains("Alpha"),
        "the exact term still matches:\n{o4:.200}"
    );
    let (o5, e5, _c) = run(&["expand-ref", "--grep", "ALPHA", &hash], &h.dir);
    assert!(
        !o5.contains("line 0 Alpha"),
        "without the flag an uppercase pattern must NOT match, got:\n{o5:.200}"
    );
    assert!(
        e5.contains("match ignoring case"),
        "and must say why:\n{e5}"
    );
}

/// A LISTED command answers under its OWN listed name, never under one of its
/// aliases.
///
/// `tokenade list-commands` is the product's own declaration of the primary
/// names, and `install::help_text::alias_to_primary` maps `"ask" | "sem" =>
/// "semantic"` — so `semantic` is the primary and `ask` is the alias. Every
/// message `cmd_semantic` emitted said the opposite: the usage line, the
/// empty-query refusal, the index-build failure and the unknown-flag refusal
/// all opened with `tokenade ask:` and pointed at `tokenade ask --help`.
/// `tokenade <alias> --help` has always answered under the PRIMARY
/// (`tokenade ask --help` prints "tokenade semantic — …"), so the refusals were
/// the one surface disagreeing with the rest of the CLI.
///
/// Measured over 60 days of this machine's transcripts: `semantic` was typed
/// 26 times, `ask` and `sem` zero times, and the scaffolded project rules
/// tokenade writes into `CLAUDE.md` teach `semantic`. So the name being taught
/// was the one name the command never answered to.
///
/// The invariant is checked against the binary's own `list-commands` rather
/// than a list written here, so a new command is covered the day it ships.
#[test]
fn e2e_a_listed_command_answers_under_its_listed_name() {
    let h = TempHome::new("refusal-name");
    let (listed, _e, code) = run(&["list-commands"], &h.dir);
    assert_eq!(code, 0, "list-commands must answer: {listed}");

    // Commands whose `--zzbogus` would do real work rather than refuse: they
    // wrap a command line (the flag belongs to the wrapped command), talk to
    // the network, write to the install, or block. Named, not pattern-matched,
    // so adding one is a deliberate act.
    #[rustfmt::skip]
    const SKIP: &[&str] = &[
        "raw", "wrap", "filter", "proxy", "install", "uninstall", "upgrade", "activate", "login",
        "logout", "auth", "dashboard", "local-serve", "agent", "scout", "watch", "index", "bench",
        "mcp-wrap", "llm-proxy", "hookwrap", "shellwrap", "shell-init", "statusline", "report",
    ];

    let mut checked = 0;
    let mut refused_nothing = Vec::new();
    for name in listed.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if SKIP.contains(&name) {
            continue;
        }
        let (out, err, _c) = run(&[name, "--zzbogus"], &h.dir);
        let all = format!("{out}\n{err}");
        // Only lines that REFUSE are in scope; a command may legitimately
        // answer something else (a usage block, a subcommand list).
        let Some(line) = all
            .lines()
            .find(|l| l.contains("unknown flag") || l.contains("unexpected flag"))
        else {
            refused_nothing.push(name.to_string());
            continue;
        };
        assert!(
            line.starts_with(&format!("tokenade {name}:")),
            "`{name}` answers under another name: {line}"
        );
        checked += 1;
    }
    assert!(
        checked >= 20,
        "the sweep proved nothing ({checked} refusals seen; \
         {} commands answered without refusing: {refused_nothing:?})",
        refused_nothing.len()
    );
}

/// Every spelling of the semantic command answers as `semantic`, on every
/// message a caller can actually reach — not just the unknown-flag one.
///
/// Two of its messages are NOT reachable from here and are pinned by the shared
/// `SEMANTIC_CMD` constant rather than by this test: the index-build failure
/// needs an unbuildable root, and the search error needs a corrupt semantic
/// index. Saying so is the point — a guard does not cover a branch it never
/// traverses.
#[test]
fn e2e_semantic_answers_as_semantic_under_all_three_spellings() {
    let h = TempHome::new("semantic-name");
    for spelling in ["semantic", "sem", "ask"] {
        // No arguments at all → the usage line.
        let (o1, e1, c1) = run(&[spelling], &h.dir);
        assert_eq!(c1, 2, "`{spelling}` with no query must refuse");
        // An unknown flag → the refusal.
        let (o2, e2, _c2) = run(&[spelling, "--zzbogus", "a question"], &h.dir);
        // An empty query → the whitespace refusal.
        let (o3, e3, _c3) = run(&[spelling, "   "], &h.dir);
        // A bad `-o` VALUE → the output-mode refusal, which is a DIFFERENT
        // declarer from the three above: `--zzbogus` is caught by the
        // positional collector and never reaches it.
        let (o4, e4, _c4) = run(&[spelling, "-o", "bogus", "a question"], &h.dir);

        for (what, text) in [
            ("usage", format!("{o1}{e1}")),
            ("unknown flag", format!("{o2}{e2}")),
            ("empty query", format!("{o3}{e3}")),
            ("bad -o value", format!("{o4}{e4}")),
        ] {
            assert!(
                text.contains("tokenade semantic"),
                "`{spelling}` {what} does not name `semantic`: {text}"
            );
            assert!(
                !text.contains("tokenade ask"),
                "`{spelling}` {what} still speaks as the alias: {text}"
            );
        }
    }
}

/// Every unknown-flag refusal names the command and quotes the flag.
///
/// This family has been repaired one member at a time — `web`, then `map`,
/// then `exec` and `install`, then `expand-ref` — and nothing pinned the SHAPE,
/// so each new command drifted on its own. Two were out of line when this was
/// written: `pack` printed the flag unquoted and pointed at a bare `--help`
/// that names no command, and `expand-ref` dropped the `tokenade ` prefix.
///
/// The invariant is self-consistency rather than a fixed string: a command may
/// answer under its canonical name (`semantic` answers as `ask`, `exec` as
/// `execute`), and may say more than the minimum, but the name it opens with
/// and the name in the help pointer must be the SAME name.
#[test]
fn e2e_every_unknown_flag_refusal_names_the_command_and_quotes_the_flag() {
    let h = TempHome::new("refusal-shape");
    let f = h.dir.join("f.txt");
    std::fs::write(&f, "hi\n").unwrap();
    let path = f.to_string_lossy().to_string();

    // Each command with the positionals it needs to get PAST "usage" and reach
    // its flag parser.
    let cases: Vec<(&str, Vec<String>)> = vec![
        ("read", vec![path.clone()]),
        ("web", vec![path.clone()]),
        ("skeleton", vec![path.clone()]),
        ("query", vec!["some_symbol".into()]),
        ("impact", vec![path.clone()]),
        ("map", vec![]),
        ("semantic", vec!["a question".into()]),
        ("search", vec!["a query".into()]),
        ("pack", vec![]),
        ("expand-ref", vec!["abcdef123456".into()]),
        ("discover", vec![]),
        ("gain", vec![]),
        ("dashboard", vec![]),
    ];

    let mut checked = 0;
    for (sub, extra) in &cases {
        let mut argv: Vec<&str> = vec![sub, "--zzbogus"];
        for e in extra {
            argv.push(e);
        }
        let (out, err, code) = run(&argv, &h.dir);
        let all = format!("{out}\n{err}");
        let Some(line) = all.lines().find(|l| l.contains("unknown flag")) else {
            panic!("`{sub} --zzbogus` did not refuse:\n{all}");
        };
        assert_eq!(code, 2, "`{sub}` must exit 2 on an unknown flag: {line}");

        // Opens with `tokenade <name>: unknown flag '<flag>'`.
        let named = line
            .strip_prefix("tokenade ")
            .and_then(|r| r.split_once(": unknown flag "))
            .unwrap_or_else(|| {
                panic!("`{sub}` does not open with `tokenade <name>: unknown flag`: {line}")
            });
        let (name, rest) = named;
        assert!(
            rest.starts_with("'--zzbogus'"),
            "`{sub}` must QUOTE the flag, so a flag with odd bytes is unambiguous: {line}"
        );
        // ...and the help it points at is the SAME command.
        assert!(
            line.contains(&format!("`tokenade {name} --help`")),
            "`{sub}` opens as {name:?} but points somewhere else: {line}"
        );
        checked += 1;
    }
    assert!(
        checked >= 13,
        "the case list went empty, so this proved nothing ({checked} checked)"
    );
}

/// Every passage of an answer drawn from a document with no headings claimed
/// to be that document's intro. The label exists to say WHERE a passage came
/// from, and on a 1 500-line blob it said "intro" about one opening at line
/// 128 and another opening at line 902.
#[test]
fn e2e_a_passage_from_the_middle_is_not_called_the_intro() {
    let h = TempHome::new("rag-label");
    let blob = h.dir.join("code.txt");
    let mut body = String::new();
    for i in 0..1500 {
        body.push_str(&format!(
            "fn handler_{i}(req: Request) -> Response {{ route(req, {i}) }}\n"
        ));
        if i == 900 {
            body.push_str("// The retry budget is 7 attempts with jitter\n");
        }
    }
    std::fs::write(&blob, body).unwrap();
    let (o, e, _c) = run(&["read", &blob.to_string_lossy()], &h.dir);
    let both = format!("{o}\n{e}");
    let hash = both
        .split(|c: char| !c.is_ascii_hexdigit())
        .find(|t| {
            t.len() == 12
                && t.chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
        })
        .unwrap_or_else(|| panic!("no ref in:\n{both}"))
        .to_string();

    let (ans, _e, code) = run(
        &["expand-ref", "--prompt", "what is the retry budget", &hash],
        &h.dir,
    );
    assert_eq!(code, 0, "the question must be answerable");
    let labels: Vec<&str> = ans.lines().filter(|l| l.starts_with("> ")).collect();
    assert!(
        labels.len() >= 2,
        "expected several passages, got: {labels:?}"
    );
    assert!(
        labels.iter().filter(|l| l.contains("intro")).count() <= 1,
        "at most the opening passage may call itself the intro, got: {labels:?}"
    );
    assert!(
        ans.contains("retry budget"),
        "and the answering line must still be retrieved:\n{}",
        &ans[..ans.len().min(300)]
    );
}

/// The command run to RECOVER folded bytes gets the bytes, not another pointer.
///
/// `expand-ref` is what the banner and the shipped guidance both point at when
/// a compactor has folded something away. Run through `wrap`, its answer was
/// compacted and deduped like any other output, so it came back as a §ref
/// marker naming the hash it had just been handed — the recovery path was a
/// closed loop. Measured 2026-09-03 in a live session, on
/// `timeout 60 tokenade expand-ref 3ebeec7cd4e7 --raw`.
///
/// The launcher prefix is the point of the fixture, not decoration: the
/// PreToolUse hook skips a command whose program IS tokenade, so the bare
/// spelling never reaches `wrap` at all. `timeout`, `env FOO=1` and `nice`
/// each make the program something else, and all three were rewritten into a
/// wrap when this was measured.
///
/// The control below is what keeps the fix from being "never fold anything":
/// the same repeated command, without the recovery verb, must still fold.
#[test]
fn e2e_wrap_serves_a_recovery_command_verbatim() {
    let h = TempHome::new("wrap-recovery");
    let bin = binary();
    let payload = h.dir.join("payload.txt");
    let body: String = (1..=4000).map(|i| format!("line {i}\n")).collect();
    std::fs::write(&payload, &body).expect("write payload");

    let cat = format!("cat {}", payload.display());
    let mut fold = String::new();
    for _ in 0..2 {
        let out = std::process::Command::new(&bin)
            .args(["wrap", &cat])
            .env("HOME", h.path())
            .env("TOKENADE_SESSION", "e2e-recovery")
            .output()
            .expect("run wrap");
        fold = String::from_utf8_lossy(&out.stdout).into_owned();
    }
    let hash = fold
        .split("expand-ref ")
        .nth(1)
        .and_then(|r| r.split(|c: char| !c.is_ascii_hexdigit()).next())
        .map(str::to_string)
        .filter(|h| h.len() >= 8)
        .unwrap_or_else(|| panic!("the second identical run must fold and name a hash:\n{fold}"));

    let recover = format!("timeout 60 {} expand-ref {hash} --raw", bin.display());
    let out = std::process::Command::new(&bin)
        .args(["wrap", &recover])
        .env("HOME", h.path())
        .env("TOKENADE_SESSION", "e2e-recovery")
        .output()
        .expect("run recovery");
    let got = String::from_utf8_lossy(&out.stdout);
    assert!(
        !got.contains("\u{a7}ref:"),
        "recovery answered with a pointer instead of the bytes:\n{:.400}",
        got
    );
    assert!(
        got.contains("line 1\n") && got.contains("line 4000"),
        "recovery must hand back the stored bytes:\n{:.400}",
        got
    );

    // Control: without the recovery verb the same repeated command still folds,
    // so the fix cannot be "stop folding".
    let out = std::process::Command::new(&bin)
        .args(["wrap", &cat])
        .env("HOME", h.path())
        .env("TOKENADE_SESSION", "e2e-recovery")
        .output()
        .expect("run control");
    let ctl = String::from_utf8_lossy(&out.stdout);
    assert!(
        ctl.len() < body.len() / 4,
        "an ordinary repeated command must still be folded, got {} bytes",
        ctl.len()
    );
}

/// A repeat folds to a `§ref:` only when the pointer is smaller than the bytes
/// it replaces.
///
/// `MIN_BYTES_TO_DEDUPE` says outputs below it "skip the dedup path entirely",
/// and the hook paths honour it; this one did not. The marker is ~147 bytes:
/// measured 2026-09-04, reading a 188-byte file twice returned the pointer the
/// second time — the content minus 41 bytes, plus a whole turn to get it back —
/// and reading an EMPTY file returned those same 147 bytes for ZERO, telling
/// the agent to "ask it what you need" about nothing, and calling a file it had
/// never seen "identical to a prior call".
#[test]
fn e2e_a_repeat_too_small_to_be_worth_a_pointer_is_served_whole() {
    let h = TempHome::new("read-dedup-floor");
    let home = h.path();

    // Empty, tiny, and — the one the margin is about — bigger than the marker
    // but not twice it: folding 250 bytes into 154 saves 96 and costs a turn.
    let just_under = "x".repeat(250);
    for payload in [
        "",
        "une ligne courte, repetee, et sous le plancher\n",
        &just_under,
    ] {
        let first = read_with_session(home, Some("floorsess"), payload);
        let second = read_with_session(home, Some("floorsess"), payload);
        assert!(
            !second.contains("§ref:"),
            "{} bytes came back as a pointer that costs more than they do:\n{second}",
            payload.len()
        );
        assert_eq!(
            first, second,
            "and the second delivery is the same answer as the first"
        );
    }

    // Anti-vacuity: this is a FLOOR, not a switch. Above it the fold still
    // happens, on the same session, in the same test.
    let big: String = (0..80)
        .map(|i| {
            format!(
                "note {i}: le compacteur ne peut pas replier ceci en un gabarit, \
                 chaque ligne dit autre chose ({}).\n",
                "abcdefghijklmnopqrstuvwxyz".repeat(1 + i % 3)
            )
        })
        .collect();
    let _ = read_with_session(home, Some("floorsess"), &big);
    let again = read_with_session(home, Some("floorsess"), &big);
    assert!(
        again.contains("§ref:"),
        "a repeat worth folding must still fold:\n{again}"
    );

    // And the floor is stated against a marker whose size is MEASURED. The
    // constant it derives from was chosen when this marker cost ~575 bytes; a
    // floor computed from a stale figure is how the bug got here.
    assert!(
        again.len() <= tokenade_core::dedup_ref::REF_MARKER_BYTES,
        "the marker is {} bytes, above the declared bound of {} — re-measure \
         REF_MARKER_BYTES before the floor derived from it drifts:\n{again}",
        again.len(),
        tokenade_core::dedup_ref::REF_MARKER_BYTES
    );
}

/// A poll answer is a claim about the COMMAND, so it is checked against the
/// command's own bytes.
///
/// `serve_exec_dedup` compared the two COMPACTED strings while telling the
/// agent "byte-identical output to the previous run of this command", and the
/// comment at its call site called that "truthful by construction — fires only
/// on byte equality". True of the fold, not of the command: a fold that drops
/// a middle makes two different outputs deliver identically.
///
/// REPRODUCED 2026-09-05: 400 lines of prose, ONE line in the elided middle
/// rewritten, and the second `cat` answered `unchanged — byte-identical output
/// to the previous run`. An agent polling that file is told nothing moved.
///
/// What this test does NOT claim is that the rewritten line comes back — it is
/// in the middle a `cat` above the threshold drops on purpose, and the banner
/// now says how many lines that is. The defect was the false claim of
/// identity, and that is what is asserted here.
#[test]
fn a_poll_answer_is_checked_against_the_commands_own_bytes() {
    let h = TempHome::new("poll-truthful");
    let home = h.path();
    let f = home.join("notes.md");
    let body: String = (0..400)
        .map(|i| {
            format!(
                "{} {} {} {} {} {}\n",
                ["alpha", "bravo", "charlie", "delta"][i % 4],
                ["echo", "foxtrot", "golf"][i % 3],
                ["hotel", "india"][i % 2],
                ["juliet", "kilo", "lima", "mike", "november"][i % 5],
                ["oscar", "papa", "quebec"][i % 3],
                i
            )
        })
        .collect();
    std::fs::write(&f, &body).unwrap();
    let cmd = format!("cat {}", f.display());

    let _ = run(&["wrap", &cmd], home).0;
    // A real change, in the middle the slice elides.
    let mut lines: Vec<String> = body.lines().map(String::from).collect();
    lines[220] = "zulu whiskey victor uniform the decision was reversed".to_string();
    std::fs::write(&f, lines.join("\n") + "\n").unwrap();

    let after_change = run(&["wrap", &cmd], home).0;
    assert!(
        !after_change.contains("byte-identical output to the previous run"),
        "the file changed and the poll called it unchanged:\n{after_change}"
    );
    assert!(
        !after_change.contains("still unchanged"),
        "the file changed and the streak answer fired:\n{after_change}"
    );

    // Anti-vacuity: a genuinely unchanged re-run must STILL answer the poll,
    // or this "fix" is just a disabled feature.
    let unchanged = run(&["wrap", &cmd], home).0;
    assert!(
        unchanged.contains("exec-dedup"),
        "an unchanged re-run stopped answering the poll:\n{unchanged}"
    );
}

/// A repeated read of ONE file is answered with the file.
///
/// The dedup answer handed back a stash hash whatever the command was. For a
/// `cat <file>` that is the wrong pointer twice over: the stash holds what was
/// DELIVERED (already compacted), and the file itself is still on disk, where
/// `Read` takes offset/limit and a re-read of an unchanged path costs a diff.
/// MEASURED 2026-09-05 across this machine's transcripts: 73 of 105 dedup
/// answers were a single-file `cat`, and 40 of those were followed within
/// three turns by an `expand-ref` pulling the content back, median 2 319
/// tokens spent to undo the fold.
///
/// Both answer shapes are asserted. The 2nd+ "still unchanged" one-liner drops
/// the tail by design, so before this it carried no way back except the hash.
#[test]
fn e2e_a_repeated_read_of_one_file_names_the_file() {
    let h = TempHome::new("dedup-names-the-file");
    let home = h.path();
    let f = home.join("notes.md");
    // Varied on purpose: a uniform body folds to a single template line, and a
    // delivery that small is never deduped at all.
    let body: String = (0..400)
        .map(|i| {
            format!(
                "{} {} {} {} {}\n",
                ["alpha", "bravo", "charlie", "delta"][i % 4],
                ["echo", "foxtrot", "golf"][i % 3],
                ["hotel", "india"][i % 2],
                ["juliet", "kilo", "lima", "mike", "november"][i % 5],
                i
            )
        })
        .collect();
    std::fs::write(&f, &body).unwrap();
    let cmd = format!("cat {}", f.display());
    let want = format!("Read {}", f.display());

    let _ = run(&["wrap", &cmd], home).0;
    let first = run(&["wrap", &cmd], home).0;
    let again = run(&["wrap", &cmd], home).0;
    for (shape, answer) in [("first repeat", &first), ("second repeat", &again)] {
        assert!(
            answer.contains("exec-dedup"),
            "{shape}: the repeat stopped being answered at all:\n{answer}"
        );
        assert!(
            answer.contains(&want),
            "{shape}: the way back must name the file, not only a stash hash:\n{answer}"
        );
    }

    // A command that reads no single nameable file keeps the hash-only
    // wording: there is no path to point at, and claiming one would be a
    // statement about bytes that never came from a file.
    let dir = home.join("tree");
    std::fs::create_dir_all(&dir).unwrap();
    for i in 0..60 {
        std::fs::write(dir.join(format!("f{i}.txt")), "x\n").unwrap();
    }
    let lscmd = format!("ls -la {}", dir.display());
    let _ = run(&["wrap", &lscmd], home).0;
    let ls_again = run(&["wrap", &lscmd], home).0;
    assert!(
        ls_again.contains("exec-dedup"),
        "the negative half proves nothing unless the repeat was answered:\n{ls_again}"
    );
    assert!(
        !ls_again.contains("(offset/limit)"),
        "a command that reads no single file must not point at one:\n{ls_again}"
    );
    assert!(
        ls_again.contains("tokenade expand-ref"),
        "with no file to name, the stash pointer stays the way back:\n{ls_again}"
    );
}

/// The same log, read two ways, gets the recovery each way actually has.
///
/// `tokenade read -` has no copy of the bytes but the stash, so it hands out a
/// hash. `tokenade read <file>` has the file, so it names the file and writes
/// nothing. What neither may do is say nothing: measured 2026-09-04 before
/// this, the file route returned **154 bytes for 4,608,890** — one correct,
/// complete, shaped line — and named no next step, while the guidance shipped
/// with the product tells the agent to run `expand-ref` with "the hash printed
/// in the compactor's banner".
#[test]
fn e2e_a_folded_file_read_names_the_file_and_a_piped_one_names_a_hash() {
    let h = TempHome::new("read-recovery-shape");
    let home = h.path();
    let log: String = (0..2_000)
        .map(|i| {
            format!(
                "2026-09-04T10:00:00Z INFO worker-{} handled request {i} status=200\n",
                i % 7
            )
        })
        .collect();
    let path = home.join("build.log");
    std::fs::write(&path, &log).unwrap();

    let from_file = run(&["read", path.to_str().unwrap()], home).0;
    assert!(
        from_file.len() < log.len() / 100,
        "the fold is the premise of this test: {} bytes",
        from_file.len()
    );
    assert!(
        from_file.contains(path.to_str().unwrap()),
        "a folded file read must name the file as the way back:\n{from_file}"
    );
    assert!(
        !from_file.contains("expand-ref"),
        "no stash is written for a file, so no hash may be promised:\n{from_file}"
    );

    let piped = run_with_stdin(&["read", "-"], home, &log).0;
    assert!(
        piped.contains("expand-ref"),
        "piped bytes have no copy but the stash, so the hash is the way back:\n{piped}"
    );

    // And a file small enough that nothing is dropped gets neither line.
    let small = home.join("short.txt");
    std::fs::write(&small, "alpha\nbeta\ngamma\n").unwrap();
    let quiet = run(&["read", small.to_str().unwrap()], home).0;
    assert_eq!(
        quiet, "alpha\nbeta\ngamma\n",
        "nothing folded, nothing said"
    );
}

/// Tokenade's own plumbing is not compacted, and not counted, even when
/// something on PATH turns it into a wrapped command.
///
/// `pack` asks `git ls-files` which files exist and reads the answer ITSELF.
/// Put an interception layer in front of `git` — the shape of the PATH shim
/// shipped before 0.9.5, which the installer removes but an install that never
/// upgraded still carries — and measured 2026-09-04 the run booked
/// `wrap / git-ls-files 8059 -> 249` plus a `recovery:offered` row, a saving on
/// 2,699 paths no agent was ever going to receive, while pack's own answer came
/// back different because it had parsed the FOLD instead of the file list.
#[test]
fn e2e_an_intercepted_internal_subprocess_is_neither_folded_nor_counted() {
    if cfg!(windows) {
        return;
    }
    let Some(git) = which_on_path("git") else {
        return;
    };
    let h = TempHome::new("internal-subprocess");
    let home = h.path();
    let repo = home.join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    for i in 0..40 {
        std::fs::write(
            repo.join("src").join(format!("mod{i}.rs")),
            format!("pub fn f{i}() -> usize {{ {i} }}\n"),
        )
        .unwrap();
    }
    for args in [
        vec!["init", "-q"],
        vec!["add", "-A"],
        vec![
            "-c",
            "user.email=t@e",
            "-c",
            "user.name=t",
            "commit",
            "-qm",
            "x",
        ],
    ] {
        let out = std::process::Command::new(&git)
            .args(&args)
            .current_dir(&repo)
            .env("HOME", home)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    // The tap lives OUTSIDE the tree being packed: a file that appears between
    // the two runs would change the answer all by itself.
    let tap = std::env::temp_dir().join(format!("tokenade-shimtap-{}", std::process::id()));
    let _ = std::fs::remove_file(&tap);
    // A `git` that hands the command to `tokenade wrap`, then gets out of its
    // own PATH so the wrap's inner shell finds the real one.
    let shim_dir = std::env::temp_dir().join(format!("tokenade-shim-{}", std::process::id()));
    std::fs::create_dir_all(&shim_dir).unwrap();
    let shim = shim_dir.join("git");
    std::fs::write(
        &shim,
        format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\n\
             PATH=\"$(echo \"$PATH\" | tr ':' '\\n' | grep -v '{}' | paste -sd:)\"\n\
             export PATH\nexec '{}' wrap \"git $*\"\n",
            tap.display(),
            shim_dir.display(),
            binary().display()
        ),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let pack = |shimmed: bool| -> String {
        let mut c = std::process::Command::new(binary());
        c.args(["pack", "--budget", "1200", repo.to_str().unwrap()])
            .current_dir(&repo);
        e2e_env(&mut c, home);
        c.env("TOKENADE_FORCE_GAIN", "1");
        if shimmed {
            let path = std::env::var("PATH").unwrap_or_default();
            c.env("PATH", format!("{}:{path}", shim_dir.display()));
        }
        let out = c.output().expect("run pack");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };

    let clean = pack(false);
    assert!(
        clean.contains("git-tracked"),
        "pack has to be reading the git file list, or nothing here is intercepted:\n{}",
        &clean[..clean.len().min(400)]
    );
    let before = ledger_rows(home);
    let intercepted = pack(true);

    // Anti-vacuity first: the interception layer has to have RUN, or this is
    // one code path compared against itself.
    let tapped = std::fs::read_to_string(&tap).unwrap_or_default();
    let _ = std::fs::remove_file(&tap);
    let _ = std::fs::remove_dir_all(&shim_dir);
    assert!(
        tapped.contains("ls-files"),
        "the shim never saw pack's `git ls-files`: {tapped:?}"
    );

    assert_eq!(
        clean, intercepted,
        "an intercepted `git ls-files` changed what pack answered — it parsed the fold, \
         not the file list"
    );
    assert_eq!(
        ledger_rows(home),
        before,
        "tokenade's own subprocess was booked as a saving"
    );

    // The other half, or the guard would be "never compact anything": an
    // agent asking for a wrap by name still gets one.
    let (asked, _e, _c) = run(
        &["wrap", &format!("ls -la {}", repo.join("src").display())],
        home,
    );
    assert!(
        asked.contains("[tokenade:"),
        "an explicit `tokenade wrap` must still fold:\n{asked}"
    );
    assert!(ledger_rows(home) > before, "and must still be counted");
}

/// Where `prog` really is, ignoring anything this test puts on PATH.
fn which_on_path(prog: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|p| {
        std::env::split_paths(&p)
            .map(|d| d.join(prog))
            .find(|c| c.is_file())
    })
}

/// How many rows the savings ledger holds under `home`.
fn ledger_rows(home: &Path) -> usize {
    let dir = home.join(".tokenade");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("gain"))
        .map(|e| {
            std::fs::read_to_string(e.path())
                .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
                .unwrap_or(0)
        })
        .sum()
}

/// A user's compactor is an instruction, not a candidate.
///
/// The post-passes may shrink the answer a user filter produced; they may not
/// go back to the raw bytes. `fold_repetitive_runs` folds the RAW input, and a
/// fold of the raw can beat the filter's answer on size while reinstating the
/// very lines the filter was written to drop. MEASURED 2026-09-04 on a filter
/// that strips a runner's download and cache chatter: the delivered answer was
/// the generic fold of the raw output — 6,852 bytes with every `Downloading …`
/// and `[cache] …` line back in — against the filter's own 17,958 with none.
#[test]
fn e2e_a_user_filter_is_not_overruled_by_a_fold_of_the_raw_bytes() {
    let h = TempHome::new("user-filter-instruction");
    let home = h.path();
    let toml = home.join("flakeci.toml");
    std::fs::write(
        &toml,
        "name = \"flakeci\"\n\
         match_command = \"^flakeci\\\\s+run\"\n\
         strip_lines_matching = [ \"^\\\\[cache\\\\] \", \"^Downloading \" ]\n\
         replace = [ { from = \"^PASS \\\\((\\\\d+)ms\\\\) \", to = \"ok \" } ]\n",
    )
    .unwrap();
    let (out, err, code) = run(&["add-compactor", "--file", toml.to_str().unwrap()], home);
    assert_eq!(code, 0, "{out}{err}");

    let mut raw = String::new();
    for s in 0..30 {
        raw.push_str(&format!(
            "Downloading suite-{s} fixtures from registry.test ...\n"
        ));
        raw.push_str(&format!("[cache] hit 47 miss 3 for suite-{s}\n"));
        for t in 0..12 {
            if s == 7 && t == 4 {
                raw.push_str("FAIL suite_7::case_04 expected frames == 60, got 47\n");
            } else {
                raw.push_str(&format!(
                    "PASS ({}ms) suite_{s}::case_{t:02} verifies the widget contract\n",
                    10 + t
                ));
            }
        }
    }
    let (folded, _e, _c) = run_with_stdin(&["filter", "flakeci", "run"], home, &raw);

    for banned in ["Downloading ", "[cache] "] {
        assert!(
            !folded.contains(banned),
            "the filter was told to drop {banned:?} and the answer put it back:\n{}",
            &folded[..folded.len().min(300)]
        );
    }
    assert!(
        folded.contains("FAIL suite_7::case_04"),
        "and the failure has to survive:\n{folded}"
    );
    // The post-passes still do their work ON the filter's answer: 360 lines of
    // `ok …` collapse to their shape.
    assert!(
        folded.len() < raw.len() / 20,
        "the repetition fold must still apply: {} bytes of {}",
        folded.len(),
        raw.len()
    );
}

/// The `--budget` note must not contradict the answer it is attached to.
/// It used to end "the one target you asked for is always shown in full",
/// printed on stderr one line under a stdout footer reading "truncated at
/// ~2500 tokens; +8 more symbol(s) hidden". Two different bounds — `--budget`
/// chooses WHICH targets are shown, the per-file skeleton ceiling bounds how
/// much of one — and the message claimed the one it does not control. Both
/// halves are asserted here, because a note that is honest on a small file
/// and false on a large one passes any test that only looks at one size.
#[test]
fn e2e_a_budget_note_does_not_contradict_its_own_output() {
    let h = TempHome::new("skeleton-budget-truth");
    let mk = |name: &str, fns: usize| {
        let p = h.dir.join(name);
        let mut src = String::new();
        for i in 0..fns {
            src.push_str(&format!(
                "pub fn handler_number_{i}(request: u32, retries: u32) -> Result<u32, String> \
                 {{\n    let x = request + {i};\n    Ok(x * 2 + retries)\n}}\n\n"
            ));
        }
        std::fs::write(&p, src).unwrap();
        p.to_string_lossy().into_owned()
    };
    const CUT: &str = "tokenade skeleton — truncated";

    // Big enough that the per-file ceiling cuts it.
    let big = mk("big.rs", 700);
    let (out, err, code) = run(&["skeleton", &big, "--budget", "20"], &h.dir);
    assert_eq!(code, 0, "skeleton failed: {err}");
    assert!(
        out.contains(CUT),
        "fixture must actually hit the per-file ceiling, else this guard \
         asserts nothing:\n{}",
        &out[out.len().saturating_sub(300)..]
    );
    assert!(
        !err.contains("shown in full"),
        "stderr claims the file is shown in full while stdout says it was cut:\n{err}"
    );
    assert!(
        err.contains("per-file skeleton ceiling"),
        "the note must name the bound that DID bind:\n{err}"
    );

    // Small enough to be shown whole: then, and only then, may the note say
    // there is nothing to withhold.
    let small = mk("small.rs", 6);
    let (out2, err2, _c) = run(&["skeleton", &small, "--budget", "20"], &h.dir);
    assert!(
        !out2.contains(CUT),
        "small fixture must not be cut:\n{out2}"
    );
    assert!(
        err2.contains("nothing to withhold"),
        "an uncut single target should say so:\n{err2}"
    );
    assert!(
        !err2.contains("per-file skeleton ceiling"),
        "nothing was cut; do not name a bound that did not bind:\n{err2}"
    );
}

/// The fold banner must not send the caller to a command that will refuse.
///
/// It advertised `tokenade expand-ref <hash>` — and `--raw` — as the way to the
/// complete output, for blobs the expand gate refuses above
/// `threshold_tokens()`. Measured 2026-09-06: a fold of 1 225 395 tokens
/// pointed at a bare expand whose whole answer was 333 bytes of "too large to
/// dump whole, ask instead". The bytes and their size were both in hand when
/// the banner was written, so that round trip bought nothing.
///
/// Three arms, and the third is the one that cannot rot: what the banner says
/// in advance must be what the command itself says.
#[test]
fn e2e_the_fold_banner_says_up_front_when_a_bare_expand_would_be_refused() {
    let home = TempHome::new("expand-advice");

    // Big enough that the stash is over the expand gate. Unique content: the
    // dedup is content-addressed and a reused fixture answers from cache.
    let big = home.path().join("big.log");
    let mut s = String::new();
    for i in 0..40_000u64 {
        s.push_str(&format!(
            "2026-09-06 w-{} req {} took {}ms tag={}\n",
            i % 9,
            900_000 + i,
            3 + i % 40,
            (i * 2_654_435_761) % 1_000_000_007
        ));
    }
    std::fs::write(&big, &s).unwrap();

    let (out, _err, code) = run_with_env(
        &["wrap", &format!("cat {}", big.to_string_lossy())],
        home.path(),
        &[("TOKENADE_FORCE_GAIN", "1")],
    );
    assert_eq!(code, 0, "wrap must succeed");
    let banner = out
        .lines()
        .rev()
        .find(|l| l.contains("tokens folded away"))
        .expect("a fold banner")
        .to_string();

    assert!(
        banner.contains("too large to dump whole"),
        "the banner must say so BEFORE the caller spends a turn: {banner}"
    );
    for verb in ["--grep", "--prompt"] {
        assert!(
            banner.contains(verb),
            "and name the narrowing verb {verb}: {banner}"
        );
    }

    // The hash it names must be the one whose bare expand refuses, and the
    // refusal must be the SAME sentence — a banner that predicts something the
    // command does not do is worse than no prediction.
    let hash = banner
        .split("expand-ref ")
        .nth(1)
        .and_then(|r| r.split_whitespace().next())
        .expect("a hash in the banner")
        .to_string();
    let (dump, _e, code) = run(&["expand-ref", &hash], home.path());
    assert_eq!(code, 0, "the bare expand still answers");
    assert!(
        dump.contains("too large to dump whole"),
        "the banner predicted a refusal, so one must happen: {dump:.300}"
    );
    let advice = "too large to dump whole";
    let from_banner = &banner[banner.find(advice).unwrap()..];
    let from_dump = &dump[dump.find(advice).unwrap()..];
    let shared = from_banner
        .chars()
        .zip(from_dump.chars())
        .take_while(|(a, b)| a == b)
        .count();
    assert!(
        shared > 120,
        "banner and refusal must be one spelling, diverged after {shared} chars:\n{from_banner:.200}\n{from_dump:.200}"
    );

    // A MID-SIZE blob is where the two ceilings could disagree: over the
    // expand gate, far under the `--raw` budget twelve times higher. If the
    // banner asked the wrong one it would stay silent here and the command
    // would still refuse — the exact round trip this exists to remove.
    let mid = home.path().join("mid.log");
    let mut m = String::new();
    // 1 600 lines measured at ~40 000 tokens: comfortably over the expand gate
    // (5 000) and comfortably under twelve times it. A fixture outside that
    // window cannot tell the two ceilings apart, and a first draft at 4 000
    // lines (~101 000 tokens) could not.
    for i in 0..1_600u64 {
        m.push_str(&format!(
            "2026-09-06 mid-{} req {} took {}ms tag={}\n",
            i % 7,
            500_000 + i,
            2 + i % 30,
            (i * 1_103_515_245) % 1_000_000_007
        ));
    }
    std::fs::write(&mid, &m).unwrap();
    let (out, _err, code) = run_with_env(
        &["wrap", &format!("cat {}", mid.to_string_lossy())],
        home.path(),
        &[("TOKENADE_FORCE_GAIN", "1")],
    );
    assert_eq!(code, 0, "wrap must succeed");
    let mid_banner = out
        .lines()
        .rev()
        .find(|l| l.contains("tokens folded away"))
        .expect("a fold banner for the mid-size blob")
        .to_string();
    let mid_hash = mid_banner
        .split("expand-ref ")
        .nth(1)
        .and_then(|r| r.split_whitespace().next())
        .expect("a hash")
        .to_string();
    let (mid_dump, _e, _c) = run(&["expand-ref", &mid_hash], home.path());
    assert_eq!(
        mid_banner.contains("too large to dump whole"),
        mid_dump.contains("too large to dump whole"),
        "the banner must predict what the command does, at every size:\nbanner {mid_banner:.200}\ndump {mid_dump:.200}"
    );

    // Anti-vacuity: a blob the gate would HAND OVER keeps the old, shorter
    // offer. A fix that simply always warned would pass everything above.
    let small = home.path().join("small.log");
    let mut t = String::new();
    for i in 0..400u64 {
        t.push_str(&format!("r{}={}\n", i, (i * 7_919) % 1_000_000_007));
    }
    std::fs::write(&small, &t).unwrap();
    let (out, _err, code) = run_with_env(
        &["wrap", &format!("cat {}", small.to_string_lossy())],
        home.path(),
        &[("TOKENADE_FORCE_GAIN", "1")],
    );
    assert_eq!(code, 0, "wrap must succeed");
    if let Some(banner) = out.lines().rev().find(|l| l.contains("tokens folded away")) {
        assert!(
            !banner.contains("too large to dump whole"),
            "a blob under the gate is dumpable and must be offered as such: {banner}"
        );
    }
}

/// **`filter --deliver` delivers BOTH stages, like `wrap`.**
///
/// `filter` exists for ALREADY captured output, and it applied only the
/// first stage: the command's compactor. What `wrap` does next —
/// stashing what remains too big, then pointing to what the fold
/// threw away — was out of reach of a capture. MEASURED on the battery's 265
/// captures: `filter` returns **301 618 bytes**, `filter --deliver` returns
/// **240 776**, i.e. 60 842 fewer (−20 %).
/// **The repeated sentence folds on the DELIVERY path.**
///
/// MEASURED 2026-09-08 on the corpus's 265 captures by
/// `residue_census --delivered`: **237 180 → 227 965 bytes**. `mount` goes
/// from 7 787 to 6 602, `tasklist` from 11 274 to 9 095, `ps aux` from 7 000 to 5 579.
/// This test holds the WIRING: the fold had existed for months and was only called
/// by `npm.rs`, and a unit test on the function would not have seen it.
/// **Column decoration also goes when output is routed by its
/// command NAME.**
///
/// `collapse_decorative_segments` had only one caller, the `auto` path, which
/// decides by CONTENT. MEASURED 2026-09-09 on the 265 captures: the delivered
/// corpus goes from 221 784 to 207 724 bytes once this fold is reachable from both
/// sides and the indented header stops fabricating an empty column.
#[test]
fn e2e_column_padding_goes_on_the_command_path_too() {
    let h = TempHome::new("padding-deliver");
    // Header NOT indented, on the left. My first fixture opened with
    // `  PID  PPID …` — the shape of `ps` — and the detector still refuses it:
    // the header's indentation fabricates a phantom column, empty on every
    // data line. It is a characterised defect NOT fixed here (it
    // made `filter_command` non-idempotent on `netstat`), so the fixture
    // must not assume it settled.
    let mut payload = String::from("PID    PPID   USER             COMM\n");
    for i in 0..40 {
        payload.push_str(&format!(
            "{:<6} {:<6} root             /usr/libexec/service{i}\n",
            i + 1,
            i / 3
        ));
    }
    let (delivered, _e, code) = run_with_stdin(&["filter", "--deliver", "ps"], h.path(), &payload);
    assert_eq!(code, 0);
    assert!(
        delivered.len() < payload.len(),
        "the decoration stayed: {} → {}",
        payload.len(),
        delivered.len()
    );
    // Nothing disappears: the values are all there.
    for v in ["PPID", "/usr/libexec/service39", "40"] {
        assert!(delivered.contains(v), "`{v}` disappeared:\n{delivered}");
    }
}

#[test]
fn e2e_a_repeated_sentence_is_folded_on_the_delivery_path() {
    let h = TempHome::new("template-deliver");
    let payload: String = (0..15)
        .map(|i| {
            format!("/var/lib/snapd/snaps/pkg{i}_5.snap on /var/lib/snapd/snap/pkg{i}/5 type squashfs (ro,nodev,relatime,errors=continue,threads=single)\n")
        })
        .collect();
    let (plain, _e, code) = run_with_stdin(&["filter", "mount"], h.path(), &payload);
    assert_eq!(code, 0);
    let (delivered, _e, code) =
        run_with_stdin(&["filter", "--deliver", "mount"], h.path(), &payload);
    assert_eq!(code, 0);
    assert!(
        delivered.contains("(×15)"),
        "the repeated sentence was not folded on the delivered path:\n{delivered}"
    );
    assert!(
        delivered.len() < plain.len(),
        "the delivered output is not shorter"
    );
    // And no value disappears: that is what separates this fold from the
    // post-pass that keeps a count. The common root is stated ONCE in
    // the template and the values carry the rest — same rule as path
    // listing, where a full path re-forms by gluing the two back together.
    assert!(
        delivered.contains("/var/lib/snapd/snap/{}"),
        "the common root was not lifted into the template:\n{delivered}"
    );
    for i in 0..15 {
        assert!(
            delivered.contains(&format!("pkg{i}/5")),
            "value {i} disappeared"
        );
    }
    // The root is no longer copied under each line.
    assert_eq!(
        delivered.matches("/var/lib/snapd/snap/").count(),
        1,
        "the root is still repeated:\n{delivered}"
    );
}

#[test]
fn e2e_filter_deliver_runs_the_second_stage_too() {
    let h = TempHome::new("filter-deliver");
    // An output that no per-command rule folds and that the second stage
    // stashes: all-different lines, well above the
    // stash threshold.
    // An output the compactors LEAVE: very wide lines, all
    // different, with no recognisable shape — it is the case `filter`'s help
    // cites itself (`cat /proc/interrupts`, 81 very wide lines,
    // `filter` 0 %, `wrap` 83 %).
    let payload: String = (0..600)
        .map(|i| {
            let cells: String = (0..14)
                .map(|c| format!(" {:>9}", (i * 37 + c * 911) % 99_991))
                .collect();
            format!("CPU{i:03}{cells}   IR-PCI-MSI {i}-edge xhci_hcd_{i}\n")
        })
        .collect();
    let (plain, _e, code) = run_with_stdin(&["filter", "cat", "x.log"], h.path(), &payload);
    assert_eq!(code, 0, "plain filter must succeed");
    let (delivered, _e, code) =
        run_with_stdin(&["filter", "--deliver", "cat", "x.log"], h.path(), &payload);
    assert_eq!(code, 0, "filter --deliver must succeed");
    assert_ne!(delivered, plain, "the second stage did not run");
    // What the second stage ADDS here is a pointer to what the fold
    // threw away — on this input it stashes nothing, and that is the case that
    // cost two iterations: the two stages answer each other, one can cut
    // where the other adds. What is checkable on both sides is that
    // the stage RUNS and that it ANNOUNCES itself.
    assert!(
        delivered.contains("[tokenade") || delivered.contains("[disclose:"),
        "the second stage does not announce itself: {}",
        &delivered[delivered.len().saturating_sub(400)..]
    );
    // And the recovery it announces is reachable: a pointer that
    // leads nowhere is worse than no pointer.
    assert!(
        delivered.contains("expand-ref") || delivered.contains("stash"),
        "the announcement names no way back: {}",
        &delivered[delivered.len().saturating_sub(400)..]
    );
    // Without the flag, nothing changes for whoever already used `filter` in a
    // pipe: the flag is a REQUEST, not a new default.
    let (again, _e, _) = run_with_stdin(&["filter", "cat", "x.log"], h.path(), &payload);
    assert_eq!(plain, again, "plain filter must stay what it was");
    // And the flag is NOT part of the command whose output is folded.
    // Left in the argv, it changes what routing sees and what the ledger
    // names — `cat --deliver x.log` is not a command anyone
    // ran.
    let ledger = std::fs::read_to_string(h.path().join(".tokenade").join("gain.jsonl"))
        .expect("the ledger is written");
    assert!(
        !ledger.contains("--deliver"),
        "the flag leaked into the folded command: {ledger}"
    );
    assert!(
        ledger.contains("cat x.log"),
        "the folded command must be named: {ledger}"
    );
}

/// **A single chaining of the two stages for the COMMAND path.**
///
/// `wrap` runs the command, `filter --deliver` receives a capture, and both
/// deliver the same thing: that is true because they call the same block. Two
/// compositions would drift where the delivered output does not forgive it — once
/// 3 569 bytes were folded at the first stage and the delivered output GREW from
/// 16 122 to 20 397, because the two stages answer each other.
#[test]
fn e2e_the_command_path_has_a_single_second_stage() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    // The module's FAMILY, not the file: `second_stage` moved down into
    // `wrap/second_stage.rs`, and reading the trunk alone would have made this
    // guard say the function no longer exists.
    let read = |name: &str| {
        let src = tokenade_core::source_guard::module_family(&root, name);
        // By the ONE declarer: cutting at the first `#[cfg(test)]` made
        // this guard judge the part of the file before its tests, and
        // 14 % of the repo's production code was measured invisible that way.
        tokenade_core::srcsweep::production_lines(&src)
            .into_iter()
            .map(|(_, l)| l)
            .collect::<Vec<_>>()
            .join("\n")
    };
    let wrap = read("wrap");
    // In `wrap.rs`, stashing is called only once, and it is
    // in `second_stage`. The FAMILY (`maybe_stash`, `…_with_threshold`):
    // the threshold of a code read has been chosen there for a long time.
    assert_eq!(
        wrap.matches("disclose::maybe_stash(").count()
            + wrap
                .matches("disclose::maybe_stash_with_threshold(")
                .count(),
        1,
        "wrap.rs composes the stashing in more than one place"
    );
    let after = wrap
        .split_once("pub(crate) fn second_stage(")
        .expect("second_stage existe")
        .1;
    let body = after.split("\n}\n").next().unwrap_or(after);
    assert!(
        (body.contains("disclose::maybe_stash(")
            || body.contains("disclose::maybe_stash_with_threshold("))
            && body.contains("attach_drop_recovery("),
        "second_stage no longer carries both halves of the stage"
    );
    // And `filter` goes through it rather than recomposing. `main.rs` is read WHOLE:
    // it declares its test modules at the top, so cutting at the first
    // `#[cfg(test)]` leaves zero lines of code — the guard found nothing any more
    // and said so, which is the only reason we know it.
    // `cmd_filter` moved down from `main.rs` into `commands/filter_cmd.rs`:
    // filtering an output we are GIVEN is not dispatching. The file
    // is found, not named.
    let main = tokenade_core::source_guard::the_one_source_under(&root, "fn cmd_filter(").1;
    let cmd = main
        .split_once("fn cmd_filter(")
        .expect("cmd_filter existe")
        .1
        .split("\nfn ")
        .next()
        .unwrap_or_default()
        .to_string();
    assert!(
        cmd.contains("wrap::second_stage("),
        "cmd_filter does not go through the shared block"
    );
    assert!(
        !cmd.contains("maybe_stash(") && !cmd.contains("attach_drop_recovery("),
        "cmd_filter recomposes the stage instead of calling it"
    );
}

/// **The regular fact, stated once, on the capture that led to finding it.**
///
/// `pacman -Q`: 110 markers for 4 173 of the 8 296 bytes the first stage
/// returns — half — and 96 of them announce the same twenty-line gap.
/// That is not 96 facts, it is one. Delivered, the capture goes from **8 726 to 5 136
/// bytes**.
///
/// Since 2026-10-04 the step is 19 out of 108 gaps: six packages
/// (`libgpg-error`, `perl-error`, `python-pytest-timeout`…) were fished out
/// as FAILURES and broke the regularity; a word inside a name is no
/// longer one.
///
/// The fixture comes from `tokenade-core` because that is where the battery lives;
/// no synthetic output reproduces this sampling, and a test that
/// does not have the real input says nothing about the real path.
#[test]
fn e2e_a_regular_sampling_announces_its_rule_once() {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tokenade-core/tests/fixtures/linux/pacman_q.txt");
    let body = std::fs::read_to_string(&fixture)
        .unwrap_or_else(|e| panic!("the capture {} is readable: {e}", fixture.display()));
    let h = TempHome::new("uniform-banner");
    let (out, _e, code) = run_with_stdin(&["filter", "--deliver", "pacman", "-Q"], h.path(), &body);
    assert_eq!(code, 0, "exit 0 attendu");
    // The rule is stated once, with the total of the gaps IT REPLACES.
    assert!(
        out.contains("in 108 runs of 19"),
        "the regular rule is not stated: {}",
        &out[..out.len().min(400)]
    );
    // And the gaps that deviate from it keep their marker: the reader knows
    // « one line in 21, except where it is marked ».
    let markers = out.matches("lines elided by tokenade").count();
    assert!(
        (2..40).contains(&markers),
        "110 markers before, a handful of deviations after — saw {markers}"
    );
    assert_eq!(
        out.matches("… [19 lines elided by tokenade] …").count(),
        0,
        "the dominant gap is still restated"
    );
    // What the view REPRESENTS stays readable: without it, it loses its summary and
    // its recovery pointer at the next stage.
    let banner = out
        .lines()
        .find(|l| l.contains("runs of 19"))
        .expect("the banner is there");
    assert_eq!(tokenade_core::elision::weight_of(banner), Some(2_052));
}

/// **A marker nobody folded is CONTENT.**
///
/// A log that QUOTES our elision marker — a saved transcript, a
/// CI log of a tokenade session, this repo's documentation — saw its
/// lines disappear and the output claim a fold that had not happened.
/// MEASURED 2026-09-08 on 1 139 bytes quoting fourteen times
/// `… [20 lines elided by tokenade] …`: the fourteen lines were
/// deleted and replaced by « 280 lines elided by tokenade », in a
/// process that had folded NONE — and the next stage's reader
/// reads that 280 back.
#[test]
fn e2e_a_document_quoting_our_marker_keeps_its_lines() {
    let h = TempHome::new("forged-markers");
    let path = h.path().join("journal.log");
    let body: String = (0..14)
        .map(|i| {
            format!(
                "2026-09-08 agent a lu le fichier numero {i}\n… [20 lines elided by tokenade] …\n"
            )
        })
        .collect();
    std::fs::write(&path, &body).unwrap();
    let (out, _e, code) = run(&["wrap", &format!("cat {}", path.display())], h.path());
    assert_eq!(code, 0, "exit 0 attendu — {out}");
    // The user's fourteen lines are ALL counted. They are
    // identical, so the repeated-sentence fold states them once with its
    // count — which is lossless and reads back: fourteen lines, not one.
    assert!(
        out.contains("… [20 lines elided by tokenade] …  (×14)"),
        "the markers quoted by the document are no longer counted:\n{out}"
    );
    // And no fold is CLAIMED on them: this process folded none
    // of the twenty lines this document says are missing.
    assert!(
        !out.contains("runs of"),
        "a fold is claimed on markers we did not write:\n{out}"
    );
    assert!(
        out.contains("agent a lu le fichier numero {}") && out.contains("\n  13"),
        "the content must survive:\n{out}"
    );
}

/// **A range already printed is not reprinted — and only that.**
///
/// `sed -n '30,90p'` after `sed -n '1,120p'` of the same unchanged file, same
/// session: a pointer (the `Read` path did it, `sed` did not).
/// A range that OVERFLOWS, another session, a modified file: the lines.
#[test]
fn e2e_a_sed_range_already_printed_is_a_pointer() {
    let h = TempHome::new("sed-range-dedup");
    let f = h.path().join("src.rs");
    // A real source file — read verbatim, as `sed` reads are (a fixture of
    // alike lines would be template-folded and never registered).
    let body = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tk-fold/src/compress.rs"),
    )
    .expect("a source file to read");
    let line = |n: usize| body.lines().nth(n - 1).unwrap().to_string();
    std::fs::write(&f, &body).unwrap();
    let p = f.display().to_string();
    let read = |range: &str, sess: &str| {
        run_with_session(&["wrap", &format!("sed -n '{range}p' {p}")], h.path(), sess).0
    };
    let first = read("1,120", "s1");
    assert!(first.contains(&line(110)), "{first}");
    let inside = read("30,90", "s1");
    assert!(
        inside.contains("[tokenade:read-range-dedup unchanged]"),
        "{inside}"
    );
    assert!(inside.len() < 400, "{inside}");
    let beyond = read("100,180", "s1");
    assert!(
        beyond.contains(&line(170)),
        "a range that goes past what was read is served:\n{beyond}"
    );
    let other = read("30,90", "s2");
    assert!(
        !other.contains("read-range-dedup"),
        "another session never saw it:\n{other}"
    );
    std::fs::write(&f, format!("{body}// edited\n")).unwrap();
    let edited = read("30,90", "s1");
    assert!(
        !edited.contains("read-range-dedup"),
        "an edited file is read again:\n{edited}"
    );
}

/// **A `Read` also covers a `sed`, and its slice is the RIGHT one.**
///
/// `Read`'s `offset` is a 1-based line number; the recorded
/// slice skipped `offset` lines — shifted by one, so that
/// `expand-ref` returned under « lines 1–40 » lines 2 to 41. And a `sed`
/// read inside a `Read` went out whole again (598 in October).
#[test]
fn e2e_a_read_covers_a_later_sed_and_keeps_its_lines() {
    let h = TempHome::new("read-covers-sed");
    let f = h.path().join("src.rs");
    let body = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tk-fold/src/compress.rs"),
    )
    .expect("a source file to read");
    std::fs::write(&f, &body).unwrap();
    let read = |offset: u64, limit: u64| {
        let payload = format!(
            r#"{{"hook_event_name":"PreToolUse","session_id":"xr","transcript_path":"/tmp/xr.jsonl","tool_name":"Read","tool_input":{{"file_path":{:?},"offset":{offset},"limit":{limit}}}}}"#,
            f.to_str().unwrap()
        );
        run_hook(&["hookwrap-pretool"], h.path(), &payload).0
    };
    let _ = read(1, 200);
    let again = read(40, 100);
    assert!(
        again.contains("read-range-dedup"),
        "a Read inside a Read is a pointer:\n{again}"
    );
    assert!(again.contains("lines 40–139"), "{again}");
    let at = again.find("expand-ref ").expect("a recovery command");
    let hash: String = again[at + 11..]
        .chars()
        .take_while(char::is_ascii_hexdigit)
        .collect();
    let (recovered, _e, _r) = run_bin(&binary(), &["expand-ref", &hash, "--raw"], h.path());
    assert!(
        recovered
            .trim_start()
            .starts_with(body.lines().next().unwrap().trim()),
        "the stored slice starts at line 1:\n{}",
        &recovered[..recovered.len().min(300)]
    );
    let sed = run_with_session(
        &["wrap", &format!("sed -n '50,150p' {}", f.display())],
        h.path(),
        "xr",
    )
    .0;
    assert!(
        sed.contains("[tokenade:read-range-dedup unchanged]"),
        "a sed inside a Read is a pointer:\n{sed}"
    );
}

/// A read of SOURCE is never rewritten into templates: the template fold
/// gathers lines by shape, so `let repo = a_hand_built_repo("…");` from forty
/// test functions became one `(×40)` line plus values — code out of order,
/// impossible to copy back. The same bytes read as a log still fold.
#[test]
fn a_source_read_is_never_template_folded() {
    let home = TempHome::new("src-no-template");
    let body: String = (0..40)
        .map(|i| {
            format!(
                "#[test]\nfn case_{i}() {{\n    let repo = a_hand_built_repo(\"repo-{i}\");\n    let ctx = ctx_for(&repo, \"session-{i}\");\n    assert!(ctx.ready(), \"case {i}\");\n}}\n\n"
            )
        })
        .collect();
    let read = |name: &str| -> String {
        let f = home.path().join(name);
        std::fs::write(&f, &body).unwrap();
        let mut c = Command::new(binary());
        c.args(["wrap", &format!("sed -n '1,400p' {}", f.display())])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let out = e2e_env(&mut c, home.path())
            .output()
            .expect("spawn tokenade");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let code = read("cases.rs");
    assert!(
        !code.contains("(×"),
        "source rewritten into templates:\n{code}"
    );
    assert!(
        code.contains("fn case_17() {\n    let repo = a_hand_built_repo(\"repo-17\");"),
        "{code}"
    );
    let log = read("cases.log");
    assert!(
        log.contains("(×"),
        "the control (a non-source read) must still fold:\n{log}"
    );
}
