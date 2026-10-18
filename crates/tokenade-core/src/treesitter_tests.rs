// The tests of `crate::treesitter`, moved out of the production file.
//
// Mounted by `include!`, not by `mod`: the tokens are spliced INTO
// `treesitter`, so every `super::` keeps the meaning it had. Split by
// `srcsweep::production_lines`.

#[cfg(test)]
mod embedded_script {
    use super::*;

    fn names(src: &str, ext: &str) -> Vec<(String, usize)> {
        extract(src, ext)
            .map(|(s, _)| s.into_iter().map(|x| (x.name, x.line)).collect())
            .unwrap_or_default()
    }

    /// `<scriptish>` is not a script tag. The tag name must end at the `>`
    /// or at whitespace — a day was once lost to `<p[^>]*>`
    /// cheerfully matching `<pre>`.
    #[test]
    fn a_longer_tag_name_is_not_a_script_tag() {
        let html = "<scriptish>function ghost(){}</scriptish>";
        assert!(names(html, "html").is_empty(), "matched a different tag");
        let real = "<script>function seen(){}</script>";
        assert_eq!(names(real, "html").len(), 1, "the real tag was missed");
    }

    /// A symbol whose line points inside the extracted fragment sends the
    /// reader to the wrong place. Lines are rebased onto the HOST file.
    #[test]
    fn a_symbol_keeps_the_line_it_has_in_the_host_file() {
        let html = "<html>\n<body>\n<div>x</div>\n<script>\nfunction late(){}\n</script>\n</html>";
        let got = names(html, "html");
        assert_eq!(got.len(), 1, "got: {got:?}");
        // `function late` is on physical line 5 (1-based).
        assert_eq!(got[0].1, 5, "line was not rebased onto the host: {got:?}");
    }

    /// Each block carries its own offset, not the first one's.
    #[test]
    fn several_blocks_each_get_their_own_offset() {
        let html = "<script>\nfunction one(){}\n</script>\n<p>x</p>\n<script>\nfunction two(){}\n</script>";
        let mut got = names(html, "html");
        got.sort();
        assert_eq!(
            got,
            vec![("one".to_string(), 2), ("two".to_string(), 6)],
            "got: {got:?}"
        );
    }

    /// A `type=` that means DATA is not code. Measured over 37 623 real
    /// script tags: 2 471 json/template/plain blocks, which a JS parser
    /// happily turns into 186 phantom declarations.
    #[test]
    fn a_data_typed_block_contributes_nothing() {
        for ty in [
            "application/json",
            "application/ld+json",
            "text/template",
            "text/plain",
            "text/html",
        ] {
            let html = format!("<script type=\"{ty}\">function ghost(){{}}</script>");
            assert!(names(&html, "html").is_empty(), "{ty} was parsed as code");
        }
        // A lazy-loader type DOES carry javascript — the rule names what to
        // reject, not what to accept.
        let lazy = "<script type=\"rocketlazyloadscript\">function real(){}</script>";
        assert_eq!(
            names(lazy, "html").len(),
            1,
            "a lazy-loaded script was dropped"
        );
    }

    /// The formats that had NO grammar at all, and so no symbols whatever.
    #[test]
    fn a_single_file_component_yields_its_declarations() {
        let vue = "<template><div/></template>\n<script setup>\nfunction submit(){}\nclass Model {}\n</script>\n<style>.a{}</style>";
        let got = names(vue, "vue");
        let ns: Vec<&str> = got.iter().map(|(n, _)| n.as_str()).collect();
        assert!(
            ns.contains(&"submit") && ns.contains(&"Model"),
            "got: {got:?}"
        );
        // `<script setup>` — an attribute with no value, and the tag still ends.
        assert_eq!(got[0].1, 3, "line was not rebased: {got:?}");
    }

    /// HTML tag names are case-insensitive, and the two halves of this
    /// scanner disagreed: `</script` was matched case-insensitively while
    /// `<script` was not, so `<SCRIPT>` opened nothing. Rare — 3 tags in
    /// 37 295 on the local corpus — and free to get right.
    #[test]
    fn an_uppercase_script_tag_is_a_script_tag() {
        for src in [
            "<SCRIPT>function upper(){}</SCRIPT>",
            "<ScRiPt>function mixed(){}</ScRiPt>",
            "<SCRIPT TYPE=\"text/javascript\">function attrs(){}</SCRIPT>",
        ] {
            assert_eq!(names(src, "html").len(), 1, "missed: {src:?}");
        }
        // The tag-name boundary still holds, case-insensitively.
        assert!(names("<SCRIPTISH>function ghost(){}</SCRIPTISH>", "html").is_empty());
        // And a DATA type spelled in capitals is still data.
        assert!(names(
            "<SCRIPT TYPE=\"application/json\">function g(){}</SCRIPT>",
            "html"
        )
        .is_empty());
    }

