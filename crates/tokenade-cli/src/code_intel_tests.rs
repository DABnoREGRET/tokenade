// The tests of `code_intel`.
//
// Spliced in by `include!` from `code_intel.rs`: the same module, hence the same
// private access and the same `super::` paths, without a single visibility line
// changed. This is the form this repository already uses for 33 other modules.
//
// A `//!` is impossible here: a file spliced in by `include!` is not a module,
// and an INNER doc comment has nothing to qualify.

#[cfg(test)]
mod query_modal_totals_tests {
    use super::*;

    /// Ranking by call-graph centrality needs the call graph, and since it
    /// moved to its own column an index only carries it
    /// when somebody asked. A caller that forgets gets a map of zeros — every
    /// symbol equally central, which is indistinguishable from "no symbol is
    /// central" and silently drops the ranking to its legacy fallback.
    ///
    /// `scout` was exactly that, for two days, found by this sweep's question
    /// rather than by any failing assertion. The rule is asserted on the
    /// SOURCE because there is nothing at runtime to observe: the degraded
    /// answer is a valid answer.
    #[test]
    fn every_centrality_caller_holds_the_call_graph() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut checked = 0usize;
        let mut stack = vec![src];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().and_then(|x| x.to_str()) != Some("rs") {
                    continue;
                }
                let Ok(body) = std::fs::read_to_string(&p) else {
                    continue;
                };
                // Never read our own assertion as production code — the same
                // trap, one file over. Through `srcsweep`, the declarer: cutting
                // at the FIRST `#[cfg(test)]` also throws away all production
                // code that follows a test module mounted mid-file.
                let prod = tokenade_core::srcsweep::production_lines(&body)
                    .into_iter()
                    .map(|(_, l)| l)
                    .collect::<Vec<_>>()
                    .join("\n");
                let prod = prod.as_str();
                if !prod.contains(".centrality()") {
                    continue;
                }
                checked += 1;
                assert!(
                    prod.contains("fresh_index_with_calls") || prod.contains("load_calls"),
                    "{} ranks by centrality without ever loading the call graph",
                    p.display()
                );
            }
        }
        assert!(
            checked >= 2,
            "the sweep found {checked} centrality callers — it has stopped looking"
        );
    }

    fn idx_with_symbols(names: &[(&str, &str)]) -> tokenade_core::index::Index {
        let mut idx = idx_with_calls(Vec::new());
        idx.symbols = names
            .iter()
            .enumerate()
            .map(|(i, (name, file))| tokenade_core::index::Symbol {
                name: (*name).into(),
                kind: "fn".into(),
                file: (*file).into(),
                line: i + 1,
                end_line: i + 1,
                decorators: Vec::new(),
                structural_hash: String::new(),
                self_hash: String::new(),
                bases: Vec::new(),
                owner: String::new(),
            })
            .collect();
        idx
    }

    /// `find_in` matches case-insensitively and everything after it compares
    /// the name exactly, so a query in the wrong case listed the declarations
    /// and then said NOTHING about callers — on a command whose help promises
    /// both halves. Measured on a six-file Go tree: `query Run` answered
    /// `called by: Caller, Other`, `query run` answered nothing.
    #[test]
    fn a_query_in_the_wrong_case_still_answers_who_calls_it() {
        let names: Vec<(String, String)> = (0..6)
            .map(|i| ("Run".to_string(), format!("mod{i}.go")))
            .collect();
        let refs: Vec<(&str, &str)> = names.iter().map(|(n, f)| (n.as_str(), f.as_str())).collect();
        let idx = idx_with_symbols(&refs);
        let (hits, _) = idx.find_in("run", 30, &[]);
        // Anti-vacuity: the search really does answer a lower-case query with
        // upper-case rows, or there is nothing here to resolve.
        assert_eq!(hits.len(), 6, "the search did not match across case");
        assert_eq!(
            attribution_for(&idx, &hits, "run", &[]),
            Attribution { name: "Run", declarations: 6, decl_file: None }
        );
    }

    /// The three halves must describe the SAME symbol. With one declaration
    /// the caveat is not printed and the callers read as fact, so the file
    /// attribution is pinned to — and it has to be the resolved spelling's.
    #[test]
    fn a_sole_declaration_is_pinned_under_the_name_the_index_holds() {
        let idx = idx_with_symbols(&[("Run", "only.go"), ("Runner", "other.go")]);
        let (hits, _) = idx.find_in("run", 30, &[]);
        assert_eq!(
            attribution_for(&idx, &hits, "run", &[]),
            Attribution { name: "Run", declarations: 1, decl_file: Some("only.go") },
            "the sole declaration was not found under the indexed spelling"
        );
    }

    /// The right spelling is left alone, and a tree that really holds BOTH
    /// spellings keeps them apart — they are different symbols, so the query
    /// stays exact rather than picking one.
    #[test]
    fn a_tree_holding_both_spellings_keeps_them_distinct() {
        let idx = idx_with_symbols(&[("Run", "a.go"), ("Run", "b.go")]);
        let (hits, _) = idx.find_in("run", 30, &[]);
        assert_eq!(resolved_spelling(&hits, "Run"), "Run");

        let both = idx_with_symbols(&[("Run", "a.go"), ("run", "b.go")]);
        let (hits, _) = both.find_in("run", 30, &[]);
        assert_eq!(hits.len(), 2, "fixture does not hold both spellings");
        assert_eq!(
            resolved_spelling(&hits, "run"),
            "run",
            "two real spellings were collapsed into one"
        );
        assert_eq!(resolved_spelling(&hits, "Run"), "Run");
    }

    /// The two halves answer the SAME question, so they must answer it the
    /// same way. `--json` kept the typed name for one iteration after the text
    /// half stopped, and its silence is louder: `"callers": []` beside
    /// `"count": 6` is a positive claim made to a machine.
    #[test]
    fn the_json_half_answers_what_the_text_half_answers() {
        let mut idx = idx_with_calls(vec![
            ("Caller", "Run", "main.go", 6),
            ("Other", "Run", "main.go", 10),
        ]);
        idx.symbols = (0..6)
            .map(|i| tokenade_core::index::Symbol {
                name: "Run".into(),
                kind: "fn".into(),
                file: format!("mod{i}.go"),
                line: 3,
                end_line: 5,
                decorators: Vec::new(),
                structural_hash: String::new(),
                self_hash: String::new(),
                bases: Vec::new(),
                owner: String::new(),
            })
            .collect();
        let (hits, _) = idx.find_in("run", 30, &[]);
        let at = attribution_for(&idx, &hits, "run", &[]);

        let line = callers_line(&idx, &at, true);
        let (arr, total) = callers_json(&idx, &at, true);
        // Anti-vacuity: if the text half said nothing either, the two agree on
        // nothing and this proves nothing.
        assert!(line.contains("Caller (main.go:6)"), "text half is silent: {line}");
        assert_eq!(total, serde_json::Value::from(2));
        assert_eq!(arr.as_array().map(|a| a.len()), Some(2));
    }

    /// The caveat says how far the callers line may be trusted, so its number
    /// has to be the real one. Counting exact matches among `find_in`'s rows
    /// counts the DISPLAY CAP instead, and the same output then contradicts
    /// itself: "30 of 972 matches shown" above, "30 declarations share it"
    /// below.
    #[test]
    fn the_ambiguity_caveat_counts_declarations_not_the_display_window() {
        let mut names: Vec<(String, String)> = (0..40)
            .map(|i| ("target".to_string(), format!("src/m{i}.rs")))
            .collect();
        // Substring matches too, so `find_in`'s total is not the answer either.
        names.extend((0..5).map(|i| ("target_helper".to_string(), format!("src/h{i}.rs"))));
        let refs: Vec<(&str, &str)> = names
            .iter()
            .map(|(n, f)| (n.as_str(), f.as_str()))
            .collect();
        let idx = idx_with_symbols(&refs);

        let (hits, total) = idx.find_in("target", 30, &[]);
        let in_window = hits.iter().filter(|s| s.name == "target").count();
        // Anti-vacuity: the fixture must actually be capped, or the two
        // readings agree and the test proves nothing.
        assert_eq!(in_window, 30, "fixture is not capped: {in_window}");
        assert_eq!(total, 45);

        assert_eq!(
            declarations_for(&idx, &hits, "target", &[]),
            40,
            "the caveat is counting the display window, not the declarations"
        );
    }

    /// A scoped query must report the ambiguity INSIDE its scope; counting the
    /// whole index would make a narrowed query look as ambiguous as a wide one.
    #[test]
    fn the_count_is_scoped_the_way_the_search_is() {
        let idx = idx_with_symbols(&[
            ("target", "src/a.rs"),
            ("target", "src/b.rs"),
            ("target", "vendor/c.rs"),
        ]);
        assert_eq!(declarations_for(&idx, &[], "target", &[]), 3);
        assert_eq!(
            declarations_for(&idx, &[], "target", &["src".to_string()]),
            2
        );
    }

    fn idx_with_calls(calls: Vec<(&str, &str, &str, usize)>) -> tokenade_core::index::Index {
        tokenade_core::index::Index {
            root: String::new(),
            symbols: Vec::new(),
            imports: std::collections::BTreeMap::new(),
            files_indexed: 0,
            mtimes: std::collections::BTreeMap::new(),
            built_by: String::new(),
            rebased: false,
            calls: calls
                .into_iter()
                .map(|(caller, callee, file, line)| tokenade_core::index::Call {
                    caller_name: caller.into(),
                    callee_name: callee.into(),
                    file: file.into(),
                    line,
                    qualifier: String::new(),
                })
                .collect(),
        }
    }

    /// `query --help` promises "where is this defined, and who calls it".
    /// The second half was never rendered.
    #[test]
    fn the_callers_line_names_them_and_counts_the_rest() {
        let calls: Vec<(&str, &str, &str, usize)> = (0..8)
            .map(|i| {
                (
                    ["a", "b", "c", "d", "e", "f", "g", "h"][i],
                    "target",
                    [
                        "a.rs", "b.rs", "c.rs", "d.rs", "e.rs", "f.rs", "g.rs", "h.rs",
                    ][i],
                    i + 1,
                )
            })
            .collect();
        let idx = idx_with_calls(calls);
        let line = callers_line(&idx, &Attribution { name: "target", declarations: 1, decl_file: None }, true);
        assert!(line.starts_with("called by: a (a.rs:1), "), "{line}");
        assert_eq!(
            line.matches(".rs:").count(),
            QUERY_CALLERS_SHOWN,
            "the cap must hold"
        );
        assert!(line.contains("+3 more"), "{line}");
        assert!(!line.contains("by NAME"), "one declaration, no caveat");
        assert!(line.ends_with('\n'));
    }

    /// The graph is keyed on the NAME. When several declarations share it the
    /// list cannot be attributed to one of them, and the honest answer to a
    /// question the data cannot separate is to say so.
    #[test]
    fn the_callers_line_says_when_a_name_has_several_declarations() {
        let idx = idx_with_calls(vec![("caller", "Run", "m.rs", 4)]);
        let one = callers_line(&idx, &Attribution { name: "Run", declarations: 1, decl_file: None }, true);
        assert!(!one.contains("by NAME"), "{one}");
        let many = callers_line(&idx, &Attribution { name: "Run", declarations: 18, decl_file: None }, true);
        assert!(many.contains("18 declarations share it"), "{many}");
    }

    /// An absent scan and a clean scan must not serialise to the same thing.
    /// Silence on this line means "nothing calls it" — a positive claim about
    /// the code. When the graph could not be read there is no claim to make,
    /// and saying nothing would make one.
    #[test]
    fn an_unreadable_call_graph_is_not_an_absence_of_callers() {
        let idx = idx_with_calls(vec![("caller", "target", "m.rs", 4)]);
        let ok = callers_line(&idx, &Attribution { name: "target", declarations: 1, decl_file: None }, true);
        assert!(ok.starts_with("called by: caller"), "{ok}");

        let unknown = callers_line(&idx, &Attribution { name: "target", declarations: 1, decl_file: None }, false);
        assert!(unknown.contains("could not be read"), "{unknown}");
        assert!(
            unknown.contains("NOT"),
            "the line must deny the reading it replaces: {unknown}"
        );
        // And it fires even where the graph would have been empty anyway —
        // that is the whole point: the two cases must not look alike.
        let empty = idx_with_calls(Vec::new());
        assert!(callers_line(&empty, &Attribution { name: "target", declarations: 1, decl_file: None }, true).is_empty());
        assert!(!callers_line(&empty, &Attribution { name: "target", declarations: 1, decl_file: None }, false).is_empty());
    }

    /// The same rule on the machine-readable side, where it matters more: a
    /// script cannot read a caveat, only the shape of the value.
    #[test]
    fn json_callers_are_null_not_empty_when_the_graph_is_unreadable() {
        let idx = idx_with_calls(vec![("caller", "target", "m.rs", 4)]);
        let (arr, total) = callers_json(&idx, &Attribution { name: "target", declarations: 1, decl_file: None }, true);
        assert_eq!(arr.as_array().map(|a| a.len()), Some(1));
        assert_eq!(total, serde_json::Value::from(1u64));

        let (arr, total) = callers_json(&idx, &Attribution { name: "target", declarations: 1, decl_file: None }, false);
        assert!(
            arr.is_null(),
            "an unreadable graph must not serialise as []"
        );
        assert!(total.is_null());

        // Nothing calls it, and we DID look: an empty array, not null.
        let (arr, total) = callers_json(&idx, &Attribution { name: "nobody", declarations: 1, decl_file: None }, true);
        assert_eq!(arr.as_array().map(|a| a.len()), Some(0));
        assert_eq!(total, serde_json::Value::from(0u64));
    }

    /// Nothing calls it: no line at all, not an empty header.
    #[test]
    fn the_callers_line_is_absent_when_nothing_calls_it() {
        let idx = idx_with_calls(vec![("caller", "elsewhere", "m.rs", 4)]);
        assert!(callers_line(&idx, &Attribution { name: "target", declarations: 1, decl_file: None }, true).is_empty());
        assert!(callers_line(&idx_with_calls(Vec::new()), &Attribution { name: "target", declarations: 1, decl_file: None }, true).is_empty());
    }

    fn sym(name: &str, file: &str) -> tokenade_core::index::Symbol {
        tokenade_core::index::Symbol {
            name: name.into(),
            kind: "fn".into(),
            file: file.into(),
            line: 1,
            end_line: 1,
            decorators: Vec::new(),
            bases: Vec::new(),
            owner: String::new(),
            structural_hash: String::new(),
            self_hash: String::new(),
        }
    }

    /// `count` exists to answer "how many?". It answered with the display cap,
    /// which is indistinguishable from a genuine result of exactly that size —
    /// the one mode where truncation cannot be seen.
    #[test]
    fn counting_matches_reports_how_many_matched_not_how_many_fit() {
        let hits: Vec<_> = (0..30)
            .map(|i| sym("detect", &format!("m{i}.rs")))
            .collect();
        let results = vec![("detect".to_string(), hits)];

        let counted =
            output_mode_body_query_with_totals(OutputMode::Count, &results, false, &[2985], None);
        assert_eq!(counted.trim(), "2985", "the answer is the match count");

        let paths =
            output_mode_body_query_with_totals(OutputMode::Paths, &results, false, &[2985], None);
        assert!(
            paths.contains("of 2985 matches shown"),
            "a capped path list must say so: {paths}"
        );

        // Nothing capped → no notice, and the count is the plain length.
        let small = vec![("only_once".to_string(), vec![sym("only_once", "solo.rs")])];
        let c = output_mode_body_query_with_totals(OutputMode::Count, &small, false, &[1], None);
        assert_eq!(c.trim(), "1");
        let p = output_mode_body_query_with_totals(OutputMode::Paths, &small, false, &[1], None);
        assert!(!p.contains("matches shown"), "no notice when complete: {p}");

        // Batched: each query owns its own total. Reading the first one for
        // every query reports the busy name's count under the rare one, and
        // `count` is the mode a caller trusts to be exact.
        let batched = vec![
            ("detect".to_string(), vec![sym("detect", "m0.rs")]),
            ("only_once".to_string(), vec![sym("only_once", "solo.rs")]),
        ];
        let c2 =
            output_mode_body_query_with_totals(OutputMode::Count, &batched, true, &[2985, 1], None);
        assert_eq!(c2, "detect: 2985\nonly_once: 1\n", "per-query totals: {c2}");
    }

    /// `outline` is the mode whose whole output IS the list of declarations,
    /// and it was the one rendering that handed back the cap in silence.
    /// `paths`, `full` and `--json` all disclose, and `count` reports the
    /// pre-cap figure — four routes agreeing and one not.
    #[test]
    fn an_outlined_list_that_was_cut_says_so() {
        let hits: Vec<_> = (0..30).map(|i| sym("new", &format!("m{i}.rs"))).collect();
        let results = vec![("new".to_string(), hits)];
        let out =
            output_mode_body_query_with_totals(OutputMode::Outline, &results, false, &[77], None);
        assert_eq!(
            out.lines().filter(|l| !l.starts_with('[')).count(),
            30,
            "every declaration still printed: {out}"
        );
        assert!(
            out.contains("[30 of 77 matches shown"),
            "an outline that was cut must say so: {out}"
        );
    }

    /// …and a complete list grows nothing. A notice on an uncut answer is
    /// the same defect wearing the other face: it invents a hidden rest.
    #[test]
    fn a_complete_outline_grows_no_notice() {
        let results = vec![("only_once".to_string(), vec![sym("only_once", "solo.rs")])];
        let out =
            output_mode_body_query_with_totals(OutputMode::Outline, &results, false, &[1], None);
        assert!(
            !out.contains("matches shown"),
            "no notice when complete: {out}"
        );
    }

    /// Batched queries are rendered under one `==>` heading each, so the
    /// notice belongs to ITS query. A single trailing note would attach the
    /// second query's cut to the first one's list.
    #[test]
    fn each_batched_query_carries_its_own_notice() {
        let cut: Vec<_> = (0..30).map(|i| sym("new", &format!("m{i}.rs"))).collect();
        let results = vec![
            ("new".to_string(), cut),
            ("only_once".to_string(), vec![sym("only_once", "solo.rs")]),
        ];
        let out =
            output_mode_body_query_with_totals(OutputMode::Outline, &results, true, &[77, 1], None);
        let cut_at = out.find("[30 of 77 matches shown").expect("notice missing");
        let second = out
            .find("==> only_once <==")
            .expect("second heading missing");
        assert!(
            cut_at < second,
            "the notice must sit under the query it belongs to: {out}"
        );
        assert_eq!(
            out.matches("matches shown").count(),
            1,
            "the complete query must not borrow it: {out}"
        );
    }

    /// One sentence, one declarer. `paths` here and `full` in the caller
    /// wrote it out separately; a third copy is how a wording drifts.
    #[test]
    fn every_capped_rendering_says_it_in_the_same_words() {
        let note = truncation_note(30, 77);
        let hits: Vec<_> = (0..30).map(|i| sym("new", &format!("m{i}.rs"))).collect();
        let results = vec![("new".to_string(), hits)];
        for mode in [OutputMode::Paths, OutputMode::Outline] {
            let out = output_mode_body_query_with_totals(mode, &results, false, &[77], None);
            assert!(
                out.contains(note.trim()),
                "{mode:?} words it differently: {out}"
            );
        }
        // Built in two halves: spelled whole, this needle would find
        // ITSELF in the file it is scanning. And the scan is the whole
        // crate, not just this file — the first sweep looked only here,
        // so re-inlining the sentence in `output.rs` (where the other
        // copy actually lived) stayed green.
        let needle = format!("{} — {}", "matches shown", "narrow the name");
        let mut copies = 0usize;
        let mut stack = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in entries.flatten() {
                let path = e.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|x| x.to_str()) != Some("rs") {
                    continue;
                }
                let Ok(body) = std::fs::read_to_string(&path) else {
                    continue;
                };
                copies += body.matches(&needle).count();
            }
        }
        assert_eq!(
            copies, 1,
            "the sentence must be written once, in truncation_note"
        );
    }
}

