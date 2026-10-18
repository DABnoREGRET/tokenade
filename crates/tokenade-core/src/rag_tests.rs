// The tests of `crate::rag`, moved out of the production file.
//
// Mounted by `include!`: the tokens are spliced INTO `rag`, so every
// `super::` keeps its meaning. Split by `srcsweep::production_lines`.

#[cfg(test)]
mod tests {
    /// The emit loop writes a breadcrumb above each run; the budget did not
    /// count it. Harmless while headings are short — and 5 648 tokens over
    /// budget on a page where `segment` reads whole sentences as headings
    /// (MEASURED 2026-08-01, `tokenade.net/llms.txt`: 500-token budget,
    /// 6 117-token extract, of which 469 were the sections it had paid for).
    #[test]
    fn a_paragraph_long_heading_cannot_smuggle_itself_past_the_budget() {
        let sentence = "this whole sentence is parsed as a heading because the page has no real hierarchy at all";
        let mut page = String::new();
        for k in 0..6 {
            page.push_str(&format!("## {sentence} number {k}\n"));
            page.push_str(&"body text about keepalive timeouts and defaults ".repeat(12));
            page.push('\n');
        }
        let budget = 300;
        let out = prompt_scoped_extract_rag(&page, "what about keepalive timeouts", budget)
            .expect("anchored");
        let got = crate::tokenizer::estimate(&out);
        // Budget + the documented fusion tail (budget / fusion_div), with slack
        // for the gap markers.
        let ceiling = budget + (budget as f64 / fusion_div()) as usize + 40;
        assert!(got <= ceiling, "extract {got} tokens, ceiling {ceiling}");
    }

    /// A document with no headings becomes one synthetic section that
    /// `subsplit` cuts into blocks, and every block claimed to be the intro.
    /// Measured on a 1 500-line blob: a two-passage answer labelled BOTH
    /// passages `(intro)`, one opening at line 128 and the other at line 902.
    #[test]
    fn only_the_opening_block_of_an_unheaded_document_is_the_intro() {
        let unheaded = |level: usize| Section {
            level,
            heading_line: String::new(),
            heading: String::new(),
            breadcrumb: Vec::new(),
            body_lines: vec!["fn handler_902() {}".to_string()],
            group: 0,
            has_code: false,
            has_table: false,
            has_param: false,
        };
        // `subsplit` keeps the section's own level on the first block and
        // gives every later one `level + 1`.
        assert_eq!(unheaded(0).breadcrumb_label(), "> (intro)");
        assert_eq!(unheaded(1).breadcrumb_label(), "> (continued)");
        assert_eq!(unheaded(2).breadcrumb_label(), "> (continued)");
        // Whatever it says, it must not claim a position it cannot know.
        for l in 1..=3 {
            assert!(
                !unheaded(l).breadcrumb_label().contains("intro"),
                "level {l} is not the opening block"
            );
        }
    }

    /// ...and a real heading still produces a real breadcrumb, which is the
    /// case this label exists for.
    #[test]
    fn a_heading_still_names_where_the_passage_came_from() {
        let sec = Section {
            level: 2,
            heading_line: "## Retries".to_string(),
            heading: "Retries".to_string(),
            breadcrumb: vec!["Guide".to_string()],
            body_lines: vec!["body".to_string()],
            group: 0,
            has_code: false,
            has_table: false,
            has_param: false,
        };
        assert_eq!(sec.breadcrumb_label(), "> Guide > Retries");
        // An intro section in a document that DOES have headings is still an
        // intro: it really is what comes before the first heading.
        let intro = Section {
            level: 0,
            heading_line: String::new(),
            heading: String::new(),
            breadcrumb: Vec::new(),
            body_lines: vec!["preamble".to_string()],
            group: 0,
            has_code: false,
            has_table: false,
            has_param: false,
        };
        assert_eq!(intro.breadcrumb_label(), "> (intro)");
    }