    /// The close tag used to be found by lowercasing the REMAINDER of the
    /// document once per block, which is quadratic. Measured before: 20 000
    /// small blocks in 500 KB took **7.9 s**, and the worst real page in the
    /// corpus (513 KB, 412 tags) 270 ms. After one lowercase pass up front:
    /// 216 ms and 98 ms.
    ///
    /// The bound is deliberately loose — this asserts that the QUADRATIC is
    /// gone, not a particular speed, so a slow machine or a debug build still
    /// passes while the shape that took eight seconds does not.
    #[test]
    fn many_small_blocks_do_not_cost_quadratic_time() {
        let many = "<script>let a=1;</script>".repeat(20_000);
        let t0 = std::time::Instant::now();
        let _ = extract(&many, "html");
        let ms = t0.elapsed().as_millis();
        assert!(
            ms < 3_000,
            "20 000 blocks took {ms}ms — the quadratic scan is back"
        );
    }

    /// A bundled blob inlined in a page declares `r`, `i`, `o`, `e` and
    /// outranks the real API by sheer count. The minified-code detector is reused
    /// rather than given a second opinion.
    #[test]
    fn a_minified_block_contributes_nothing() {
        // One long line of real javascript, past the detector's floors.
        let bundle = (0..90)
            .map(|i| format!("function f{i}(a,b){{return a+b}};var v{i}=f{i}(1,2);"))
            .collect::<String>();
        assert!(
            bundle.len() > 4_000 && bundle.len() > 1_500,
            "fixture too small"
        );
        assert!(
            crate::minified::detect(&bundle),
            "the fixture must be admitted as minified, or it proves nothing"
        );
        let html = format!("<script>{bundle}</script>");
        assert!(names(&html, "html").is_empty(), "a bundle was indexed");

        // The SAME declarations, written out over lines, are kept: it is the
        // shape that is rejected and not the content.
        let readable = (0..90)
            .map(|i| format!("function f{i}(a,b) {{\n  return a + b\n}}\n"))
            .collect::<String>();
        assert!(!crate::minified::detect(&readable));
        let html2 = format!("<script>{readable}</script>");
        assert!(
            names(&html2, "html").len() >= 90,
            "hand-written code was dropped"
        );
    }

    /// Nobody runs `tokenade query "r"`. Measured over what survives the
    /// minified detector: 1 830 one-character names against 92 two-character
    /// ones, while hand-written templates carry nothing under four. The cut
    /// is at ONE — `id`, `db` and `ok` are names somebody could have chosen.
    #[test]
    fn a_single_character_name_is_dropped_and_a_two_character_one_is_not() {
        let html = "<script>\nfunction r(){}\nfunction id(){}\nfunction submit(){}\n</script>";
        let got: Vec<String> = names(html, "html").into_iter().map(|(n, _)| n).collect();
        assert!(
            !got.contains(&"r".to_string()),
            "a one-character name survived: {got:?}"
        );
        assert!(
            got.contains(&"id".to_string()),
            "a two-character name was cut: {got:?}"
        );
        assert!(got.contains(&"submit".to_string()), "got: {got:?}");
    }