#[cfg(test)]
mod collapse_repeats_tests {
    /// The owner prefix is kept exactly where it tells two entries apart.
    #[test]
    fn an_owner_that_discriminates_is_kept() {
        let names = ["Server.Timeout", "Client.Timeout", "Server.Name"];
        let out = super::drop_useless_owners(&names);
        assert_eq!(out, vec!["Server.Timeout", "Client.Timeout", "Name"]);
    }

    /// …and dropped everywhere else. A file with one enum repeated its name
    /// on every variant, spending twelve characters nine times to say
    /// nothing and eating the twelve-name budget.
    #[test]
    fn an_owner_that_discriminates_nothing_is_dropped() {
        let names = ["E.Io", "E.Sql", "E.Json"];
        let out = super::drop_useless_owners(&names);
        assert_eq!(out, vec!["Io", "Sql", "Json"]);
    }

    /// Four `impl From<X> for E { fn from }` produce four IDENTICAL qualified
    /// forms, so qualifying discriminates nothing and they must still
    /// collapse to `from ×4` rather than to `E.from ×4`.
    #[test]
    fn identical_qualified_forms_still_collapse() {
        let names = ["E.from", "E.from", "E.from", "E.from"];
        let line = super::render_kind_line("fn", &names, 12);
        assert!(line.contains("from \u{d7}4"), "got {line:?}");
        assert!(!line.contains("E.from"), "got {line:?}");
    }

