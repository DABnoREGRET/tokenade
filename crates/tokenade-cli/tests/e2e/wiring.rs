//! A helper that exists, compiles, and is never CALLED.
//!
//! Three findings turned on the same shape: `try_source_strip`,
//! `emit_host_cut_advisory` and the second call site of the
//! same could each be deleted with every unit guard still green,
//! because a unit guard exercises the helper directly and never asks whether
//! production reaches it. Another finding had to be written down with "this
//! is not pinned by a test" for the same reason.
//!
//! Rust does not catch it: `dead_code` is silent for anything `pub(crate)` or
//! above, and silent for anything a test module touches. So a call site can be
//! removed, the definition stays tidy, and the suite says nothing.
//!
//! This is the census that says something. For each entry it counts CALLS in
//! production code — occurrences of `name(` that are not the `fn name(`
//! definition and not inside a `#[cfg(test)]` module — and requires at least
//! the stated number.

use std::path::Path;

/// `(module, helper, minimum call sites)`.
///
/// A MODULE, not a path. The column carried
/// `crates/tokenade-core/src/json.rs` until 2026-09-13, and that day
/// `json` moved into `tk-formats`: the guard PANICKED on a file it could
/// not find. It is the shape the project's notes call « a guard that
/// names its subject by a PATH accuses the product », and it is the fifth
/// time it bites here. `source_guard::workspace_module_root` finds the
/// crate, and checks that only one carries it.
///
/// Naming the TOP-LEVEL module also covers the children:
/// `gemini_hook_command` lives in `install/gemini_hooks.rs` and
/// `bash_seen_marker` in `wrap/dedup.rs` — the family contains them, and a
/// split that moves one of them from one file to another no longer breaks anything.
///
/// Two is not decoration: `emit_host_cut_advisory` serves both terminal exits
/// of the Bash PostToolUse handler, and the first version of its guard pinned
/// only one — the other could be deleted with the suite green.
const WIRED: &[(&str, &str, usize)] = &[
    ("compactors", "try_source_strip", 1),
    ("hookwrap", "emit_host_cut_advisory", 2),
    ("install", "gemini_hook_command", 2),
    ("wrap", "bash_seen_marker", 2),
    ("code_strip", "strip_for_delivery", 0),
    ("web_fetch", "retry_is_worth_it", 1),
    ("json", "telemetry_key_holding_a_value", 1),
    ("agent_caps", "host_cut_advisory", 0),
];

/// Remove every `#[cfg(test)]` item, braces balanced, so a test module's use of
/// a helper never counts as production wiring.
/// The half of a file that ships, from the ONE declarer.
///
/// This guard used to carry its own stripper, counting braces raw. Its note
/// argued that a `{` inside a Rust string is nearly always a balanced
/// `format!` placeholder — `json.rs` disproves it, and the raw counter
/// swallowed everything after that file's first test module. Measured
/// 2026-09-01, the shared sweep had the same hole and hid 24 shipped items in
/// `license.rs`; both are fixed in `tokenade_core::srcsweep`.
fn strip_test_modules(src: &str) -> String {
    tokenade_core::srcsweep::production_source(src)
}
fn call_sites(src: &str, name: &str) -> usize {
    let needle = format!("{name}(");
    let mut n = 0;
    let mut from = 0;
    while let Some(i) = src[from..].find(&needle) {
        let at = from + i;
        let before = src[..at].trim_end();
        let is_definition = before.ends_with("fn");
        // `foo.bar(` and `Type::name(` are calls; `fn name(` is not.
        let boundary_ok = src[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric() && c != '_');
        if !is_definition && boundary_ok {
            n += 1;
        }
        from = at + needle.len();
    }
    n
}