    #[test]
    fn a_breadcrumb_is_a_label_not_a_payload() {
        let long = "word ".repeat(400);
        let sec = Section {
            level: 2,
            heading_line: format!("## {long}"),
            heading: long.clone(),
            breadcrumb: vec![long.clone(), long.clone()],
            body_lines: vec!["body".to_string()],
            group: 0,
            has_code: false,
            has_table: false,
            has_param: false,
        };
        let label = sec.breadcrumb_label();
        assert!(
            label.chars().count() <= 161,
            "{} chars",
            label.chars().count()
        );
        assert!(label.ends_with('…'));
        // A short one is untouched.
        let plain = Section {
            level: 2,
            heading_line: "## Methods".to_string(),
            heading: "Methods".to_string(),
            breadcrumb: vec!["Vec".to_string()],
            body_lines: vec![],
            group: 0,
            has_code: false,
            has_table: false,
            has_param: false,
        };
        assert_eq!(plain.breadcrumb_label(), "> Vec > Methods");
    }

    #[test]
    fn code_comments_do_not_split_sections() {
        // Sibling of `markdown::outline`'s fence guard. Here a `#` comment
        // decided SECTION BOUNDARIES, so it split a retrievable passage in
        // two and titled it with the comment — degrading the
        // `expand-ref --prompt` lookups documentation depends on once stashed.
        let md = "## Creating tasks\n\nUse create_task to run a coroutine.\n\n```python\n# Wait until both tasks are completed\nawait task1\n```\n\nThe task is scheduled immediately.\n";
        let secs = segment(md);
        assert!(
            secs.iter()
                .all(|s| !s.heading.contains("Wait until both tasks")),
            "code comment became a section heading: {:?}",
            secs.iter().map(|s| &s.heading).collect::<Vec<_>>()
        );
    }

    use super::*;

    const DOC: &str = "\
intro line one
intro line two

# Vec

A contiguous growable array type.

## Methods

### remove

Removes and returns the element at position index.

### swap_remove

Removes an element from the vector and returns it, replacing it with the last element. This does not preserve ordering but is O(1).

## Capacity

### with_capacity

Constructs a new, empty Vec with at least the specified capacity.
";

    /// A title page heading with nothing under it says nothing alone; under a
    /// heading whose next section is emitted, it names what follows.
    #[test]
    fn a_lone_empty_heading_is_not_a_passage() {
        let secs = segment("# Guide\n## Fourth Edition\n## Install\nRun the installer.\n");
        let idx = |h: &str| secs.iter().position(|s| s.heading == h).unwrap();
        let (ed, inst) = (idx("Fourth Edition"), idx("Install"));
        assert_eq!(inst, ed + 1);
        let alone: std::collections::BTreeSet<usize> = [ed].into_iter().collect();
        assert_eq!(super::super::rag::pack::emit_selected(&secs, &alone), "");
        let with_next: std::collections::BTreeSet<usize> = [ed, inst].into_iter().collect();
        assert!(
            super::super::rag::pack::emit_selected(&secs, &with_next).contains("## Fourth Edition")
        );
    }

    #[test]
    fn segments_headings_and_intro() {
        let secs = segment(DOC);
        // intro + Vec + Methods + remove + swap_remove + Capacity + with_capacity
        assert_eq!(secs.len(), 7);
        assert_eq!(secs[0].level, 0);
        assert_eq!(secs[1].heading, "Vec");
        assert_eq!(secs[1].level, 1);
        // breadcrumb of `remove` is Vec > Methods.
        let rm = secs.iter().find(|s| s.heading == "remove").unwrap();
        assert_eq!(
            rm.breadcrumb,
            vec!["Vec".to_string(), "Methods".to_string()]
        );
    }

    #[test]
    fn subtree_covers_whole_section() {
        let secs = segment(DOC);
        // `Methods` (index 2) owns remove + swap_remove but NOT Capacity.
        let methods_idx = secs.iter().position(|s| s.heading == "Methods").unwrap();
        let end = subtree_end(&secs, methods_idx);
        let covered: Vec<&str> = secs[methods_idx..end]
            .iter()
            .map(|s| s.heading.as_str())
            .collect();
        assert_eq!(covered, vec!["Methods", "remove", "swap_remove"]);
    }

