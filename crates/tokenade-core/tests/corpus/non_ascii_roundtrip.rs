//! Text that is not ASCII must survive every compactor that claims to keep it.
//!
//! This is a whole CLASS of defect, not one bug. A compactor whose delimiters
//! are all ASCII — quotes, `//`, `#`, `<`, `>` — is correctly written as a byte
//! scanner: UTF-8 is self-synchronising, so no ASCII byte ever appears inside a
//! multibyte sequence and the scan cannot desynchronise. What it must not do is
//! *emit* those bytes one at a time as `char`, because `b as char` reinterprets
//! every byte above 0x7F as its Latin-1 codepoint. One inline tag, one string
//! literal, one comment marker on the line, and the agent is handed mojibake
//! that reads exactly like the document.
//!
//! Found the first time by surveying real files on disk. Every unit test in the
//! crate was written in ASCII, so none of them could see it. This file exists so
//! the next one is caught by the suite instead.

/// Scripts an agent actually meets, plus the accented Latin that is easiest to
/// break and hardest to notice. Written as escapes: this source stays ASCII so
/// no tool in the chain can corrupt the expectation itself.
fn samples() -> Vec<(&'static str, String)> {
    vec![
        (
            "hangul",
            "\u{c548}\u{b155}\u{d558}\u{c138}\u{c694}".to_string(),
        ),
        (
            "kanji",
            "\u{65e5}\u{672c}\u{8a9e}\u{306e}\u{30c6}\u{30ad}\u{30b9}\u{30c8}".to_string(),
        ),
        (
            "cyrillic",
            "\u{41f}\u{440}\u{438}\u{432}\u{435}\u{442}".to_string(),
        ),
        (
            "greek",
            "\u{39a}\u{3b1}\u{3bb}\u{3b7}\u{3bc}\u{3ad}\u{3c1}\u{3b1}".to_string(),
        ),
        ("arabic", "\u{645}\u{631}\u{62d}\u{628}\u{627}".to_string()),
        (
            "accents",
            "caf\u{e9} cr\u{e8}me br\u{fb}l\u{e9}e na\u{ef}ve".to_string(),
        ),
        ("emoji", "\u{1f680}\u{1f4a1}\u{2705}".to_string()),
    ]
}