    /// An `impl Trait for Type` label carries no dot; the whole string is the
    /// discriminator and must survive untouched.
    #[test]
    fn a_trait_impl_keeps_its_trait() {
        let names = ["Display for E", "Error for E", "E"];
        let out = super::drop_useless_owners(&names);
        assert_eq!(out, vec!["Display for E", "Error for E", "E"]);
    }

    use super::collapse_repeats;

    /// `archetypes.rs` printed `const (5): MAX_RESCUED, MAX_RESCUED,
    /// MAX_NAMES, MAX_CHARS, MAX_RESCUED` — three of five slots on one name.
    /// A file may declare the same constant in several function bodies and
    /// the index records each. Measured over 70 real sources: 9 carry a
    /// repeat, 27 redundant entries, 22 slots lost inside the twelve-name
    /// cap.
    #[test]
    fn a_repeat_becomes_one_entry_with_a_count() {
        let got = collapse_repeats(&["MAX_RESCUED", "MAX_RESCUED", "MAX_NAMES", "MAX_RESCUED"]);
        assert_eq!(got, vec![("MAX_RESCUED", 3), ("MAX_NAMES", 1)]);
    }

    /// FIRST-appearance order, because the caller shows the first twelve in
    /// DECLARATION order on purpose — a file's head is where its API lives.
    #[test]
    fn order_is_first_appearance_not_frequency() {
        let got = collapse_repeats(&["zeta", "alpha", "alpha", "alpha"]);
        assert_eq!(got, vec![("zeta", 1), ("alpha", 3)]);
    }