    /// An unterminated tag must not hang or panic, and must not swallow the
    /// rest of the document as a body.
    #[test]
    fn an_unterminated_script_tag_is_dropped() {
        assert!(names("<script>function never(){}", "html").is_empty());
        assert!(names("<script", "html").is_empty());
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_extracts_fn_in_real_file() {
        let src = r#"
            // fn this_is_a_comment_not_a_function() {}
            fn hello_world() -> &'static str {
                "fn nested_string_not_a_function"
            }
            struct Foo { bar: u32 }
        "#;
        let (syms, _) = extract(src, "rs").expect("grammar should be present");
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"hello_world"));
        assert!(names.contains(&"Foo"));
        // The killer test: comments and string literals must NOT
        // produce false-positive symbols.
        assert!(!names.contains(&"this_is_a_comment_not_a_function"));
        assert!(!names.contains(&"nested_string_not_a_function"));
    }

    /// Every language's declared call kinds must actually produce edges.
    ///
    /// This is the sweep that found the defects, kept as a test rather than
    /// as three fixes: PHP declared four call kinds and produced ZERO edges,
    /// Go and C# and Swift lost every method call, and Java named the
    /// RECEIVER as the callee. A language whose `calls` list is wired to a
    /// resolver that does not know its identifier node fails silently — the
    /// graph is simply smaller, and nothing says so.
    ///
    /// `rb` is the one asymmetry and it is the grammar, not us: a bare `g`
    /// in Ruby is an identifier, indistinguishable from a local read, so
    /// only the explicit `o.m` is a call node.
    #[test]
    fn every_language_resolves_the_call_shapes_it_has() {
        let cases: &[(&str, &str, &[&str])] = &[
            ("rs", "fn f() { g(); h::k(); }", &["g", "k"]),
            ("py", "def f():\n    g()\n    o.m()\n", &["g", "m"]),
            (
                "js",
                "function f() { g(); o.m(); new W(); }",
                &["g", "m", "W"],
            ),
            (
                "ts",
                "function f() { g(); o.m(); new W(); }",
                &["g", "m", "W"],
            ),
            ("go", "func f() { g(); o.M() }", &["g", "M"]),
            (
                "java",
                "class A { void f() { g(); o.m(); new W(); } }",
                &["g", "m", "W"],
            ),
            ("c", "void f() { g(); }", &["g"]),
            ("cpp", "void f() { g(); new W(); }", &["g", "W"]),
            (
                "cs",
                "class A { void F() { G(); o.M(); new W(); } }",
                &["G", "M", "W"],
            ),
            (
                "php",
                "<?php\nfunction f() { g(); $o->m(); K::s(); new W(); }\n",
                &["g", "m", "s", "W"],
            ),
            ("rb", "def f\n  g\n  o.m\nend\n", &["m"]),
            ("swift", "func f() { g(); o.m() }", &["g", "m"]),
            ("kt", "fun f() { g(); o.m() }", &["g", "m"]),
            (
                "scala",
                "def f() = { g(); o.m(); new W() }",
                &["g", "m", "W"],
            ),
        ];
        for (ext, src, want) in cases {
            let (_, _, calls) = extract_full(src, ext).expect("grammar should be present");
            let mut got: Vec<&str> = calls.iter().map(|c| c.callee_name.as_str()).collect();
            got.sort_unstable();
            let mut want: Vec<&str> = want.to_vec();
            want.sort_unstable();
            assert_eq!(got, want, "{ext}");
        }
    }

    /// Constructing a class IS calling it, and for a class in these languages
    /// it is the only call shape there is — so a class with no `new` edge has
    /// no callers at all. Measured on workweave-router: 175 `new X()` over
    /// 131 TS/JS files, none of them in the graph.
    #[test]
    fn a_constructor_call_names_the_class() {
        for (ext, src) in [
            ("ts", "function make() { return new Widget(1, other); }"),
            ("js", "function make() { return new Widget(1); }"),
            ("cpp", "void f() { Widget* w = new Widget(); }"),
            ("scala", "def f() = { val w = new Widget() }"),
            ("php", "<?php\nfunction f() { $w = new Widget(); }\n"),
        ] {
            let (_, _, calls) = extract_full(src, ext).expect("grammar should be present");
            assert!(
                calls.iter().any(|c| c.callee_name == "Widget"),
                "{ext}: constructing Widget is not an edge to Widget"
            );
        }
    }

    /// `o.m()` calls `m`. Java answered `o` — the receiver — which is not a
    /// missing edge but a WRONG one: it credited the variable and lost the
    /// method, so "who calls m" was empty while "who calls o" was invented.
    #[test]
    fn a_method_call_names_the_method_not_the_receiver() {
        let (_, _, calls) =
            extract_full("class A { void f() { o.m(); } }", "java").expect("grammar");
        let names: Vec<&str> = calls.iter().map(|c| c.callee_name.as_str()).collect();
        assert_eq!(names, vec!["m"]);
    }

    #[test]
    fn python_extracts_class_and_def() {
        let src = "class Foo:\n    def bar(self):\n        return 1\n";
        let (syms, _) = extract(src, "py").expect("grammar should be present");
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Foo"));
        assert!(names.contains(&"bar"));
    }

    /// Every field, owned by its class, in the five languages that spell a
    /// field as an assignment through the receiver instead of declaring it.
    #[test]
    fn receiver_fields_are_indexed_with_their_class_as_owner() {
        for (ext, src, field) in [
            (
                "py",
                "class Account:\n    def __init__(self, n):\n        self.entries = {}\n",
                "entries",
            ),
            (
                "js",
                "class W {\n  constructor(o) {\n    this.retryBudget = o.n;\n  }\n}\n",
                "retryBudget",
            ),
            (
                "ts",
                "class R {\n  constructor() {\n    this.upstreamHost = \"x\";\n  }\n}\n",
                "upstreamHost",
            ),
            (
                "rb",
                "class M\n  def initialize(n)\n    @sample_window = n\n  end\nend\n",
                "sample_window",
            ),
            (
                "php",
                "<?php\nclass M {\n  function __construct($n) { $this->sampleWindow = $n; }\n}\n",
                "sampleWindow",
            ),
        ] {
            let (syms, _) = extract(src, ext).expect("grammar should be present");
            let hit = syms
                .iter()
                .find(|s| s.name == field)
                .unwrap_or_else(|| panic!("{ext}: {field} missing from the index"));
            assert_eq!(hit.kind, "field", "{ext}");
            assert!(!hit.owner.is_empty(), "{ext}: a field with no class");
        }
    }

    /// The narrow rule stays narrow. An assignment to anything but the
    /// receiver is a WRITE, and module-level `X = …` is the case the `py`
    /// members reserve declines on purpose — neither becomes a symbol, and
    /// no identifier from the VALUE side ever does.
    #[test]
    fn only_the_receiver_declares_a_field() {
        let src = concat!(
            "MODULE_CONST = 3\n",
            "class Foo:\n",
            "    def run(self, other):\n",
            "        other.remote_attr = 1\n",
            "        local_name = compute(payload_value)\n",
            "        self.kept = 2\n",
        );
        let (syms, _) = extract(src, "py").expect("grammar should be present");
        let fields: Vec<&str> = syms
            .iter()
            .filter(|s| s.kind == "field")
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(fields, vec!["kept"]);
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        for leaked in ["MODULE_CONST", "remote_attr", "local_name", "payload_value"] {
            assert!(!names.contains(&leaked), "{leaked} leaked into the index");
        }
    }

    /// A receiver assignment outside any class declares nothing: the class is
    /// the gate, since the scope gate that guards `members` cannot be — the
    /// declaration site here IS a function body.
    #[test]
    fn a_receiver_assignment_outside_a_class_is_not_a_field() {
        let src = "def helper(self):
    self.dangling = 1
";
        let (syms, _) = extract(src, "py").expect("grammar should be present");
        assert!(syms.iter().all(|s| s.kind != "field"));
    }

    /// Set on three code paths, printed once.
    #[test]
    fn a_field_assigned_repeatedly_is_one_row() {
        let src = concat!(
            "class C:\n",
            "    def __init__(self):\n",
            "        self.api_base = None\n",
            "        self.api_base = \"x\"\n",
            "    def reset(self):\n",
            "        self.api_base = None\n",
        );
        let (syms, _) = extract(src, "py").expect("grammar should be present");
        let rows: Vec<_> = syms.iter().filter(|s| s.name == "api_base").collect();
        assert_eq!(rows.len(), 1, "one field, one row");
        assert_eq!(rows[0].line, 3, "the first assignment wins");
    }

    /// `this[key] = v` names no field. Nothing in `receiver_field_name`
    /// tests for it: the grammars model a subscript as a node with no
    /// property field, so the extraction returns None on its own. This test
    /// is what holds that — it goes red if a grammar changes its mind.
    #[test]
    fn a_computed_receiver_access_names_no_field() {
        let (syms, _) = extract(
            "class W {\n  constructor(k) {\n    this[k] = 1;\n  }\n}\n",
            "js",
        )
        .expect("grammar should be present");
        assert!(syms.iter().all(|s| s.kind != "field"));
    }

    /// The refactor that let receiver fields through a method body must NOT
    /// let a function-local `const` claim the enclosing type: that gate is
    /// what keeps `Server::BUF` from appearing on `Server`'s surface, and the
    /// two callers now share one upward walk.
    #[test]
    fn a_function_local_const_still_has_no_owner() {
        let src = "impl Server {
    fn run(&self) {
        const BUF: usize = 8;
    }
}
";
        let (syms, _) = extract(src, "rs").expect("grammar should be present");
        let buf = syms
            .iter()
            .find(|s| s.name == "BUF")
            .expect("function-local consts are indexed");
        assert!(buf.owner.is_empty(), "a local must not borrow Server");
    }

    #[test]
    fn typescript_extracts_interface_and_type_alias() {
        let src = "export interface Foo { x: number } export type Bar = string;";
        let (syms, _) = extract(src, "ts").expect("grammar should be present");
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Foo"));
        assert!(names.contains(&"Bar"));
    }

    #[test]
    fn unknown_extension_yields_none() {
        assert!(extract("anything", "xyz-bogus").is_none());
    }

    #[test]
    fn imports_captured_for_python() {
        let src = "import os\nfrom typing import List\n";
        let (_, imports) = extract(src, "py").expect("grammar");
        assert!(!imports.is_empty());
    }
}
#[cfg(test)]
mod ast_depth_guard {
    use super::*;

