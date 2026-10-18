//! **Where the labels come from.** Commit message → files touched,
//! a file's header sentence, and the PARAPHRASED variant of it.

#[allow(unused_imports)]
use super::*;
use std::collections::BTreeSet;
use std::path::Path;
use tokenade_core::semantic;

/// A commit that touches more than this no longer describes a place.
pub(crate) const MAX_FILES: usize = 5;
/// A shorter message is not a query.
pub(crate) const MIN_WORDS: usize = 3;

pub(crate) struct Case {
    pub(crate) query: String,
    pub(crate) gold: BTreeSet<String>,
    /// True when the query shares NO word with the expected paths.
    ///
    /// Without this cut, the bench lies to itself: a commit message often quotes
    /// the name of the file or the function, and BM25 wins by copying. The
    /// hard subset is the one where an agent asks "where do we limit the
    /// rate" of a file called `throttle.rs` — THAT is where the dense
    /// arm must pay off, and nowhere else.
    pub(crate) hard: bool,
}

/// The words of a path: segments, dash, underscore, camel case.
pub(crate) fn path_words(file: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for seg in file.split(['/', '.', '-', '_']) {
        let mut cur = String::new();
        for ch in seg.chars() {
            if ch.is_uppercase() && !cur.is_empty() {
                out.insert(std::mem::take(&mut cur).to_lowercase());
            }
            cur.push(ch);
        }
        if cur.len() >= 3 {
            out.insert(cur.to_lowercase());
        }
    }
    out
}

pub(crate) fn git(repo: &Path, args: &[&str]) -> String {
    std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// The (message, files) pairs of the last `n` commits, filtered.
pub(crate) fn cases(repo: &Path, n: usize, indexed: &BTreeSet<String>) -> Vec<Case> {
    let log = git(
        repo,
        &[
            "log",
            "--no-merges",
            "-n",
            &n.to_string(),
            "--format=%x1fSUBJ%x1f%s",
            "--name-only",
        ],
    );
    let mut out = Vec::new();
    let mut subject = String::new();
    let mut files: BTreeSet<String> = BTreeSet::new();
    let mut flush = |subject: &str, files: &mut BTreeSet<String>| {
        let words = subject.split_whitespace().count();
        if words >= MIN_WORDS && !files.is_empty() && files.len() <= MAX_FILES {
            let q: BTreeSet<String> = subject
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| w.len() >= 3)
                .map(|w| w.to_lowercase())
                .collect();
            let hard = !files
                .iter()
                .any(|f| path_words(f).iter().any(|w| q.contains(w)));
            out.push(Case {
                query: subject.to_string(),
                gold: std::mem::take(files),
                hard,
            });
        } else {
            files.clear();
        }
    };
    for line in log.lines() {
        if let Some(rest) = line.strip_prefix('\u{1f}') {
            flush(&subject, &mut files);
            subject = rest.trim_start_matches("SUBJ\u{1f}").to_string();
            continue;
        }
        let f = line.trim();
        if f.is_empty() {
            continue;
        }
        // Only files the index CONTAINS can be found: a
        // file deleted since, or a `.md`, is not a possible answer.
        if indexed.contains(f) {
            files.insert(f.to_string());
        }
    }
    flush(&subject, &mut files);
    out
}

/// The vectors of ALL documents, computed once.
pub(crate) fn doc_vectors(sem: &semantic::SemanticIndex) -> Vec<Vec<f32>> {
    let texts: Vec<String> = sem
        .docs
        .iter()
        .map(|d| {
            if d.snippet.is_empty() {
                d.file.clone()
            } else {
                format!("{} {}", d.file, d.snippet)
            }
        })
        .collect();
    tokenade_core::embedding::embed_batch(&texts).unwrap_or_default()
}

/// **Second family of queries: a file's header sentence.**
///
/// A commit message states the INTENT of a change; that is only one of the
/// uses. An agent also asks "where is X handled", and the sentence that opens
/// a module's documentation is the closest thing to it without
/// inventing a corpus. Two variants:
///
/// - `doc`: the sentence as is — easy case, its words are in the
///   file, BM25 must win;
/// - `hard`: the same sentence STRIPPED of every word that appears in the path or
///   in the file's symbols. It is the poor man's paraphrase: no
///   copying possible, only the meaning remains.
pub(crate) fn doc_cases(repo: &Path, sem: &semantic::SemanticIndex, hard: bool) -> Vec<Case> {
    let mut out = Vec::new();
    for d in &sem.docs {
        let Ok(body) = std::fs::read_to_string(repo.join(&d.file)) else {
            continue;
        };
        // The first sentence of the module doc (`//!`, `"""`, `/** */`).
        let line = body
            .lines()
            .map(str::trim)
            .filter_map(|l| {
                l.strip_prefix("//!")
                    .or_else(|| l.strip_prefix("///"))
                    .or_else(|| l.strip_prefix("#"))
                    .or_else(|| l.strip_prefix("*"))
            })
            .map(|l| l.trim().trim_matches('*').trim())
            .find(|l| l.split_whitespace().count() >= 6);
        let Some(line) = line else { continue };
        let sentence: String = line
            .split(". ")
            .next()
            .unwrap_or(line)
            .chars()
            .take(160)
            .collect();
        let banned = path_words(&d.file);
        let query: String = if hard {
            sentence
                .split_whitespace()
                .filter(|w| {
                    let lw: String = w
                        .chars()
                        .filter(|c| c.is_alphanumeric())
                        .collect::<String>()
                        .to_lowercase();
                    lw.len() >= 3
                        && !banned.contains(&lw)
                        && !d.snippet.to_lowercase().contains(&lw)
                })
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            sentence.clone()
        };
        if query.split_whitespace().count() < MIN_WORDS {
            continue;
        }
        out.push(Case {
            query,
            gold: [d.file.clone()].into_iter().collect(),
            hard,
        });
    }
    out
}