    /// And the cap now measures DISTINCT names: twelve repeats of one name
    /// used to fill the whole line.
    #[test]
    fn twelve_repeats_of_one_name_are_one_entry() {
        let names = vec!["same"; 12];
        let got = collapse_repeats(&names);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0], ("same", 12));
    }

    #[test]
    fn nothing_repeated_is_left_exactly_alone() {
        let got = collapse_repeats(&["a", "b", "c"]);
        assert_eq!(got, vec![("a", 1), ("b", 1), ("c", 1)]);
    }
    /// The raw count stays raw, the cap counts DISTINCT, and a repeat wears
    /// its multiplier.
    #[test]
    fn a_kind_line_counts_declarations_but_caps_on_distinct_names() {
        let line = super::render_kind_line(
            "const",
            &[
                "MAX_RESCUED",
                "MAX_RESCUED",
                "MAX_NAMES",
                "MAX_CHARS",
                "MAX_RESCUED",
            ],
            12,
        );
        assert_eq!(
            line, "  const (5): MAX_RESCUED \u{d7}3, MAX_NAMES, MAX_CHARS\n",
            "got {line}"
        );
    }

    /// Twelve repeats of one name used to fill the whole line; now they are
    /// one entry and eleven other names fit beside them.
    #[test]
    fn repeats_do_not_spend_the_cap() {
        let mut names = vec!["same"; 12];
        names.extend(["a", "b", "c"]);
        let line = super::render_kind_line("fn", &names, 12);
        assert!(
            line.starts_with("  fn (15): same \u{d7}12, a, b, c\n"),
            "got {line}"
        );
        assert!(!line.contains("more"), "nothing should overflow: {line}");
    }

    /// And the overflow is counted in DISTINCT names too, or the number
    /// disagrees with the list beside it.
    #[test]
    fn the_overflow_counts_distinct_names() {
        let names: Vec<String> = (0..20).map(|i| format!("n{i}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let line = super::render_kind_line("fn", &refs, 12);
        assert!(line.contains("+8 more"), "got {line}");
    }
}

#[cfg(test)]
mod the_callers_line_asks_about_a_declaration {
    use super::{callers_line, Attribution};

    /// `query` must ask about the DECLARATION, not the name, whenever there is
    /// only one — because that is exactly when it prints no caveat and the
    /// callers read as fact.
    ///
    /// MEASURED 2026-09-06 on this fixture with the released 1.1.8:
    /// `called by: load_rows (app/db.py:7), make (app/real_user.py:6)` — one
    /// real caller and one invented from a `queue.Queue`. `impact` on the same
    /// tree had already stopped reporting the invented one, so the two commands
    /// contradicted each other about the same index.
    fn tree() -> tokenade_core::index::Index {
        let d = std::env::temp_dir().join(format!(
            "tkd-callers-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::create_dir_all(&d);
        let mk = |rel: &str, body: &str| {
            let p = d.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        };
        mk(
            "fixtures/doubles.py",
            "class StubCursor:\n    def zqexecute(self, sql):\n        return sql\n",
        );
        mk(
            "app/db.py",
            "import queue\n\n\ndef load_rows():\n\
             \u{20}   zqcur = queue.Queue()\n\
             \u{20}   return zqcur.zqexecute(\"select 1\")\n",
        );
        mk(
            "app/real_user.py",
            "from fixtures.doubles import StubCursor\n\n\ndef make():\n\
             \u{20}   zqreal = StubCursor()\n\
             \u{20}   return zqreal.zqexecute(\"select 1\")\n",
        );
        let idx = tokenade_core::index::Index::build(&d).unwrap();
        let _ = std::fs::remove_dir_all(&d);
        idx
    }

    #[test]
    fn one_declaration_names_only_the_callers_that_can_reach_it() {
        let idx = tree();
        let line = callers_line(&idx, &Attribution { name: "zqexecute", declarations: 1, decl_file: Some("fixtures/doubles.py") }, true);
        assert!(
            line.contains("make"),
            "the genuine caller is missing: {line:?}"
        );
        assert!(
            !line.contains("load_rows"),
            "a queue.Queue was named as a caller: {line:?}"
        );
    }

    /// And `--json` must ask the same question. It is the half that kept the
    /// typed name after the text half stopped, so it gets the same fixture and
    /// the same assertion rather than being trusted to have followed.
    #[test]
    fn the_json_half_asks_about_the_declaration_too() {
        use super::{callers_json, Attribution};
        let idx = tree();
        let pinned = Attribution {
            name: "zqexecute",
            declarations: 1,
            decl_file: Some("fixtures/doubles.py"),
        };
        let (arr, _) = callers_json(&idx, &pinned, true);
        let names: Vec<String> = arr
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap_or("").to_string())
            .collect();
        // Anti-vacuity: the name-keyed answer DOES carry the phantom, so the
        // assertion below is about the pinning and not about an empty result.
        let by_name = Attribution {
            name: "zqexecute",
            declarations: 1,
            decl_file: None,
        };
        let (loose, _) = callers_json(&idx, &by_name, true);
        let loose_names: Vec<String> = loose
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap_or("").to_string())
            .collect();
        assert!(
            loose_names.iter().any(|n| n == "load_rows"),
            "the fixture no longer produces the phantom, so this proves nothing: {loose_names:?}"
        );
        assert!(
            !names.iter().any(|n| n == "load_rows"),
            "a queue.Queue was named as a caller in --json: {names:?}"
        );
    }

    /// Anti-vacuity: without the declaration the old name-keyed answer comes
    /// back, phantom and all. If this passed too, the test above would be
    /// proving nothing about the plumbing.
    /// The two lines that decide WHICH question `query` asks.
    #[test]
    fn one_declaration_is_recognised_and_several_are_not() {
        use super::sole_declaration_file;
        let mk = |name: &str, file: &str| tokenade_core::index::Symbol {
            name: name.into(),
            kind: "fn".into(),
            file: file.into(),
            line: 1,
            end_line: 1,
            decorators: Vec::new(),
            bases: Vec::new(),
            owner: String::new(),
            structural_hash: String::new(),
            self_hash: String::new(),
        };
        let one = [mk("zqexecute", "a.py"), mk("other", "b.py")];
        let refs: Vec<&_> = one.iter().collect();
        assert_eq!(
            sole_declaration_file(&refs, "zqexecute"),
            Some("a.py"),
            "a single declaration must be recognised, or `query` never asks the \
             reachability question at all"
        );
        let two = [mk("zqexecute", "a.py"), mk("zqexecute", "b.py")];
        let refs2: Vec<&_> = two.iter().collect();
        assert_eq!(
            sole_declaration_file(&refs2, "zqexecute"),
            None,
            "two declarations must fall back to the name-keyed answer, which \
             carries its own caveat"
        );
        let none: Vec<&tokenade_core::index::Symbol> = Vec::new();
        assert_eq!(sole_declaration_file(&none, "zqexecute"), None);
    }

    #[test]
    fn without_the_declaration_the_name_keyed_answer_still_names_both() {
        let idx = tree();
        let line = callers_line(&idx, &Attribution { name: "zqexecute", declarations: 1, decl_file: None }, true);
        assert!(
            line.contains("load_rows") && line.contains("make"),
            "the name-keyed path changed too, so the comparison proves nothing: {line:?}"
        );
    }
}

#[cfg(test)]
mod miss_line_tests {
    use super::{miss_line, semantic_miss_line};

    /// A miss says what was searched and where to go next.
    ///
    /// MEASURED 2026-09-04: `query zzz_no_such_symbol` answered
    /// `no symbol matching 'zzz_no_such_symbol'` and nothing else, against
    /// 25 278 symbols in 1 680 files — a confident negative that cannot be
    /// told apart from standing in the wrong directory. The declarant's own
    /// doc already called that "the shape this repo keeps fixing".
    #[test]
    fn a_miss_names_the_corpus_and_a_way_out() {
        let mut idx = tokenade_core::index::Index {
            files_indexed: 1680,
            ..Default::default()
        };
        // A corpus with real symbols in it, none of them close to the query:
        // the count must be the INDEX's own, and an empty index cannot tell a
        // dropped count from a true zero.
        for n in ["alpha_handler", "beta_writer", "gamma_reducer"] {
            idx.symbols.push(tokenade_core::index::Symbol {
                name: n.to_string(),
                kind: "fn".to_string(),
                file: "src/lib.rs".to_string(),
                line: 1,
                decorators: Vec::new(),
                structural_hash: String::new(),
                self_hash: String::new(),
                end_line: 1,
                bases: Vec::new(),
                owner: String::new(),
            });
        }
        let m = miss_line("zqxjv_no_such_symbol", &idx);
        assert!(
            m.starts_with("no symbol matching 'zqxjv_no_such_symbol'"),
            "{m}"
        );
        assert!(
            m.contains("3 symbols in 1680 indexed file(s) searched"),
            "the corpus is not named: {m}"
        );
        assert!(
            m.contains("tokenade semantic"),
            "no way out is offered: {m}"
        );

        let s = semantic_miss_line("xqjv wgtb", 1680);
        assert!(
            s.starts_with("no match for 'xqjv wgtb'"),
            "the echo is lost: {s}"
        );
        assert!(
            s.contains("1680 indexed file(s) searched"),
            "the corpus is not named: {s}"
        );
        assert!(
            s.contains("grep -rn") && s.contains("tokenade query"),
            "no way out: {s}"
        );
        // NOT `--limit`: a query that matched nothing was cut by relevance,
        // and raising the depth changes nothing.
        assert!(
            !s.contains("--limit"),
            "points at a flag that cannot help: {s}"
        );
    }
}