#[test]
fn every_wired_helper_is_actually_called() {
    let mut failures = Vec::new();
    for (file, name, min) in WIRED {
        if *min == 0 {
            continue; // declared elsewhere; see the cross-file pass below
        }
        // The module AND its children. The column names a MODULE, not a
        // file: `try_source_strip` moved down from `compactors.rs` into
        // `compactors/snapshot.rs` with its caller, and this guard announced
        // « 0 call site(s) » — a perfectly wired helper, declared orphaned
        // because the sweep looked at the trunk and not the branch. It is the
        // fourth time this shape bites; this file's header already
        // counts three.
        let src = tokenade_core::source_guard::workspace_module_family(file);
        let n = call_sites(&strip_test_modules(&src), name);
        if n < *min {
            failures.push(format!(
                "{file}: `{name}` has {n} call site(s), needs {min}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "orphaned helpers:\n{}",
        failures.join("\n")
    );
}

/// The cross-file half: a helper DECLARED in one crate and called from another.
/// `min: 0` above means "not in its own file" — it still has to be reached.
#[test]
fn a_cross_crate_helper_is_reached_from_somewhere() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf();
    let mut sources = Vec::new();
    let mut stack = vec![root.join("crates")];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            let nm = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if nm == "target" || nm == "tests" || nm.starts_with('.') {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().unwrap_or_default() == "rs" {
                if let Ok(s) = std::fs::read_to_string(&p) {
                    sources.push((p, strip_test_modules(&s)));
                }
            }
        }
    }
    let mut failures = Vec::new();
    for (file, name, min) in WIRED {
        if *min != 0 {
            continue;
        }
        let declared = tokenade_core::source_guard::workspace_module_trunk(file);
        let n: usize = sources
            .iter()
            .filter(|(p, _)| *p != declared)
            .map(|(_, s)| call_sites(s, name))
            .sum();
        if n == 0 {
            failures.push(format!(
                "{file}: `{name}` is called from nowhere in production"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "orphaned helpers:\n{}",
        failures.join("\n")
    );
}

/// Every place that tells an agent batching is "the CHEAPEST option in tokens
/// spent" must say WHY, because measured on output tokens alone it is not:
/// three `--prompt` questions asked separately against a 15,167-token document
/// return 3,687 tokens and the same three asked in one call return 3,667 —
/// half a percent. The saving is the round-trip and the context not re-sent
/// once per question, which six of the nine sites said and three did not,
/// including both halves of the CLAUDE.md tokenade scaffolds into a project.
/// A bare superlative about tokens is a claim the measurement does not support.
#[test]
fn every_cheapest_claim_says_why_it_is_cheapest() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/");
    let mut checked = 0usize;
    let mut bare: Vec<String> = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("readable dir").flatten() {
            let path = entry.path();
            if path.is_dir() {
                // `tests/` is not shipped prose; `target/` is not source.
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                if name != "target" && name != "tests" {
                    stack.push(path);
                }
            } else if path.extension().is_some_and(|e| e == "rs") {
                let src = std::fs::read_to_string(&path).unwrap_or_default();
                let lines: Vec<&str> = src.lines().collect();
                let production: std::collections::HashSet<usize> =
                    tokenade_core::srcsweep::production_lines(&src)
                        .into_iter()
                        .map(|(n, _)| n)
                        .collect();
                for (i, line) in lines.iter().enumerate() {
                    if !line.contains("CHEAPEST") || !production.contains(&i) {
                        continue;
                    }
                    checked += 1;
                    let lo = i.saturating_sub(3);
                    let hi = (i + 7).min(lines.len());
                    if !lines[lo..hi].iter().any(|l| l.contains("round-trip")) {
                        bare.push(format!("{}:{}", path.display(), i + 1));
                    }
                }
            }
        }
    }
    assert!(
        checked >= 8,
        "the sweep found only {checked} CHEAPEST claims — it is not reading the sources"
    );
    assert!(
        bare.is_empty(),
        "these claim `CHEAPEST` without naming the round-trip that makes it \
         true, which measures at half a percent on output tokens alone: {bare:?}"
    );
}

/// And the rule cannot be re-spelled locally: no shipped source may pair a
/// `write_file_600` with a `rename`. That pairing IS the dance, and the whole
/// point is that it lives in one place.
#[test]
fn no_source_spells_the_0600_dance_out_again() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/");
    let mut offenders: Vec<String> = Vec::new();
    let mut scanned = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).expect("readable").flatten() {
            let p = e.path();
            if p.is_dir() {
                let name = p
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                if name != "target" && name != "tests" {
                    stack.push(p);
                }
            } else if p.extension().is_some_and(|x| x == "rs") {
                let src = std::fs::read_to_string(&p).unwrap_or_default();
                let lines: Vec<&str> = src.lines().collect();
                let production: std::collections::HashSet<usize> =
                    tokenade_core::srcsweep::production_lines(&src)
                        .into_iter()
                        .map(|(n, _)| n)
                        .collect();
                for (i, l) in lines.iter().enumerate() {
                    if !l.contains("write_file_600(") || !production.contains(&i) {
                        continue;
                    }
                    scanned += 1;
                    // The declarer itself is allowed to rename.
                    if p.ends_with("platform.rs") {
                        continue;
                    }
                    let hi = (i + 4).min(lines.len());
                    if lines[i..hi].iter().any(|x| x.contains("rename")) {
                        offenders.push(format!("{}:{}", p.display(), i + 1));
                    }
                }
            }
        }
    }
    assert!(
        scanned >= 2, // the licence cache writer (third site) left in 2.0
        "the sweep found only {scanned} calls — it is not reading the sources"
    );
    assert!(
        offenders.is_empty(),
        "these spell the 0600 write-then-rename out again instead of calling \
         `platform::write_atomic_600`, and a locally chosen temp name is shared \
         by every process: {offenders:?}"
    );
}

