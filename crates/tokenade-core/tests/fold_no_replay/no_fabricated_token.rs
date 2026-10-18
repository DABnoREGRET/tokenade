//! **A word the page does not contain.**
//!
//! The repo's ratchets measure what SURVIVES — fidelity, richness, bytes.
//! Nothing measured what is INVENTED, and two iterations paid for it: the
//! conversion welded two neighbours that CSS separates, and the word born of
//! the weld exists neither in the page nor in its source code. MEASURED on
//! 2026-09-09 on this corpus: **60 words over 4 pages**, including 54
//! `amountinteger` and the like on an API reference — the document where an
//! agent copies field names — and an `8232` that was someone's reputation,
//! wrong. Fixed since; this file is what prevents it from coming back.
//!
//! **The reference keeps its TAGS.** Removing them first would glue
//! `<span>amount</span><span>integer</span>` into `amountinteger`, which would
//! then be found in the "source" — the detector would validate the fault it is
//! looking for. The price of that rigour is one exception, named and explained
//! below.

use std::path::PathBuf;

pub(crate) fn pages_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pages")
}

/// The LEGITIMATE welds: an element followed by TEXT, hence the two halves of
/// a single word, which the conversion is right to glue back.
///
/// `docs.rs/serde` writes `for <em><strong>ser</strong></em>ializing and
/// <em><strong>de</strong></em>serializing Rust`: "deserializing" is the word
/// the page DISPLAYS, and it appears nowhere in one piece in its source code.
/// That is exactly the case the `web_tree` boundary spares — the exception is
/// therefore proof that the boundary holds, not a debt.
const LEGITIMATE_JOINS: &[&str] = &["deserializing"];

/// Long enough for an accidental collision to be implausible, short enough to
/// catch `amountinteger` (13) and `badges196196` (12).
const LONG_ENOUGH_TO_BE_A_WORD: usize = 12;

/// The product's text, not the page's: these lines carry OUR counts and our
/// recovery paths, which have no reason to be in the source.
pub(crate) fn ours(line: &str) -> bool {
    line.starts_with("[tokenade")
        || line.starts_with("[disclose")
        || line.starts_with("[stashed")
        || line.starts_with('⟨')
}

fn words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            cur.push(c);
        } else {
            if cur.chars().count() >= LONG_ENOUGH_TO_BE_A_WORD {
                out.push(std::mem::take(&mut cur));
            } else {
                cur.clear();
            }
        }
    }
    if cur.chars().count() >= LONG_ENOUGH_TO_BE_A_WORD {
        out.push(cur);
    }
    out
}

/// **Replay memory must be OUT of this test.**
///
/// These three tests sweep the same corpus in the same process. Replay memory
/// replaces a page ALREADY delivered in the session with a short pointer: the
/// first test sees whole pages, the next ones see pointers. Under nextest,
/// each test has its process and the question does not arise; under
/// `cargo test --test-threads=1` — the release runner — the third one examined
/// only 130 numbers instead of 500, and its reach floor said so rather than
/// passing empty. Replay is not at fault: it does its job. It is this test
/// that measures the delivery of a page, not the behaviour of a session.
fn without_replay_memory() {
    // SAFETY: this test binary only contains tests that want this,
    // and a test binary is its own process.
    unsafe { std::env::set_var("TOKENADE_NO_REPLAY", "1") };
}

