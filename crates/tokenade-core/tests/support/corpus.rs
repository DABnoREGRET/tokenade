//! The SINGLE enumerator of the captured corpus.
//!
//! Two corpus-driven guards existed — `richness_corpus.rs` (no compactor
//! keeps less rare information than before) and `corpus_error_survival.rs`
//! (no fold loses the line saying something failed) — and **each enumerated
//! the corpus on its own**, with `["macos", "windows"]` hard-coded and the
//! `.txt` extension hard-coded. Two copies of the same decision: widening the
//! corpus would have served only one.
//!
//! ## What the duplication hid
//!
//! MEASURED 2026-09-07 on the real usage ledger: the sixteen most used
//! `auto:*` compactors — `ini` 209×, `html` 197×, `json` 180×,
//! `markdown` 151×, `diff` 140×, `grep` 127×, `stacktrace` 86× — **are in
//! neither guard**. Zero out of sixteen. The ratchet displayed 211 lines and
//! 0 drops while looking elsewhere than where the product works, and the
//! header of `corpus_error_survival.rs` already named the weakness this
//! reproduces: a guard with hand-written cases requires that "someone thinks
//! of the compactor first".
//!
//! ## The two routes
//!
//! A COMMAND fixture is named after the program that produced it and goes
//! through `filter_command`. A FORMAT fixture has no program:
//! `auto::compact_with_path_hint` routes it by its content and extension,
//! which is exactly the path a `tokenade read` takes.
//!
//! ## And linux, the platform this product runs on
//!
//! For a long time the corpus only carried macos and windows captures.
//! MEASURED 2026-09-07: the product counts **935 compactors** — 672 under
//! `cmd/linux`, 129 macos, 134 windows — and the ratchet exercised only 29.
//! Zero linux fixtures, on the platform of most installs.
//!
//! The linux captures are REAL, taken on the development machine, then
//! cleaned with a deliberately narrow set of substitutions: the user name,
//! MAC addresses, private IPs, the host name in `uname`. The cleaning was
//! DIFFED against the raw capture, line by line, before being committed — a
//! first version replaced the host name (two letters) everywhere those two
//! letters appeared, and took `user@1000.service` for an e-mail address.
//!
//! Format fixtures are **real** artefacts of the repo, frozen: a real
//! `Cargo.toml`, a real README, a real diff, a real `grep` output, a real
//! JSON report. Never synthetic — a fabricated corpus cannot see the signal
//! it does not contain: a rule gained 32 points on one and, on real data,
//! ate kernel socket addresses that were the whole difference between two
//! lines.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Named after a program: goes through `filter_command`.
    Command,
    /// Routed by its content: goes through the `auto` path.
    Format,
}

#[derive(Debug, Clone)]
pub struct Fixture {
    /// `macos/df_h`, `formats/toml_workspace` — the ratchet key.
    pub key: String,
    pub path: PathBuf,
    pub route: Route,
    pub body: String,
}