/// No source applies the diff floor with a literal of its own.
///
/// Three diff paths had each written the win-ratio test out inline, and one of
/// them used 20 where the other two used 40 — so the same edit was answered
/// with a diff on a paginated read and with the whole file on an exact one,
/// while `main.rs` told the agent the floor was 40. The floor lives on
/// `MIN_DIFF_WIN_PCT`; a fourth path must call it rather than pick a number.
#[test]
fn no_source_picks_its_own_diff_floor() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/");
    let mut offenders: Vec<String> = Vec::new();
    let mut scanned = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).expect("readable").flatten() {
            let p = e.path();
            if p.is_dir() {
                let name = p
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                if name != "target" && name != "tests" {
                    stack.push(p);
                }
            } else if p.extension().is_some_and(|x| x == "rs") {
                let src = std::fs::read_to_string(&p).unwrap_or_default();
                let lines: Vec<&str> = src.lines().collect();
                let production: std::collections::HashSet<usize> =
                    tokenade_core::srcsweep::production_lines(&src)
                        .into_iter()
                        .map(|(n, _)| n)
                        .collect();
                for (i, l) in lines.iter().enumerate() {
                    if !l.contains("win_pct") || !production.contains(&i) {
                        continue;
                    }
                    scanned += 1;
                    // The declarer states the floor; everyone else asks it.
                    if l.contains("MIN_DIFF_WIN_PCT") {
                        continue;
                    }
                    // `win_pct >= <number>` — a floor chosen locally.
                    let cmp = l.split("win_pct").nth(1).unwrap_or("");
                    let cmp = cmp.trim_start();
                    if (cmp.starts_with(">=") || cmp.starts_with('>') || cmp.starts_with("<"))
                        && cmp.chars().any(|c| c.is_ascii_digit())
                    {
                        offenders.push(format!("{}:{}: {}", p.display(), i + 1, l.trim()));
                    }
                }
            }
        }
    }
    assert!(
        scanned >= 3,
        "the sweep found only {scanned} win_pct lines — it is not reading the sources"
    );
    assert!(
        offenders.is_empty(),
        "these compare a diff win ratio against a number of their own instead of \
         calling `diff_is_worth_serving`, which is how one path came to serve a \
         diff at 20% while the other two required 40%: {offenders:?}"
    );
}

