// The tests of `skeleton`.
//
// Spliced by `include!` from `skeleton.rs`: the same module, so the same
// private access and the same `super::` paths, without a single visibility
// line changed.
//
// They used to live in `compress_tests.rs` because `skeleton` MOVED OUT of
// `compress`: rendering a file by its structure needs its syntax tree and its
// declaration patterns, which is the job of the code layer, not the fold
// layer. The test file, however, had stayed behind — and the 2026-09-13 crate
// boundary said so in thirty-five compile errors: 23 of the 35 tests of
// `mod tests` and two whole modules named `crate::skeleton`, which no module
// of `tk-fold` can reach.
//
// A `//!` is impossible here: a file spliced by `include!` is not a module,
// and an INNER doc comment has nothing to qualify.

#[cfg(test)]
mod grammarless_skeleton_tests {

    fn elixir() -> String {
        let mut s = String::from(
            "defmodule My.Mod do\n  @moduledoc \"docs\"\n  alias My.Other.Helper\n  import Enum\n",
        );
        for i in 0..10 {
            s.push_str(&format!(
                "\n  @doc \"handles {i}\"\n  def handle_{i}(arg) do\n    case arg do\n      %{{a: a}} -> a\n      _ -> :error\n    end\n\n    with {{:ok, v}} <- Helper.load(arg) do\n      v = v + {i}\n      Logger.info(\"handled\")\n      {{:ok, v}}\n    end\n  end\n"
            ));
        }
        s.push_str("end\n");
        s
    }

    /// The symbol index knows 73 extensions; the tree-sitter registry the
    /// skeleton drives from carries 16. For the other 57 the skeleton fell
    /// back to a language-agnostic guess while this table already held the
    /// exact shape of a declaration in that language.
    #[test]
    fn a_language_without_a_grammar_uses_the_index_spec() {
        let src = elixir();
        let out = crate::skeleton::skeleton_ext(&src, Some("ex"));
        assert!(
            out.contains("def handle_0"),
            "a declaration is missing: {out}"
        );
        assert!(
            !out.contains("IO.puts"),
            "a body line survived the skeleton: {out}"
        );
        assert!(
            out.len() < src.len() / 2,
            "barely shrunk: {} vs {}",
            out.len(),
            src.len()
        );
        // And it must beat the language-agnostic view, which is what it
        // replaced: the heuristic keeps every declaration-LOOKING line, so on
        // real Elixir it kept nearly twice as much (0.53 of the file against
        // 0.29, measured over 46 sources).
        let generic = crate::skeleton::skeleton(&src);
        assert!(
            out.len() < generic.len(),
            "no better than the generic heuristic: {} vs {}",
            out.len(),
            generic.len()
        );
        // The import rows come from the same spec and are part of what the
        // file offers.
        assert!(
            out.contains("alias My.Other.Helper"),
            "an import row was dropped: {out}"
        );
    }

    /// An import line is a declaration for this purpose: it says what the file
    /// depends on, and the index's spec is where that pattern already lives.
    #[test]
    fn an_unknown_extension_answers_none() {
        assert!(crate::index::declaration_rows("qqq", "x = 1\n").is_none());
        assert!(crate::index::declaration_rows("ex", "def f do\nend\n").is_some());
    }

