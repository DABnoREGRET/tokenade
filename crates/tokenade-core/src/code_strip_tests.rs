// The tests of `crate::code_strip`, moved out of the production file.
//
// Mounted by `include!`: the tokens are spliced INTO `code_strip`, so every
// `super::` keeps its meaning. Split by `srcsweep::production_lines`.

#[cfg(test)]
mod tests {

    /// A backtick opens a string in JS and TS. Without that, everything after
    /// the first `//` inside a template literal was cut — and the literal is
    /// where a query, a prompt or a URL usually lives.
    #[test]
    fn a_template_literal_is_a_string() {
        let src = "const t = `line // not a comment\n${x} /* nor this */`;\nconst y = 3;\n";
        let out = strip(src, Language::TypeScript, StripMode::Comments);
        assert!(
            out.contains("line // not a comment"),
            "the template was cut: {out}"
        );
        assert!(
            out.contains("/* nor this */"),
            "a block inside the template went: {out}"
        );
        assert!(out.contains("const y = 3;"));
    }

    /// The `'x` lifetime heuristic belongs to Rust. Applied to TypeScript it
    /// read `'it\'s …'` as a lifetime, never entered the string, and stripped
    /// from the two slashes on — leaving the literal unterminated.
    #[test]
    fn an_apostrophe_string_is_not_a_lifetime_outside_rust() {
        let src = "const s = 'it\\'s // fine';\nlet z = 4;\n";
        let out = strip(src, Language::TypeScript, StripMode::Comments);
        assert!(
            out.contains("// fine"),
            "the string body was stripped: {out}"
        );
        assert!(out.contains("let z = 4;"));
    }

    /// And Rust keeps the heuristic: `&'static str` is not a char literal.
    #[test]
    fn rust_still_reads_a_lifetime_as_a_lifetime() {
        let src = "let s: &'static str = \"x\"; // real\nlet t = 1;\n";
        let out = strip(src, Language::Rust, StripMode::Comments);
        assert!(
            out.contains("&'static str"),
            "the lifetime was mangled: {out}"
        );
        assert!(!out.contains("// real"), "the comment survived: {out}");
        assert!(out.contains("let t = 1;"));
    }

    /// CSS has no line comment, and every stylesheet has a `//` inside a URL.
    #[test]
    fn css_has_no_line_comment() {
        let src = "a { background: url(http://x); /* real */ }\n";
        let out = strip(src, Language::Css, StripMode::Comments);
        assert!(out.contains("url(http://x)"), "the URL was cut: {out}");
        assert!(out.contains('}'), "the rule lost its closing brace: {out}");
        assert!(
            !out.contains("/* real */"),
            "the block comment survived: {out}"
        );
    }
    use super::*;

    #[test]
    fn extension_to_language_basic() {
        assert_eq!(language_for_extension("rs"), Some(Language::Rust));
        assert_eq!(language_for_extension("py"), Some(Language::Python));
        assert_eq!(language_for_extension("TSX"), Some(Language::TypeScript));
        assert_eq!(language_for_extension("unknown"), None);
    }

    #[test]
    fn lite_mode_only_collapses_blanks() {
        let src = "fn foo() {\n\n\n    // hi\n\n\n}\n";
        let out = strip(src, Language::Rust, StripMode::Lite);
        // Comment kept, blank runs collapsed to max 1 blank line.
        assert!(out.contains("// hi"));
        assert!(!out.contains("\n\n\n"));
    }

    #[test]
    fn comments_mode_strips_line_comments_keeps_doc_comments() {
        let src = "// regular comment\n/// doc comment\nfn foo() {}\n";
        let out = strip(src, Language::Rust, StripMode::Comments);
        assert!(!out.contains("regular comment"));
        assert!(out.contains("/// doc comment"));
        assert!(out.contains("fn foo()"));
    }

    #[test]
    fn comments_mode_strips_block_comments_keeps_doc_blocks() {
        let src = "/* regular block */\n/** doc block */\nfn foo() {}\n";
        let out = strip(src, Language::Rust, StripMode::Comments);
        assert!(!out.contains("regular block"));
        assert!(out.contains("/** doc block */"));
    }