/// No source decides for itself whether a stream needs a closing newline.
///
/// The two ends of `wrap` got this wrong in OPPOSITE directions while each
/// spelling the test out inline: the merged path terminated output it had
/// passed through (a byte the command never printed, under `> file`), and
/// `--split-streams` terminated nothing, so a folded stream lost the newline
/// the fold strips — 7 of 14 real commands, stdout and stderr alike. The rule
/// is `wrap::needs_terminator`, which takes "was this passed through?" as an
/// argument precisely because that is the half each site forgot.
#[test]
fn no_source_decides_its_own_stream_terminator() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/");
    let mut offenders: Vec<String> = Vec::new();
    let mut scanned = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).expect("readable").flatten() {
            let p = e.path();
            if p.is_dir() {
                let name = p
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                if name != "target" && name != "tests" {
                    stack.push(p);
                }
            } else if p.extension().is_some_and(|x| x == "rs") {
                let src = std::fs::read_to_string(&p).unwrap_or_default();
                let lines: Vec<&str> = src.lines().collect();
                let production: std::collections::HashSet<usize> =
                    tokenade_core::srcsweep::production_lines(&src)
                        .into_iter()
                        .map(|(n, _)| n)
                        .collect();
                for (i, l) in lines.iter().enumerate() {
                    if !l.contains("ends_with('\\n')") || !production.contains(&i) {
                        continue;
                    }
                    scanned += 1;
                    // The declarant states the rule; everyone else asks it.
                    if l.contains("fn needs_terminator") {
                        continue;
                    }
                    // What we are after is the shape that got it wrong twice: a
                    // line that decides to print a newline. Reading for one is
                    // not the same as deciding to add one.
                    let window = lines[i..(i + 4).min(lines.len())].join(" ");
                    if window.contains("println!()") || window.contains("eprintln!()") {
                        offenders.push(format!("{}:{}: {}", p.display(), i + 1, l.trim()));
                    }
                }
            }
        }
    }
    assert!(
        scanned >= 3,
        "the sweep found only {scanned} trailing-newline tests — it is not reading the sources"
    );
    assert!(
        offenders.is_empty(),
        "these decide for themselves whether to terminate a stream instead of \
         calling `wrap::needs_terminator`, which is how one end of `wrap` came to \
         terminate a passthrough while the other end terminated nothing: {offenders:?}"
    );
}

/// No source decides "this row is approximated" for itself.
///
/// Two ledger scanners walk the same rows, and only one applied the test — so
/// `tokenade gain` printed a total with no mention of the slice while the
/// signed receipt disclosed it. The rule is `coefficients::approximated_slice`;
/// pairing `credit_is_approximate` with a cap comparison by hand is how a
/// third scanner gets it subtly wrong (forgetting that a payload UNDER the cap
/// is delivered whole and its saving exact).
#[test]
fn no_source_restates_the_approximated_row_test() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/");
    let mut offenders: Vec<String> = Vec::new();
    let mut scanned = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).expect("readable").flatten() {
            let p = e.path();
            if p.is_dir() {
                let name = p
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                if name != "target" && name != "tests" {
                    stack.push(p);
                }
            } else if p.extension().is_some_and(|x| x == "rs") {
                let src = std::fs::read_to_string(&p).unwrap_or_default();
                let lines: Vec<&str> = src.lines().collect();
                let production: std::collections::HashSet<usize> =
                    tokenade_core::srcsweep::production_lines(&src)
                        .into_iter()
                        .map(|(n, _)| n)
                        .collect();
                for (i, l) in lines.iter().enumerate() {
                    if !l.contains("credit_is_approximate") || !production.contains(&i) {
                        continue;
                    }
                    scanned += 1;
                    // The declarer is where the pairing belongs — and the
                    // declarer is a MODULE, not a file: the rule moved
                    // down into `coefficients/channels.rs` on 2026-09-13,
                    // and an exemption named by path then accuses its
                    // own declarer.
                    let declarer = p.ends_with("coefficients.rs")
                        || p.parent()
                            .and_then(|d| d.file_name())
                            .is_some_and(|d| d == "coefficients");
                    if declarer {
                        continue;
                    }
                    let hi = (i + 6).min(lines.len());
                    if lines[i..hi]
                        .iter()
                        .any(|x| x.contains("delivery_cap_tokens"))
                    {
                        offenders.push(format!("{}:{}", p.display(), i + 1));
                    }
                }
            }
        }
    }
    assert!(
        scanned >= 2,
        "the sweep found only {scanned} uses — it is not reading the sources"
    );
    assert!(
        offenders.is_empty(),
        "these pair `credit_is_approximate` with a cap comparison of their own \
         instead of calling `coefficients::approximated_slice`: {offenders:?}"
    );
}

