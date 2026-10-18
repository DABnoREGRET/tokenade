//! A test module compiled into the SHIPPED library.
//!
//! `mod tests { … }` without `#[cfg(test)]` still runs its `#[test]`
//! functions, so nothing fails and nothing looks wrong. What it also does is
//! compile the module's fixtures into every non-test build of the crate. The
//! only visible symptom is a `dead_code` warning on whatever helper the tests
//! call — and a warning that has always been there is a warning nobody reads.
//!
//! Found by that warning, on a module named
//! `a_clock_without_a_date_is_still_a_log`. Swept 2026-08-27: **ten more**
//! across nine files, including one plainly named `mod tests`.
//!
//! The sweep is the guard, not a `#[cfg(test)]` added to the eleven: a fix at
//! the eleven sites says nothing about the twelfth. Same shape as the
//! path-hint sweep and the listener sweep.
//!
//! `src/` ONLY. A file under `tests/` is already compiled as a test target
//! and `cfg(test)` is NOT set there — gating an integration-test module would
//! delete the tests instead of gating them.

use std::path::{Path, PathBuf};

fn source_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            source_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Is `line` a `mod name {` opener?
fn module_opener(line: &str) -> bool {
    let t = line.trim();
    let t = t.strip_prefix("pub ").unwrap_or(t);
    t.starts_with("mod ") && t.ends_with('{') && !t.contains(';')
}

#[test]
fn no_test_module_is_compiled_into_the_library() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for crate_dir in std::fs::read_dir(root.join("crates"))
        .expect("crates/")
        .flatten()
    {
        source_files(&crate_dir.path().join("src"), &mut files);
    }
    assert!(
        files.len() > 100,
        "the sweep found only {} source file(s) — it is not reading the tree",
        files.len()
    );

    let mut offenders = Vec::new();
    let mut gated = 0usize;
    for f in &files {
        let Ok(text) = std::fs::read_to_string(f) else {
            continue;
        };
        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if !module_opener(line) {
                continue;
            }
            // Nearest preceding attribute/comment/blank run.
            let mut has_cfg_test = false;
            let mut j = i;
            while j > 0 {
                let prev = lines[j - 1].trim();
                if prev.is_empty() || prev.starts_with("//") || prev.starts_with("#[") {
                    if prev.contains("cfg(test)") {
                        has_cfg_test = true;
                        break;
                    }
                    j -= 1;
                } else {
                    break;
                }
            }
            // Only modules that actually hold tests. Depth-tracked so a
            // sibling module below is not read as this one's body.
            let mut depth = 1i32;
            let mut holds_tests = false;
            for l in &lines[i + 1..] {
                depth += l.matches('{').count() as i32 - l.matches('}').count() as i32;
                if l.trim().starts_with("#[test]") {
                    holds_tests = true;
                }
                if depth <= 0 {
                    break;
                }
            }
            if !holds_tests {
                continue;
            }
            if has_cfg_test {
                gated += 1;
            } else {
                offenders.push(format!(
                    "{}:{} — {}",
                    f.strip_prefix(&root).unwrap_or(f).display(),
                    i + 1,
                    line.trim()
                ));
            }
        }
    }
    assert!(
        gated >= 50,
        "only {gated} gated test module(s) seen — the sweep is not recognising them"
    );
    assert!(
        offenders.is_empty(),
        "test module(s) compiled into the shipped library — add #[cfg(test)]:\n{}",
        offenders.join("\n")
    );
}

/// A function that LOOKS like a test, sits in a test module, asserts, and is
/// never registered or called — so it never runs.
///
/// Found by the same channel as the module sweep above and with the same
/// ending: `warning: function
/// drop_build_artifact_lines_strips_noise_from_find is never used`, one line
/// in a build that prints many, on the PRIMARY case of a family whose other
/// three members (lookalikes, passthrough, ls/tree) were all registered. It
/// had been dead since it was written.
///
/// Three conditions, and all three are needed:
///   * no `#[test]`-shaped attribute, or it runs;
///   * the name appears nowhere else in the file, or it is a helper some test
///     calls — which runs, inside its caller;
///   * the body asserts, or it is a fixture builder and its deadness is an
///     ordinary `dead_code` question, not a lost guard.
#[test]
fn no_test_module_holds_an_unregistered_test() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for crate_dir in std::fs::read_dir(root.join("crates"))
        .expect("crates/")
        .flatten()
    {
        source_files(&crate_dir.path().join("src"), &mut files);
        source_files(&crate_dir.path().join("tests"), &mut files);
    }
    assert!(
        files.len() > 100,
        "the sweep found only {} source file(s) — it is not reading the tree",
        files.len()
    );

    let mut offenders = Vec::new();
    let mut registered = 0usize;
    for f in &files {
        let Ok(text) = std::fs::read_to_string(f) else {
            continue;
        };
        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let t = line.trim();
            let Some(rest) = t.strip_prefix("fn ") else {
                continue;
            };
            if line.starts_with("fn ") {
                // Column zero: a free function, not a member of a test module.
                continue;
            }
            let Some(name) = rest.split(['(', '<']).next() else {
                continue;
            };
            // Everything above the `fn` up to the previous item, read as ONE
            // block. Walking line by line and stopping at the first line that
            // is not `#[…]` reads a MULTI-LINE attribute as the end of the
            // run: `#[test]` followed by a `#[cfg_attr(…, ignore = "…")]`
            // spanning four lines was reported as unregistered, which is a
            // sweep that fails on the most carefully annotated tests in the
            // tree. A blank line, or one closing or opening a block, ends it.
            let mut attributed = false;
            let mut j = i;
            while j > 0 {
                let prev = lines[j - 1].trim();
                if prev.is_empty()
                    || prev.ends_with('}')
                    || prev.ends_with('{')
                    || prev.ends_with(';')
                {
                    break;
                }
                if prev.contains("test]") || prev.contains("bench]") || prev.contains("test)]") {
                    attributed = true;
                    break;
                }
                j -= 1;
            }
            if attributed {
                registered += 1;
                continue;
            }
            // Called anywhere else in the file? Then it runs inside its caller.
            if text.matches(name).count() > 1 {
                continue;
            }
            // Does it assert? Walk its body by brace depth.
            let mut depth = 0i32;
            let mut asserts = false;
            for l in &lines[i..] {
                depth += l.matches('{').count() as i32 - l.matches('}').count() as i32;
                if l.contains("assert") {
                    asserts = true;
                }
                if depth <= 0 {
                    break;
                }
            }
            if asserts {
                offenders.push(format!(
                    "{}:{} — fn {name} asserts, is never called, and has no #[test]",
                    f.strip_prefix(&root).unwrap_or(f).display(),
                    i + 1
                ));
            }
        }
    }
    assert!(
        registered >= 1000,
        "only {registered} registered test(s) seen — the sweep is not recognising them"
    );
    assert!(
        offenders.is_empty(),
        "test(s) that never run — add #[test]:\n{}",
        offenders.join("\n")
    );
}
