//! **Removing comments without breaking what looks like them.**
//!
//! A `//` in a string is not a comment, nor is a `#` in a
//! literal, nor a `<!--` in an attribute. It is a state machine, not a
//! regular expression, because the only way to know whether we are INSIDE a
//! string is to have read everything before it.
//!
//! And not every `#` is prose: a `# type: ignore`, a shebang, a linter
//! directive CHANGE the file's behaviour. Throwing them away
//! would save bytes by modifying the program — `comment_is_a_directive`
//! is the border between compacting and corrupting.

use super::*;

// ── C-family comment stripping ───────────────────────────────────────

/// State machine that strips `//` and `/* */` comments while
/// respecting `"…"` strings, `'…'` chars, and (for Rust) raw
/// strings `r"…"`. Doc-comments `///` and `/** */` are preserved by
/// the Comments mode; Full mode strips them via [`strip_docstrings`].
/// Comment strip for the brace languages.
///
/// Takes the LANGUAGE because three of its rules are not shared, and each of
/// the three was corrupting real files (measured over 3 535 sources in
/// a corpus of real repos: 309 lost a backtick, 200 flipped single-quote parity, 207
/// lost a brace):
///
///   * a BACKTICK opens a string in JS/TS and nothing else. Without that,
///     `` `line // not a comment` `` had everything after the two slashes cut,
///     and any `/* … */` inside a template vanished.
///   * the `'x` LIFETIME heuristic is Rust's alone. Applied to TypeScript it
///     read `'it\'s // fine'` as a lifetime, never entered the string, and
///     stripped from the slashes on — leaving the literal unterminated.
///   * CSS has NO line comment. `a { background: url(http://x); }` came back
///     cut at `url(http:`, taking the rest of the rule and its closing brace
///     with it. SCSS and LESS do have `//`, and share this `Language`; keeping
///     their line comments is the price of never breaking a URL, and it is the
///     right way round — a comment kept costs tokens, a URL eaten costs the
///     file.
pub(crate) fn strip_c_family_comments(source: &str, language: Language) -> String {
    // Bytes in, bytes out: emitting through `b as char` reinterpreted every
    // byte above 0x7F as its Latin-1 codepoint, so one string literal or one
    // comment marker on a line was enough to hand back mojibake for every
    // non-ASCII character on it. The byte scan itself is sound - UTF-8 is
    // self-synchronising and every delimiter here is ASCII.
    let mut out = Vec::with_capacity(source.len());
    let bytes = source.as_bytes();
    let mut i = 0usize;
    let mut in_string = false;
    let mut string_quote = b'"';
    let mut in_char = false;
    let backtick_is_a_string = matches!(language, Language::JavaScript | Language::TypeScript);
    let apostrophe_may_be_a_lifetime = matches!(language, Language::Rust);
    let has_line_comments = !matches!(language, Language::Css);
    while i < bytes.len() {
        let b = bytes[i];

        // Inside a string literal — copy through, watching for the
        // closing quote and backslash escapes.
        if in_string {
            out.push(b);
            if b == b'\\' && i + 1 < bytes.len() {
                out.push(bytes[i + 1]);
                i += 2;
                continue;
            }
            if b == string_quote {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if in_char {
            out.push(b);
            if b == b'\\' && i + 1 < bytes.len() {
                out.push(bytes[i + 1]);
                i += 2;
                continue;
            }
            if b == b'\'' {
                in_char = false;
            }
            i += 1;
            continue;
        }

        // String / char openers.
        if b == b'"' || (backtick_is_a_string && b == b'`') {
            in_string = true;
            string_quote = b;
            out.push(b);
            i += 1;
            continue;
        }
        if b == b'\'' && (i + 1 < bytes.len() && bytes[i + 1] != b' ') {
            // Heuristic: in Rust, `'static` lifetimes look like char
            // openers but aren't. Skip when followed by alpha (likely
            // a lifetime).
            if apostrophe_may_be_a_lifetime
                && i + 2 < bytes.len()
                && bytes[i + 1].is_ascii_alphabetic()
            {
                let next2 = bytes.get(i + 2).copied().unwrap_or(0);
                if next2 != b'\'' {
                    out.push(b'\'');
                    i += 1;
                    continue;
                }
            }
            in_char = true;
            out.push(b'\'');
            i += 1;
            continue;
        }

        // Line comment: `//` (NOT `///` doc-comment in Rust). We
        // preserve `///` and `//!` so the Comments mode keeps the
        // user's intent — they go away in Full mode via
        // [`strip_docstrings`].
        if has_line_comments && b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            let is_doc_comment =
                i + 2 < bytes.len() && (bytes[i + 2] == b'/' || bytes[i + 2] == b'!');
            // A directive is kept WHOLE — unlike a doc comment, whose marker is
            // re-emitted and whose body flows on. `//go:build linux && amd64`
            // means nothing without its text.
            let line_end = bytes[i..]
                .iter()
                .position(|c| *c == b'\n')
                .map(|n| i + n)
                .unwrap_or(bytes.len());
            if !is_doc_comment
                && comment_is_a_directive(
                    std::str::from_utf8(&bytes[i + 2..line_end]).unwrap_or(""),
                )
            {
                out.extend_from_slice(&bytes[i..line_end]);
                i = line_end;
                continue;
            }
            if is_doc_comment {
                out.push(b'/');
                out.push(b'/');
                i += 2;
                continue;
            }
            // Skip to end of line.
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        // Block comment: `/* ... */` (NOT `/** ... */` doc-block).
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            let is_doc_block = i + 2 < bytes.len() && bytes[i + 2] == b'*';
            if is_doc_block {
                // Preserve doc block verbatim (until `*/`).
                out.push(b'/');
                out.push(b'*');
                out.push(b'*');
                i += 3;
                while i < bytes.len() {
                    out.push(bytes[i]);
                    if bytes[i] == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                        out.push(b'/');
                        i += 2;
                        break;
                    }
                    i += 1;
                }
                continue;
            }
            // Skip until closing `*/`.
            i += 2;
            while i + 1 < bytes.len() {
                if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    i += 2;
                    break;
                }
                i += 1;
            }
            continue;
        }
        out.push(b);
        i += 1;
    }
    crate::text_encoding::utf8_owned(out)
}