/// No source builds the credit-once key for itself.
///
/// Three ledger scanners had each written the tuple out inline, and they had
/// already drifted: two fell back to the legacy `filter` field when `op` was
/// absent and one keyed the empty string, so the same row had two identities
/// depending on which surface was asking. The key is
/// `gain::credit_key`; a fourth scanner must call it.
#[test]
fn no_source_builds_the_credit_key_inline() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/");
    let mut offenders: Vec<String> = Vec::new();
    let mut scanned = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).expect("readable").flatten() {
            let p = e.path();
            if p.is_dir() {
                let name = p
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                if name != "target" && name != "tests" {
                    stack.push(p);
                }
            } else if p.extension().is_some_and(|x| x == "rs") {
                let src = std::fs::read_to_string(&p).unwrap_or_default();
                let lines: Vec<&str> = src.lines().collect();
                let production: std::collections::HashSet<usize> =
                    tokenade_core::srcsweep::production_lines(&src)
                        .into_iter()
                        .map(|(n, _)| n)
                        .collect();
                for (i, l) in lines.iter().enumerate() {
                    if !l.contains("credited.insert(") || !production.contains(&i) {
                        continue;
                    }
                    scanned += 1;
                    // Two declarers, both legitimate: `credit_key` is the
                    // local identity, `server_dedup_key` the coarser one the
                    // server's ingest uses. What is refused is a THIRD key
                    // spelled out at the call site. This guard caught the
                    // metered credit set the day it was added, which is what
                    // an exhaustiveness sweep is for.
                    if l.contains("credit_key(") || l.contains("server_dedup_key(") {
                        continue;
                    }
                    offenders.push(format!("{}:{}: {}", p.display(), i + 1, l.trim()));
                }
            }
        }
    }
    assert!(
        scanned >= 3,
        "the sweep found only {scanned} credit sets — it is not reading the sources"
    );
    assert!(
        offenders.is_empty(),
        "these build a credit-once key themselves instead of calling \
         `gain::credit_key` or `gain::server_dedup_key`, which is how two of \
         them came to key `op` differently: {offenders:?}"
    );
}

/// Every store that keys rows by a project path drops test tempdirs.
///
/// #734 added that rule for the gain ledger and it stayed there for a year,
/// while two other stores keyed rows the same way and were left open: measured
/// 2026-09-03 on a real install, 85 of 180 rows in `index.db` (47%) and 38 of
/// 104 in `semantic.db` (37%) were throwaway tempdirs written by the test
/// suite, and `tokenade semantic-status` reported 52 projects where 14 existed.
/// A fourth store must consult `gain::is_test_cwd` too.
#[test]
fn every_project_keyed_store_drops_test_tempdirs() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/");
    let mut offenders: Vec<String> = Vec::new();
    let mut scanned = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).expect("readable").flatten() {
            let p = e.path();
            if p.is_dir() {
                let name = p
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                if name != "target" && name != "tests" {
                    stack.push(p);
                }
            } else if p.extension().is_some_and(|x| x == "rs") {
                let src = std::fs::read_to_string(&p).unwrap_or_default();
                let production: std::collections::HashSet<usize> =
                    tokenade_core::srcsweep::production_lines(&src)
                        .into_iter()
                        .map(|(n, _)| n)
                        .collect();
                let lines: Vec<&str> = src.lines().collect();
                for (i, l) in lines.iter().enumerate() {
                    // A write that keys a row on the project path.
                    if !l.contains("INSERT INTO") || !l.contains("project_path") {
                        continue;
                    }
                    if !production.contains(&i) {
                        continue;
                    }
                    scanned += 1;
                    // The guard belongs in the same function, before the write.
                    let lo = i.saturating_sub(30);
                    if !lines[lo..i]
                        .iter()
                        .any(|x| x.contains("write_would_leak_into_real_store"))
                    {
                        offenders.push(format!("{}:{}", p.display(), i + 1));
                    }
                }
            }
        }
    }
    assert!(
        scanned >= 2,
        "the sweep found only {scanned} project-keyed writes — it is not reading the sources"
    );
    assert!(
        offenders.is_empty(),
        "these write a row keyed by project path without consulting \
         `gain::write_would_leak_into_real_store`, so a harness that redirects \
         the cwd but not HOME \
         lands in the user's real store: {offenders:?}"
    );
}