/// Every case is `(what it is, the text to compact, the compactor)`.
type Case = (&'static str, Box<dyn Fn(&str) -> String>);

fn cases() -> Vec<Case> {
    use tokenade_core::code_strip::{Language, StripMode};
    let mut v: Vec<Case> = Vec::new();
    for (name, lang) in [
        ("rust", Language::Rust),
        ("python", Language::Python),
        ("javascript", Language::JavaScript),
        ("go", Language::Go),
        ("java", Language::Java),
        ("ruby", Language::Ruby),
        ("shell", Language::Shell),
        ("php", Language::Php),
    ] {
        for (mode_name, mode) in [("comments", StripMode::Comments), ("full", StripMode::Full)] {
            let label: &'static str =
                Box::leak(format!("code_strip:{name}:{mode_name}").into_boxed_str());
            v.push((
                label,
                Box::new(move |s: &str| tokenade_core::code_strip::strip(s, lang, mode)),
            ));
        }
    }
    v.push(("proto", Box::new(tokenade_core::proto::compact)));
    v.push(("sql_source", Box::new(tokenade_core::sql_source::compact)));
    v.push(("hcl", Box::new(tokenade_core::hcl::compact)));
    v.push(("markdown", Box::new(tokenade_core::markdown::compact)));
    v
}

/// A source file whose STRING LITERALS and identifiers are not ASCII. Comments
/// may legitimately be removed; the code around them may not be rewritten.
fn source_for(case: &str, text: &str) -> String {
    match case.split(':').nth(1).unwrap_or(case) {
        "rust" => format!(
            "// leading comment\nfn main() {{\n    let msg = \"{text}\";\n    println!(\"{{msg}}\");\n}}\n"
        ),
        "python" | "shell" => format!("# leading comment\nmsg = \"{text}\"\nprint(msg)\n"),
        "ruby" => format!("# leading comment\nmsg = \"{text}\"\nputs msg\n"),
        "php" => format!("<?php\n// leading comment\n$msg = \"{text}\";\necho $msg;\n"),
        _ => format!(
            "// leading comment\nfunction main() {{\n    const msg = \"{text}\";\n    return msg;\n}}\n"
        ),
    }
}

fn document_for(case: &str, text: &str) -> String {
    match case {
        // The value must sit where the compactor is meant to KEEP it. A
        // comment is fair game — that is the compactor's job — so the sample
        // goes in a default, which is data.
        "proto" => format!(
            "syntax = \"proto3\";\n/* block */\nmessage M {{\n  // field\n  string label = 1 [default = \"{text}\"];\n  string body = 2;\n}}\n"
        ),
        "sql_source" => format!(
            "/* header */\nSELECT id, name\nFROM t\nWHERE label = '{text}'\n  AND active = true;\n"
        ),
        "hcl" => format!(
            "# header\nresource \"aws_s3_bucket\" \"b\" {{\n  bucket = \"{text}\"\n  tags = {{ Name = \"{text}\" }}\n}}\n"
        ),
        "markdown" => format!(
            "# Title\n\n{text} <sub>note</sub> {text}\n\n{text} ![diagram](https://e.co/a.png)\n\n- one\n- two\n"
        ),
        c => source_for(c, text),
    }
}

#[test]
fn no_compactor_turns_non_ascii_into_mojibake() {
    let mut broken = Vec::new();
    for (name, compact) in cases() {
        for (script, text) in samples() {
            let input = document_for(name, &text);
            let out = compact(&input);
            if !out.contains(text.as_str()) {
                // Report the smallest useful evidence: the surviving line.
                let near = out
                    .lines()
                    .find(|l| l.contains("msg") || l.contains("label") || l.contains("bucket"))
                    .unwrap_or("<line gone>")
                    .chars()
                    .take(90)
                    .collect::<String>();
                broken.push(format!("{name}/{script}: {near}"));
            }
        }
    }
    assert!(
        broken.is_empty(),
        "compactors rewrote non-ASCII text ({} case(s)):\n{}",
        broken.len(),
        broken.join("\n")
    );
}

/// The same text, run through the compactor twice, must not drift further.
/// A byte-level rewrite is often idempotent-looking on ASCII and cumulative on
/// anything else, which is how one gets mojibake of mojibake in a cached path.
#[test]
fn compacting_twice_changes_nothing_more_than_once() {
    for (name, compact) in cases() {
        for (script, text) in samples() {
            let once = compact(&document_for(name, &text));
            let twice = compact(&once);
            assert_eq!(
                once.contains(text.as_str()),
                twice.contains(text.as_str()),
                "{name}/{script}: a second pass changed whether the text survives"
            );
        }
    }
}

/// A wall served in Japanese is still a wall. The visible-text ceilings that
/// separate a challenge page from a real one counted BYTES, so any non-Latin
/// page measured two to three times longer than it is and sailed over them —
/// the wall was then handed back as if it were the site.
#[test]
fn a_non_latin_page_is_measured_in_characters_not_bytes() {
    let short = "\u{4e00}".repeat(40); // 40 characters, 120 bytes
    let wall = format!(
        "<html><head><title>\u{4e00}\u{4e8c}</title></head><body><div>{short}</div></body></html>"
    );
    let latin = wall.replace(&short, &"x".repeat(40));
    assert_eq!(
        tokenade_core::stealth::detect_challenge(&wall).is_some(),
        tokenade_core::stealth::detect_challenge(&latin).is_some(),
        "the same page in two scripts got two different verdicts"
    );
}
