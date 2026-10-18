//! A handle we may LOCK must have been opened with read access.
//!
//! `LockFileEx` requires `GENERIC_READ` or `GENERIC_WRITE` on the handle.
//! Rust's `.append(true)` asks for `FILE_GENERIC_WRITE` MINUS
//! `FILE_WRITE_DATA`, which is neither, so on Windows an append-only handle
//! cannot be locked at all: the lock call answers `ERROR_ACCESS_DENIED`
//! (os error 5). `flock` asks nothing of the open mode, so the whole class is
//! INVISIBLE on Unix and macOS — the suite is green there with every site
//! wrong.
//!
//! MEASURED in the 1.1.9 release build, the first one to POLL for the lock
//! instead of blocking on it: 25 tests failed on the Windows leg and nowhere
//! else. Twenty were one line — `if !lock_briefly(&lock) { return }` — taking
//! the "somebody else holds it" branch on a temp directory the test had
//! created three lines earlier. The blocking `lock_exclusive()` it replaced
//! failed identically; discarding its result (`let _ = …`) is what kept it
//! quiet for as long as it did.
//!
//! So: in any file that locks, an `OpenOptions` chain carrying
//! `.append(true)` must also carry `.read(true)`.

use std::path::{Path, PathBuf};

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

/// Does this file take a file lock at all? Only those are held to the rule —
/// an append handle nobody locks loses nothing by being write-only, and
/// widening the sweep to every log writer would make it noise.
fn file_locks(body: &str) -> bool {
    body.lines()
        .map(|l| l.split("//").next().unwrap_or(""))
        .any(|code| {
            code.contains("lock_briefly(")
                || code.contains(".lock_exclusive()")
                || code.contains(".try_lock_exclusive()")
        })
}

/// The `OpenOptions` chains in `body` that append without asking to read.
///
/// A chain runs from `OpenOptions::new()` to the `.open(` that closes it, and
/// may be written on one line or seven — both spellings are in this tree, and
/// a sweep that only understood one would have missed four of the six sites
/// that were actually broken.
fn append_chains_without_read(rel: &str, body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let lines: Vec<&str> = body.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let code = line.split("//").next().unwrap_or("");
        if !code.contains("OpenOptions::new()") {
            continue;
        }
        // Gather the chain: this line, plus following lines until `.open(`.
        let mut chain = String::from(code);
        let mut j = i;
        while !chain.contains(".open(") && j + 1 < lines.len() && j - i < 12 {
            j += 1;
            chain.push_str(lines[j].split("//").next().unwrap_or(""));
        }
        if chain.contains(".append(true)") && !chain.contains(".read(true)") {
            out.push(format!("{rel}:{}: {}", i + 1, line.trim()));
        }
    }
    out
}

/// The rule itself, on source that is not in the tree — so weakening the sweep
/// shows up here even when the tree is clean.
#[test]
fn the_rule_reads_both_spellings_of_a_chain() {
    let one_line = "let f = fs::OpenOptions::new().create(true).append(true).open(&p)?;";
    assert_eq!(
        append_chains_without_read("a.rs", one_line).len(),
        1,
        "a one-line append chain with no read must be reported"
    );
    let one_line_ok =
        "let f = fs::OpenOptions::new().create(true).read(true).append(true).open(&p)?;";
    assert!(append_chains_without_read("a.rs", one_line_ok).is_empty());

    let split =
        "let f = fs::OpenOptions::new()\n    .create(true)\n    .append(true)\n    .open(&p)?;";
    assert_eq!(
        append_chains_without_read("a.rs", split).len(),
        1,
        "a chain split over four lines must be reported too"
    );
    let split_ok =
        "let f = fs::OpenOptions::new()\n    .create(true)\n    .read(true)\n    .append(true)\n    .open(&p)?;";
    assert!(append_chains_without_read("a.rs", split_ok).is_empty());

    // A write-only open that never appends is not this rule's business, and
    // neither is prose about one.
    let benign = "let f = fs::OpenOptions::new().write(true).open(&p)?;\n// .append(true) one day";
    assert!(append_chains_without_read("a.rs", benign).is_empty());

    // A file that never locks is out of scope even when it appends.
    assert!(!file_locks(one_line));
    assert!(file_locks(
        "if !crate::platform::lock_briefly(&f) { return; }"
    ));
    assert!(
        !file_locks("// one day this will lock_briefly(&f)"),
        "prose is not a lock"
    );
}

#[test]
fn every_lockable_append_is_readable() {
    let root = workspace_root();
    let mut files = Vec::new();
    for d in ["crates/tokenade-core/src", "crates/tokenade-cli/src"] {
        rust_sources(&root.join(d), &mut files);
    }
    assert!(files.len() > 100, "the sweep found no sources to read");

    let mut offenders = Vec::new();
    let mut locking_files = 0usize;
    for path in &files {
        let Ok(body) = std::fs::read_to_string(path) else {
            continue;
        };
        if !file_locks(&body) {
            continue;
        }
        locking_files += 1;
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        offenders.extend(append_chains_without_read(&rel, &body));
    }
    assert!(
        locking_files >= 4,
        "the sweep found only {locking_files} file(s) that lock — it stopped seeing the code it guards"
    );
    assert!(
        offenders.is_empty(),
        "an append handle these files may lock cannot be locked on Windows \
         (LockFileEx wants GENERIC_READ/GENERIC_WRITE; append gives neither). \
         Add `.read(true)` to:\n  {}",
        offenders.join("\n  ")
    );
}