    #[test]
    fn full_mode_strips_rust_doc_comments_and_imports() {
        let src = "use std::io;\n/// doc\nfn foo() {}\n";
        let out = strip(src, Language::Rust, StripMode::Full);
        assert!(!out.contains("doc"));
        assert!(!out.contains("use std::io"));
        assert!(out.contains("fn foo()"));
    }

    #[test]
    fn python_hash_comments_stripped_strings_preserved() {
        let src = "x = \"hash # not a comment\"  # real comment\nprint(x)\n";
        let out = strip(src, Language::Python, StripMode::Comments);
        // String literal hash preserved.
        assert!(out.contains("hash # not a comment"));
        // Trailing real comment dropped.
        assert!(!out.contains("# real comment"));
    }

    #[test]
    fn python_full_strips_docstring() {
        let src = "def foo():\n    \"\"\"this is a docstring\"\"\"\n    return 42\n";
        let out = strip(src, Language::Python, StripMode::Full);
        assert!(!out.contains("docstring"));
        assert!(out.contains("return 42"));
    }

    #[test]
    fn python_full_strips_multiline_docstring() {
        let src = "def foo():\n    \"\"\"\n    multi\n    line\n    \"\"\"\n    return 42\n";
        let out = strip(src, Language::Python, StripMode::Full);
        assert!(!out.contains("multi"));
        assert!(!out.contains("line"));
        assert!(out.contains("return 42"));
    }

    #[test]
    fn c_family_does_not_break_strings_with_comment_chars() {
        let src = "let url = \"https://example.com\"; // real comment\n";
        let out = strip(src, Language::Rust, StripMode::Comments);
        assert!(out.contains("https://example.com"));
        assert!(!out.contains("real comment"));
    }

    #[test]
    fn html_block_comments_stripped() {
        let src = "<div>\n<!-- a comment -->\n<p>hi</p>\n</div>\n";
        let out = strip(src, Language::Html, StripMode::Comments);
        assert!(!out.contains("a comment"));
        assert!(out.contains("<p>hi</p>"));
    }

    #[test]
    fn savings_on_well_commented_source() {
        // ~50% comments / blanks should give 30%+ savings in Comments mode.
        let src = "// header\n// another\n\n\nfn foo() {\n    // body comment\n    let x = 1;\n}\n";
        let before = src.len();
        let out = strip(src, Language::Rust, StripMode::Comments);
        let after = out.len();
        assert!(after < before, "expected shorter output");
        let ratio = (before - after) as f64 / before as f64;
        assert!(
            ratio > 0.2,
            "expected ≥20% size reduction, got {:.2}%",
            ratio * 100.0
        );
    }

    #[test]
    fn handles_rust_lifetimes_without_breaking() {
        let src = "fn foo<'a>(x: &'a str) -> &'static str { x }\n";
        let out = strip(src, Language::Rust, StripMode::Comments);
        assert!(out.contains("'a"));
        assert!(out.contains("'static"));
    }
    /// Four extensions that `is_source_code_extension` recognises (so
    /// they are never auto-compacted) and that `language_for_extension` did
    /// not map (so nothing stripped them either). That pairing is the shape a
    /// competitor named on 2026-08-26 as "indexed as nothing": the file is
    /// handed back whole and every layer reports success.
    ///
    /// All four have the comment syntax of a language already in the map, so
    /// this is a table completion, not a new rule.
    #[test]
    fn the_source_extensions_with_a_known_comment_syntax_are_all_mapped() {
        for (ext, want) in [
            // Cython and Windows-GUI Python: Python source, Python comments.
            ("pyx", Language::Python),
            ("pyw", Language::Python),
            // A C++ header. `hpp` and `hxx` were already here.
            ("hh", Language::Cpp),
            // fish has `#` line comments and no block form, like sh.
            ("fish", Language::Shell),
        ] {
            assert_eq!(
                language_for_extension(ext),
                Some(want),
                "`.{ext}` is source with a known comment syntax and no language"
            );
        }
    }