    /// The comparison that makes the change safe: a file whose declarations
    /// are SCATTERED pays a fold marker per gap, and on Lua and Clojure the
    /// precise view came out bigger than the heuristic it replaced — bigger
    /// than the SOURCE, in the Lua case. Taking the shorter view means no
    /// language can regress.
    #[test]
    fn the_cheaper_of_the_two_views_wins() {
        let mut lua = String::new();
        for i in 0..8 {
            lua.push_str(&format!(
                "-- note {i}\nlocal function f{i}()\n  return {i}\nend\n\nprint(f{i}())\n\n"
            ));
        }
        let mut clj = String::new();
        for i in 0..6 {
            clj.push_str(&format!(
                "(defn f{i} [x]\n  (+ x {i}))\n\n(println (f{i} 1))\n\n"
            ));
        }
        for (ext, src) in [("lua", &lua), ("clj", &clj)] {
            let out = crate::skeleton::skeleton_ext(src, Some(ext));
            assert!(
                out.len() <= crate::skeleton::skeleton(src).len(),
                "{ext}: the precise view was kept though it costs more: {} vs {}",
                out.len(),
                crate::skeleton::skeleton(src).len()
            );
            assert!(
                out.len() < src.len(),
                "{ext}: the skeleton inflated the file"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write;

    #[test]
    fn the_elision_marker_is_a_comment_in_the_files_own_language() {
        // The marker was hardcoded to `//`, so a Python skeleton shipped
        // `    // … 13 lines …` — a line that is not a comment in Python, in
        // output an agent may copy. Observed on a real file while building the
        // showcase corpus.
        let py = "def alpha(x):\n    a = 1\n    b = 2\n    c = 3\n    d = 4\n    return a\n\ndef beta(y):\n    e = 5\n    f = 6\n    g = 7\n    h = 8\n    return e\n";
        let out = crate::skeleton::skeleton_for_ext(py, Some("py"));
        assert!(
            !out.contains("// …"),
            "a Python skeleton must not carry a C comment marker:\n{out}"
        );
        // Named, not merely "not //": an assertion that only forbids the old
        // value passes on any replacement, including a broken one.
        assert!(out.contains("# …"), "expected a `#` elision marker:\n{out}");

        let lua = crate::skeleton::skeleton_for_ext(py, Some("lua"));
        assert!(
            lua.contains("-- …"),
            "expected a `--` marker for lua:\n{lua}"
        );

        // Unknown and absent extensions keep the historical `//`.
        assert_eq!(crate::skeleton::line_comment_for(None), "//");
        assert_eq!(crate::skeleton::line_comment_for(Some("rs")), "//");
        assert_eq!(crate::skeleton::line_comment_for(Some("qqq")), "//");
        // A leading dot and upper case are both accepted spellings.
        assert_eq!(crate::skeleton::line_comment_for(Some(".PY")), "#");
    }
    /// A skeleton is asked "what does this file offer?". A well-tested file
    /// answers mostly with its tests unless they are folded — and folding them
    /// must not cost the two things that depend on knowing where a test is:
    /// resolving a panic frame to its own signature, and finding a test by name.
    #[test]
    fn a_skeleton_folds_the_test_module_but_still_locates_a_panic_in_it() {
        let src = "pub fn api_one() {}\n\
                   pub fn api_two() {}\n\
                   #[cfg(test)]\n\
                   mod tests {\n\
                   fn checks_api_one() { let _ = 1; }\n\
                   fn checks_api_two() { let _ = 2; }\n\
                   }\n";
        let sk = crate::skeleton::skeleton_ext(src, Some("rs"));
        assert!(
            sk.contains("api_one") && sk.contains("api_two"),
            "API dropped: {sk}"
        );
        assert!(sk.contains("mod tests"), "the fold must stay visible: {sk}");
        assert!(
            !sk.contains("checks_api_one") && !sk.contains("checks_api_two"),
            "test bodies must be folded away: {sk}"
        );

        // A panic inside a test still names the test, not the module.
        let sig = crate::treesitter::signature_enclosing_line(src, "rs", 5)
            .expect("a frame inside the test module resolves");
        assert!(sig.contains("checks_api_one"), "frame resolved to {sig:?}");

        // A module that merely carries an attribute is NOT a test module.
        let plain = "#[allow(dead_code)]\n\
                     mod helpers {\n\
                     fn helper_one() {}\n\
                     }\n";
        assert!(
            crate::skeleton::skeleton_ext(plain, Some("rs")).contains("helper_one"),
            "only cfg(test) modules fold"
        );
    }
    #[test]
    fn one_file_is_not_charged_for_the_seven_it_did_not_ask_about() {
        // A file with more declarations than a 500-token outline can hold.
        let mut src = String::new();
        for i in 0..200 {
            src.push_str(&format!(
                "pub fn handler_number_{i}(request: &Request, context: &mut Context) -> Result<Response> {{\n    todo!()\n}}\n\n"
            ));
        }
        let (one, _) = crate::skeleton::skeleton_with_savings_ext_for(&src, Some("rs"), 1);
        let (eight, _) = crate::skeleton::skeleton_with_savings_ext_for(&src, Some("rs"), 8);
        assert!(
            crate::tokenizer::estimate(&one) > crate::tokenizer::estimate(&eight),
            "asking for ONE file must not be rationed like a batch of eight"
        );
        // The batch invariant the 500-token bound was derived from: eight
        // files still fit under the inline-disclosure threshold.
        assert!(
            crate::tokenizer::estimate(&eight) * 8 < 5_000,
            "eight files no longer fit inline: {} each",
            crate::tokenizer::estimate(&eight)
        );
        // And a single file is still bounded — this is an outline, not the file.
        assert!(
            crate::tokenizer::estimate(&one) <= crate::skeleton::SKELETON_PER_FILE_CEILING + 50
        );
    }
    #[test]
    fn skeleton_hides_bodies() {
        let src =
            "pub fn add(a: i32, b: i32) -> i32 {\n    let x = a;\n    let y = b;\n    x + y\n}\n";
        let sk = crate::skeleton::skeleton(src);
        assert!(sk.contains("pub fn add"));
        assert!(sk.contains("lines …"));
        let (_, sav) = crate::skeleton::skeleton_with_savings(src);
        assert!(sav.ratio() > 0.0);
        // #1 — small input: inline marker only, no verbose footer (would
        // invert the savings), and the gain stays positive.
        assert!(
            !sk.contains("symbol_find"),
            "footer must not fire below threshold"
        );
    }
    #[test]
    fn skeleton_keeps_buried_declaration_that_headtail_would_drop() {
        // #2 (0.4.4 Cobra hallucination) — a license banner + imports at the
        // top, then many bodies, then the declaration the agent wants buried
        // in the MIDDLE. head/tail truncation keeps the license and drops the
        // middle declaration; `skeleton` must keep the declaration + its
        // identifier and elide the bodies instead.
        // Kept under the per-skeleton token budget (`crate::skeleton::SKELETON_MAX_TOKENS`)
        // so this exercises the buried-decl property in isolation; the
        // budget-truncation behaviour has its own test below.
        let mut src = String::new();
        for _ in 0..13 {
            src.push_str("// Copyright 2013 The Cobra Authors — Licensed under Apache 2.0\n");
        }
        src.push_str("package cmd\n");
        src.push_str("import (\n\t\"fmt\"\n)\n");
        for f in 0..8 {
            let _ = writeln!(src, "func filler{f}() {{");
            for b in 0..8 {
                let _ = writeln!(src, "\tx{b} := compute({b})");
            }
            src.push_str("}\n");
        }
        // The buried target, deep in the middle.
        src.push_str("func initDefaultCompletionCmd() *cobra.Command {\n");
        src.push_str("\treturn nil\n}\n");
        for f in 8..16 {
            let _ = writeln!(src, "func tail{f}() {{");
            src.push_str("\treturn\n}\n");
        }

        let sk = crate::skeleton::skeleton(&src);
        assert!(
            sk.contains("func initDefaultCompletionCmd"),
            "skeleton must preserve the buried declaration identifier"
        );
        // The license boilerplate must be folded, not kept verbatim.
        assert!(
            !sk.contains("The Cobra Authors"),
            "license boilerplate must be elided FIRST, not preserved"
        );
        // head/tail would have dropped the buried symbol — prove the contrast.
        let headtail = crate::compress::truncate_middle(&src, 20, 10);
        assert!(
            !headtail.contains("func initDefaultCompletionCmd"),
            "sanity: head/tail is expected to drop the buried symbol (the bug)"
        );
    }
    #[test]
    fn skeleton_large_body_emits_recovery_footer() {
        // #1 — past the amortisation threshold the actionable footer fires.
        let mut src = String::from("pub fn big() {\n");
        for i in 0..40 {
            let _ = writeln!(src, "    let v{i} = compute({i});");
        }
        src.push_str("}\n");
        let sk = crate::skeleton::skeleton(&src);
        assert!(sk.contains("lines …"), "inline marker missing");
        assert!(
            sk.contains("tokenade query") && sk.contains("Read the file"),
            "recovery footer must name the structural recovery routes: {sk}"
        );
        let (_, sav) = crate::skeleton::skeleton_with_savings(&src);
        assert!(
            sav.ratio() > 0.0,
            "footer must not invert savings on large input"
        );
    }
    #[test]
    fn skeleton_caps_at_token_budget() {
        // #2 — a skeleton with many declarations must stop at the
        // `crate::skeleton::SKELETON_MAX_TOKENS` budget (truncated at the next line
        // boundary) and report how many symbols remain unshown.
        let mut src = String::new();
        for i in 0..400 {
            let _ = writeln!(src, "pub fn function_number_{i}(arg: i32) -> i32 {{");
            let _ = writeln!(src, "    arg + {i}");
            src.push_str("}\n");
        }
        let sk = crate::skeleton::skeleton_ext(&src, Some("rs"));
        assert!(
            crate::tokenizer::estimate(&sk) <= crate::skeleton::SKELETON_MAX_TOKENS + 60,
            "skeleton must respect the ~{}-token budget (+footer); got {}",
            crate::skeleton::SKELETON_MAX_TOKENS,
            crate::tokenizer::estimate(&sk)
        );
        assert!(
            sk.contains("truncated at") && sk.contains("more symbol(s)"),
            "must report dropped symbol count: {sk}"
        );
        // Early decls survive; late ones are dropped (recoverable via query).
        assert!(sk.contains("function_number_0"), "first decl kept");
        assert!(
            !sk.contains("function_number_399"),
            "last decl dropped by cap"
        );
    }
    #[test]
    fn skeleton_ast_keeps_multiline_signature_whole() {
        // #1 — the AST path keeps a signature spanning several physical
        // lines as one unit and never matches decls inside comments/strings.
        let src = "\
// fn commented_out_decoy() {}
fn wrapped(
    a: i32,
    b: i32,
) -> i32 {
    let s = \"fn string_literal_decoy() {}\";
    a + b
}
";
        let sk = crate::skeleton::skeleton_ext(src, Some("rs"));
        assert!(sk.contains("fn wrapped("), "header kept");
        assert!(sk.contains("b: i32,"), "multi-line signature kept whole");
        assert!(sk.contains("-> i32 {"), "signature tail (with brace) kept");
        assert!(!sk.contains("commented_out_decoy"), "comment decl not kept");
        assert!(
            !sk.contains("string_literal_decoy"),
            "string-literal decl not kept"
        );
        assert!(sk.contains("lines …"), "body folded");
    }
    #[test]
    fn structural_truncate_doc_keeps_buried_section_headtail_would_drop() {
        // A long markdown doc whose key section is in the MIDDLE — exactly
        // what head/tail truncation discards.
        let mut md = String::from("# Intro\n\nWelcome to the guide.\n\n");
        for i in 0..60 {
            let _ = writeln!(md, "Filler intro paragraph line {i} with prose.");
        }
        md.push_str("\n## Authentication\n\nUse an API key in the header.\n\n");
        for i in 0..60 {
            let _ = writeln!(md, "More filler about authentication line {i}.");
        }
        md.push_str("\n## Troubleshooting\n\nCheck the logs.\n");

        let (out, sav) = crate::skeleton::structural_truncate(&md, "guide.md");
        assert!(
            out.contains("# Intro"),
            "outline must keep the intro heading"
        );
        assert!(
            out.contains("## Authentication"),
            "outline must keep the BURIED middle heading; got:\n{out}"
        );
        assert!(
            out.contains("## Troubleshooting"),
            "outline must keep the trailing heading"
        );
        assert!(sav.tokens_after < sav.tokens_before, "must shrink");

        // The positional head/tail net would have dropped the middle heading.
        let headtail = crate::compress::truncate_middle(&md, 20, 10);
        assert!(
            !headtail.contains("## Authentication"),
            "sanity: head/tail is expected to lose the buried section"
        );
    }
    #[test]
    fn structural_truncate_code_uses_skeleton_not_headtail() {
        // Unrecognised-extension source: declaration density is high, so we
        // skeleton-ise (keep signatures) rather than head/tail.
        let mut src = String::from("// license header line\n");
        for _ in 0..12 {
            src.push_str("// boilerplate license line\n");
        }
        // Kept under `crate::skeleton::SKELETON_MAX_TOKENS` so every decl survives — the
        // cap's tail-truncation is covered by `skeleton_caps_at_token_budget`.
        for i in 0..12 {
            let _ = writeln!(src, "fn handler_{i}() {{");
            let _ = writeln!(src, "    let x = compute({i});");
            let _ = writeln!(src, "    do_work(x);");
            src.push_str("}\n");
        }
        let (out, sav) = crate::skeleton::structural_truncate(&src, "weird.zig");
        assert!(out.contains("fn handler_0"), "skeleton keeps first decl");
        assert!(
            out.contains("fn handler_11"),
            "skeleton keeps the LAST decl that head/tail would drop"
        );
        assert!(out.contains("lines …"), "bodies folded with inline marker");
        assert!(sav.tokens_after < sav.tokens_before, "must shrink");
    }
    #[test]
    fn structural_truncate_flat_data_falls_back_to_headtail() {
        // Pure flat data, no headings, no declarations → head/tail fallback.
        // Lines are long enough that the kept content dominates the elision
        // banners (short unique lines are a pathological case where head/tail
        // can't win — there the caller keeps the original, as designed).
        let mut data = String::new();
        for i in 0..400 {
            let _ = writeln!(
                data,
                "record {i}: payload field alpha={} beta={} gamma=some-fairly-long-value-{i}",
                i * 7 % 13,
                i * 31 % 97
            );
        }
        let (out, _sav) = crate::skeleton::structural_truncate(&data, "rows.dat");
        // The decisive assertion: flat data routes through the positional
        // head/tail path verbatim (not outline, not skeleton).
        assert_eq!(
            out,
            crate::compress::generic(&data),
            "flat data must use the positional head/tail path verbatim"
        );
    }
    #[test]
    fn yaml_skeleton_caps_arrays_and_preserves_top_keys() {
        let mut y = String::from("name: redis\nnamespace: prod\ncontainers:\n");
        for i in 0..40 {
            let _ = writeln!(y, "  - name: c{i}");
        }
        let (out, sav) = crate::skeleton::skeleton_yaml_with_savings(&y);
        assert!(out.contains("name: redis"));
        assert!(out.contains("namespace: prod"));
        assert!(out.contains("- name: c0"));
        assert!(out.contains("- name: c4"));
        assert!(!out.contains("- name: c10"), "should cap at 5");
        assert!(out.contains("more item(s) elided"));
        assert!(
            sav.percent() > 50.0,
            "expected >50% savings on long-array YAML, got {:.1}%",
            sav.percent()
        );
    }
    #[test]
    fn yaml_skeleton_redacts_long_inline_secrets() {
        let mut y = String::from("apiVersion: v1\nkind: Secret\ndata:\n  token: ");
        y.push_str(&"a".repeat(800));
        y.push('\n');
        let out = crate::skeleton::skeleton_yaml(&y);
        assert!(out.contains("chars elided"), "huge secret not folded");
        assert!(!out.contains(&"a".repeat(100)));
    }
    #[test]
    fn yaml_skeleton_never_inflates_tiny_input() {
        let y = "a: 1\nb: 2\n";
        let (_, sav) = crate::skeleton::skeleton_yaml_with_savings(y);
        assert!(sav.tokens_after <= sav.tokens_before + 5);
    }
    #[test]
    fn markdown_skeleton_keeps_headings_and_one_blurb() {
        let mut md = String::from("# Title\n\nfirst paragraph short enough to keep.\n\n");
        md.push_str("## Section 1\n\n");
        for _ in 0..40 {
            md.push_str("body line that contributes to the long blurb.\n");
        }
        md.push_str("\n## Section 2\n\nshort body\n");
        let (out, sav) = crate::skeleton::skeleton_markdown_with_savings(&md);
        assert!(out.contains("# Title"));
        assert!(out.contains("## Section 1"));
        assert!(out.contains("## Section 2"));
        assert!(
            out.contains("more line(s) elided"),
            "long section not folded"
        );
        assert!(
            sav.percent() > 40.0,
            "expected >40% savings on long markdown, got {:.1}%",
            sav.percent()
        );
    }
    #[test]
    fn markdown_skeleton_summarises_code_blocks() {
        let mut md = String::from("# Doc\n\n```rust\n");
        for i in 0..100 {
            let _ = writeln!(md, "let x{i} = {i};");
        }
        md.push_str("```\n\n## After\n");
        let out = crate::skeleton::skeleton_markdown(&md);
        assert!(out.contains("```rust"));
        assert!(out.contains("100 lines hidden"));
        assert!(out.contains("## After"));
        assert!(!out.contains("let x50"));
    }
    #[test]
    fn terraform_skeleton_keeps_resource_headers_drops_bodies() {
        let mut tf = String::new();
        for i in 0..10 {
            let _ = write!(
                tf,
                "resource \"aws_instance\" \"web_{i}\" {{\n  \
                 ami           = \"ami-123\"\n  \
                 instance_type = \"t2.micro\"\n  \
                 tags          = {{\n    Name = \"web_{i}\"\n    Env  = \"prod\"\n  }}\n}}\n"
            );
        }
        let (out, sav) = crate::skeleton::skeleton_terraform_with_savings(&tf);
        for i in 0..10 {
            assert!(
                out.contains(&format!("\"web_{i}\"")),
                "resource header missing for {i}"
            );
        }
        // Body attributes folded.
        assert!(out.contains("line(s) elided"));
        assert!(!out.contains("ami-123"));
        assert!(
            sav.percent() > 50.0,
            "expected >50% savings on terraform, got {:.1}%",
            sav.percent()
        );
    }
    #[test]
    fn terraform_skeleton_handles_nested_blocks() {
        let tf = "resource \"a\" \"b\" {\n  tags = {\n    foo = \"bar\"\n  }\n}\n";
        let out = crate::skeleton::skeleton_terraform(tf);
        // Should still emit a closing brace for the outer block.
        assert!(out.contains("resource \"a\" \"b\""));
        assert!(out.contains('}'));
    }
}

#[cfg(test)]
mod type_body_tests {

    #[test]
    fn a_struct_keeps_its_fields() {
        let src = "/// Doc.\npub struct Plan {\n    /// what it fetches\n    pub fetch_url: String,\n    pub kind: Kind,\n}\n\nfn work(a: u8) -> u8 {\n    let x = a + 1;\n    x * 2\n}\n";
        let out = crate::skeleton::skeleton(src);
        assert!(out.contains("pub fetch_url: String"), "field lost: {out}");
        assert!(out.contains("pub kind: Kind"), "field lost: {out}");
        // The function body is still implementation and still goes.
        assert!(!out.contains("x * 2"), "function body kept: {out}");
        // A doc comment BETWEEN fields must not become an elision marker.
        // (One above the declaration still elides — that is existing behaviour
        // and not what this test is about.)
        let body: String = out
            .lines()
            .skip_while(|l| !l.contains("struct Plan"))
            .take_while(|l| !l.trim().starts_with('}'))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !body.contains("lines …"),
            "a comment between fields was announced instead of dropped: {body}"
        );
    }

    #[test]
    fn an_enum_keeps_its_variants() {
        let src = "pub enum Kind {\n    RedditThread,\n    CratesApi,\n}\n";
        let out = crate::skeleton::skeleton(src);
        assert!(
            out.contains("RedditThread") && out.contains("CratesApi"),
            "{out}"
        );
    }

    #[test]
    fn an_impl_block_is_still_implementation() {
        let src = "impl Plan {\n    fn go(&self) -> u8 {\n        let secret = 41;\n        secret + 1\n    }\n}\n";
        let out = crate::skeleton::skeleton(src);
        assert!(!out.contains("secret + 1"), "impl body kept: {out}");
    }

    #[test]
    fn a_python_class_body_is_still_implementation() {
        // No braces, so the brace-counted scan never fires — which is right:
        // a Python class body is methods.
        let src = "class Thing:\n    def go(self):\n        total = 1 + 1\n        return total\n";
        let out = crate::skeleton::skeleton(src);
        assert!(!out.contains("total = 1 + 1"), "python body kept: {out}");
    }

    #[test]
    fn a_generated_enum_is_capped() {
        let mut src = String::from("pub enum Big {\n");
        for i in 0..200 {
            src.push_str(&format!("    V{i},\n"));
        }
        src.push_str("}\n");
        let out = crate::skeleton::skeleton(&src);
        assert!(out.contains("V0,"), "start lost: {out}");
        assert!(
            !out.contains("V199,"),
            "a 200-variant listing was dumped whole"
        );
    }
}