    /// A generated union type, the shape that killed `tokenade map`.
    ///
    /// Left-nested in the TypeScript grammar: 12 000 members become 12 000
    /// nested `union_type` nodes. tree-sitter parses it iteratively; our walks
    /// recurse, so without [`MAX_AST_DEPTH`] this SEGFAULTS the process — a
    /// crash of the whole command, not a missed symbol.
    fn generated_union(members: usize) -> String {
        let mut s = String::from("export type IamAction =\n");
        for i in 0..members {
            s.push_str(&format!("  | \"svc:Operation{i}\"\n"));
        }
        s.push_str(";\n");
        s
    }

    #[test]
    fn a_deeply_nested_generated_file_does_not_take_the_process_with_it() {
        let src = generated_union(12_000);
        let (syms, _imports, _calls) = extract_full(&src, "ts").expect("parse");
        // The declaration that CONTAINS the monster sits at depth 2 and must
        // survive: capping the descent must not cost the thing we index for.
        assert!(
            syms.iter().any(|s| s.name == "IamAction"),
            "the cap swallowed the declaration itself: {:?}",
            syms.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_skeleton_walk_survives_the_same_file() {
        let src = generated_union(12_000);
        let spans = skeleton_spans(&src, "ts").expect("parse");
        assert!(!spans.is_empty(), "no skeleton span for the declaration");
    }

    /// The THIRD walk, reached only through this entry point. Each of the
    /// three recurses on its own, so each needs the cap: a guard on two of
    /// them leaves the third free to crash.
    #[test]
    fn the_test_body_walk_survives_the_same_file() {
        let src = generated_union(12_000);
        let (spans, tests) = skeleton_spans_and_test_bodies(&src, "ts").expect("parse");
        assert!(!spans.is_empty());
        assert!(
            tests.is_empty(),
            "a generated union declares no test module"
        );
    }

    /// Ordinary code is nowhere near the cap. Measured over 90 corpus
    /// repositories, lowering it to 120 leaves the symbol count IDENTICAL
    /// (417 120) and only at 60 does anything move — so 500 keeps a margin of
    /// more than four times the deepest real tree.
    #[test]
    fn everyday_nesting_is_untouched() {
        let mut src = String::from("fn outer_fn() {\n");
        for i in 0..40 {
            src.push_str(&"    ".repeat(i + 1));
            src.push_str(&format!("if x{i} {{\n"));
        }
        for i in (0..40).rev() {
            src.push_str(&"    ".repeat(i + 1));
            src.push_str("}\n");
        }
        src.push_str("}\nfn inner_fn() {}\n");
        let (syms, _, _) = extract_full(&src, "rs").expect("parse");
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"outer_fn") && names.contains(&"inner_fn"),
            "{names:?}"
        );
    }
}
#[cfg(test)]
mod every_language_still_speaks {
    use super::*;