#[test]
fn no_page_delivers_a_word_its_source_does_not_contain() {
    without_replay_memory();
    let mut seen = 0usize;
    let mut examined = 0usize;
    let mut faults: Vec<String> = Vec::new();
    for e in std::fs::read_dir(pages_dir())
        .expect("the pages directory")
        .flatten()
    {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("html") {
            continue;
        }
        let Ok(html) = std::fs::read_to_string(&p) else {
            continue;
        };
        seen += 1;
        let name = p
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        // Entities decoded, whitespace removed, **tags kept**.
        let source: String = tokenade_core::web::decode_entities(&html)
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let d = tokenade_core::web_pipeline::distill_page(
            &html,
            &format!("https://example.test/{name}"),
            Some("text/html; charset=utf-8"),
        );
        let md =
            tokenade_core::disclose::maybe_stash(d.md, tokenade_core::disclose::SourceHint::web());
        let delivered: String = md
            .lines()
            .filter(|l| !ours(l))
            .collect::<Vec<_>>()
            .join("\n");
        let mut missing: Vec<String> = Vec::new();
        for w in words(&delivered) {
            examined += 1;
            if LEGITIMATE_JOINS.contains(&w.as_str()) {
                continue;
            }
            if !source.contains(&w) && !missing.contains(&w) {
                missing.push(w);
            }
        }
        if !missing.is_empty() {
            missing.sort();
            missing.truncate(6);
            faults.push(format!("{name}: {}", missing.join(", ")));
        }
    }
    assert!(
        seen >= 30,
        "only {seen} page(s) swept: the corpus is no longer there"
    );
    // **And the detector did LOOK.** Without this floor, a corpus that stopped
    // producing long words would turn the test green without checking
    // anything.
    assert!(
        examined >= 200,
        "only {examined} word(s) examined: the detector no longer looks at anything"
    );
    assert!(
        faults.is_empty(),
        "{} page(s) deliver a word absent from their source — two neighbours were \
         welded, and the agent reads an identifier that does not exist:\n  {}",
        faults.len(),
        faults.join("\n  ")
    );
}

/// The LOOKALIKES: a character the product puts in place of a character of the
/// page. It does not make a twelve-letter word, so the detector above does not
/// see it — and it is the worst place to put one, because an agent copies a
/// documentation cell as is.
///
/// `¦` (U+00A6) is the only one this repo has ever emitted: a cell's vertical
/// bar, replaced so the line keeps its columns. `string ¦ number` is not what
/// the page says, and code copied from it is wrong.
const LOOKALIKES: &[(char, char)] = &[('\u{a6}', '|')];

#[test]
fn no_page_delivers_a_lookalike_for_a_character_of_its_own() {
    without_replay_memory();
    let mut seen = 0usize;
    let mut escaped_seen = 0usize;
    let mut faults: Vec<String> = Vec::new();
    for e in std::fs::read_dir(pages_dir())
        .expect("the pages directory")
        .flatten()
    {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("html") {
            continue;
        }
        let Ok(html) = std::fs::read_to_string(&p) else {
            continue;
        };
        seen += 1;
        let name = p
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let source = tokenade_core::web::decode_entities(&html);
        let d = tokenade_core::web_pipeline::distill_page(
            &html,
            &format!("https://example.test/{name}"),
            Some("text/html; charset=utf-8"),
        );
        let md =
            tokenade_core::disclose::maybe_stash(d.md, tokenade_core::disclose::SourceHint::web());
        escaped_seen += md.matches("\\|").count();
        for (lookalike, real) in LOOKALIKES {
            let delivered = md.matches(*lookalike).count();
            let in_source = source.matches(*lookalike).count();
            if delivered > in_source {
                faults.push(format!(
                    "{name}: {delivered} × `{lookalike}` delivered for {in_source} in the \
                     source — the page writes `{real}`"
                ));
            }
        }
    }
    assert!(
        seen >= 30,
        "only {seen} page(s) swept: the corpus is no longer there"
    );
    // **The corpus does reach the rule.** A bar in a cell is rare; without
    // this floor, a corpus that no longer carried one would turn this test
    // green without checking anything. `pages/hn_thread` carries 4, in a
    // comment's author line.
    assert!(
        escaped_seen >= 4,
        "only {escaped_seen} escaped bar(s) in the whole corpus: no page puts a \
         vertical bar in a cell any more, so this test proves nothing"
    );
    assert!(
        faults.is_empty(),
        "{} page(s) deliver a lookalike in place of the page's character:\n  {}",
        faults.len(),
        faults.join("\n  ")
    );
}

/// **A NUMBER the page does not contain.**
///
/// The detector above looks for a word of twelve characters or more. A wrong
/// number is shorter and more serious: it is a price, a quantity, a
/// reputation, a size. The weld that produced `amountinteger` also produced
/// `8232` — someone's reputation on Stack Exchange, born of `823` followed by
/// `2 gold badges` — and **no test would have seen that one come back**, four
/// digits being fewer than twelve characters.
///
/// MEASURED 2026-09-10 on the 39 pages: **zero**. The detector is not useless
/// for all that: the `web_tree` boundary is what holds that zero, and the
/// mutation harness shows it — making `welds_a_token` always false turns THIS
/// test red.
///
/// The source keeps its tags, as above, and for the same reason reversed:
/// `<span>82</span><span>3</span>` reduced to alphanumerics gives
/// `82spanspan3`, so `823` is NOT in it. A source stripped of its tags would
/// validate the weld being looked for.
mod a_number_is_a_fact {
    use super::{ours, pages_dir};

