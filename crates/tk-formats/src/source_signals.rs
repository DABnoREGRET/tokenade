//! #298 — what a line of SOURCE CODE opens with, declared once.
//!
//! Three detectors asked the same question with three hand-written lists, and
//! each was missing what the others had:
//!
//! * `logs::looks_like_source_code` knew `#include`, `func ` and `=> {`, and
//!   not `use std::`, `pub struct` or `//!`.
//! * `stealth::looks_like_source_or_doc` knew those and not `#include`'s
//!   companions or `func `.
//! * `graphql`'s SDL veto knew `impl`, `mod` and `crate::` and nothing about
//!   comments — which is exactly how one regression happened: a Rust file whose head held
//!   no keyword from that particular list was rendered as an empty GraphQL
//!   schema, 38 063 bytes to 0.
//!
//! What genuinely differs between the three is the MATCHING POLICY, not the
//! membership: `logs` and `stealth` anchor at the start of a line, the SDL veto
//! has to see `crate::` in the middle of one. So the membership lives here and
//! each caller keeps its own policy. Merging the policies too would have
//! repeated an earlier trap, where a predicate was reused on the strength of its
//! NAME while its contract said something else.

/// Tokens that OPEN a source-code line, matched at the start of a trimmed line.
pub const LINE_OPENERS: &[&str] = &[
    // Comments, every dialect that matters here.
    "//!",
    "///",
    "// ",
    "//",
    "/*",
    "* ",
    "#!",
    // Rust.
    "pub fn ",
    "fn ",
    "pub struct ",
    "struct ",
    "pub enum ",
    "enum ",
    "impl ",
    "mod ",
    "pub mod ",
    "use crate::",
    "use std::",
    "use ",
    "let ",
    "match ",
    // C / C++ / preprocessor.
    "#include",
    "#define",
    "#pragma",
    // Go.
    "func ",
    "package ",
    // Java / C#.
    "public ",
    "private ",
    "protected ",
    // Python.
    "def ",
    "class ",
    "from ",
    "async def ",
    // JS / TS.
    "function ",
    "async function ",
    "export ",
    "import ",
    "const ",
    "var ",
    "module.exports",
    "interface ",
    "type ",
];

/// Substrings that only appear inside source code, matched ANYWHERE in a line.
///
/// Kept apart from [`LINE_OPENERS`] on purpose: a veto that has to catch
/// `crate::` mid-line cannot use an anchored list, and an anchored caller must
/// not start matching these loosely — `-> ` inside prose is not Rust.
pub const INLINE_MARKERS: &[&str] = &["crate::", "=> {", "-> ", "::<", " struct "];

/// Does this line OPEN a source-code construct?
///
/// Anchored: the line is trimmed of leading whitespace and must START with one
/// of [`LINE_OPENERS`]. This is the policy `logs` and `stealth` want.
pub fn line_opens_source(line: &str) -> bool {
    let t = line.trim_start();
    if t.is_empty() {
        return false;
    }
    LINE_OPENERS.iter().any(|k| t.starts_with(k))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_dialect_the_three_lists_knew_is_still_known() {
        for line in [
            "//! module doc",
            "/// item doc",
            "// a comment",
            "#include <stdio.h>",
            "func main() {",
            "package main",
            "def go():",
            "class Thing:",
            "fn go() {",
            "pub fn go() {",
            "use std::fmt;",
            "use crate::thing;",
            "pub struct S {",
            "pub enum E {",
            "impl S {",
            "mod inner;",
            "function go() {",
            "async function go() {",
            "const x = 1;",
            "let y = 2;",
            "var z = 3;",
            "import foo from 'bar';",
            "export default thing;",
            "module.exports = {};",
        ] {
            assert!(line_opens_source(line), "must be source: {line:?}");
        }
    }

    #[test]
    fn leading_whitespace_is_not_a_disqualifier() {
        assert!(line_opens_source("    fn nested() {"));
    }

    #[test]
    fn ordinary_prose_is_not_source() {
        for line in [
            "The function of this tool is to compact output.",
            "2026-08-26T10:00:00Z INFO worker started",
            "",
            "   ",
            "Total: 42 files",
        ] {
            assert!(!line_opens_source(line), "must not be source: {line:?}");
        }
    }

    /// The inline markers are NOT anchored, and must not be borrowed by an
    /// anchored caller: `-> ` appears in ordinary prose about arrows.
    #[test]
    fn an_inline_marker_is_not_a_line_opener() {
        assert!(!line_opens_source("the mapping a -> b is total"));
        assert!(INLINE_MARKERS.contains(&"-> "));
    }
}

#[cfg(test)]
mod the_membership_reaches_every_caller {
    /// #298 — the point of one home is that a member added or removed here
    /// changes every detector at once. Before this, `logs` knew `#include` and
    /// `stealth` did not, `stealth` knew `use std::` and `logs` did not, and
    /// the SDL veto knew neither — which is how a Rust file whose head is doc
    /// comments and `use` lines was rendered as an empty GraphQL schema.
    ///
    /// Measured over 155 312 real source files: the shared membership changes
    /// the verdict on 10 597 of them against the old `logs` list and 6 807
    /// against the old `stealth` one, always towards recognising source. On
    /// 1 088 469 non-source files it newly calls 169 source — 0.016 %, and the
    /// samples are markdown that is almost entirely code blocks.
    #[test]
    fn a_rust_head_of_doc_comments_and_uses_is_source_everywhere() {
        let rust = "//! A module doc.\n//! More doc.\nuse std::fmt;\nuse crate::thing;\n\npub struct S {\n    a: u32,\n}\n";
        assert!(!crate::graphql::detect(rust), "the SDL veto must refuse it");
        // The lines that OPEN a construct, which is what the predicate is
        // named for. A struct FIELD like `a: u32,` opens nothing, and the
        // first version of this guard asserted otherwise — the predicate was
        // right and the assertion was greedy.
        for l in [
            "//! A module doc.",
            "use std::fmt;",
            "use crate::thing;",
            "pub struct S {",
        ] {
            assert!(super::line_opens_source(l), "must read as source: {l:?}");
        }
    }

    /// …and real SDL is still SDL. `type` is shared with TypeScript, which is
    /// why the veto excludes it — the exclusion is the local policy, not a hole
    /// in the membership.
    #[test]
    fn real_sdl_survives_the_shared_membership() {
        let sdl = "schema {\n  query: Query\n}\n\ntype Query {\n  me: User\n}\n";
        assert!(crate::graphql::detect(sdl));
    }
}