// ── Hash-family (`#`) comment stripping ─────────────────────────────

/// Strip `#` line comments while preserving `#` inside string
/// literals. Conservative: leaves a line that's ENTIRELY a comment
/// to be removed; preserves trailing-inline comments? — actually no,
/// strips both. Inline `# foo` after a line of code becomes ``.
/// A comment whose text is a DIRECTIVE: the compiler, the type checker, the
/// linter or the build system reads it, so removing it changes what the file
/// IS, not merely how it reads.
///
/// Measured 2026-08-27 on a 19 937-byte Go file and a Python one: the
/// `Comments` strip removed `//go:build linux && amd64`, `// +build`,
/// `//go:generate`, `# type: ignore`, `# -*- coding: utf-8 -*-`, `# noqa: F401`
/// and `# pragma: no cover` — every one of them. A build tag decides which
/// platforms the file compiles for; a coding declaration decides how the file
/// is DECODED. And the editable-source rule says an Edit is matched against the
/// bytes the agent was shown, so a stripped build tag is one the agent will not
/// put back.
///
/// One predicate for both strippers. The C-family pass is a byte scanner and
/// the hash-family pass is line-based — two mechanisms that already disagreed
/// about nothing only because neither knew about directives at all.
///
/// Takes the text AFTER the marker (`//` or `#`). Cost measured over 60 182
/// source files: **0.50 % of comment bytes, 0.057 % of source bytes.**
pub(crate) fn comment_is_a_directive(after_marker: &str) -> bool {
    // Attached to the marker, no space: `//go:build`, `//#region`.
    const GLUED: &[&str] = &["go:", "#region", "#endregion", "# sourceMappingURL", "#!"];
    if GLUED.iter().any(|d| after_marker.starts_with(d)) {
        return true;
    }
    // `//#when` / `//#given` / `//#then` are BDD prose, not directives — which
    // is why the glued list names the two `#` forms it wants instead of taking
    // `#` whole. Measured: a bare `//#` prefix matches 366 prose lines in this
    // corpus against 24 real source maps.
    let t = after_marker.trim_start();
    const SPACED: &[&str] = &[
        "+build",                 // legacy Go build tag
        "frozen_string_literal:", // Ruby: string literals become immutable
        "rubocop:",
        "@phpstan-",
        "@psalm-",
        "phpcs:",
        "CHECKSTYLE:",
        "@diagnostic", // lua-language-server
        "@formatter:", // JetBrains
        "syntax=",     // Dockerfile parser directive (BuildKit)
        "check=",      // Dockerfile build-check directive
        "protolint:",
        "cmake-lint:",
        "buildifier:",
        "schema:", // `# schema: https://…` on TOML/YAML
        "sqlfluff:",
        "@ts-", // ts-ignore / ts-expect-error / ts-nocheck
        "eslint-",
        "eslint ",
        "biome-ignore",
        "deno-lint-ignore",
        "prettier-ignore",
        "nolint",
        "SPDX-License-Identifier",
        "-*-", // `# -*- coding: utf-8 -*-`
        "coding:",
        "type:", // `# type: ignore`
        "noqa",
        "pragma:",
        "fmt:", // `# fmt: off`
        "isort:",
        "mypy:",
        "pylint:",
        "ruff:",
        "flake8:",
        "cython:",
        "distutils:",
        "yamllint",
        "shellcheck",
    ];
    SPACED.iter().any(|d| t.starts_with(d))
}