    /// And the mapping actually strips: a fish script whose comments are a
    /// third of its bytes comes back without them.
    #[test]
    fn a_fish_script_loses_its_comments() {
        let src = "# set up the environment\n                   set -x PATH $HOME/.local/bin $PATH\n                   # the next line is the one that matters\n                   set -x EDITOR nvim\n";
        let out = strip(src, Language::Shell, StripMode::Comments);
        assert!(!out.contains("the one that matters"), "{out}");
        assert!(out.contains("set -x EDITOR nvim"), "{out}");
    }
}
#[cfg(test)]
mod declaration {
    use super::*;

    #[test]
    fn nothing_removed_says_nothing() {
        let t = "a\nb\n";
        assert_eq!(declare_strip(t, t.to_string(), "x", None), t);
        assert_eq!(declare_strip(t, t.to_string(), "x", Some(0)), t);
    }

    #[test]
    fn a_removal_is_declared_with_its_magnitude() {
        let t = "# why this is pinned, at length so the note pays for itself\n\
                 # and a second line of reasoning that is also quite long here\n\
                 value = 1\n";
        let out = declare_strip(t, "value = 1\n".to_string(), "ini", None);
        assert!(out.starts_with("[tokenade:ini] "), "{out}");
        assert!(out.contains("2 line(s) fewer"), "{out}");
        assert!(out.contains("the file on disk is unchanged"), "{out}");
    }

    /// An inline comment trimmed off a line removes text without removing a
    /// line — the line delta is zero and the declaration must still fire.
    #[test]
    fn an_inline_trim_with_no_line_delta_is_still_declared() {
        let t = "value = 1  # pinned by the security policy, do not bump this \
                 without asking the platform team first, see the incident review\n";
        let out = declare_strip(t, "value = 1\n".to_string(), "ini", None);
        assert!(out.contains("inline comments trimmed"), "{out}");
    }

    /// The note must never make the answer bigger than the file it describes.
    #[test]
    fn a_note_that_costs_more_than_it_saves_is_not_emitted() {
        let t = "# x\na\n";
        assert_eq!(declare_strip(t, "a\n".to_string(), "ini", None), t);
    }

    #[test]
    fn an_exact_count_is_used_when_the_compactor_has_one() {
        let t = "# a comment long enough that declaring it is worth the bytes here\n\
                 # and another one just as long so the note stays cheaper than it\n\
                 x\n";
        let out = declare_strip(t, "x\n".to_string(), "hcl", Some(2));
        assert!(out.contains("2 comment line(s) stripped"), "{out}");
    }
}
#[cfg(test)]
mod declaration_cost {
    /// Declaring what a stripper removed must cost ONE LINE, whatever the file,
    /// and must never make the answer bigger than the input.
    ///
    /// The per-language `real_world_ratio_*` tests measure the STRIP, which is
    /// the compactor's own claim. This measures what saying so costs, once, for
    /// every language — so the two cannot be confused for each other and a
    /// language added later cannot quietly pay more.
    #[test]
    fn declaration_costs_at_most_one_line() {
        // Each language, with a payload that is genuinely worth stripping so
        // the cost gate does not short-circuit and leave nothing to measure.
        let cases: Vec<(&str, String, fn(&str) -> String, fn(&str) -> String)> = vec![
            (
                "ini",
                (0..40)
                    .map(|i| format!("; explanatory comment number {i}, at some length\nk{i} = {i}\n"))
                    .collect(),
                crate::ini::compact,
                crate::ini::strip_only,
            ),
            (
                "cmake",
                (0..40)
                    .map(|i| format!("# explanatory comment number {i}, at some length\nset(V{i} {i})\n"))
                    .collect(),
                crate::cmake::compact,
                crate::cmake::strip_only,
            ),
            (
                "bazel",
                (0..40)
                    .map(|i| format!("# explanatory comment number {i}, at some length\nx{i} = {i}\n"))
                    .collect(),
                crate::bazel::compact,
                crate::bazel::strip_only,
            ),
            (
                "dockerfile",
                (0..40)
                    .map(|i| format!("# explanatory comment number {i}, at some length\nRUN echo {i}\n"))
                    .collect(),
                crate::dockerfile::compact,
                crate::dockerfile::strip_only,
            ),
            (
                "toml", // over the delivery threshold: under it TOML keeps its comments
                (0..1500)
                    .map(|i| format!("# explanatory comment number {i}, at some length\nk{i} = {i}\n"))
                    .collect(),
                crate::toml_compact::compact,
                crate::toml_compact::strip_only,
            ),
            (
                "sql",
                (0..40)
                    .map(|i| format!("-- explanatory comment number {i}, at some length\nSELECT {i};\n"))
                    .collect(),
                crate::sql_source::compact,
                crate::sql_source::strip_only,
            ),
            (
                "proto",
                (0..40)
                    .map(|i| format!("// explanatory comment number {i}, at some length\nint32 f{i} = {i};\n"))
                    .collect(),
                crate::proto::compact,
                crate::proto::strip_only,
            ),
        ];
        // One line of prose. Wide enough that a language name or a longer count
        // fits, tight enough that a paragraph would not.
        const ONE_LINE: usize = 128;
        for (name, text, compact, strip) in cases {
            let declared = compact(&text);
            let stripped = strip(&text);
            assert!(
                declared.len() <= stripped.len() + ONE_LINE,
                "`{name}` pays {} bytes to declare, over the {ONE_LINE}-byte budget",
                declared.len() - stripped.len()
            );
            assert!(
                declared.len() <= text.len(),
                "`{name}` returned MORE than it was given: {} vs {}",
                declared.len(),
                text.len()
            );
            assert!(
                declared.starts_with("[tokenade:"),
                "`{name}` stripped {} bytes without declaring it:\n{}",
                text.len() - stripped.len(),
                &declared[..80.min(declared.len())]
            );
        }
    }
}
#[cfg(test)]
mod delivery_selection {
    use super::*;