    #[test]
    fn parse_heading_requires_space() {
        assert_eq!(
            parse_heading("## Methods"),
            Some((2, "Methods".to_string()))
        );
        assert_eq!(parse_heading("###### deep"), Some((6, "deep".to_string())));
        assert!(parse_heading("#no-space").is_none());
        assert!(parse_heading("####### too many").is_none());
        assert!(parse_heading("plain text").is_none());
        // Four spaces / tab before `# ` = indented code, not H1.
        assert!(parse_heading("    # Wait until both tasks").is_none());
        assert!(parse_heading("\t# comment").is_none());
        // Nested ATX still matches when indented (PDF extractors).
        assert_eq!(
            parse_heading("                  ## Introduction"),
            Some((2, "Introduction".to_string()))
        );
        assert_eq!(parse_heading("  ## Still"), Some((2, "Still".to_string())));
    }

    #[test]
    fn classify_detects_shapes() {
        let mut sec = Section {
            level: 1,
            heading_line: "# H".into(),
            heading: "H".into(),
            breadcrumb: vec![],
            group: 0,
            body_lines: vec![
                "```rust".into(),
                "let x = 1;".into(),
                "```".into(),
                "- name: the parameter".into(),
                "| a | b |".into(),
            ],
            has_code: false,
            has_table: false,
            has_param: false,
        };
        classify(&mut sec);
        assert!(sec.has_code && sec.has_param && sec.has_table);
    }

    #[test]
    fn subsplit_splits_fat_sections_and_keeps_fences_atomic() {
        let long_para = "word ".repeat(80); // ~400 chars
        let doc = format!(
            "# Big\n\n{long_para}\n\n{long_para}\n\n```rust\nlet a = 1;\nlet b = 2;\n```\n\n{long_para}\n"
        );
        let secs = subsplit(segment(&doc));
        assert!(secs.len() > 1, "fat section must split, got {}", secs.len());
        // First block keeps the heading line; continuations don't repeat it.
        assert_eq!(secs[0].heading_line, "# Big");
        assert!(secs[1].heading_line.is_empty());
        // Continuations keep heading text + sit one level deeper so the
        // parent's subtree covers them.
        assert_eq!(secs[1].heading, "Big");
        assert_eq!(secs[1].level, secs[0].level + 1);
        assert_eq!(subtree_end(&secs, 0), secs.len());
        // The code fence is atomic: its open/close live in the same block.
        for s in &secs {
            let opens = s
                .body_lines
                .iter()
                .filter(|l| l.trim_start().starts_with("```"))
                .count();
            assert_eq!(
                opens % 2,
                0,
                "fence split across blocks: {:?}",
                s.body_lines
            );
        }
        // Every block within the size envelope (atomic oversize excepted).
        for s in &secs {
            let chars: usize = s.body_lines.iter().map(|l| l.chars().count() + 1).sum();
            assert!(chars <= SPLIT_MAX_CHARS + 120, "block too fat: {chars}");
        }
    }

    /// Prose ending before an indented block does not join that block: the
    /// "everything so far is code" state is tracked line by line (it used to
    /// be re-scanned on every line — quadratic on a 24 000-line file).
    #[test]
    fn subsplit_does_not_glue_prose_to_the_indented_code_after_it() {
        let long_para = "word ".repeat(100);
        let code: String = (0..25).map(|i| format!("    fn f{i}() {{}}\n")).collect();
        for sep in ["\n\n", "\n"] {
            let doc = format!("# Big\n\n{long_para}{sep}{code}");
            let secs = subsplit(segment(&doc));
            assert!(
                !secs
                    .iter()
                    .any(|s| s.body_lines.iter().any(|l| l.starts_with("word"))
                        && s.body_lines.iter().any(|l| l.starts_with("    fn f"))),
                "prose glued to code ({sep:?}): {:?}",
                secs.iter().map(|s| &s.body_lines).collect::<Vec<_>>()
            );
        }
    }

    /// A block is a SLICE of its source: units cut without a blank between
    /// them are rejoined without one. A source file cut every function between
    /// `fn f() {` and its 4-space body, and the blank put back there was a
    /// line the file does not have (`tokenade read x.rs --prompt`).
    #[test]
    fn subsplit_restores_only_the_blank_lines_the_source_had() {
        // Every length from 20 to 90 functions, so some cut ends on a tiny
        // trailing block that is merged into the one before it.
        for n in 20..90 {
            let src: String = (0..n)
                .map(|i| format!("#[test]\nfn case_{i}() {{\n    let repo = a_hand_built_repo(\"repo-{i}\");\n    assert!(repo.ready());\n}}\n\n"))
                .chain(std::iter::once("fn tail() {\n    x();\n".to_string()))
                .collect();
            let secs = subsplit(segment(&src));
            assert!(secs.len() > 1, "the fixture must split");
            for s in &secs {
                let block = s.body_lines.join("\n");
                assert!(
                    src.contains(&block),
                    "n={n}: block is not a slice of the source:\n{block}"
                );
            }
        }
        // And a real paragraph break is still restored.
        let prose = format!(
            "# Big\n\n{}\n\n{}\n",
            "alpha ".repeat(120),
            "beta ".repeat(120)
        );
        assert!(subsplit(segment(&prose))
            .iter()
            .all(|s| prose.contains(&s.body_lines.join("\n"))));
    }

