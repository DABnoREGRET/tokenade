//! **What is TEST code, and why the index must know.**
//!
//! A symbol declared in a test module is not an answer to "where is
//! `parse_config`?"; yet it weighs as much as any other in a ranking, and this
//! repo carries more test lines than production ones. The question arises at
//! three scales — the file path, the module, the symbol — and the three
//! answers do not coincide: a sibling file `x_tests.rs` is not in a `tests/`
//! directory, and a `#[cfg(test)] mod` lives in the middle of a production
//! file.

use super::*;

/// Spans of the `mod tests` / `mod test` BLOCKS among `symbols`.
///
/// `end_line > line` on purpose: a bare `mod test;` declaration points at
/// another file and encloses nothing here, and its one-line span would
/// otherwise swallow whatever shares that line.
/// Is this module declaration a test module?
///
/// By its ATTRIBUTE first, and only then by its name. `#[cfg(test)]` is what
/// actually says "none of this is part of the crate", and a module wearing it
/// may be called anything: MEASURED on this repository, **296 of 1 491
/// `#[cfg(test)]` modules — one in five — are not named `test` or `tests`**,
/// and every helper inside them was reported as a declaration of its file.
/// `tokenade impact` on `pem.rs` listed `handshake`, a fixture builder that
/// exists only under `cfg(test)`, among the five functions the file declares.
///
/// The harm is not only cosmetic. `impact_of` builds its `declared` set from
/// these names and its ambiguity map from every symbol sharing one, so a test
/// helper called `out` or `sym` made a real name look as though it had two
/// homes — which the comment on `declared` already describes, having fixed
/// exactly half of it.
///
/// The name test stays for the languages that have no such attribute: Python
/// and TypeScript mark tests by naming, and the path rule below carries them.
pub(super) fn module_is_test(s: &Symbol) -> bool {
    s.decorators
        .iter()
        .any(|d| d.replace(' ', "").contains("cfg(test)"))
        || s.name == "tests"
        || s.name == "test"
}

pub(crate) fn test_module_spans<'a>(
    symbols: impl Iterator<Item = &'a Symbol>,
) -> Vec<(usize, usize)> {
    symbols
        .filter(|s| s.kind == "module" && s.end_line > s.line && module_is_test(s))
        .map(|s| (s.line, s.end_line))
        .collect()
}

/// Does this PATH name a test file, by the conventions every test runner uses?
///
/// The attribute and `mod tests` signals are Rust's, and they see nothing at
/// all in another language: measured on three indexed repositories, inline
/// detection found 44% of tokenade's own symbols, **0% of a Python project's
/// (chonkie) where the path rule finds 63%**, and 0% of a TypeScript one's.
/// Without this, `map` and `impact` read a repository that is two thirds test
/// as if all of it were code.
///
/// Directory components, not name guessing: `test_connection` is an API
/// function in plenty of codebases, and rejecting it on its name would be a
/// known mistake. `testing/` is deliberately NOT in the set — a library
/// can ship test helpers under that name for its users to import.
pub(crate) fn file_is_test_path(path: &str) -> bool {
    let mut parts = path.split('/').peekable();
    let mut base = "";
    while let Some(p) = parts.next() {
        if parts.peek().is_none() {
            base = p;
            break;
        }
        if matches!(p, "tests" | "test" | "__tests__" | "spec" | "specs") {
            return true;
        }
    }
    // `test_*` ONLY where a runner would collect it — pytest and minitest
    // do, and `src/net/test_connection.rs` is a Rust function's file, not a
    // test file.
    let stem_marks_test = (base.starts_with("test_")
        && (base.ends_with(".py") || base.ends_with(".rb")))
        || base.ends_with("_test.py")
        || base.ends_with("_test.go")
        || base.ends_with("_test.rs")
        || base.ends_with("_test.rb")
        || base.ends_with("_test.java")
        || base.ends_with("_test.kt")
        || base.ends_with("_spec.rb")
        || base == "conftest.py";
    if stem_marks_test {
        return true;
    }
    // `foo.test.ts`, `foo.spec.js` and friends.
    matches!(
        base.rsplit_once('.')
            .map(|(head, ext)| (head.rsplit_once('.').map(|(_, mid)| mid).unwrap_or(""), ext)),
        Some(("test" | "spec", "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs"))
    )
}

/// Is this declaration part of a file's tests rather than of what it offers?
///
/// ONE rule for every consumer — `impact` and `map` both ask it, and a file
/// that is half tests must not read as twice its size in one command and not
/// the other. Two signals, because each misses what the other catches:
/// measured on this workspace, 1 766 functions carry a test attribute whose
/// module span was never captured, 706 sit in a `mod tests` with no attribute
/// of their own, and 7 120 have both.
pub(crate) fn symbol_is_test(s: &Symbol, spans: &[(usize, usize)]) -> bool {
    file_is_test_path(&s.file)
        || s.has_test_attribute()
        || spans.iter().any(|(a, b)| *a <= s.line && s.line <= *b)
}