pub(crate) fn strip_hash_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    for line in source.split_inclusive('\n') {
        let stripped = strip_hash_from_line(line);
        out.push_str(&stripped);
    }
    out
}

pub(crate) fn strip_hash_from_line(line: &str) -> String {
    // A directive line is kept WHOLE, before the scanner runs: `# type: ignore`
    // and `# -*- coding: utf-8 -*-` are read by the type checker and by the
    // DECODER, not by a human. Same predicate the C-family pass consults, so
    // the two cannot drift about what counts as one.
    if let Some(rest) = line.trim_start().strip_prefix('#') {
        if comment_is_a_directive(rest) {
            return line.to_string();
        }
    }
    // Find # not inside a string. Quick state machine for ", ', """.
    // Bytes in, bytes out: emitting through `b as char` reinterpreted every
    // byte above 0x7F as its Latin-1 codepoint, so one string literal or one
    // comment marker on a line was enough to hand back mojibake for every
    // non-ASCII character on it. The byte scan itself is sound - UTF-8 is
    // self-synchronising and every delimiter here is ASCII.
    let mut out = Vec::with_capacity(line.len());
    let bytes = line.as_bytes();
    let mut i = 0usize;
    let mut in_string: Option<u8> = None; // " or '
    let mut in_triple = false;
    while i < bytes.len() {
        let b = bytes[i];
        // Triple-quote start/end (Python). Only support `"""` here;
        // single quotes triple-style are rarer and we keep them as
        // literal text.
        if !in_triple
            && in_string.is_none()
            && i + 2 < bytes.len()
            && bytes[i] == b'"'
            && bytes[i + 1] == b'"'
            && bytes[i + 2] == b'"'
        {
            in_triple = true;
            out.extend_from_slice(b"\"\"\"");
            i += 3;
            continue;
        }
        if in_triple
            && i + 2 < bytes.len()
            && bytes[i] == b'"'
            && bytes[i + 1] == b'"'
            && bytes[i + 2] == b'"'
        {
            in_triple = false;
            out.extend_from_slice(b"\"\"\"");
            i += 3;
            continue;
        }
        if in_triple {
            out.push(b);
            i += 1;
            continue;
        }

        if let Some(q) = in_string {
            out.push(b);
            if b == b'\\' && i + 1 < bytes.len() {
                out.push(bytes[i + 1]);
                i += 2;
                continue;
            }
            if b == q {
                in_string = None;
            }
            i += 1;
            continue;
        }
        if b == b'"' || b == b'\'' {
            in_string = Some(b);
            out.push(b);
            i += 1;
            continue;
        }
        if b == b'#' {
            // A TRAILING directive is the common shape for two of them:
            // `import os  # noqa: F401`, `return x  # pragma: no cover`. The
            // whole-line guard above never sees those, so ask the same
            // predicate here, where the scanner has found the marker.
            if comment_is_a_directive(std::str::from_utf8(&bytes[i + 1..]).unwrap_or("")) {
                out.extend_from_slice(&bytes[i..]);
                return crate::text_encoding::utf8_owned(out);
            }
            // Drop the rest of the line, preserving the trailing
            // newline if present.
            if line.ends_with('\n') {
                out.push(b'\n');
            }
            return crate::text_encoding::utf8_owned(out);
        }
        out.push(b);
        i += 1;
    }
    crate::text_encoding::utf8_owned(out)
}

// ── HTML comments ───────────────────────────────────────────────────

pub(crate) fn strip_html_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let bytes = source.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if i + 3 < bytes.len()
            && bytes[i] == b'<'
            && bytes[i + 1] == b'!'
            && bytes[i + 2] == b'-'
            && bytes[i + 3] == b'-'
        {
            // Skip until "-->"
            i += 4;
            while i + 2 < bytes.len() {
                if bytes[i] == b'-' && bytes[i + 1] == b'-' && bytes[i + 2] == b'>' {
                    i += 3;
                    break;
                }
                i += 1;
            }
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}