    /// CommonMark indented code is the third fence form. A blank inside
    /// the run used to flush, then the line chopper split the sample.
    #[test]
    fn subsplit_keeps_indented_code_atomic() {
        let long_para = "word ".repeat(80);
        let mut code = String::new();
        for i in 0..20 {
            if i == 10 {
                code.push('\n');
            }
            code.push_str(&format!("    fn f{i}() {{}}\n"));
        }
        let doc = format!("# Big\n\n{long_para}\n\n{code}\n{long_para}\n");
        let secs = subsplit(segment(&doc));
        let code_blocks: Vec<_> = secs
            .iter()
            .filter(|s| s.body_lines.iter().any(|l| l.starts_with("    fn f")))
            .collect();
        assert_eq!(
            code_blocks.len(),
            1,
            "indented run split across blocks: {:?}",
            code_blocks
                .iter()
                .map(|s| &s.body_lines)
                .collect::<Vec<_>>()
        );
        let body = &code_blocks[0].body_lines;
        assert!(body.iter().any(|l| l.contains("fn f0()")), "{body:?}");
        assert!(body.iter().any(|l| l.contains("fn f19()")), "{body:?}");
    }

    /// `n` data rows under a 3-column header, no blank lines (the shape a
    /// real markdown table has, and the reason it reaches the line chopper
    /// as one unit).
    fn table_doc(n: usize, with_delim: bool) -> String {
        let mut d = String::from("# Metrics\n\n| Name | Count | Ratio |\n");
        if with_delim {
            d.push_str("|---|---|---|\n");
        }
        for i in 0..n {
            d.push_str(&format!("| alpha{i:03} | {} | 0.{:04} |\n", 1000 + i, i));
        }
        d
    }

    #[test]
    fn subsplit_replicates_table_header_on_continuations() {
        // 60 rows x ~29 chars = ~1740 > 2 * SPLIT_MAX_CHARS, so the unit is
        // chopped at line boundaries — the path that used to drop the header.
        let secs = subsplit(segment(&table_doc(60, true)));
        assert!(secs.len() > 1, "fat table must split, got {}", secs.len());
        for (i, s) in secs.iter().enumerate() {
            assert!(
                s.body_lines
                    .iter()
                    .any(|l| l.trim() == "| Name | Count | Ratio |"),
                "block {i} lost its column names: {:?}",
                s.body_lines
            );
            assert!(
                s.body_lines.iter().any(|l| l.trim() == "|---|---|---|"),
                "block {i} lost the delimiter row: {:?}",
                s.body_lines
            );
        }
        // Chunk 0 keeps its NATURAL header — exactly one copy, not a dupe.
        assert_eq!(
            secs[0]
                .body_lines
                .iter()
                .filter(|l| l.trim() == "|---|---|---|")
                .count(),
            1,
            "first block must not gain a duplicate header"
        );
        // Charging the header to the char budget keeps blocks in envelope.
        for s in &secs {
            let chars: usize = s.body_lines.iter().map(|l| l.chars().count() + 1).sum();
            assert!(chars <= SPLIT_MAX_CHARS + 120, "block too fat: {chars}");
        }
    }

    #[test]
    fn subsplit_leaves_short_table_untouched() {
        // ~10 rows stays well under SPLIT_MAX_CHARS: one section, verbatim.
        let doc = table_doc(10, true);
        let before = segment(&doc);
        let after = subsplit(segment(&doc));
        assert_eq!(after.len(), 1, "short table must not split");
        assert_eq!(after[0].body_lines, before[0].body_lines, "body altered");
    }

