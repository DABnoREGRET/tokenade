//! A platform module that is a COPY of its linux twin will drift, and the drift
//! is invisible until someone reads both files.
//!
//! Found by hand three times on 2026-08-04: `git_listing` (a path census that
//! restated the search root), `docker_table` (missing record-aware folding and
//! half the problem keywords), and four modules whose `HEAD` was still the
//! scaffolded 60 while linux had tuned values — `find` handed macOS a third of
//! the listing Linux returns.
//!
//! Each was a real user-facing difference on two of three platforms, and each
//! took a manual diff to notice. This test is that diff, run every time.
//!
//! It does NOT demand that platform modules be identical: `ls`, `netstat`, `df`
//! and `top` parse genuinely different output and share almost no lines. It
//! demands that a module which is ALREADY a near-copy stop being a second
//! implementation and delegate instead — because at that similarity the copy
//! buys nothing and costs a fix.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Lines that carry logic: no comments, no blank lines, no attributes.
fn logic_lines(src: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut in_tests = false;
    for l in src.lines() {
        let t = l.trim();
        if t.starts_with("#[cfg(test)]") {
            in_tests = true;
        }
        if in_tests || t.is_empty() || t.starts_with("//") || t.starts_with("#[") {
            continue;
        }
        out.insert(t.to_string());
    }
    out
}

/// Twins deliberately kept separate, with the reason they earn it.
const JUSTIFIED: &[(&str, &str)] = &[
    // `klist` and `date` name the same command on Windows as on Unix but print
    // something else entirely — Windows `date` is interactive-prompt shaped and
    // Windows `klist` uses different Kerberos wording. Delegating would hand
    // them a `detect` keyed on Unix strings, which would then never fire.
    //
    // They are NOT clean, though: both Windows copies still carry the
    // scaffolded placeholder detect (`l.trim().len() > 2`, which accepts almost
    // any output) where linux has a specific one. Writing the real predicate
    // needs actual Windows output, which is why this is a recorded gap and not
    // a guess.
    (
        "klist",
        "Windows Kerberos output differs; detect still a scaffold placeholder",
    ),
    (
        "date",
        "Windows date output differs; detect still a scaffold placeholder",
    ),
];

fn cmd_dir(plat: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("cmd")
        .join(plat)
}

#[test]
fn a_platform_module_that_is_a_copy_delegates_instead() {
    // Above this share of linux's logic lines reproduced verbatim, the module
    // is a copy with at most a constant edited — the shape every drifted module
    // had (`logname` 99.9 %, `whereis` 100 %, `loginctl` 99.4 %, `tcpdump`
    // byte-identical).
    //
    // NOT lower: at 70 % the set includes modules with real behavioural
    // differences — `whoami` and `cp` carry a stricter `detect` on macOS than
    // on linux — and delegating those would silently change what they fire on.
    // Sharing boilerplate is not the same as being a copy.
    const COPY_THRESHOLD: f64 = 0.90;
    let linux = cmd_dir("linux");
    let mut offenders: Vec<String> = Vec::new();

    let mut compared = 0usize;
    for plat in ["macos", "windows"] {
        let dir = cmd_dir(plat);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) != Some("rs") {
                continue;
            }
            let name = p
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            if name == "mod" || JUSTIFIED.iter().any(|(m, _)| *m == name) {
                continue;
            }
            compared += 1;
            let twin = linux.join(format!("{name}.rs"));
            if !twin.exists() {
                continue;
            }
            let (a, b) = (
                std::fs::read_to_string(&twin).unwrap_or_default(),
                std::fs::read_to_string(&p).unwrap_or_default(),
            );
            let (la, lb) = (logic_lines(&a), logic_lines(&b));
            if la.is_empty() {
                continue;
            }
            // A delegating module names the linux one; that is the fix, not a
            // violation, however many lines it happens to share.
            //
            // `table_fold` is no longer in this list through delegation but
            // through DISAPPEARANCE: the table fold was called
            // `cmd/linux/docker_table.rs`, the macOS and Windows copies only
            // called it, and they are deleted. The module now lives in the
            // fold layer, outside `cmd/`, so no twin can drift from it any
            // more. The name stays here because a command module can still
            // name it.
            if b.contains(&format!("crate::cmd::linux::{name}"))
                || b.contains("crate::table_fold")
                || b.contains("crate::cmd::linux::git_listing")
            {
                continue;
            }
            // SYMMETRIC on purpose. A one-directional ratio ("how much of
            // linux is in the twin") calls a SUPERSET a copy: `macos/log_tail`
            // contains every linux line plus a `tail` helper with a different
            // signature that four other macOS modules call. Delegating it broke
            // the build — the metric, not the module, was wrong.
            let inter = la.intersection(&lb).count() as f64;
            let union = la.union(&lb).count() as f64;
            let shared = if union == 0.0 { 0.0 } else { inter / union };
            if shared >= COPY_THRESHOLD {
                offenders.push(format!(
                    "  {plat}/{name}.rs reproduces {:.0}% of linux/{name}.rs — delegate to \
                     `crate::cmd::linux::{name}` instead of keeping a second copy",
                    shared * 100.0
                ));
            }
        }
    }

    // The FLOOR. `cmd/macos/` and `cmd/windows/` moved, and this guard would
    // turn green comparing nothing — the worst of verdicts.
    assert!(
        compared >= 20,
        "only {compared} platform module(s) compared — the sweep got lost"
    );
    assert!(
        offenders.is_empty(),
        "platform modules that are copies of their linux twin (they WILL drift, and the \
         drift is only visible by diffing both files):\n{}\n\nIf a copy is genuinely \
         warranted, add it to JUSTIFIED with the reason.",
        offenders.join("\n")
    );
}