    /// Enough digits for an accidental collision to be implausible. Below
    /// that, a page writing `12` somewhere covers everything.
    const LONG_ENOUGH_TO_BE_A_NUMBER: usize = 3;

    /// A number's INTERNAL separator: comma, dot, apostrophe and the
    /// non-breaking thousands spaces. Above all not the ordinary space, which
    /// glues two neighbouring numbers — the probe would then invent what it is
    /// looking for itself (`823 2 gold badges` read as `8232`).
    fn is_a_separator(c: char) -> bool {
        matches!(c, ',' | '.' | '\'' | '\u{2019}' | '\u{202f}' | '\u{00a0}')
    }

    /// The digits of each number of the line, separators removed. What
    /// tokenade COUNTS itself is set aside by its shape, not by a list: a
    /// pointer `⟨12⟩`, an elision `[… 3 lines …]`, a fold
    /// `[×4 identical lines]`, an outline `(7 lines)`.
    fn numbers_of(line: &str) -> Vec<String> {
        let mut cleaned = String::with_capacity(line.len());
        let mut depth = 0usize;
        for c in line.chars() {
            match c {
                '⟨' | '[' | '(' => {
                    depth += 1;
                    cleaned.push(' ');
                }
                '⟩' | ']' | ')' => {
                    depth = depth.saturating_sub(1);
                    cleaned.push(' ');
                }
                _ => cleaned.push(if depth == 0 { c } else { ' ' }),
            }
        }
        let mut out: Vec<String> = Vec::new();
        let mut cur = String::new();
        let mut pending = String::new();
        for c in cleaned.chars() {
            if c.is_ascii_digit() {
                cur.push_str(&std::mem::take(&mut pending));
                cur.push(c);
            } else if !cur.is_empty() && is_a_separator(c) {
                pending.push(c);
            } else {
                pending.clear();
                if cur.chars().count() >= LONG_ENOUGH_TO_BE_A_NUMBER {
                    out.push(std::mem::take(&mut cur));
                } else {
                    cur.clear();
                }
            }
        }
        if cur.chars().count() >= LONG_ENOUGH_TO_BE_A_NUMBER {
            out.push(cur);
        }
        out.into_iter()
            .map(|n| n.chars().filter(char::is_ascii_digit).collect())
            .collect()
    }

    #[test]
    fn no_page_delivers_a_number_its_source_does_not_contain() {
        super::without_replay_memory();
        let mut examined = 0usize;
        let mut seen = 0usize;
        let mut faults: Vec<String> = Vec::new();
        for e in std::fs::read_dir(pages_dir())
            .expect("the pages directory")
            .flatten()
        {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("html") {
                continue;
            }
            let Ok(html) = std::fs::read_to_string(&p) else {
                continue;
            };
            seen += 1;
            let name = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let source: String = tokenade_core::web::decode_entities(&html)
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .collect();
            let d = tokenade_core::web_pipeline::distill_page(
                &html,
                &format!("https://example.test/{name}"),
                Some("text/html; charset=utf-8"),
            );
            let md = tokenade_core::disclose::maybe_stash(
                d.md,
                tokenade_core::disclose::SourceHint::web(),
            );
            let mut missing: Vec<String> = Vec::new();
            for line in md.lines().filter(|l| !ours(l)) {
                for n in numbers_of(line) {
                    examined += 1;
                    if !source.contains(&n) && !missing.contains(&n) {
                        missing.push(n);
                    }
                }
            }
            if !missing.is_empty() {
                missing.truncate(6);
                faults.push(format!("{name}: {}", missing.join(", ")));
            }
        }
        assert!(seen >= 30, "only {seen} page(s) swept");
        // The detector did LOOK.
        assert!(
            examined >= 500,
            "only {examined} number(s) examined: the detector no longer looks at anything"
        );
        assert!(
            faults.is_empty(),
            "{} page(s) deliver a number absent from their source — two neighbours \
             were welded, and the agent reads a figure that does not exist:\n  {}",
            faults.len(),
            faults.join("\n  ")
        );
    }
}