    #[test]
    fn subsplit_headerless_table_is_not_given_a_fabricated_header() {
        // Same 60 rows but NO `|---|` delimiter: rows only. We must split
        // without inventing a header, i.e. the first data row stays unique.
        let secs = subsplit(segment(&table_doc(60, false)));
        assert!(secs.len() > 1, "fat table must still split");
        let first_row: usize = secs
            .iter()
            .flat_map(|s| s.body_lines.iter())
            .filter(|l| l.trim() == "| Name | Count | Ratio |")
            .count();
        assert_eq!(first_row, 1, "header fabricated from a header-less table");
        assert!(secs
            .iter()
            .all(|s| !s.body_lines.iter().any(|l| l.contains("---"))));
    }

    #[test]
    fn subsplit_non_table_blocks_gain_no_pipes() {
        let long_para = "word ".repeat(80);
        let doc = format!("# Prose\n\n{long_para}\n\n{long_para}\n\n{long_para}\n");
        let secs = subsplit(segment(&doc));
        assert!(secs.len() > 1);
        for s in &secs {
            assert!(
                !s.body_lines.iter().any(|l| l.trim_start().starts_with('|')),
                "prose block gained a table header: {:?}",
                s.body_lines
            );
        }
    }

    /// The embedder is multilingual; the boundaries were not. A Chinese
    /// paragraph is one line with no space anywhere, so it used to reach the
    /// chunker as a single unsplittable unit.
    #[test]
    fn sentences_split_on_cjk_full_stop() {
        let s = "这是第一个句子，用来测试中文的断句功能。这是第二个句子，长度同样足够超过最小值。";
        let out = split_sentences(s, SENTENCE_MIN_CHARS);
        assert_eq!(out.len(), 2, "{out:?}");
        assert!(out[0].ends_with('。') && out[1].ends_with('。'), "{out:?}");
        assert_eq!(out.concat(), s, "text must be preserved verbatim");
    }

    #[test]
    fn sentences_split_on_devanagari_danda() {
        let s = "यह पहला वाक्य है जो पर्याप्त लंबा है। यह दूसरा वाक्य है जो पर्याप्त लंबा है।";
        let out = split_sentences(s, SENTENCE_MIN_CHARS);
        assert_eq!(out.len(), 2, "{out:?}");
        assert_eq!(out.concat(), s);
    }

    /// The minimum length is what keeps `.`-before-space from shredding
    /// abbreviations into two- and four-character shards.
    #[test]
    fn abbreviations_do_not_produce_fragments() {
        for s in [
            "e.g. the parser rejects it outright and returns an error to the caller.",
            "M. Dupont signed the agreement in Paris before the deadline expired.",
            "Fig. 3 shows the latency distribution across every configured backend.",
            "The U.S. release shipped before the European one by several weeks.",
        ] {
            let out = split_sentences(s, SENTENCE_MIN_CHARS);
            assert_eq!(out.len(), 1, "abbreviation split: {out:?}");
            assert_eq!(out.concat(), s);
        }
    }

    /// Real ASCII sentences still split, and a decimal point never does.
    #[test]
    fn ascii_sentences_unchanged() {
        let s = "The first sentence is long enough. The second one is as well.";
        assert_eq!(
            split_sentences(s, SENTENCE_MIN_CHARS),
            vec![
                "The first sentence is long enough. ",
                "The second one is as well."
            ]
        );
        let v = "The value is 3.14159 and it never terminates a sentence here.";
        assert_eq!(split_sentences(v, SENTENCE_MIN_CHARS), vec![v]);
    }

    /// End-to-end: a fat CJK paragraph (one line, no spaces) now yields
    /// several blocks instead of one indivisible section.
    #[test]
    fn subsplit_splits_a_fat_cjk_paragraph() {
        let para = "这是一个用于测试的中文句子，它足够长以便产生多个块。".repeat(60);
        let secs = subsplit(segment(&format!("# 标题\n\n{para}\n")));
        assert!(
            secs.len() > 1,
            "CJK paragraph stayed unsplit: {}",
            secs.len()
        );
        let rebuilt: String = secs
            .iter()
            .flat_map(|s| s.body_lines.iter())
            .cloned()
            .collect::<Vec<_>>()
            .join("");
        assert_eq!(rebuilt, para, "CJK text must survive the split verbatim");
    }