    /// One minimal file per grammar: a declaration that calls something.
    ///
    /// The point is not the parse — it is that the `calls` node kinds a
    /// [`LangSpec`] declares still exist in the grammar it names. A
    /// tree-sitter upgrade that RENAMES a node kind empties that language's
    /// call graph and nothing says so: `tokenade query --callers` simply
    /// answers "no callers" forever, which reads like an answer.
    ///
    /// Measured 2026-08-28 over 12 001 corpus files: five of the fourteen
    /// grammars produced ZERO call edges (`java`, `cpp`, `php`, `kt`,
    /// `scala`) and two produced fewer than five. That turned out to be
    /// corpus scarcity, not a defect — an AI-tooling corpus is Rust, Python,
    /// TypeScript and Go — but it is exactly the shape a real regression
    /// would have had, and nothing in the suite could have told the two
    /// apart.
    const SNIPPETS: &[(&str, &str, &str)] = &[
        ("rs", "fn a() { helper(1); }\n", "helper"),
        ("py", "def a():\n    helper(1)\n", "helper"),
        ("js", "function a() { helper(1); }\n", "helper"),
        ("ts", "function a(): void { helper(1); }\n", "helper"),
        ("go", "package m\nfunc a() { helper(1) }\n", "helper"),
        ("java", "class C { void a() { helper(1); } }\n", "helper"),
        ("c", "void a(void) { helper(1); }\n", "helper"),
        ("cpp", "void a() { helper(1); }\n", "helper"),
        ("cs", "class C { void A() { Helper(1); } }\n", "Helper"),
        ("php", "<?php function a() { helper(1); }\n", "helper"),
        ("rb", "def a\n  helper(1)\nend\n", "helper"),
        ("swift", "func a() { helper(1) }\n", "helper"),
        ("kt", "fun a() { helper(1) }\n", "helper"),
        (
            "scala",
            "object O { def a(): Unit = { helper(1) } }\n",
            "helper",
        ),
    ];

    /// The sweep half: a language added without a snippet fails HERE rather
    /// than going untested. Same shape as the `#[cfg(test)]` sweep — a fix at
    /// the fourteen says nothing about the fifteenth.
    #[test]
    fn every_grammar_that_declares_calls_has_a_snippet() {
        for l in LANGS.iter().filter(|l| !l.calls.is_empty()) {
            assert!(
                SNIPPETS.iter().any(|(e, _, _)| *e == l.name),
                "`{}` declares call kinds and has no snippet here: a grammar \
                 upgrade could empty its call graph and nothing would say so",
                l.name
            );
        }
    }

    #[test]
    fn every_snippet_yields_its_declaration_and_its_call() {
        for (ext, src, callee) in SNIPPETS {
            let Some((syms, _imports, calls)) = extract_full(src, ext) else {
                panic!("{ext}: the grammar no longer parses a minimal file");
            };
            assert!(!syms.is_empty(), "{ext}: the declaration was not indexed");
            assert!(
                calls.iter().any(|c| c.callee_name == *callee),
                "{ext}: no call edge to `{callee}` — the `calls` kinds this \
                 spec declares no longer match the grammar"
            );
        }
    }
}
#[cfg(test)]
mod structural_hash_tests {
    use super::*;

