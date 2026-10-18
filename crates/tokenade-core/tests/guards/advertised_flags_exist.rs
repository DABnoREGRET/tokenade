//! A flag a message tells you to pass must be a flag something accepts.
//!
//! `archive.rs` closed its directory rollup with "pass `--no-fold` to expand".
//! That spelling appeared exactly ONCE in the whole codebase: in the message
//! advertising it. No parser read it, no help text listed it, and an agent
//! following the instruction spends a turn on `sh: -- : option invalide` —
//! the same shape as a recovery path that loops back to itself.
//!
//! The rule is cheap and does not need to know the CLI's grammar: a flag that
//! is real is spelled at least twice — once where it is parsed or documented,
//! once where it is advertised. A flag spelled once can only be advertised.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Flags an advertisement names that belong to ANOTHER program, with that
/// program. An entry here is a claim that the flag is not tokenade's, which is
/// the only thing that makes its absence from this codebase correct.
const FLAGS_OF_OTHER_PROGRAMS: &[(&str, &str)] = &[];

/// Is `flag` advertised and nowhere else?
///
/// Two, not one: the advertisement itself is always one of the spellings, so a
/// threshold of one can never fire and the sweep would pass on everything.
fn is_phantom(flag: &str, all_sources: &str) -> bool {
    if FLAGS_OF_OTHER_PROGRAMS.iter().any(|(f, _)| *f == flag) {
        return false;
    }
    all_sources.matches(flag).count() < 2
}

/// The rule itself, on text that is not in the tree — so a weakening of the
/// sweep shows up here even when the tree happens to be clean.
#[test]
fn the_rule_reports_a_phantom_and_spares_a_real_flag() {
    assert!(
        is_phantom("--no-such-flag", "  pass `--no-such-flag` to expand\n"),
        "a flag spelled only where it is advertised is a phantom"
    );
    assert!(
        !is_phantom(
            "--accept",
            "  pass `--accept` to send\nif a == \"--accept\" { consent = true }\n"
        ),
        "a flag that is also parsed is real"
    );

    assert_eq!(
        advertised_flags("  (1481 entries — pass `--no-fold` to expand)"),
        vec!["--no-fold"]
    );
    assert!(
        advertised_flags("  * `sshpass --password-file /pw` is another tool's flag").is_empty(),
        "`sshpass` ends in pass; a note about another tool is not an instruction"
    );
    assert!(advertised_flags("  pass a path or glob to the listing").is_empty());
    assert!(
        advertised_flags("  pass -vvv to see more").is_empty(),
        "short flags are another program's grammar; this sweep is about long ones"
    );
    assert_eq!(
        advertised_flags("pass --one and then pass `--two` as well"),
        vec!["--one", "--two"],
        "every occurrence on the line, not the first"
    );
    assert_eq!(
        advertised_flags("run with --a-flag, or pass `--second-flag` to keep it"),
        vec!["--second-flag"],
        "only what the reader is told to PASS"
    );
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_sources(&p, out);
        } else if p.extension().and_then(|x| x.to_str()) == Some("rs") {
            out.push(p);
        }
    }
}

/// The token after `pass ` / ``pass `` on one line, when it is a long flag.
/// Every long flag this line tells the reader to pass.
///
/// EVERY occurrence of "pass ", not the first: the one real note about another
/// tool in this tree reads "`sshpass -f /path` — `-f` is sshpass
/// --password-file", and stopping at the first occurrence lands inside
/// `sshpass -f`, which spares that line for the wrong reason. Scanning them
/// all is what makes the word rule below the thing doing the work.
fn advertised_flags(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while let Some(rel) = line[at..].find("pass ") {
        let start = at + rel;
        at = start + "pass ".len();
        // A WORD "pass": `sshpass --password-file` in a comment is a note
        // about another tool, not an instruction to the reader.
        if line[..start].ends_with(|c: char| c.is_alphanumeric() || c == '_') {
            continue;
        }
        let tok = line[at..].trim_start_matches('`');
        if !tok.starts_with("--") {
            continue;
        }
        let end = tok[2..]
            .find(|c: char| !c.is_ascii_lowercase() && !c.is_ascii_digit() && c != '-')
            .map(|k| k + 2)
            .unwrap_or(tok.len());
        if end > 2 {
            out.push(&tok[..end]);
        }
    }
    out
}

#[test]
fn a_flag_a_message_tells_you_to_pass_is_spelled_somewhere_else_too() {
    let root = workspace_root();
    let mut files = Vec::new();
    for crate_dir in ["crates/tokenade-core/src", "crates/tokenade-cli/src"] {
        rust_sources(&root.join(crate_dir), &mut files);
    }
    assert!(files.len() > 100, "the sweep found no sources to read");

    let bodies: BTreeMap<PathBuf, String> = files
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok().map(|s| (p.clone(), s)))
        .collect();
    let all: String = bodies.values().cloned().collect::<Vec<_>>().join("\n");

    let mut advertised: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for (path, body) in &bodies {
        for line in body.lines() {
            for flag in advertised_flags(line) {
                advertised
                    .entry(flag)
                    .or_default()
                    .push(path.display().to_string());
            }
        }
    }
    assert!(
        advertised.len() >= 4, // 4 since the licence messages (and their flags) left in 2.0
        "the sweep matched almost nothing, so it proves almost nothing: {advertised:?}"
    );

    let mut phantom = Vec::new();
    for (flag, sites) in &advertised {
        if is_phantom(flag, &all) {
            phantom.push(format!(
                "{flag} — advertised in {sites:?}, spelled nowhere else"
            ));
        }
    }
    assert!(
        phantom.is_empty(),
        "a message names a flag that exists nowhere but in that message:\n  {}",
        phantom.join("\n  ")
    );
}