    #[test]
    fn a_heavily_commented_file_goes_through_comments_mode() {
        let src = (0..60)
            .map(|i| {
                format!("// explanation number {i} of what the next line does\nlet x{i} = {i};\n")
            })
            .collect::<String>();
        let d = strip_for_delivery(&src, Language::Rust).expect("clears the bar");
        assert_eq!(d.mode, "comments");
        assert!(d.pct >= 10, "{}%", d.pct);
        assert!(!d.text.contains("explanation number"));
    }

    /// The same content, twice: small enough to deliver, and not.
    ///
    /// Measured on a 404 669-byte Rust file: accepting `Comments` at -27 %
    /// delivered 293 904 bytes, where DECLINING handed the file to the
    /// disclose path and delivered 1 224. A rung that clears its bar can still
    /// be the wrong answer, and this is the only condition that says so.
    #[test]
    fn a_win_too_big_to_deliver_is_declined() {
        let unit = |i: usize| {
            format!("// explanation number {i} of what the next line does\nlet x{i} = {i};\n")
        };
        let small: String = (0..60).map(unit).collect();
        let d = strip_for_delivery(&small, Language::Rust).expect("small clears the bar");
        assert_eq!(d.mode, "comments", "the gentle rung must still apply");

        // Over the 25 000-token ceiling, with the SAME shape — so the only
        // thing that changed is the size of what would be handed over.
        let big: String = (0..30_000).map(unit).collect();
        assert!(
            crate::tokenizer::estimate(&big) > crate::disclose::read_threshold_tokens(),
            "the fixture must exceed the ceiling, or it proves nothing"
        );
        let comments = strip(&big, Language::Rust, StripMode::Comments);
        assert!(
            crate::tokenizer::estimate(&comments) > crate::disclose::read_threshold_tokens(),
            "and so must its stripped form, or the ceiling is not what declines it"
        );
        assert!(
            strip_for_delivery(&big, Language::Rust).is_none(),
            "a 25 k+ token delivery pre-empts the path that folds it to a pointer"
        );

        // And the same for the LITE rung, which a heavily-commented fixture
        // never reaches: sparse comments, so `Comments` misses its bar, blank
        // runs so `Lite` clears its own — and still far over the ceiling.
        let mut sparse = String::new();
        for i in 0..40_000 {
            sparse.push_str(&format!(
                "    let some_reasonably_long_identifier_{i} = compute({i});\n"
            ));
            if i % 2 == 0 {
                sparse.push_str("\n\n\n");
            }
        }
        let c_pct = {
            let b = crate::tokenizer::estimate(&sparse);
            let a =
                crate::tokenizer::estimate(&strip(&sparse, Language::Rust, StripMode::Comments));
            (b - a) * 100 / b
        };
        assert!(
            c_pct < 10,
            "the fixture must MISS the comments bar or lite is never reached, got {c_pct}%"
        );
        let lite = strip(&sparse, Language::Rust, StripMode::Lite);
        assert!(
            crate::tokenizer::estimate(&lite) > crate::disclose::read_threshold_tokens(),
            "and its lite form must still be over the ceiling"
        );
        assert!(
            strip_for_delivery(&sparse, Language::Rust).is_none(),
            "the lite rung needs the same ceiling as the comments rung"
        );
    }