/// **A single sentence says what `--raw` returns.**
///
/// It was written FIVE times — « for every byte » in the compactors'
/// banner, « for all bytes » twice in a page's fallback and once
/// in the refusal of a `--prompt`, « the whole stored blob » in both
/// branches of `too_large_advice` — and all five promised completeness
/// while `--raw` stops at a cap. Only the HELP told the truth, and the help
/// is a separate call the agent almost never makes.
///
/// Measured on 2026-09-09 on transcripts: 56 real calls to `expand-ref`,
/// of which 48 % piped into `head` and 19 % into `sed -n`. An agent that believes
/// `--raw` is unlimited defends itself in the shell, and its defence cuts in the wrong
/// place.
#[test]
fn no_source_promises_that_raw_returns_everything() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/");
    // The phrasings that promise completeness. The declarer uses none
    // of them: it says « bounded and resumable ».
    const PROMISES: [&str; 4] = [
        "for every byte",
        "for all bytes",
        "whole stored blob",
        "for the raw bytes",
    ];
    let mut offenders: Vec<String> = Vec::new();
    let mut scanned = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).expect("readable").flatten() {
            let p = e.path();
            if p.is_dir() {
                let name = p
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                if name != "target" && name != "tests" {
                    stack.push(p);
                }
            } else if p.extension().is_some_and(|x| x == "rs") {
                let src = std::fs::read_to_string(&p).unwrap_or_default();
                if !src.contains("--raw") {
                    continue;
                }
                scanned += 1;
                for (i, l) in tokenade_core::srcsweep::production_lines(&src) {
                    if line_is_only_a_comment(l) {
                        continue;
                    }
                    if PROMISES.iter().any(|s| l.contains(s)) {
                        offenders.push(format!("{}:{}", p.display(), i + 1));
                    }
                }
            }
        }
    }
    assert!(
        scanned >= 4,
        "the sweep only read {scanned} file(s) naming `--raw` — it does not read the sources"
    );
    assert!(
        offenders.is_empty(),
        "these lines promise that `--raw` returns EVERYTHING, which has been false since \
         it got a cap: go through `expand_ref::RAW_ADVICE`, which says \
         « bounded and resumable » : {offenders:?}"
    );
}

/// A comment is allowed to NARRATE the old sentence: that is how a
/// decision is documented, and an earlier sweep turned the tree red by biting its own
/// explanation. But ONLY a line that is NOTHING but a comment: a
/// line of code followed by a `// note` still carries the promise.
///
/// Extracted to be testable. In the sweep, the tree today carries
/// no offending line, so widening the skip changes nothing observable — the
/// mutation survived without any case being able to kill it.
fn line_is_only_a_comment(l: &str) -> bool {
    l.trim_start().starts_with("//")
}

#[test]
fn only_a_line_that_is_nothing_but_a_comment_is_skipped() {
    assert!(line_is_only_a_comment(
        "    // for every byte, historiquement"
    ));
    assert!(line_is_only_a_comment("/// the whole stored blob"));
    assert!(
        !line_is_only_a_comment("    \"--raw for all bytes\", // note"),
        "a line of code with a trailing comment still carries the promise"
    );
    assert!(!line_is_only_a_comment("    \"for every byte\""));
}
