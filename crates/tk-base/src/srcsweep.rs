//! The half of a source file that ships.
//!
//! Several guards in this crate read their own source and assert something
//! about every PRODUCTION use of a symbol — a path hint that must be refined,
//! a listener that must ask whether its address is
//! local, a common prefix that must ask the one declarer. All of
//! them have to skip test code, because a test that calls the raw form is
//! usually asking a different question on purpose.
//!
//! Two of them spelled that skip as `src.split("\n#[cfg(test)]").next()`, and
//! that spelling assumes the tests are LAST. Measured 2026-08-27: `auto.rs`
//! carries ten `#[cfg(test)]` blocks and the first is at line 1041 of 3397, so
//! the guard was reading under a third of the file and would have been blind
//! to any entry point below it. `git_listing.rs` puts a test module at line 74
//! and a production function at 123, so the same spelling hid the very call
//! site a guard had just been widened to count.
//!
//! One declarer instead, and it tracks braces rather than trusting a
//! convention.

/// Lines of `src` that are NOT inside a `#[cfg(test)]` item, as
/// `(one-based line number, line)`.
///
/// A `#[cfg(test)]` attribute applies to the item that follows it, so the skip
/// runs from that attribute to the closing brace of that item, at whatever
/// depth it sits. Braces inside string literals and comments are not parsed:
/// this is a lint over Rust source written by this repo, not a Rust parser,
/// and the floors every caller asserts are what catch a miscount.
pub fn production_lines(src: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut pending_cfg = false;
    let mut depth: i32 = 0;
    let mut in_test = false;
    let mut in_str = StrState::Code;
    for (i, line) in src.lines().enumerate() {
        let t = line.trim();
        if in_test {
            depth += braces(line, &mut in_str);
            if depth <= 0 {
                in_test = false;
                in_str = StrState::Code;
            }
            continue;
        }
        if t.starts_with("#[cfg(test)]") {
            pending_cfg = true;
            continue;
        }
        if pending_cfg {
            // Further attributes between the cfg and its item.
            if t.starts_with("#[") || t.starts_with("//") || t.is_empty() {
                continue;
            }
            pending_cfg = false;
            in_str = StrState::Code;
            depth = braces(line, &mut in_str);
            if depth > 0 {
                in_test = true;
            }
            // A one-line item (`#[cfg(test)] use x;`) is skipped either way.
            continue;
        }
        out.push((i + 1, line));
    }
    out
}

/// Net brace depth of `line`, counting only braces the compiler sees.
///
/// Counting them RAW is what this function used to do, and the module note
/// above says the floors callers assert would catch a miscount. They do not
/// catch a PARTIAL one. Measured 2026-09-01 with this very function:
/// a since-removed 3,736-line module stopped at line 1 500 and hid **24 shipped items**
/// (`pub struct UsageEvent`, `pub fn reporter_offset_for_ledger`, …) from
/// every guard built on this sweep; `claudecode_transcript.rs` hid 7,
/// `treesitter.rs` 6, `platform.rs` 4. A test module whose fixtures carry an
/// odd brace inside a string never closes, so the sweep runs to end of file —
/// the same blindness, in the same reassuring direction, that replacing
/// `split("\n#[cfg(test)]")` was meant to end.
///
/// `in_str` carries an unterminated string across lines. Strings, char
/// literals and line comments are skipped; raw strings are not modelled, and
/// the guard below asserts on the real corpus rather than on that assumption.
#[derive(Clone, Copy, PartialEq)]
enum StrState {
    Code,
    Plain,
    /// Inside `r#"…"#`, holding how many `#` close it.
    Raw(usize),
}

