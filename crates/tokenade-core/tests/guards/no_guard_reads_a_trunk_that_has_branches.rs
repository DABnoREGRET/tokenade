//! **Reading `x.rs` when `x/` exists is a guard looking the wrong way.**
//!
//! Ten times in one day of splitting, a test accused the product of having
//! lost something it had not lost: "only 2 call site(s) reached", "the sweep
//! reached only 0 arms", "`try_source_strip` has 0 call site(s)",
//! "`redact.rs` builds a store path with no `TOKENADE_REDACT_LEDGER` escape
//! hatch". Each time, the function looked for had moved down into a child
//! file and the guard only read the trunk.
//!
//! Three of those guards already carried, hand-written, the comment saying
//! why: "a split module the sweep does not name is a module the sweep does
//! not see". They had written it and did it again anyway, because nothing
//! stopped them.
//!
//! The rule is DECIDABLE, and that is what makes it holdable: reading through
//! `include_str!` an `x.rs` next to which an `x/` directory exists. No
//! judgement, no list of exceptions — the fix is always the same,
//! `source_guard::module_family` (or `library_module_family`), which returns
//! the trunk AND the branches in a deterministic order.
//!
//! Reading a file that has NO sibling directory stays allowed: it is not a
//! split module yet, and the day it becomes one, this test will say so.
//!
//! ## `mod.rs` is the only trunk that will never have a directory of its name
//!
//! The rule above saw nothing of the 2015 layout, where the trunk is called
//! `mod.rs` and its branches are its NEIGHBOURS. Thirty-five directories of
//! this tree are in that form. Splitting `hooks/mod.rs` showed it: three
//! tests read `hooks/mod.rs` while `cmd_hookwrap_postread` moved down into
//! `hooks/postread.rs`, and none would have said so. A `mod.rs` read in a
//! directory carrying other files therefore falls under exactly the same
//! refusal.

use std::path::{Path, PathBuf};

fn rs_files(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(root) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn no_source_reads_a_trunk_whose_branch_directory_exists() {
    let crates = tokenade_core::source_guard::crates_dir();
    let mut files = Vec::new();
    for e in std::fs::read_dir(&crates).into_iter().flatten().flatten() {
        rs_files(&e.path().join("src"), &mut files);
        rs_files(&e.path().join("tests"), &mut files);
    }
    assert!(
        files.len() > 500,
        "only {} file(s) walked: the descent stopped",
        files.len()
    );

    let mut bad = Vec::new();
    for f in &files {
        let Ok(body) = std::fs::read_to_string(f) else {
            continue;
        };
        let dir = f.parent().unwrap_or(Path::new("."));
        let mut rest = body.as_str();
        while let Some(at) = rest.find("include_str!(\"") {
            rest = &rest[at + "include_str!(\"".len()..];
            let Some(end) = rest.find('"') else { break };
            let arg = &rest[..end];
            rest = &rest[end..];
            let Some(stem) = arg.strip_suffix(".rs") else {
                continue;
            };
            // A relative path, resolved from the file that writes it.
            let target = dir.join(arg);
            // `mod.rs` IS the trunk of its directory: its branches are the
            // other files of the same directory, and no `mod/` will ever
            // exist to say so. The 2015 form therefore escaped the rule
            // entirely — measured while splitting `hooks/mod.rs`, three of
            // whose tests read the trunk while `cmd_hookwrap_postread` moved
            // down into a child.
            if target.file_name().is_some_and(|n| n == "mod.rs") {
                let d = target.parent().unwrap_or(Path::new("."));
                let siblings = std::fs::read_dir(d)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter(|e| {
                        e.path().extension().is_some_and(|x| x == "rs") && e.file_name() != "mod.rs"
                    })
                    .count();
                if siblings > 0 {
                    bad.push(format!(
                        "    {} → {arg} (the directory carries {siblings} other file(s))",
                        f.display()
                    ));
                }
                continue;
            }
            let branch = dir.join(stem);
            if branch.is_dir() {
                bad.push(format!(
                    "    {} → {arg} (the directory `{stem}/` exists)",
                    f.display()
                ));
            }
        }
    }
    bad.sort();
    bad.dedup();
    assert!(
        bad.is_empty(),
        "these sites read the TRUNK of a module that has BRANCHES: what \
         moves down into a child becomes invisible to them, and the guard then \
         announces a disappearance from the product where it is merely looking \
         the wrong way. The fix is `source_guard::module_family(root, name)` or \
         `library_module_family(name)`:\n{}",
        bad.join("\n")
    );
}