    /// An unpunctuated fat line is still cut, at spaces, verbatim.
    #[test]
    fn subsplit_splits_a_fat_line_with_no_sentence_end() {
        let para = "alpha beta gamma delta server config ".repeat(400);
        let para = para.trim_end().to_string();
        let secs = subsplit(segment(&format!("# T\n\n{para}\n")));
        assert!(
            secs.len() > 2,
            "unpunctuated line stayed unsplit: {}",
            secs.len()
        );
        let rebuilt: String = secs
            .iter()
            .flat_map(|s| s.body_lines.iter())
            .cloned()
            .collect::<Vec<_>>()
            .join("");
        assert_eq!(rebuilt, para);
        assert!(secs.iter().all(|s| s
            .body_lines
            .iter()
            .all(|l| l.ends_with(' ') || para.ends_with(l.as_str()))));
    }

    #[test]
    fn subsplit_leaves_small_sections_alone() {
        let secs = subsplit(segment(DOC));
        assert_eq!(secs.len(), 7, "small sections must pass through unchanged");
    }

    #[test]
    fn empty_query_returns_none() {
        assert!(prompt_scoped_extract_rag(DOC, "the this that", 800).is_none());
    }

    #[test]
    fn headingless_falls_back_to_flat() {
        let flat = "plain paragraph about swap_remove and removing elements fast\nsecond line";
        let q = "how to swap_remove an element";
        let got = prompt_scoped_extract_rag(flat, q, 800);
        let base = prompt_scoped_extract(flat, q, 800);
        assert_eq!(got, base);
    }

    #[test]
    fn deterministic_same_input_same_output() {
        let q = "how do I remove an element at an index";
        let a = prompt_scoped_extract_rag(DOC, q, 800);
        let b = prompt_scoped_extract_rag(DOC, q, 800);
        assert_eq!(a, b);
    }
}

/// **The dispatcher, and what goes through it.**
///
/// These five tests used to live in `disclose_tests.rs`, with the functions
/// they exercise. They followed them into `rag`: the choice of engine belongs
/// to whoever knows both engines.
#[cfg(test)]
mod engine_choice {
    use crate::rag::*;

    const MULTI_DOC: &str = "\
# Deployment\n\
The deploy failed because the database migration timed out after 30 seconds.\n\
Increase the statement_timeout setting to resolve the migration hang.\n\
\n\
# Billing\n\
Invoices are generated on the first day of each month in euros.\n\
A late invoice payment incurs a two percent monthly surcharge.\n";
    #[allow(unused_imports)]
    use crate::disclose::*;

    /// Over a record stream the answer must arrive NEAR THE TOP and small.
    /// Block retrieval put it on line 60 of 67 and charged 5 561 characters for
    /// the privilege; the line path answers in a few hundred.
    #[test]
    fn a_question_over_a_record_stream_answers_near_the_top() {
        let mut blob = String::new();
        for i in 0..20_000 {
            blob.push_str(&format!(
                "crates/core/src/mod{i}.rs:pub fn thing{i}(a: u8) {{\n"
            ));
        }
        blob.push_str("crates/core/src/redact.rs:fn zqx_needle_symbol(v: &str) -> bool {\n");
        let out = answer_over_blob(&blob, "zqx_needle_symbol").expect("a passage matched");
        let lines: Vec<&str> = out.lines().collect();
        let at = lines
            .iter()
            .position(|l| l.contains("zqx_needle_symbol"))
            .expect("the answer is in the result");
        assert!(at < 10, "answer on line {at} of {}: {out}", lines.len());
        assert!(out.len() < 3_000, "and it stays small: {} chars", out.len());
    }

    #[test]
    fn multi_prompt_labels_each_question() {
        let out = prompt_scoped_extract_dispatch(
            MULTI_DOC,
            "why did the deploy fail, when are invoices generated",
            400,
        )
        .expect("both questions should retrieve something");
        assert!(
            out.contains("### Q1 · why did the deploy fail"),
            "no Q1 header: {out}"
        );
        assert!(
            out.contains("### Q2 · when are invoices generated"),
            "no Q2 header: {out}"
        );
        // Each block carries content from its own topic, not the other's.
        assert!(out.contains("migration"), "Q1 answer missing: {out}");
        assert!(out.contains("Invoices"), "Q2 answer missing: {out}");
        // Q1's header precedes Q2's — order is preserved.
        assert!(out.find("### Q1").unwrap() < out.find("### Q2").unwrap());
    }