    #[test]
    fn a_sparsely_commented_file_falls_back_to_lite() {
        // `Comments` mode is a superset of `Lite`, so Lite can only win when
        // the combined saving sits between the two bars: under 10 %, at or
        // over 5 %. That is the case the fallback exists for — take the
        // less disruptive mode rather than nothing.
        let mut src = String::new();
        for i in 0..200 {
            src.push_str(&format!(
                "    let some_reasonably_long_identifier_{i} = compute({i});\n"
            ));
            if i % 2 == 0 {
                src.push_str("\n\n\n");
            }
        }
        let comments = strip(&src, Language::Rust, StripMode::Comments);
        let before = crate::tokenizer::estimate(&src);
        let after = crate::tokenizer::estimate(&comments);
        let pct = (before - after) * 100 / before;
        assert!(
            (5..10).contains(&pct),
            "fixture must land between the bars, got {pct}%"
        );
        let d = strip_for_delivery(&src, Language::Rust).expect("blank runs collapse");
        assert_eq!(d.mode, "lite", "under the comments bar, over the lite bar");
    }

    #[test]
    fn a_file_neither_mode_helps_is_left_alone() {
        let src = (0..60)
            .map(|i| format!("let x{i} = {i};\n"))
            .collect::<String>();
        assert!(
            strip_for_delivery(&src, Language::Rust).is_none(),
            "nothing to strip, so nothing is claimed"
        );
    }
}
#[cfg(test)]
mod delivered_source_is_scrubbed {
    use super::*;

    /// A real AWS example key, in a file big enough to reach the strip path.
    const KEY: &str = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";

    fn big_python_with_a_secret() -> String {
        let mut s = format!("AWS_SECRET_ACCESS_KEY = \"{KEY}\"\n");
        for i in 0..200 {
            s.push_str(&format!(
                "# what handler {i} does, at some length so the comment share counts\n\
                 def handler{i}():\n    return {i}\n"
            ));
        }
        s
    }

    #[test]
    fn a_delivery_never_carries_a_secret() {
        let src = big_python_with_a_secret();
        let d = strip_for_delivery(&src, Language::Python).expect("clears the comments bar");
        assert_eq!(d.mode, "comments", "precondition: the strip path is taken");
        assert!(
            !d.text.contains(KEY),
            "the key reached the agent in clear; this path prints and exits \
             BEFORE cmd_auto's print-site scrub"
        );
        assert!(
            d.text.contains("<redacted>"),
            "and it must say what it removed"
        );
    }

    /// Redaction routinely makes text LONGER. The banner quotes `after`, so it
    /// must count the bytes that actually leave, not the pre-scrub ones.
    #[test]
    fn the_banner_counts_the_bytes_that_leave() {
        let src = big_python_with_a_secret();
        let d = strip_for_delivery(&src, Language::Python).unwrap();
        assert_eq!(
            d.after,
            crate::tokenizer::estimate(&d.text),
            "the banner would describe bytes nobody received"
        );
    }