    fn hashes(src: &str) -> Vec<(String, String)> {
        extract(src, "rs")
            .unwrap()
            .0
            .into_iter()
            .map(|s| (s.name, s.structural_hash))
            .collect()
    }

    #[test]
    fn a_pure_rename_keeps_the_structural_hash() {
        let a = hashes("pub fn alpha(x: u32) -> u32 { x + 1 }");
        let b = hashes("pub fn beta(x: u32) -> u32 { x + 1 }");
        assert_ne!(a[0].0, b[0].0, "the names differ");
        assert_eq!(
            a[0].1, b[0].1,
            "same code under a different name must hash equal"
        );
    }

    #[test]
    fn a_changed_body_changes_the_hash() {
        let a = hashes("pub fn alpha(x: u32) -> u32 { x + 1 }");
        let b = hashes("pub fn alpha(x: u32) -> u32 { x + 2 }");
        assert_ne!(a[0].1, b[0].1);
    }

    #[test]
    fn reformatting_and_comments_do_not_change_the_hash() {
        let a = hashes("pub fn alpha(x: u32) -> u32 { x + 1 }");
        let b = hashes(
            "/// Adds one, at last.\npub fn alpha(\n    x: u32\n) -> u32 {\n    // step\n    x + 1\n}",
        );
        assert_eq!(a[0].1, b[0].1, "whitespace and comments carry no structure");
    }

    #[test]
    fn a_trailing_comma_is_a_real_token_not_formatting() {
        // Not a bug: rustfmt adding a trailing comma IS a token-stream change,
        // and a structural hash that ignored it would have to special-case
        // every grammar's optional separators. The rename phase tolerates this
        // because it runs after the exact-content phase, which already matched
        // the untouched declarations.
        let a = hashes("pub fn alpha(x: u32) -> u32 { x + 1 }");
        let b = hashes("pub fn alpha(x: u32,) -> u32 { x + 1 }");
        assert_ne!(a[0].1, b[0].1);
    }

    #[test]
    fn operand_order_is_not_a_collision() {
        // Identical token multiset, different tree: the node kinds separate them.
        let a = hashes("pub fn f() { let x = foo(bar); }");
        let b = hashes("pub fn f() { let bar = foo(x); }");
        assert_ne!(a[0].1, b[0].1);
    }

    #[test]
    fn the_hash_is_stable_across_calls() {
        assert_eq!(hashes("pub fn f() { g(); }"), hashes("pub fn f() { g(); }"));
    }
    /// A rule that splits a scope by node kind is not a scope rule.
    ///
    /// The `members` gate excluded everything inside a function body to keep
    /// `let`/`var` bindings out, while nested `fn`s went in through a
    /// different branch — so the behaviour was "locals are excluded unless
    /// they are functions". Measured on this repo 2026-08-27: 379
    /// function-local `const`/`static` in production code, 294 distinct
    /// names, 22 of a 40-name sample with no row in the index.
    #[test]
    fn a_rust_function_local_const_is_a_compile_time_binding() {
        let src = "\
pub fn outer() -> u32 {
    const LOCAL_CONST_ZQ: u32 = 42;
    static LOCAL_STATIC_ZQ: u32 = 43;
    let runtime_local_zq = 7;
    fn nested_fn_zq() -> u32 { 1 }
    LOCAL_CONST_ZQ + LOCAL_STATIC_ZQ + runtime_local_zq + nested_fn_zq()
}
";
        let (syms, _) = extract(src, "rs").expect("rust parses");
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        for want in ["LOCAL_CONST_ZQ", "LOCAL_STATIC_ZQ", "nested_fn_zq", "outer"] {
            assert!(names.contains(&want), "missing {want}: {names:?}");
        }
        assert!(
            !names.contains(&"runtime_local_zq"),
            "a `let` binding reached the index: {names:?}"
        );
    }

    /// The widening is per LANGUAGE and per NODE KIND, on the grammar rather
    /// than on taste, and it did not leak.
    ///
    /// Go declares `const_spec` in `members` exactly like Rust declares
    /// `const_item`, so it is the language that would have moved if the gate
    /// had simply been removed. It stays where it was: a Go function-local
    /// `const` is not indexed, because nothing has been measured for Go.
    #[test]
    fn the_widening_did_not_leak_to_another_language() {
        let src = "\
package main

const GO_MODULE_CONST_ZQ = 1

func outer() int {
\tconst GO_LOCAL_CONST_ZQ = 2
\treturn GO_LOCAL_CONST_ZQ
}
";
        let (syms, _) = extract(src, "go").expect("go parses");
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"GO_MODULE_CONST_ZQ"), "{names:?}");
        assert!(
            !names.contains(&"GO_LOCAL_CONST_ZQ"),
            "the Rust widening leaked into Go: {names:?}"
        );
    }

    /// The exception is NAMED, not an accident of which branch a kind falls
    /// into: every language spec must state it, and only Rust may be
    /// non-empty until another grammar is measured the same way.
    #[test]
    fn only_measured_languages_carry_a_function_scope_exception() {
        let widened: Vec<&str> = LANGS
            .iter()
            .filter(|l| !l.function_scoped_members.is_empty())
            .map(|l| l.name)
            .collect();
        assert_eq!(
            widened,
            vec!["rs"],
            "a language was widened without a measurement behind it"
        );
        assert!(LANGS.len() > 5, "the sweep is not seeing the specs");
    }

    /// A `macro_rules!` definition declares a NAME the rest of the crate
    /// calls, and the index answers "where is X declared".
    ///
    /// `tokenade query cmd_entry_points` answered "no symbol matching" for a
    /// macro invoked in roughly six hundred files of this repo. Counted on
    /// the benchmark corpus 2026-08-27: 105 definitions, 90 distinct names,
    /// none reachable.
    #[test]
    fn a_macro_rules_definition_is_a_symbol() {
        let src = "\
macro_rules! my_macro_zq {
    () => { 1 };
}