    #[test]
    fn single_prompt_is_unchanged_no_header() {
        // No comma → exact single-question path: no "### Q" scaffolding.
        let out = prompt_scoped_extract_dispatch(MULTI_DOC, "why did the deploy fail", 400)
            .expect("should retrieve the deploy passage");
        assert!(
            !out.contains("### Q"),
            "single question must not be labelled: {out}"
        );
        assert!(out.contains("migration"), "answer missing: {out}");
    }

    /// `answer_over_blob` is the ONE core behind every "ask a ref/stash" entry
    /// point. Small blob → served whole (a full serve below the threshold loses
    /// nothing and beats a lossy extract). Large blob → dense RAG, and a
    /// multi-question ask comes back labelled per question.
    #[test]
    fn answer_over_blob_serves_small_whole_and_rags_large() {
        // Small: under the threshold, returned verbatim.
        let small = "The deploy failed because the migration timed out.";
        assert_eq!(
            answer_over_blob(small, "why did the deploy fail").as_deref(),
            Some(small),
            "a small blob must be served whole, not extracted"
        );

        // Large: two separable topics, padded well past the threshold.
        let mut big = String::from("# Deployment\n");
        for i in 0..500 {
            big.push_str(&format!(
                "deploy {i} padding alpha beta gamma delta epsilon zeta\n"
            ));
        }
        big.push_str("The deploy failed because the database migration timed out.\n# Backups\n");
        for i in 0..500 {
            big.push_str(&format!(
                "backup {i} padding eta theta iota kappa lambda mu\n"
            ));
        }
        big.push_str("Nightly backups are written to the eu-west object store.\n");
        assert!(
            crate::tokenizer::estimate(&big) > threshold_tokens(),
            "fixture must exceed the threshold to exercise the RAG path"
        );

        let out = answer_over_blob(&big, "why did the deploy fail, where do backups go")
            .expect("both questions should retrieve");
        assert!(
            out.contains("### Q1 · why did the deploy fail"),
            "no Q1 header: {out}"
        );
        assert!(
            out.contains("### Q2 · where do backups go"),
            "no Q2 header: {out}"
        );
        assert!(out.contains("migration timed out"), "Q1 answer missing");
        assert!(out.contains("eu-west object store"), "Q2 answer missing");
        // The point of the whole exercise: markedly cheaper than the raw blob.
        assert!(
            crate::tokenizer::estimate(&out) < crate::tokenizer::estimate(&big) / 2,
            "a scoped answer must cost far less than dumping the blob"
        );
    }

    /// A question the cap threw away must not read as an answered one. The
    /// reply is numbered `Q1 … Q8` and looks complete; without a word about
    /// the tail, a caller that asked twenty things concludes twelve of them
    /// have no answer in the document.
    #[test]
    fn questions_the_cap_dropped_are_named_not_hidden() {
        let asked = MAX_SCOPED_PROMPTS + 12;
        let many = (0..asked)
            .map(|i| format!("what is metric {i} today"))
            .collect::<Vec<_>>()
            .join(", ");
        let (kept, dropped) = split_scoped_prompts_counted(&many);
        assert_eq!(kept.len(), MAX_SCOPED_PROMPTS);
        assert_eq!(dropped, asked - MAX_SCOPED_PROMPTS);

        // A prompt that fits drops nothing — no spurious notice.
        let (_, none) =
            split_scoped_prompts_counted("why did the deploy fail, where do backups go");
        assert_eq!(none, 0);

        // The notice reaches the rendered document, which is the only channel
        // an MCP caller ever sees.
        let doc = (0..asked)
            .map(|i| {
                format!(
                    "## Metric {i}\nThe value of metric {i} today is {}, measured hourly by the \
                     collector and stored in the warehouse for reporting.\n",
                    i * 7
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let out = prompt_scoped_extract_dispatch(&doc, &many, 400)
            .expect("at least one question matches");
        assert!(
            out.contains(&(asked - MAX_SCOPED_PROMPTS).to_string()) && out.contains("not searched"),
            "the reply must say how many questions went unasked, got: {out}"
        );
    }
}