impl Fixture {
    /// The full file name, extension included. `auto` routing reads it, so
    /// removing it would send the fixture nowhere — that is the trap that
    /// made a fuzz report "0 hit" on a version known to crash.
    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// The whole captured corpus, both routes together, sorted.
pub fn fixtures() -> Vec<Fixture> {
    let mut out = Vec::new();
    for (sub, route) in [
        ("linux", Route::Command),
        ("macos", Route::Command),
        ("windows", Route::Command),
        ("formats", Route::Format),
    ] {
        let dir = root().join(sub);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = Vec::new();
        for ent in entries.flatten() {
            let p = ent.path();
            if p.is_dir() {
                if let Ok(inner) = std::fs::read_dir(&p) {
                    paths.extend(inner.flatten().map(|e| e.path()));
                }
            } else {
                paths.push(p);
            }
        }
        // A COMMAND fixture is a `.txt`; a FORMAT fixture carries the
        // extension that routes it, so it cannot be filtered on.
        if route == Route::Command {
            paths.retain(|p| p.extension().and_then(|e| e.to_str()) == Some("txt"));
        }
        paths.sort();
        for p in paths {
            let Ok(body) = std::fs::read_to_string(&p) else {
                continue;
            };
            if body.trim().is_empty() {
                continue;
            }
            // The key carries the RELATIVE path, subdirectory included.
            // Without it, `macos/locale_ru_RU/df_h` and `macos/df_h` give the
            // same key and overwrite each other in the ratchet: 138 locale
            // fixtures would have entered the corpus to make as many vanish,
            // without any count moving in a readable way.
            let rel = p.strip_prefix(root()).unwrap_or(&p).with_extension("");
            out.push(Fixture {
                key: rel.to_string_lossy().replace('\\', "/"),
                path: p,
                route,
                body,
            });
        }
    }
    out
}

/// The COMMANDS corpus only — what the two guards saw before `formats/`
/// existed. Kept named so that a caller who really wants the old scope SAYS
/// so, instead of recreating it with a filter written in its corner.
pub fn command_fixtures() -> Vec<Fixture> {
    fixtures()
        .into_iter()
        .filter(|f| f.route == Route::Command)
        .collect()
}

/// The WEB PAGES corpus — 39 real pages captured under
/// `tests/fixtures/pages`, returned as is.
///
/// It was enumerated by THREE guards that did not talk to each other
/// (`determinism.rs`, `adversarial_inputs.rs`, `web_headroom.rs`), each with
/// its `read_dir`, its `.html` filter and its sort: the same decision in
/// three copies, so widening the corpus would have served only one. That is
/// exactly what this file's header tells of the COMMANDS corpus, a second
/// time and on the other path.
///
/// Returned as `(file name, content)`, the shape the three expected.
pub fn pages() -> Vec<(String, String)> {
    html_corpus("pages")
}

/// The recorded SEARCH RESULTS pages (`tests/fixtures/serp`). Same shape,
/// same sort, same filter — a fourth copied `read_dir` lived in
/// `determinism.rs`, on the neighbouring corpus.
pub fn serp_pages() -> Vec<(String, String)> {
    html_corpus("serp")
}

fn html_corpus(sub: &str) -> Vec<(String, String)> {
    let dir = root().join(sub);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut v: Vec<(String, String)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("html"))
        .filter_map(|p| {
            Some((
                p.file_name()?.to_string_lossy().into_owned(),
                std::fs::read_to_string(&p).ok()?,
            ))
        })
        .collect();
    v.sort();
    v
}

/// rot13 of the LABELS only: values, numbers and punctuation stay intact, so
/// the output keeps exactly its shape and loses all its language. A capture
/// scrambled this way is the only honest counter-example to "this compactor
/// does not read words".
///
/// Restricted to BARE labels (before the first `:`, only letters and word
/// punctuation): a first version ate `10.0.0.7` in `10.0.0.7:48970` and
/// `/dev/disk0 (internal, physical):`, so it destroyed DATA and fabricated
/// four false positives.
///
/// It also NORMALISES line endings, because it reassembles through
/// `lines()`. Comparing a fold on the raw input to a fold on the scrambled
/// one then compares two inputs that differ by more than the language — on
/// the `windows/systeminfo` capture, 68 fewer CRLFs. Pass BOTH sides through
/// [`normalise_newlines`] before comparing them.
pub fn mangle_labels(text: &str) -> String {
    fn rot13(s: &str) -> String {
        s.chars()
            .map(|c| match c {
                'a'..='z' => (((c as u8 - b'a' + 13) % 26) + b'a') as char,
                'A'..='Z' => (((c as u8 - b'A' + 13) % 26) + b'A') as char,
                other => other,
            })
            .collect()
    }
    fn is_a_label(s: &str) -> bool {
        !s.trim().is_empty()
            && s.chars().all(|c| {
                c.is_alphabetic() || c.is_whitespace() || matches!(c, '(' | ')' | '-' | '_' | ',')
            })
    }
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        match line.split_once(':') {
            Some((label, rest)) if is_a_label(label) => {
                out.push_str(&rot13(label));
                out.push(':');
                out.push_str(rest);
            }
            _ => out.push_str(line),
        }
        out.push('\n');
    }
    out
}

/// The same normalisation [`mangle_labels`] applies in passing, isolated so
/// that the other side of a comparison undergoes it too.
pub fn normalise_newlines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// The program this fixture is the output of.
///
/// The LONGEST prefix the router knows, tried with a hyphen as well as with
/// an underscore: `tasklist_svc_full` is `tasklist-svc`, not `tasklist`, and
/// `ps_get_psdrive` is `ps-get`, not `ps`.
///
/// Written once, because it was written FIVE times: two verbatim copies
/// (`richness_corpus.rs`, `determinism.rs`) and three cruder versions that
/// cut at the first underscore (`corpus_error_survival.rs`,
/// `idempotence.rs`, `language_free_fold.rs`). Five guards driven by the same
/// corpus did not exercise the same compactors, and nothing said so.
pub fn program_of(key: &str) -> Option<String> {
    tokenade_core::corpus_key::program_of(key)
}

impl Fixture {
    /// The program this fixture is the output of — see [`program_of`].
    pub fn program(&self) -> Option<String> {
        program_of(&self.key)
    }
}