    /// One constructor, so no future surface can build an unscrubbed one. The
    /// forty-six print-site scrubs elsewhere in this codebase are exactly the
    /// shape that let this path be forgotten.
    #[test]
    fn a_delivery_is_built_in_one_place_only() {
        // The FAMILY: the trunk now keeps only the language and the dispatch,
        // and everything these guards look for has moved down into a child.
        let src = tk_base::source_guard::library_module_family("code_strip");
        let src = src.as_str();
        // Counted over the WHOLE file, not a `#[cfg(test)]`-truncated head:
        // test modules are interleaved here, so splitting on the first one cut
        // the constructor out of the window and the guard passed on nothing.
        // `Delivery {` alone also matches the struct definition and the brace
        // of a `-> Delivery` signature, so match the CONSTRUCTION form.
        // Split so the needle does not appear whole in this file and count
        // itself — that is how the previous version of this guard failed.
        //
        // Whitespace is COLLAPSED first. The needle used to be matched against
        // the raw file, so it only held while the literal stayed on one line —
        // `cargo fmt` exploded it across five and the guard read "zero
        // constructors", which is the reassuring answer. A guard on source text
        // must not be able to go green because a formatter moved a brace.
        let flat: String = {
            let mut out = String::with_capacity(src.len());
            let mut in_ws = false;
            for c in src.chars() {
                if c.is_whitespace() {
                    if !in_ws {
                        out.push(' ');
                    }
                    in_ws = true;
                } else {
                    out.push(c);
                    in_ws = false;
                }
            }
            out
        };
        let needle = concat!("Delivery", " { ", "text");
        assert_eq!(
            flat.matches(needle).count(),
            1,
            "expected exactly one construction, inside `delivered` where the \
             scrub is"
        );
        let at = src
            .find("fn delivered(")
            .expect("the single constructor is gone");
        let end = src[at..].find("\n}").map(|i| at + i).unwrap_or(src.len());
        assert!(
            src[at..end].contains("redact_for_model"),
            "the one constructor stopped scrubbing"
        );
    }
}
#[cfg(test)]
mod a_directive_is_not_a_comment {
    use super::*;

    #[test]
    fn go_keeps_its_build_tags() {
        let src = format!(
            "//go:build linux && amd64\n// +build linux,amd64\n\npackage main\n\n\
             //go:generate stringer -type=Kind\n{}",
            (0..300)
                .map(|i| format!(
                    "// doc for handler {i} at some length\nfunc h{i}() {{ return }}\n"
                ))
                .collect::<String>()
        );
        let out = strip(&src, Language::Go, StripMode::Comments);
        for d in [
            "//go:build linux && amd64",
            "// +build linux,amd64",
            "//go:generate stringer",
        ] {
            assert!(out.contains(d), "lost {d}");
        }
        assert!(
            !out.contains("doc for handler"),
            "ordinary comments must still go"
        );
    }

    #[test]
    fn python_keeps_the_ones_the_toolchain_reads() {
        let src = format!(
            "# type: ignore\n# -*- coding: utf-8 -*-\nimport os  # noqa: F401\n{}",
            (0..250)
                .map(|i| format!("# explanation {i} at some length\ndef h{i}():\n    return {i}  # pragma: no cover\n"))
                .collect::<String>()
        );
        let out = strip(&src, Language::Python, StripMode::Comments);
        for d in [
            "# type: ignore",
            "coding: utf-8",
            "# noqa: F401",
            "# pragma: no cover",
        ] {
            assert!(out.contains(d), "lost {d}");
        }
        assert!(
            !out.contains("explanation 7 at"),
            "ordinary comments must still go"
        );
    }

    /// The trailing form is the common one for two of them, and the whole-line
    /// guard never sees it.
    #[test]
    fn a_trailing_directive_survives_but_a_trailing_note_does_not() {
        let src = "import os  # noqa: F401\nimport sys  # we need this for exit\n";
        let out = strip(src, Language::Python, StripMode::Comments);
        assert!(out.contains("# noqa: F401"));
        assert!(!out.contains("we need this for exit"));
    }