pub fn user() -> u32 { my_macro_zq!() }
";
        let (syms, _) = extract(src, "rs").expect("rust parses");
        let m = syms
            .iter()
            .find(|s| s.name == "my_macro_zq")
            .unwrap_or_else(|| {
                panic!(
                    "macro missing: {:?}",
                    syms.iter().map(|s| &s.name).collect::<Vec<_>>()
                )
            });
        assert_eq!(m.kind, "macro", "a macro is not a fn: {m:?}");
    }

    /// `abstract class Foo` is a DIFFERENT node kind from `class Foo`, so an
    /// abstract class was a class the index could not find. 195 of them on
    /// this machine's TypeScript.
    #[test]
    fn an_abstract_class_is_still_a_class() {
        let src = "\
export abstract class AbstractThingZq {
  abstract run(): void;
}

export class PlainThingZq {
  run(): void {}
}
";
        let (syms, _) = extract(src, "ts").expect("typescript parses");
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"PlainThingZq"), "{names:?}");
        assert!(
            names.contains(&"AbstractThingZq"),
            "an abstract class is missing: {names:?}"
        );
    }

    /// `#define MAX_PATH 4096` is what a C constant IS — the language has no
    /// other way to spell one at file scope — and the index could not reach
    /// a single one. Counted 2026-08-27 on this machine: 106 989 named
    /// `#define` in `.h` files.
    #[test]
    fn a_c_define_is_a_symbol() {
        let src = "\
#define MAX_PATH_ZQ 4096
#define SQUARE_ZQ(x) ((x) * (x))

struct ThingZq { int a; };
int do_thing_zq(int n) { return n; }
";
        let (syms, _) = extract(src, "c").expect("c parses");
        let by: std::collections::HashMap<&str, &str> = syms
            .iter()
            .map(|s| (s.name.as_str(), s.kind.as_str()))
            .collect();
        assert_eq!(by.get("MAX_PATH_ZQ"), Some(&"macro"), "{by:?}");
        assert_eq!(by.get("SQUARE_ZQ"), Some(&"macro"), "{by:?}");
        assert!(by.contains_key("ThingZq"), "{by:?}");
        assert!(by.contains_key("do_thing_zq"), "{by:?}");
    }

    /// A header reached through `.h` maps to the C spec, so the same must
    /// hold there — that is where the defines actually live.
    #[test]
    fn a_header_extension_reaches_the_same_spec() {
        let src = "#define HDR_ONLY_ZQ 1\nint f(void);\n";
        let (syms, _) = extract(src, "h").expect("header parses");
        assert!(
            syms.iter().any(|s| s.name == "HDR_ONLY_ZQ"),
            "{:?}",
            syms.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
    }

    /// Modern C++ names a type with `using` and with `concept`, and neither
    /// was a type the index could find.
    #[test]
    fn a_using_alias_and_a_concept_are_types() {
        let src = "\
using AliasZq = int;
template <class T> concept ConceptZq = requires(T t) { t.go(); };
#define CPP_DEFINE_ZQ 7
";
        let (syms, _) = extract(src, "cpp").expect("cpp parses");
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        for want in ["AliasZq", "ConceptZq", "CPP_DEFINE_ZQ"] {
            assert!(names.contains(&want), "missing {want}: {names:?}");
        }
    }

    /// The half of a contract somebody must implement, third language.
    ///
    /// `declare function foo(): void` is a `function_signature`, a different
    /// node kind from `function_declaration`; an `abstract` member is an
    /// `abstract_method_signature`, not a `method_signature`. Counted
    /// 2026-08-27 outside `node_modules` (which this indexer skips): 2 681
    /// declared functions and 131 abstract methods.
    #[test]
    fn a_declared_signature_is_a_symbol() {
        let src = "\
export declare function declaredFnZq(x: number): string;

export abstract class ShapeZq {
  abstract areaZq(): number;
  concreteZq(): number { return 1; }
}
";
        let (syms, _) = extract(src, "ts").expect("typescript parses");
        let by: std::collections::HashMap<&str, &str> = syms
            .iter()
            .map(|s| (s.name.as_str(), s.kind.as_str()))
            .collect();
        assert_eq!(by.get("declaredFnZq"), Some(&"fn"), "{by:?}");
        assert_eq!(by.get("areaZq"), Some(&"method"), "{by:?}");
        assert!(by.contains_key("concreteZq"), "{by:?}");
    }

    /// `type X = Y` is a `type_alias`, a different node from the `type_spec`
    /// of `type X Y`.
    #[test]
    fn a_go_type_alias_is_a_type() {
        let src = "\
package main

type AliasZq = int
type NamedZq int
";
        let (syms, _) = extract(src, "go").expect("go parses");
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"NamedZq"), "{names:?}");
        assert!(
            names.contains(&"AliasZq"),
            "the alias is missing: {names:?}"
        );
    }

    /// A trait's associated type declares a name an implementor must supply.
    /// Its sibling `type_binding` — the `Item = u8` of `Iterator<Item = u8>`
    /// — is a constraint at a USE site and must stay out, or "where is Item
    /// declared" answers with every generic bound that mentions it.
    #[test]
    fn an_associated_type_is_declared_but_a_binding_is_not() {
        let src = "\
pub trait StreamZq {
    type ItemZq;
    fn next(&mut self) -> Option<Self::ItemZq>;
}