/// The production half of `src`, as one string.
///
/// Same rule as [`production_lines`], for the guards that want to search text
/// rather than walk lines. It exists so there is ONE declarer: two more
/// hand-written strippers had grown in this repo — one in `healthcheck`, one
/// in the e2e suite — and each carried its own brace bug.
pub fn production_source(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    for (_, line) in production_lines(src) {
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn braces(line: &str, st: &mut StrState) -> i32 {
    let b = line.as_bytes();
    let mut depth = 0i32;
    let mut i = 0usize;
    while i < b.len() {
        match *st {
            StrState::Plain => {
                match b[i] {
                    b'\\' => i += 1,
                    b'"' => *st = StrState::Code,
                    _ => {}
                }
                i += 1;
                continue;
            }
            StrState::Raw(h) => {
                // No escapes in a raw string: only `"` plus h hashes ends it.
                if b[i] == b'"' && b[i + 1..].iter().take(h).filter(|c| **c == b'#').count() == h {
                    *st = StrState::Code;
                    i += 1 + h;
                    continue;
                }
                i += 1;
                continue;
            }
            StrState::Code => {}
        }
        match b[i] {
            // A raw string keeps its braces AND its quotes: reading `r#"{"a"`
            // as an ordinary string closed at the quote before `a`, which let
            // the JSON braces of a fixture count. That is what still hid
            // `claudecode_transcript.rs`'s tail after plain strings were
            // handled.
            b'r' if raw_open(&b[i..]).is_some() => {
                let h = raw_open(&b[i..]).unwrap_or(0);
                *st = StrState::Raw(h);
                i += 1 + h + 1;
                continue;
            }
            b'"' => *st = StrState::Plain,
            // `'{'` is a char literal; `'a` is a lifetime and has no closer.
            b'\'' if i + 2 < b.len() && b[i + 2] == b'\'' => i += 2,
            b'/' if i + 1 < b.len() && b[i + 1] == b'/' => return depth,
            b'{' => depth += 1,
            b'}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    depth
}

/// `Some(hashes)` when `b` starts a raw string (`r"`, `r#"`, `r##"`, …).
/// `src` with its comments and its string and char LITERALS removed.
///
/// The companion question to [`production_lines`]: that one asks which half of
/// a file ships, this one asks which bytes the compiler reads as code. A guard
/// that greps raw source counts a module named in a sentence of prose as a
/// dependency — the layer guard proved that on itself, declaring `table_fold`
/// in breach towards `cmd` because its header RECOUNTS having lived under
/// `cmd/linux/`.
///
/// Written here, and not a fourth time in a test file, because the version it
/// replaces had a hole that no floor could catch. It did not model a CHAR
/// literal, so `'"'` — one line of `disclose/scoped.rs`, a quote-stripping
/// tokeniser — opened a string that only closed at the next `"` in the file.
/// Everything between was dropped, and `crate::rag::prompt_scoped_extract_rag`
/// on line 221 sat inside that hole: an upward edge from the fold layer to the
/// code layer that the guard could not see, on a day its debt table said ONE
/// edge remained. A stripper that loses code fails SILENTLY and in the
/// reassuring direction.
///
/// Raw strings are modelled for the same reason `braces` models them: `r#"{"a"`
/// is not a string that ends before `a`.
pub fn code_only(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0usize;
    let mut block = 0usize;
    while i < b.len() {
        if block > 0 {
            if b[i..].starts_with(b"*/") {
                block -= 1;
                i += 2;
            } else if b[i..].starts_with(b"/*") {
                block += 1;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        if b[i..].starts_with(b"//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b[i..].starts_with(b"/*") {
            block = 1;
            i += 2;
            continue;
        }
        if let Some(h) = raw_open(&b[i..]) {
            i += 1 + h + 1;
            while i < b.len() {
                if b[i] == b'"' && b[i + 1..].iter().take(h).filter(|c| **c == b'#').count() == h {
                    i += 1 + h;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if b[i] == b'"' {
            i += 1;
            while i < b.len() {
                match b[i] {
                    b'\\' => i += 2,
                    b'"' => {
                        i += 1;
                        break;
                    }
                    _ => i += 1,
                }
            }
            continue;
        }
        // A char literal, and only a char literal: `'a'`, `'\''`, `'"'`. A
        // LIFETIME (`&'a str`, `Foo<'_>`) wears the same apostrophe and must
        // stay, so the closing quote is REQUIRED — without it the sweep would
        // swallow the rest of the file on every generic bound.
        if b[i] == b'\'' {
            let end = if b.get(i + 1) == Some(&b'\\') {
                (b.get(i + 3) == Some(&b'\'')).then_some(i + 4)
            } else {
                (b.get(i + 2) == Some(&b'\'')).then_some(i + 3)
            };
            if let Some(e) = end {
                i = e;
                continue;
            }
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

fn raw_open(b: &[u8]) -> Option<usize> {
    if b.first() != Some(&b'r') {
        return None;
    }
    let hashes = b[1..].iter().take_while(|c| **c == b'#').count();
    if b.get(1 + hashes) == Some(&b'"') {
        Some(hashes)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hole that made this function a declarer instead of a fourth copy.
    ///
    /// `'"'` is one character of Rust, and read as the opening of a string it
    /// swallows every byte up to the next quote. In `disclose/scoped.rs` that
    /// hid `crate::rag::…` from the layer guard — an upward edge invisible on a
    /// day the debt table claimed one edge remained.
    #[test]
    fn a_char_literal_holding_a_quote_does_not_open_a_string() {
        let src = "match ch { '\"' => quote = Some(ch), _ => {} }\nlet _ = crate::rag::go();\n";
        assert!(code_only(src).contains("crate::rag"), "{}", code_only(src));
    }

    /// The apostrophe of a LIFETIME is not a literal, and treating it as one
    /// would swallow the rest of the file at the first generic bound — the same
    /// silent loss, arriving from the other side.
    #[test]
    fn a_lifetime_is_not_a_char_literal() {
        let src = "fn f<'a>(x: &'a str) -> Cow<'_, str> { crate::paths::of(x) }\n";
        let out = code_only(src);
        assert!(out.contains("crate::paths"), "{out}");
    }

    #[test]
    fn comments_and_literals_go_and_code_stays() {
        let src = "// crate::commented\n\
                   let s = \"crate::in_a_string\";\n\
                   let r = r#\"crate::in_a_raw{\"#;\n\
                   /* crate::in_a_block /* nested */ still_block */\n\
                   crate::real::call();\n";
        let out = code_only(src);
        assert!(out.contains("crate::real"), "{out}");
        for hidden in [
            "commented",
            "in_a_string",
            "in_a_raw",
            "in_a_block",
            "still_block",
        ] {
            assert!(!out.contains(hidden), "`{hidden}` survived: {out}");
        }
    }

    /// An escaped quote inside a string does not end it.
    #[test]
    fn an_escaped_quote_does_not_end_the_string() {
        let src = "let s = \"a\\\"b crate::hidden\"; crate::kept::go();\n";
        let out = code_only(src);
        assert!(out.contains("crate::kept"), "{out}");
        assert!(!out.contains("crate::hidden"), "{out}");
    }

    #[test]
    fn a_test_module_before_production_code_does_not_hide_it() {
        let src = "fn a() {}\n\
                   #[cfg(test)]\n\
                   mod t {\n\
                   fn helper() { let _ = 1; }\n\
                   }\n\
                   fn b() {}\n";
        let kept: Vec<&str> = production_lines(src).into_iter().map(|(_, l)| l).collect();
        assert!(kept.iter().any(|l| l.contains("fn b()")), "{kept:?}");
        assert!(
            !kept.iter().any(|l| l.contains("helper")),
            "test body leaked: {kept:?}"
        );
    }

    /// A test module whose fixtures carry braces inside STRINGS still ends.
    ///
    /// Counting braces raw, an odd `{` in a string left the sweep inside the
    /// module to end of file. Measured with this function on 2026-09-01:
    /// a since-removed 3,736-line module stopped at line 1 500 and hid 24 shipped items —
    /// `pub struct UsageEvent`, `pub fn reporter_offset_for_ledger` and 22
    /// more — from every guard built on this sweep, plus 7 in
    /// `claudecode_transcript.rs` and 4 in `platform.rs`. The module note
    /// above says the floors callers assert would catch a miscount; they
    /// catch a TOTAL collapse, not a file read to 40 %.
    #[test]
    fn a_brace_inside_a_string_does_not_swallow_the_rest_of_the_file() {
        // Plain string, raw string with its own quotes and braces, char
        // literal, lifetime, line comment — one of each, because each is a
        // way the counter desynchronised.
        // Every brace below is UNMATCHED on purpose. Counted, any one of them
        // leaves the sweep inside the module for the rest of the file — a
        // balanced fixture proves nothing, because mis-parsing it changes no
        // outcome.
        let src = "fn a() {}\n\
                   #[cfg(test)]\n\
                   mod t {\n\
                   fn f() { let s = \"an odd { brace\"; }\n\
                   fn g() { let j = r#\"a\"{\"#; }\n\
                   fn h() { let c = '{'; }\n\
                   fn i<'a>(x: &'a str) -> u32 { 1 }\n\
                   // a lone { in a comment\n\
                   fn k() { let m = \"opens { here\n\
                   and closes on the next line\"; }\n\
                   }\n\
                   #[cfg(test)]\n\
                   mod u {\n\
                   fn second() { let z = 1; }\n\
                   }\n\
                   pub fn after_the_tests() {}\n";
        let kept: Vec<&str> = production_lines(src).into_iter().map(|(_, l)| l).collect();
        assert!(
            kept.iter().any(|l| l.contains("after_the_tests")),
            "the file's tail was swallowed: {kept:?}"
        );
        // And nothing from INSIDE the module leaked, which is what an early
        // close looks like.
        // The SECOND module is what proves the string state is reset when a
        // module ends: carried over, its `mod u {` reads as string content,
        // the module is never entered, and its body leaks.
        for leaked in [
            "let s =", "let j =", "let c =", "let m =", "-> u32", "let z =",
        ] {
            assert!(
                !kept.iter().any(|l| l.contains(leaked)),
                "`{leaked}` leaked out of the test module: {kept:?}"
            );
        }
    }

    /// **The real corpus, not a fixture**: a PRODUCTION item declared
    /// BELOW a test module must stay visible. A fixture written next to
    /// the rule proves it right by construction; the repo does not.
    ///
    /// The specimen is SEARCHED for, not named. This test used to read one named
    /// file, and it went red saying "`pub struct UsageEvent` sits below this file's test
    /// module" the day that type moved down into a submodule — a
    /// file move reported as a defect of the sweep.
    ///
    /// The search does NOT READ the lines to guess which ones are code:
    /// a first version did and accused `makefile.rs`, whose
    /// "`pub fn other`" lives in a raw string `r#"…"#` of a test
    /// fixture. That is precisely what the sweep must set aside — asking it
    /// where it sees shipped code is the only well-posed question.
    #[test]
    fn a_production_item_below_a_test_module_is_seen_in_this_repo() {
        let mut specimens = Vec::new();
        for root in crate::source_guard::crate_srcs() {
            for (path, src) in crate::source_guard::rust_sources_whole(&root) {
                if path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().ends_with("_tests.rs"))
                {
                    continue;
                }
                let Some(at) = src.lines().position(|l| l.starts_with("#[cfg(test)]")) else {
                    continue;
                };
                // `production_lines` returns the NUMBER of every shipped line. A
                // shipped line past the opening of the test module IS the
                // case to cover, and it is the only way to spot it without
                // re-reading the file oneself.
                if production_lines(&src).iter().any(|(n, _)| *n > at + 1) {
                    specimens.push(path);
                }
            }
        }
        // MEASURED on 2026-09-13: **57 files** carry this shape — it is
        // the rule, not a curiosity. The floor refuses the easy green — if
        // the sweep stopped seeing below a test module, the list
        // would drop to zero and this test would pass without measuring anything.
        assert!(
            specimens.len() >= 20,
            "only {} file(s) carry a shipped item BELOW a test \
             module: either the sweep stopped seeing them, or this test no \
             longer measures anything — {specimens:?}",
            specimens.len()
        );
    }

    #[test]
    fn a_cfg_test_function_is_skipped_like_a_module() {
        let src = "#[cfg(test)]\n#[allow(dead_code)]\nfn only_for_tests() {\n    let _ = 1;\n}\nfn shipped() {}\n";
        let kept: Vec<&str> = production_lines(src).into_iter().map(|(_, l)| l).collect();
        assert!(kept.iter().any(|l| l.contains("fn shipped()")), "{kept:?}");
        assert!(
            !kept.iter().any(|l| l.contains("only_for_tests")),
            "{kept:?}"
        );
    }

    #[test]
    fn line_numbers_are_one_based_and_survive_the_skip() {
        let src = "a\n#[cfg(test)]\nmod t {\n}\nb\n";
        let got = production_lines(src);
        assert_eq!(got.first().map(|(n, _)| *n), Some(1));
        assert_eq!(got.last().map(|(n, _)| *n), Some(5));
    }
}