    #[test]
    fn js_and_c_keep_theirs() {
        let js = "// @ts-expect-error narrowing\n// eslint-disable-next-line no-console\n\
                  //# sourceMappingURL=app.js.map\n// just a note here\nconst a = 1;\n";
        let out = strip(js, Language::TypeScript, StripMode::Comments);
        for d in [
            "@ts-expect-error",
            "eslint-disable-next-line",
            "sourceMappingURL",
        ] {
            assert!(out.contains(d), "lost {d}");
        }
        assert!(!out.contains("just a note"));

        let c = "// SPDX-License-Identifier: MIT\n// implementation note\nvoid f(void) {}\n";
        let out = strip(c, Language::C, StripMode::Comments);
        assert!(out.contains("SPDX-License-Identifier: MIT"));
        assert!(!out.contains("implementation note"));
    }

    /// `//#when` / `//#given` / `//#then` are BDD prose. A bare `//#` prefix
    /// matched 366 of them in this corpus against 24 real source maps, which is
    /// why the glued list names the two `#` forms it wants.
    #[test]
    fn bdd_prose_is_not_a_directive() {
        for l in [
            "//#when the user logs in",
            "//#given a fresh session",
            "//#then it works",
        ] {
            assert!(!comment_is_a_directive(&l[2..]), "kept prose: {l}");
        }
        assert!(comment_is_a_directive("# sourceMappingURL=a.map"));
        assert!(comment_is_a_directive("#region setup"));
    }

    /// One predicate, both strippers. The C-family pass is a byte scanner and
    /// the hash-family pass is line-based; two copies of this list would drift
    /// and each would drift on a different language.
    /// The eight config/data compactors — sql, proto, bzl, ini, cmake,
    /// Dockerfile, toml, hcl — each strip comments too, and each lost its
    /// directives. `# syntax=docker/dockerfile:1.7` selects the BuildKit
    /// frontend: a Dockerfile served without it is built by a different parser
    /// than the one on disk.
    #[test]
    fn the_config_languages_keep_theirs_too() {
        let dockerfile = format!(
            "# syntax=docker/dockerfile:1.7\n# check=skip=JSONArgsRecommended\nFROM alpine\n{}",
            (0..200)
                .map(|i| format!("# note {i} at some length so comments count\nRUN echo {i}\n"))
                .collect::<String>()
        );
        let out = crate::dockerfile::compact(&dockerfile);
        assert!(
            out.contains("syntax=docker/dockerfile:1.7"),
            "the parser directive is gone"
        );
        assert!(
            out.contains("check=skip="),
            "the build-check directive is gone"
        );
        assert!(
            !out.contains("note 7 at"),
            "ordinary comments must still go"
        );

        let sql = format!(
            "-- noqa: disable=L044\n{}",
            (0..300)
                .map(|i| format!(
                    "-- note {i} at some length so comments count\nSELECT {i} AS c{i};\n"
                ))
                .collect::<String>()
        );
        let out = crate::sql_source::strip_only(&sql);
        assert!(out.contains("noqa: disable=L044"));
        assert!(!out.contains("note 7 at"));
    }

    #[test]
    fn both_strippers_ask_the_same_predicate() {
        // The FAMILY: the trunk now keeps only the language and the dispatch,
        // and everything these guards look for has moved down into a child.
        let src = tk_base::source_guard::library_module_family("code_strip");
        let src = src.as_str();
        let needle = concat!("comment_is_a", "_directive(");
        // The definition, plus one call in the C-family scanner and two in the
        // hash-family pass (whole-line and trailing).
        assert!(
            src.matches(needle).count() >= 4,
            "a stripper stopped asking; found {} call sites",
            src.matches(needle).count()
        );
        for f in ["fn strip_c_family_comments", "fn strip_hash_from_line"] {
            let at = src.find(f).unwrap_or_else(|| panic!("{f} is gone"));
            let end = src[at..].find("\n}").map(|i| at + i).unwrap_or(src.len());
            assert!(
                src[at..end].contains(needle),
                "{f} keeps its own idea of a directive"
            );
        }
    }
}
