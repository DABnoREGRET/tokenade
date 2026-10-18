//! **An ignored test must say WHY.**
//!
//! A bare `#[ignore]` is a test nobody can revive: it says neither what it
//! waits for (a key, a corpus, a model, a machine) nor whether it measures
//! instead of checking. The re-architecture plan counted 46 bare against 32
//! reasoned; the count of 2026-09-13 finds **30 bare against 34**, and the
//! thirty received their reason — live network, local corpus, the
//! developer's real install, dump, measurement, or regeneration of a tracked
//! file.
//!
//! The rule is the plan's: probes are excluded by TARGET (`--test probes`),
//! not by `--ignored`, and for that one must know what each one waits for.

use std::path::{Path, PathBuf};

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .to_path_buf()
}

#[test]
fn no_test_is_ignored_without_saying_why() {
    let mut bare: Vec<String> = Vec::new();
    let mut reasoned = 0usize;
    let mut stack = vec![crates_dir()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            let name = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if name == "target" || name.starts_with('.') {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            if p.extension().is_some_and(|x| x == "rs") {
                let Ok(s) = std::fs::read_to_string(&p) else {
                    continue;
                };
                for (i, line) in s.lines().enumerate() {
                    let t = line.trim();
                    // The ATTRIBUTE, not the word: `#[ignore]` quoted in prose
                    // or in an error message is not an ignored test, and this
                    // file quotes several.
                    if t == "#[ignore]" {
                        let rel = p.strip_prefix(crates_dir()).unwrap_or(&p);
                        bare.push(format!("    {}:{}", rel.display(), i + 1));
                    } else if t.starts_with("#[ignore = ") || t.starts_with("#[ignore=") {
                        reasoned += 1;
                    }
                }
            }
        }
    }
    assert!(
        reasoned >= 30,
        "only {reasoned} reasoned `#[ignore]` read — the sweep got lost"
    );
    assert!(
        bare.is_empty(),
        "these tests are ignored without saying why. A bare `#[ignore]` cannot \
         be revived: write what it waits for — live network, local \
         corpus, model, developer's machine — or that it MEASURES instead of \
         checking:\n{}",
        bare.join("\n")
    );
}

/// **A dead-code allowance must say why.**
///
/// `#[allow(dead_code)]` silences the only warning that says "nobody calls
/// this". Silenced without a reason, it turns a reserve into oblivion: the
/// next reader cannot know whether the item awaits its second caller or
/// should have gone with the first.
///
/// Count of 2026-09-13: **31 allowances, 30 already carried their reason**.
/// The thirty-first covered a function whose whole body was a call to another
/// function, with a TEST as its only caller — deleted.
#[test]
fn no_dead_code_allowance_is_silent() {
    let mut silent: Vec<String> = Vec::new();
    let mut reasoned = 0usize;
    let mut stack = vec![crates_dir()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            let name = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if name == "target" || name.starts_with('.') {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            if !p.extension().is_some_and(|x| x == "rs") {
                continue;
            }
            let Ok(s) = std::fs::read_to_string(&p) else {
                continue;
            };
            let lines: Vec<&str> = s.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                let t = line.trim();
                if t != concat!("#[allow(dead", "_code)]")
                    && t != concat!("#![allow(dead", "_code)]")
                {
                    continue;
                }
                // The reason is the comment preceding it — the form the thirty
                // others already use.
                // Walking back OVER blank lines: a module's
                // `#![allow(...)]` follows its `//!` after a blank, and that is
                // the form two shared helpers use.
                let mut k = i;
                let has_reason = loop {
                    if k == 0 {
                        break false;
                    }
                    k -= 1;
                    let prev = lines[k].trim_start();
                    if prev.is_empty() {
                        continue;
                    }
                    break prev.starts_with("//");
                };
                if has_reason {
                    reasoned += 1;
                } else {
                    let rel = p.strip_prefix(crates_dir()).unwrap_or(&p);
                    silent.push(format!("    {}:{}", rel.display(), i + 1));
                }
            }
        }
    }
    assert!(
        reasoned >= 25,
        "only {reasoned} reasoned allowances read — the sweep got lost"
    );
    assert!(
        silent.is_empty(),
        "these dead-code allowances do not say why. Write the reason \
         right above — declared reserve, called by another crate, \
         built by a macro — or remove the item:\n{}",
        silent.join("\n")
    );
}
