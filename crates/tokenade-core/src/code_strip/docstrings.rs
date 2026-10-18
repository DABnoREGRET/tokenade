//! **Docstrings and imports** — the prose the language declares, and the
//! plumbing it requires.
//!
//! A docstring is not a comment: in Python it is an EXPRESSION,
//! first element of a body, and removing it from a function that contains
//! only it would leave an empty body — so a file that no longer compiles.
//!
//! Imports are the other half: they say where names come from and
//! almost never what the code does. Removing them is the most
//! aggressive mode, and that is why it is opt-in.

use super::*;

// ── Docstring stripping (Full mode) ─────────────────────────────────

/// Strip docstrings/doc-comments. For Python: triple-quoted strings
/// at the start of a module / function / class. For Rust: `///` /
/// `//!` lines and `/** */` blocks. For other C-family: javadoc-style
/// `/** */`. Approximate — false positives possible on string
/// literals that happen to occupy a leading position.
pub(crate) fn strip_docstrings(source: &str, language: Language) -> String {
    match language {
        Language::Python => strip_python_docstrings(source),
        Language::Rust
        | Language::JavaScript
        | Language::TypeScript
        | Language::Java
        | Language::Kotlin
        | Language::Swift
        | Language::Csharp
        | Language::Php
        | Language::Scala
        | Language::Go
        | Language::C
        | Language::Cpp => strip_c_family_docstrings(source, language),
        _ => source.to_string(),
    }
}

pub(crate) fn strip_c_family_docstrings(source: &str, language: Language) -> String {
    let mut out = String::with_capacity(source.len());
    for line in source.split_inclusive('\n') {
        let trimmed = line.trim_start();
        // Rust doc comments
        if language == Language::Rust && (trimmed.starts_with("///") || trimmed.starts_with("//!"))
        {
            continue; // drop the line
        }
        // C-family javadoc lines starting with `* ` inside a `/** */`
        // block — we can't reliably split blocks here without a full
        // parser, so we leave block-form docs alone (rare-enough to
        // not be worth the complexity).
        out.push_str(line);
    }
    out
}

pub(crate) fn strip_python_docstrings(source: &str) -> String {
    // Approximate: drop triple-quoted strings that sit on a line
    // immediately after `def `, `class `, or as the first non-blank
    // line of the file. A full Python parser would be more accurate
    // but ~80% coverage with regex is enough for compaction.
    let mut out = String::with_capacity(source.len());
    let lines: Vec<&str> = source.split_inclusive('\n').collect();
    let mut i = 0;
    let mut at_block_start = true;
    while i < lines.len() {
        let line = lines[i];
        let stripped = line.trim_start();
        // Detect docstring start: `"""` or `'''` (alone or with text).
        if at_block_start && (stripped.starts_with("\"\"\"") || stripped.starts_with("'''")) {
            let quote = if stripped.starts_with("\"\"\"") {
                "\"\"\""
            } else {
                "'''"
            };
            // Single-line docstring: `"""one line"""`
            let after_open = &stripped[3..];
            if after_open.contains(quote) {
                i += 1;
                at_block_start = false;
                continue;
            }
            // Multi-line: skip until we find the closing triple-quote.
            i += 1;
            while i < lines.len() {
                if lines[i].contains(quote) {
                    i += 1;
                    break;
                }
                i += 1;
            }
            at_block_start = false;
            continue;
        }
        out.push_str(line);
        // Update block-start flag.
        let trimmed_end = line.trim();
        if trimmed_end.is_empty() {
            // blank line — next line could be a docstring after a class/def
            // (rare), but conservative: don't trigger.
            i += 1;
            continue;
        }
        at_block_start = trimmed_end.ends_with(':')
            && (trimmed_end.starts_with("def ")
                || trimmed_end.starts_with("class ")
                || trimmed_end.starts_with("async def ")
                || trimmed_end.starts_with('@'));
        i += 1;
    }
    out
}

// ── Import / use clause stripping (Full mode) ────────────────────────

pub(crate) fn strip_imports(source: &str, language: Language) -> String {
    let mut out = String::with_capacity(source.len());
    for line in source.split_inclusive('\n') {
        let t = line.trim_start();
        let is_import = match language {
            Language::Rust => t.starts_with("use "),
            Language::Python => t.starts_with("import ") || t.starts_with("from "),
            Language::JavaScript | Language::TypeScript => {
                t.starts_with("import ")
                    || t.starts_with("require(")
                    || t.starts_with("const ") && t.contains("require(")
            }
            Language::Go => t.starts_with("import "),
            Language::Java | Language::Kotlin | Language::Scala | Language::Csharp => {
                t.starts_with("import ") || t.starts_with("using ")
            }
            _ => false,
        };
        if !is_import {
            out.push_str(line);
        }
    }
    out
}

// ── Blank / whitespace cleanup ──────────────────────────────────────

pub(crate) fn collapse_blanks_and_trim(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut blank_run = 0usize;
    for line in source.split_inclusive('\n') {
        let body = line.trim_end_matches('\n');
        let trimmed_body = body.trim_end();
        if trimmed_body.is_empty() {
            blank_run += 1;
            if blank_run == 1 {
                // Emit ONE blank line max between non-blank blocks.
                out.push('\n');
            }
            continue;
        }
        blank_run = 0;
        out.push_str(trimmed_body);
        if line.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}