pub fn consume(_: impl Iterator<Item = u8>) {}
";
        let (syms, _) = extract(src, "rs").expect("rust parses");
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"ItemZq"),
            "associated type missing: {names:?}"
        );
        assert!(
            !names.contains(&"Item"),
            "a generic BINDING was indexed as a declaration: {names:?}"
        );
    }

    /// Two doors, one file, two answers.
    ///
    /// `query MIN_INPUT` found a module-scope const all along, while
    /// `skeleton` on the same file folded the line into `… N lines …`. A
    /// `const NAME: T = v;` declares a name and has no body to elide — it IS
    /// a signature. Measured 2026-08-27 over real sources: 36 of 324
    /// top-level Rust declarations and 60 of 86 C ones were missing from
    /// their skeleton.
    #[test]
    fn what_the_index_records_the_skeleton_shows() {
        let rs = "\
pub const MODULE_CONST_ZQ: usize = 4;
static MODULE_STATIC_ZQ: usize = 5;

pub struct HolderZq {
    pub field_zq: u8,
}

pub fn work() -> usize {
    const INNER_ZQ: usize = 1;
    MODULE_CONST_ZQ + INNER_ZQ
}
";
        let out = crate::skeleton::skeleton_ext(rs, Some("rs"));
        assert!(
            out.contains("MODULE_CONST_ZQ"),
            "module const missing:\n{out}"
        );
        assert!(
            out.contains("MODULE_STATIC_ZQ"),
            "module static missing:\n{out}"
        );
        assert!(out.contains("HolderZq"), "{out}");
    }

    /// MODULE scope only, and the type body is not the case that proves it.
    ///
    /// A struct's fields are already kept whole — they ARE its signature —
    /// so the restriction earns its place elsewhere: a `const` inside a
    /// function body is a detail of that body, and the skeleton exists to
    /// elide bodies. Without the scope test it would be lifted out and
    /// printed beside the module's own declarations.
    #[test]
    fn a_function_local_const_does_not_reach_the_skeleton() {
        let rs = "\
pub const MODULE_ONLY_ZQ: usize = 1;

pub fn work() -> usize {
    const FUNCTION_LOCAL_ZQ: usize = 2;
    MODULE_ONLY_ZQ + FUNCTION_LOCAL_ZQ
}
";
        let out = crate::skeleton::skeleton_ext(rs, Some("rs"));
        assert!(out.contains("MODULE_ONLY_ZQ"), "{out}");
        assert!(
            !out.contains("FUNCTION_LOCAL_ZQ"),
            "a function-local const was lifted into the skeleton:\n{out}"
        );
    }

    /// `#define` is C's only file-scope constant, and the skeleton dropped
    /// every one: `semantic.c` lost 21 of its 22.
    #[test]
    fn a_c_define_is_in_the_skeleton() {
        let c = "\
#include <stdio.h>
#define MAX_PATH_ZQ 4096
#define SQUARE_ZQ(x) ((x) * (x))

struct ThingZq { int a; };

int do_thing_zq(int n) {
    int local = n + 1;
    return local;
}
";
        let out = crate::skeleton::skeleton_ext(c, Some("c"));
        assert!(out.contains("MAX_PATH_ZQ"), "define missing:\n{out}");
        assert!(
            out.contains("SQUARE_ZQ"),
            "function-like define missing:\n{out}"
        );
        assert!(out.contains("do_thing_zq"), "{out}");
        assert!(!out.contains("int local"), "a body leaked:\n{out}");
    }
}
