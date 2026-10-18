// The tests of `crate::markdown`, moved out of the production file.
//
// Mounted by `include!`, not by `mod`: the tokens are spliced INTO
// `markdown`, so every `super::` keeps the meaning it had. Split by
// `srcsweep::production_lines`.

#[cfg(test)]
mod boundary_tests {
    /// Non-ASCII text, written as escapes so this source stays pure ASCII and
    /// cannot be corrupted by a tool that mangles encodings on the way in.
    fn hangul(words: usize) -> String {
        (0..words)
            .map(|i| format!("\u{ac00}\u{b098}\u{b2e4}{i} "))
            .collect()
    }

    /// A README whose text is not ASCII must not take the compactor down.
    /// `is_badge_line` scanned by byte and sliced by `&str`, so the first
    /// multibyte character before a badge cluster panicked the whole
    /// compaction - and a hook that panics fails open, so the user simply
    /// stopped being compacted with nothing said. Found by surveying real
    /// documents on disk, not by inventing an input.
    #[test]
    fn a_non_ascii_document_does_not_panic() {
        for prefix in [
            hangul(4),
            "caf\u{e9} \u{2615} ".to_string(),
            "\u{65e5}\u{672c}\u{8a9e} ".to_string(),
            "\u{1f389}\u{1f389}\u{1f389} ".to_string(),
        ] {
            let doc = format!(
                "# Title\n\n{prefix}[![build](https://img.shields.io/x)](https://e.co)\n\n- one\n- two\n"
            );
            let _ = super::compact(&doc);
        }
    }

    /// Non-ASCII text must survive the inline passes byte for byte. Both
    /// `strip_inline_html` and `transform_inline_images` emitted
    /// `bytes[i] as char`, so one `<sub>` or one `![]()` on a line turned every
    /// multibyte character on it into Latin-1 mojibake — silently, in the text
    /// handed to the agent as the document.
    #[test]
    fn an_inline_tag_does_not_mangle_the_text_around_it() {
        let word = "\u{ac00}\u{b098}\u{b2e4}";
        let accent = "caf\u{e9} cr\u{e8}me br\u{fb}l\u{e9}e";
        for line in [
            format!("{word} <sub>note</sub> {word} {accent}"),
            format!("{word} ![diagram](https://e.co/a.png) {word} {accent}"),
            format!("{accent} <a href=\"https://e.co\">{word}</a> {word}"),
        ] {
            let doc = format!("# T\n\n{line}\n\n- one\n- two\n");
            let out = super::compact(&doc);
            assert!(out.contains(word), "lost {word:?} in:\n{out}");
            assert!(out.contains(accent), "lost {accent:?} in:\n{out}");
        }
    }

    /// The badge ratio compares like with like. Counting badge BYTES against
    /// total CHARS made every non-Latin line look two to three times more
    /// decorative than it is, which is how real prose got collapsed to
    /// `[badges: ...]`.
    #[test]
    fn non_latin_prose_around_one_badge_is_not_a_badge_line() {
        let marker = "\u{ac00}\u{b098}\u{b2e4}7";
        let line = format!(
            "{}[![b](https://img.shields.io/x)](https://e.co)",
            hangul(12)
        );
        let doc = format!("# T\n\n{line}\n\n- one\n- two\n");
        let out = super::compact(&doc);
        assert!(
            out.contains(marker),
            "non-Latin prose collapsed as a badge cluster:\n{out}"
        );
    }
}
#[cfg(test)]
mod tests {

    /// The LABELLED promoter had no defence against a
    /// sentence that happens to start with one of its label words. `rfc5322.txt`
    /// gave three different false positives before the tests below were right.
    #[test]
    fn label_promoter_rejects_prose_that_starts_with_a_label_word() {
        let body = "filler line\n".repeat(220);
        let promoted = |head: &str| {
            let doc = format!("{head}{body}");
            super::outline(&doc)
        };
        // Real headings, from four real documents.
        let real = "CHAPTER I\n\nGeneral provisions\n\nArticle 1\n\nx\n\nArticle 2\n\nx\n\nArticle 3\n\nx\n\nArticle 4\n\nx\n\nArticle 5\n\nx\n\n";
        let out = promoted(real);
        assert!(
            out.contains("## Article 1") && out.contains("## CHAPTER I"),
            "{out}"
        );

        // A cross-reference wrapped to the start of a line — lowercase label.
        let xref = (1..=6)
            .map(|i| format!("section 3.2.{i}. Comments may nest.\n\nprose\n\n"))
            .collect::<String>();
        let o = promoted(&xref);
        // Assert on the HEADING MARKER, not the text: when nothing is promoted
        // the outline falls back to a head preview, which naturally contains
        // the line verbatim. The first version of this test asserted on the
        // text and failed against correct behaviour.
        assert!(
            !o.contains("## section"),
            "lowercase label is a reference: {o}"
        );

        // A sentence continuing in lowercase after the number.
        let sentence = (1..=6)
            .map(|i| format!("Section {i} lays out the general description here\n\nprose\n\n"))
            .collect::<String>();
        let o = promoted(&sentence);
        assert!(
            !o.contains("## Section 1"),
            "lowercase after number is prose: {o}"
        );

        // Table-of-contents entries carry dot leaders and a page number.
        let toc = (1..=6)
            .map(|i| format!("Appendix {i}. Acknowledgements . . . . . . . . {i}\n\n"))
            .collect::<String>();
        assert!(
            !promoted(&toc).contains("## Appendix 1"),
            "dot leaders mean contents page"
        );
    }

    /// Plain-text RFCs number their sections with no label word, so
    /// `rfc8259.txt` (JSON) outlined as one block. The safety test is that the
    /// numbering must NEST: a specification has `1`, `1.1`, `8.3`; a numbered
    /// list is flat.
    #[test]
    fn numbered_sections_promote_only_when_the_numbering_nests() {
        let mut spec = String::new();
        for i in 1..=6 {
            spec.push_str(&format!("{i}. Section Title {i}\n\nprose\n\n"));
            spec.push_str(&format!("{i}.1. Subsection\n\nmore prose\n\n"));
            spec.push_str(&"body line\n".repeat(30));
        }
        let out = super::outline(&spec);
        assert!(out.contains("## 1. Section Title 1"), "top level: {out}");
        assert!(
            out.contains("### 1.1. Subsection"),
            "nested one level deeper: {out}"
        );

        // A FLAT numbered list is not a document structure.
        let mut list = String::new();
        for i in 1..=9 {
            list.push_str(&format!("{i}. Install the package and run it\n\n"));
            list.push_str(&"detail line\n".repeat(25));
        }
        assert!(
            !super::outline(&list).contains("## 1. Install"),
            "flat numbering must not promote: {}",
            super::outline(&list)
        );

        // A table of contents (indented, dot leaders) must never be promoted —
        // otherwise every spec outlines its own contents page as well as its
        // body.
        let mut toc = String::from("Table of Contents\n\n");
        for i in 1..=6 {
            toc.push_str(&format!("   {i}. Section Title . . . . . . . . {i}\n"));
            toc.push_str(&format!("   {i}.1. Subsection . . . . . . . . {i}\n"));
        }
        toc.push_str(&"filler\n".repeat(220));
        let o = super::outline(&toc);
        assert!(
            !o.contains("## 1. Section Title"),
            "toc must not promote: {o}"
        );
    }

    /// The converter emits `#### 3.2.1 General Syntax` for an RFC
    /// and then wraps the surrounding `<pre>` in a fence, so the outline walk
    /// skips headings the pipeline itself produced. rfc2616 outlined as ONE
    /// section of 9 343 lines. Opening the fences is only safe because source
    /// code cannot fake the test: a comment is always one `#`.
    #[test]
    fn fences_open_for_documents_and_stay_shut_for_code() {
        let mut doc = String::from("# Spec\n\n```\n");
        for i in 1..=6 {
            doc.push_str(&format!(
                "## {i} Section\n\n### {i}.1 Sub\n\nprose about it\n\n"
            ));
            doc.push_str(&"body line\n".repeat(45));
        }
        doc.push_str("```\n");
        let out = super::outline(&doc);
        assert!(
            out.contains("1 Section") && out.contains("1.1 Sub"),
            "revealed: {out}"
        );

        // Source code inside a fence must stay shut: `# comment` lines are all
        // depth 1, and nothing in Perl/shell/Make writes `###`.
        let mut code = String::from("# Script\n\n```perl\n");
        for i in 1..=8 {
            code.push_str(&format!("# (c) 200{i}, Some Author\n"));
            code.push_str(&"my $x = 1;\n".repeat(45));
        }
        code.push_str("```\n");
        let out2 = super::outline(&code);
        assert!(
            !out2.contains("Some Author"),
            "code comments must stay fenced: {out2}"
        );
    }

    /// Legal texts, standards and public-domain books carry their
    /// structure as `Article 17` / `SEC. 101.` / `Chapter I` on a line, not as
    /// `<h2>`. eur-lex's GDPR is 3 692 lines with ZERO headings, so the outline
    /// fell through to "first 20 lines" and delivered EUR-Lex's search tips.
    #[test]
    fn unheaded_legal_structure_is_promoted_only_as_a_fallback() {
        let mut doc = String::new();
        for i in 1..=8 {
            doc.push_str(&format!(
                "Article {i}\n\nSome provision text for article {i}.\n\n"
            ));
            doc.push_str(&"filler line\n".repeat(30));
        }
        let out = super::outline(&doc);
        assert!(out.contains("## Article 1"), "promoted: {out}");
        assert!(out.contains("## Article 8"), "promoted: {out}");

        // A document that ALREADY has headings is never touched, even when it
        // also mentions articles — this must not compete with real structure.
        let mut structured =
            String::from("# Real Title\n\n## Real Section\n\n## Another\n\n## Third\n\n");
        for i in 1..=8 {
            structured.push_str(&format!("Article {i}\n\ntext\n\n"));
            structured.push_str(&"filler\n".repeat(30));
        }
        let out2 = super::outline(&structured);
        assert!(
            !out2.contains("## Article 1"),
            "must not promote over real headings: {out2}"
        );

        // Short documents are left alone: the fallback is for things too long
        // to read, not for every heading-less snippet.
        let short = "Article 1\n\nx\n\nArticle 2\n\ny\n\nArticle 3\n\nz\n\nArticle 4\n\nw\n\nArticle 5\n\nv\n";
        assert!(
            !super::outline(short).contains("## Article 1"),
            "short doc untouched"
        );

        // One fence spanning the whole document is preformatted text, not code
        // — govinfo.gov serves every federal bill that way.
        let mut fenced = String::from("```\n");
        for i in 1..=8 {
            fenced.push_str(&format!("SEC. {i}. SHORT TITLE.\n\ntext\n\n"));
            fenced.push_str(&"filler\n".repeat(30));
        }
        fenced.push_str("```\n");
        assert!(
            super::outline(&fenced).contains("SEC. 1."),
            "document-wide fence opened"
        );
    }

    /// **A preview that quotes OUR marker, or a wiki's edit pencil,
    /// says nothing about the section** — and it is what an agent reads to choose
    /// what to expand.
    #[test]
    fn a_preview_prefers_prose_over_our_own_marker_and_over_a_service_link() {
        // Enough body for the outline to be WORTH it: `outline` returns an
        // empty string when the summary would not be shorter than the document,
        // and a ten-line fixture hits that.
        let filler = "Une phrase de remplissage qui ne dit rien de particulier.\n".repeat(8);
        let md = format!(
            "# Encyclopédie\n\n\
             ## Histoire\n\
             [[edit](</w/index.php?title=X&action=edit&section=2>)]\n\
             … [21 lines elided by tokenade] …\n\
             La discipline naît en 1956 lors de la conférence de Dartmouth.\n\
             {filler}\n\
             ## Méthodes\n\
             … [9 «1»\n\
             Les approches symboliques ont dominé jusqu'aux années 1980.\n\
             {filler}\n\
             ## Images\n\
             [编辑⟨104⟩]\n\
             [image: Un microscope électronique à balayage]\n\
             \n\
             ## Sources\n\
             [编辑⟨104⟩]\n"
        );
        let md = md.as_str();
        let out = super::outline(md);
        let histoire = out
            .lines()
            .find(|l| l.contains("Histoire"))
            .unwrap_or_else(|| panic!("no Histoire section in:\n{out}"));
        assert!(
            histoire.contains("La discipline naît"),
            "the preview does not quote the section's prose: {histoire}"
        );
        assert!(
            !histoire.contains("elided by tokenade") && !histoire.contains("action=edit"),
            "the preview still quotes decoration: {histoire}"
        );
        // The FOLDED form of the marker is the one we deliver: the
        // "said-once" fold rewrites the common sentence, and a rule that looks for
        // `elided by tokenade` no longer sees it.
        let methodes = out
            .lines()
            .find(|l| l.contains("Méthodes"))
            .unwrap_or_else(|| panic!("no Méthodes section in:\n{out}"));
        assert!(
            methodes.contains("Les approches symboliques"),
            "the preview quotes the folded marker instead of the prose: {methodes}"
        );
        // Between two lines that are only constructs, the FIGURE wins:
        // a caption says what the section is about, a pencil says nothing. Without
        // this priority, the first of the two wins — and it is the pencil,
        // which wikis put right under the heading.
        let images = out
            .lines()
            .find(|l| l.contains("Images"))
            .unwrap_or_else(|| panic!("no Images section in:\n{out}"));
        assert!(
            images.contains("microscope"),
            "the pencil beat the caption: {images}"
        );

        // A PREFERENCE, never a rejection: a section that has ONLY that keeps it,
        // otherwise the outline becomes too thin to be accepted and the whole
        // article is served instead (the britannica lesson, above).
        let sources = out
            .lines()
            .find(|l| l.contains("Sources"))
            .unwrap_or_else(|| panic!("no Sources section in:\n{out}"));
        assert!(
            sources.contains("编辑"),
            "the section that only has this link lost its preview: {sources}"
        );
    }

    /// **A section reduced to our marker is COUNTED, not quoted** —
    /// and the announced number is the marker's, not that of the kept lines.
    #[test]
    fn a_section_left_with_only_our_marker_says_how_much_is_gone() {
        let filler = "Une phrase de remplissage qui ne dit rien de particulier.\n".repeat(8);
        let md = format!(
            "# Article\n\n\
             ## Corps\n\
             La discipline naît en 1956.\n\
             {filler}\n\
             ## Déplié\n\
             … [9 lines elided by tokenade] …\n\
             \n\
             ## Replié\n\
             … [21 «1»\n\
             \n\
             ## Mixte\n\
             … [4 lines elided by tokenade] …\n\
             Ce paragraphe a survécu à la coupe et doit être cité.\n\
             \n\
             ## Image seule\n\
             [image]\n"
        );
        let out = super::outline(md.as_str());
        for (section, gone) in [("Déplié", 9), ("Replié", 21)] {
            let line = out
                .lines()
                .find(|l| l.contains(section))
                .unwrap_or_else(|| panic!("no {section} section in:\n{out}"));
            assert!(
                line.contains(&format!("({gone} lines elided)")),
                "the section does not announce what it lost: {line}"
            );
            assert!(
                !line.contains(" · \""),
                "it still quotes our own marker: {line}"
            );
        }
        // **A section that KEPT something is quoted normally.** The
        // elision count is only right if the section has nothing else left; otherwise
        // it would replace a true quote with a number, which is the loss
        // this rule is meant to avoid.
        let mixte = out
            .lines()
            .find(|l| l.contains("Mixte"))
            .unwrap_or_else(|| panic!("no Mixte section in:\n{out}"));
        assert!(
            mixte.contains("Ce paragraphe a survécu"),
            "a section that kept prose was counted instead of quoted: {mixte}"
        );

        // And a figure WITHOUT a caption is not a caption: it says nothing
        // more than a service link. It is still served when the section has
        // only that — a preference, never a rejection.
        let img = out
            .lines()
            .find(|l| l.contains("Image seule"))
            .unwrap_or_else(|| panic!("no Image seule section in:\n{out}"));
        assert!(img.contains("[image]"), "{img}");
    }

    /// A heading label repeated many times is a UI widget, not
    /// structure. stackoverflow.com/q/11227809 outlined 26 `N Comments`
    /// widgets and no answers; kubernetes.io/…/deployment/ spent 11 lines on
    /// `Note:` callouts and had its real sections clamped out.
    #[test]
    fn repeated_widget_headings_collapse_but_are_not_deleted() {
        let mut md = String::from("# Doc\n\nintro line\n");
        for i in 0..6 {
            md.push_str(&format!("\n## {i} Comments\n\nsome comment text {i}\n"));
        }
        md.push_str("\n## Real Section\n\nthe actual content\n");
        let out = super::outline(&md);
        assert_eq!(
            out.matches("Comments").count(),
            1,
            "collapsed to one entry: {out}"
        );
        assert!(
            out.contains("×6 on this page, collapsed"),
            "count disclosed: {out}"
        );
        assert!(out.contains("Real Section"), "real structure kept: {out}");

        // Four occurrences are under the threshold and stay as they are —
        // a document with a few repeated headings is still a document.
        let mild = "# D\n\nx\n\n## Notes\n\na\n\n## Notes\n\nb\n\n## Notes\n\nc\n";
        assert_eq!(
            super::outline(mild).matches("Notes").count(),
            3,
            "under threshold"
        );
    }

    /// A section that OPENS with a code block previewed as "```".
    /// On man7.org/…/openat.2.html that was all thirteen sections; on
    /// pkg.go.dev/net/http, all 181. The code's first line is the payload.
    #[test]
    fn outline_preview_skips_delimiters_for_the_line_that_carries_meaning() {
        let md = "## func Handle\n\n```go\nfunc Handle(pattern string, handler Handler)\n```\n\n## Table\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";
        let out = super::outline(md);
        assert!(
            out.contains("func Handle(pattern string"),
            "signature: {out}"
        );
        assert!(
            !out.contains("\"```\""),
            "fence must never be the preview: {out}"
        );
        assert!(
            out.contains("| a | b |"),
            "table header, not its rule: {out}"
        );
    }
    #[test]
    fn code_comments_are_not_headings() {
        // docs.python.org/3/library/asyncio-task outlined itself out of its
        // own examples: `# Wait until both tasks are completed (should take`
        // and `# Nothing happens if we just call "nested()".` became H1
        // sections. Shell, Ruby, YAML and TOML samples carry the same hazard.
        let md = "# Real Heading\n\ntext\n\n```python\n# Wait until both tasks are completed\nawait task1\n# Nothing happens if we just call nested()\n```\n\n## Second Real Heading\n\nmore text\n";
        let out = outline(md);
        assert!(out.contains("Real Heading"));
        assert!(out.contains("Second Real Heading"));
        assert!(
            !out.contains("Wait until both tasks"),
            "code comment became a section: {out}"
        );
        assert!(
            !out.contains("Nothing happens"),
            "code comment became a section: {out}"
        );
    }

    /// CommonMark indented code is the third fence form. `parse_heading`
    /// used `trim_start()`, so `    # comment` became H1 — the same hole
    /// the fence walk already closed for ``` / `~~~`.
    #[test]
    fn indented_code_comments_are_not_headings() {
        let md = "# Real Heading\n\ntext\n\n    # Wait until both tasks are completed\n    await task1\n\n## Second Real Heading\n\nmore text\n";
        let out = outline(md);
        assert!(out.contains("Real Heading"));
        assert!(out.contains("Second Real Heading"));
        assert!(
            !out.contains("Wait until both tasks"),
            "indented code comment became a section: {out}"
        );
        // 0–3 spaces remain a real ATX heading (CommonMark).
        let spaced = "# Doc\n\n  ## Still A Heading\n\nbody\n";
        assert!(
            outline(spaced).contains("Still A Heading"),
            "2-space ATX dropped: {}",
            outline(spaced)
        );
    }

    use super::*;

    #[test]
    fn detects_markdown() {
        // #551 — requires ≥2 markers. `# Heading` + `Text` is one
        // marker; supply a second so the assertion remains a positive
        // detection test (rather than testing the threshold itself —
        // covered in `rejects_single_marker_input` below).
        assert!(looks_like_markdown("# Heading\n\n- bullet"));
        assert!(looks_like_markdown("- bullet 1\n- bullet 2"));
        assert!(!looks_like_markdown("just plain prose with no markers"));
    }

    /// Files dominated by `# ` lines (Python comments, shell
    /// scripts, requirements.txt, .gitignore) used to match markdown
    /// because the detector counted `# ` as a marker. They never had
    /// any stronger structure (`## `, fence, list bullet, link), so
    /// the markdown compactor wasted banner overhead for ~5–8 % gain.
    /// The new rule requires ≥1 strong marker alongside any number
    /// of weak `# ` lines.
    #[test]
    fn rejects_hash_comment_only_files() {
        // Real spacy/tests/package/requirements.txt — 4 `# Section`
        // lines surrounded by pip requirement specifiers. No other
        // markdown structure.
        let req = "# Our libraries\n\
                   spacy-legacy>=3.0.11,<3.1.0\n\
                   cymem>=2.0.2,<2.1.0\n\
                   # Third party dependencies\n\
                   numpy>=1.15.0\n\
                   requests>=2.13.0,<3.0.0\n\
                   # Official Python utilities\n\
                   setuptools\n\
                   packaging>=20.0\n\
                   # Development dependencies\n\
                   pre-commit>=2.13.0\n";
        assert!(
            !looks_like_markdown(req),
            "Python requirements.txt with `# Section` comments must NOT be markdown"
        );
        // .gitignore with multiple `# ` comments + glob patterns.
        let gi = "# Build artifacts\n\
                  target/\n\
                  *.o\n\
                  # Test outputs\n\
                  *.log\n\
                  coverage/\n\
                  # IDE\n\
                  .vscode/\n";
        assert!(
            !looks_like_markdown(gi),
            ".gitignore with comments must NOT be markdown"
        );
        // README that DOES have a header + a subheader → still markdown.
        let readme = "# My Project\n\
                      A short description.\n\
                      ## Install\n\
                      Run the installer.\n";
        assert!(
            looks_like_markdown(readme),
            "Real README with `# Title` + `## Section` must still match"
        );
    }

    /// #551 — single-marker inputs no longer trigger markdown
    /// detection. Eliminates bash/find/ls output false-positives where
    /// one coincidental line matched (a `# comment`, a `- entry`, a
    /// `[text](url)` URL formatted line, etc.).
    #[test]
    fn rejects_single_marker_input() {
        // Bash script with one `# comment` line.
        let bash = "echo hello\n# this is a comment\necho world\n";
        assert!(
            !looks_like_markdown(bash),
            "single `# comment` line must NOT trigger markdown detection"
        );
        // find/ls output with one filename starting with `- `.
        let find_out = "/path/foo.rs\n/path/bar.rs\n- something.txt\n/path/baz.rs\n";
        assert!(
            !looks_like_markdown(find_out),
            "single `- ` line must NOT trigger markdown detection"
        );
        // Error message with one [text](url) reference.
        let err = "Build failed: see [docs](https://example.com/help) for details\n";
        assert!(
            !looks_like_markdown(err),
            "single link line must NOT trigger markdown detection"
        );
    }

    #[test]
    fn folds_large_code_block() {
        let mut md = String::from("# README\n\n```rust\n");
        for i in 0..80 {
            let _ = writeln!(md, "fn f{i}() {{}}");
        }
        md.push_str("```\n\nMore text.\n");
        let out = compact_capped(&md, Caps::DEFAULT);
        assert!(out.contains("# README"));
        // `rust` aliases to `rs` via `canonical_fence_lang` (see
        // sweep-#2 port from nadimtuhin/claude-token-optimizer).
        assert!(out.contains("```rs"));
        assert!(out.contains("[code block elided: 80 lines]"));
        assert!(out.contains("More text."));
        assert!(!out.contains("fn f25()"));
    }

    /// A 30-line example is what a README is read for: under the old cap of
    /// 12 it came back as a marker, and seven times in ten the agent fetched
    /// the page again to see it.
    #[test]
    fn a_typical_example_block_is_delivered_whole() {
        let mut md = String::from("# Setup\n\nConfigure the client:\n\n```python\n");
        md.push_str("from client import Client, Retry\n\n");
        for i in 0..28 {
            let _ = writeln!(md, "cfg_{i} = Client(host=\"h{i}.example\", retry=Retry(n={}))", i % 4);
        }
        md.push_str("```\n\nThen run it.\n");
        let out = compact(&md);
        assert!(!out.contains("code block elided"), "{out}");
        assert!(out.contains("cfg_27 = Client(host=\"h27.example\", retry=Retry(n=3))"));
        assert!(out.contains("```py"));
    }

    /// CommonMark allows `~~~` as a fence. `unpad_tables` already skipped
    /// those blocks; the elider did not, so a 50-line tilde fence was
    /// delivered whole. One `fence_marker` now opens both.
    #[test]
    fn folds_a_tilde_fence_the_same_way_as_backticks() {
        let mut md = String::from("# README\n\n~~~rust\n");
        for i in 0..80 {
            let _ = writeln!(md, "fn f{i}() {{}}");
        }
        md.push_str("~~~\n\nMore text.\n");
        let out = compact_capped(&md, Caps::DEFAULT);
        assert!(out.contains("# README"));
        assert!(out.contains("[code block elided: 80 lines]"));
        assert!(out.contains("More text."));
        assert!(!out.contains("fn f25()"));
        assert!(
            out.contains("~~~rs") || out.contains("```rs"),
            "elided tilde fence must keep a labelled closer: {out}"
        );
    }

    /// CommonMark indented code (4 spaces) is the third fence form. RAG
    /// `classify` already flags it; the elider only saw ``` / `~~~`.
    #[test]
    fn folds_an_indented_code_block_the_same_way_as_fences() {
        // `## Code` is the strong marker: a lone `# README` is not markdown
        // (`looks_like_markdown`), so compact would hand the body back.
        let mut md = String::from("# README\n\n## Code\n\n");
        for i in 0..80 {
            let _ = writeln!(md, "    fn f{i}() {{}}");
        }
        md.push_str("\nMore text.\n");
        let out = compact_capped(&md, Caps::DEFAULT);
        assert!(out.contains("# README"));
        assert!(out.contains("[code block elided: 80 lines]"));
        assert!(out.contains("More text."));
        assert!(!out.contains("fn f25()"));
    }

    #[test]
    fn indented_code_at_the_start_of_the_document_still_folds() {
        let mut md = String::from("# README\n\n## Code\n");
        for i in 0..80 {
            let _ = writeln!(md, "    fn f{i}() {{}}");
        }
        md.push_str("\nMore text.\n");
        let out = compact_capped(&md, Caps::DEFAULT);
        assert!(out.contains("[code block elided: 80 lines]"), "{out}");
        assert!(!out.contains("fn f25()"));
    }

    #[test]
    fn keeps_a_short_indented_code_block() {
        let md = "# Title\n\n## Install\n\n    echo hi\n    echo world\n\nDone.\n";
        let out = compact(md);
        assert!(out.contains("echo hi"), "{out}");
        assert!(out.contains("echo world"), "{out}");
        assert!(!out.contains("elided"));
    }

    #[test]
    fn nested_list_items_are_not_indented_code() {
        let mut md = String::from("# README\n\n- top\n");
        for i in 0..20 {
            let _ = writeln!(md, "    - nested {i}");
        }
        let out = compact(&md);
        assert!(out.contains("- nested 10"), "list body dropped: {out}");
        assert!(!out.contains("elided"));
    }

    #[test]
    fn list_item_continuation_is_not_indented_code() {
        let mut md = String::from("# README\n\n- intro\n");
        for i in 0..20 {
            let _ = writeln!(md, "    continuation {i}");
        }
        let out = compact(&md);
        assert!(
            out.contains("continuation 10"),
            "list continuation folded as code: {out}"
        );
        assert!(!out.contains("elided"));
    }

    #[test]
    fn fence_lang_aliases_apply() {
        let md = "```javascript\nconst x=1;\n```\n";
        let out = compact(md);
        // Long form rewritten to short alias even for non-elided blocks.
        assert!(out.contains("```js"), "expected `js` alias, got {out}");
        assert!(!out.contains("```javascript"));
        assert!(out.contains("const x=1;"));
    }

    #[test]
    fn keeps_small_code_block() {
        let md = "# Title\n\n```sh\necho hi\necho world\n```\n";
        let out = compact(md);
        assert!(out.contains("echo hi"));
        assert!(out.contains("echo world"));
    }

    #[test]
    fn strips_link_ref_block() {
        // #551 — added `## Refs` second heading so the input has ≥2
        // markdown markers (was: one `# Title`). Compactor behaviour
        // unchanged; just satisfies the tighter detector threshold.
        let md = "# Title\n\n## Refs\n\nText with a [link][a] and [another][b].\n\n[a]: https://example.com/a\n[b]: https://example.com/b\n[c]: https://example.com/c\n";
        let out = compact(md);
        assert!(out.contains("[link][a]"));
        assert!(!out.contains("https://example.com/a"));
        assert!(out.contains("link-reference definition(s) elided"));
    }

    #[test]
    fn strips_front_matter() {
        // #551 — added `## Section` second heading.
        let md = "---\ntitle: x\ndate: 2024-01-01\n---\n\n# Body\n\n## Section\n\nText.\n";
        let out = compact(md);
        assert!(!out.contains("title: x"));
        assert!(out.contains("# Body"));
    }

    #[test]
    fn inline_image_simplified() {
        // #551 — added second heading.
        let md = "# X\n\n## Y\n\n![A nice diagram](https://example.com/diag.png)\n\nText after.\n";
        let out = compact(md);
        assert!(out.contains("[image: A nice diagram]"));
        assert!(!out.contains("https://example.com/diag.png"));
    }

    /// The caps are lossy, so they wait until the whole document would not
    /// be delivered anyway. A 25-entry docs index (~330 tokens) lost five
    /// entries to the list cap, under a marker that did not count them.
    #[test]
    fn a_short_document_keeps_every_item_and_every_line() {
        let pages = ["install", "hooks", "proxy", "stash", "expand-ref", "web", "search", "read", "exec", "wrap", "map", "skeleton", "query", "impact", "semantic", "gain", "dashboard", "license", "upgrade", "uninstall", "telemetry", "privacy", "faq", "changelog", "contributing"];
        let mut md = String::from("# Docs index\n\n## Pages\n\n");
        for (k, p) in pages.iter().enumerate() {
            let _ = writeln!(md, "- [{p}](docs/{p}.md) — section {k}, see `{p}` for details");
        }
        md.push_str("\n## Example\n\n```sh\n");
        for i in 0..70 {
            let _ = writeln!(md, "tokenade wrap 'step-{i} --flag {}'", i * 3);
        }
        md.push_str("```\n");
        let out = compact(&md);
        assert!(out.contains("[contributing](docs/contributing.md)"), "{out}");
        assert!(out.contains("step-69 --flag 207"), "{out}");
        assert!(!out.contains("elided"), "{out}");

        // A document too large to deliver keeps the caps.
        let mut big = String::from("# Big\n\n## List\n\n");
        for i in 0..2_000 {
            let _ = writeln!(big, "- item {i}: the value of field_{} is {}", i % 17, i * 31);
        }
        assert!(!tk_base::delivery::fits(&big));
        assert!(compact(&big).contains("- … list continues (1980 more item(s) elided) …"));
    }

    #[test]
    fn caps_long_lists() {
        let mut md = String::from("# L\n\n");
        for i in 0..80 {
            let _ = writeln!(md, "- item {i}");
        }
        md.push_str("\nAfter the list.\n");
        let out = compact_capped(&md, Caps::DEFAULT);
        assert!(out.contains("- item 0"));
        assert!(out.contains("- … list continues (60 more item(s) elided) …"));
        assert!(out.contains("After the list."));
        assert!(!out.contains("- item 40"));
    }

    #[test]
    fn no_panic_on_garbage() {
        let _ = compact("");
        let _ = compact("not markdown at all, just prose");
    }

    #[test]
    fn badge_cluster_collapses_to_single_marker() {
        let md = "# Title\n\n\
                  [![users](https://img.shields.io/badge/users-1k-green)](https://example.com)\n\
                  [![npm](https://img.shields.io/npm/v/foo)](https://npm.com)\n\
                  [![ci](https://img.shields.io/github/actions/workflow/status/x/y/ci.yml)](https://github.com/x/y)\n\
                  [![license](https://img.shields.io/badge/license-MIT-blue)](LICENSE)\n\n\
                  ## Section\n\nActual content.\n";
        let out = compact(md);
        assert!(out.contains("# Title"));
        assert!(out.contains("[badges:"), "missing badge marker:\n{out}");
        assert!(out.contains("## Section"));
        assert!(out.contains("Actual content"));
        // The badge URLs must NOT survive.
        assert!(!out.contains("shields.io"));
    }

    #[test]
    fn details_block_collapses_to_summary() {
        let md = "# Title\n\n\
                  <details>\n<summary>Click me</summary>\n\
                  Hidden content goes here.\nWith multiple lines.\nAnd more.\n\
                  </details>\n\n## Next\n";
        let out = compact(md);
        assert!(out.contains("[details: Click me — 6 line(s) not shown]"), "got:\n{out}");
        assert!(!out.contains("Hidden content"));
        assert!(out.contains("## Next"));
    }

    #[test]
    fn picture_block_dropped() {
        let md = "# Title\n\n\
                  <picture>\n<source srcset=\"dark.png\" media=\"(prefers-color-scheme: dark)\">\n\
                  <img src=\"light.png\" alt=\"logo\">\n</picture>\n\n## After\n";
        let out = compact(md);
        assert!(out.contains("# Title"));
        assert!(out.contains("## After"));
        assert!(!out.contains("srcset"));
        assert!(!out.contains("dark.png"));
    }

    #[test]
    fn inline_html_tags_stripped_text_kept() {
        // #551 — added `- bullet` second marker.
        let md =
            "# X\n\n- intro\n\nVisit <a href=\"https://x.com\">x.com</a> for more, or <sub>tiny</sub> text.\n";
        let out = compact(md);
        // Anchor href and tag names dropped, but the inner text survives.
        assert!(out.contains("Visit x.com for more"), "got:\n{out}");
        assert!(out.contains("tiny"));
        assert!(!out.contains("href"));
        assert!(!out.contains("<sub>"));
    }

    #[test]
    fn real_world_readme_yields_real_savings() {
        // Synthetic real-world README pattern: badges + HTML + small code +
        // prose. The pre-fix compactor reported 4.2 % on the bench fixture;
        // this version should clear 35 % on a structurally similar input.
        let mut md = String::from("# My Project\n\n");
        // Badge cluster (10 badges).
        for i in 0..10 {
            let _ = writeln!(
                md,
                "[![b{i}](https://img.shields.io/badge/x-{i}-blue)](https://example.com/{i})"
            );
        }
        md.push('\n');
        // <p align="center"> wrapper with <a><img> children.
        md.push_str("<p align=\"center\">\n<a href=\"#\"><img src=\"https://img.shields.io/badge/X-1\" alt=\"X\" /></a>\n");
        md.push_str("<a href=\"#\"><img src=\"https://img.shields.io/badge/Y-1\" alt=\"Y\" /></a>\n</p>\n\n");
        // Details block.
        md.push_str("<details>\n<summary>Advanced config</summary>\n\nLots of hidden text.\nMore.\nMore.\n</details>\n\n");
        // Real prose.
        md.push_str("## What it does\n\nThis project does the thing.\n\n");
        for _ in 0..20 {
            md.push_str("Some prose explaining the project in detail.\n");
        }
        let (out, sav) = compact_with_savings(&md);
        assert!(out.contains("# My Project"));
        assert!(out.contains("## What it does"));
        assert!(
            sav.percent() > 35.0,
            "expected >35% savings on realistic README, got {:.1}%\n{out}",
            sav.percent()
        );
    }

    #[test]
    fn never_grows_already_compact_doc() {
        // A clean, well-formed doc should not be bloated by the compactor.
        let md = "# A\n\n## B\n\nShort prose.\n";
        let (_, sav) = compact_with_savings(md);
        assert!(sav.tokens_after <= sav.tokens_before + 3);
    }

    #[test]
    fn savings_real_shape() {
        let mut md = String::from("# Title\n\n");
        // Past the delivery threshold — under it, the block is delivered whole
        // (`a_short_document_keeps_every_item_and_every_line`).
        md.push_str("```python\n");
        for i in 0..1_500 {
            let _ = writeln!(md, "def fn{i}(): return {i}");
        }
        md.push_str("```\n\n");
        md.push_str("Some real prose afterwards.\n\n[a]: u1\n[b]: u2\n[c]: u3\n");
        let (out, sav) = compact_with_savings(&md);
        assert!(out.contains("# Title"));
        assert!(out.contains("Some real prose"));
        assert!(sav.percent() > 80.0, "got {:?}", sav);
    }
}
#[cfg(test)]
mod fence_language_tests {
    use super::attach_orphan_fence_language as fix;

    #[test]
    fn a_language_chip_goes_back_onto_its_fence() {
        let md = "Some prose.\n\njs\n\n```\nfetch(url)\n```\n";
        let out = fix(md);
        assert!(out.contains("```js"), "language not restored: {out:?}");
        assert!(!out.contains("\njs\n"), "chip left behind: {out:?}");
        assert!(out.contains("fetch(url)"), "code lost: {out:?}");
    }

    #[test]
    fn a_fence_that_already_names_its_language_is_untouched() {
        let md = "python\n\n```rust\nfn main() {}\n```\n";
        assert_eq!(fix(md), md);
    }

    #[test]
    fn a_sentence_is_not_a_language_chip() {
        let md = "Now run this in bash\n\n```\nls -la\n```\n";
        assert_eq!(fix(md), md, "prose was eaten as a language label");
    }

    #[test]
    fn a_list_item_is_not_a_language_chip() {
        // `go` sits under a list, so the line above it is not blank.
        let md = "Languages:\n- rust\n- go\n\n```\ncode\n```\n";
        assert_eq!(fix(md), md);
    }

    #[test]
    fn a_word_that_is_not_a_language_stays_where_it_is() {
        let md = "before\n\nnotalanguage\n\n```\ncode\n```\n";
        assert_eq!(fix(md), md);
    }
}
#[cfg(test)]
mod placeholder_probe {
    use super::*;

    /// Exercises `strip_inline_html` DIRECTLY. `compact()` on a short document
    /// takes a passthrough branch and never reaches it, so the first version of
    /// these guards passed against the broken code — a test that cannot reach
    /// the defect is not a test of it.
    #[test]
    fn a_placeholder_survives_the_inline_html_stripper() {
        let got = strip_inline_html("Usage: `tokenade expand-ref <hash>` puis `--grep <pattern>`.");
        assert_eq!(
            got, "Usage: `tokenade expand-ref <hash>` puis `--grep <pattern>`.",
            "a CLI placeholder was deleted"
        );
        // …outside code too: `<file>` is no more a tag than `<hash>` is.
        assert_eq!(
            strip_inline_html("Voir <file> et <cmd> hors code."),
            "Voir <file> et <cmd> hors code."
        );
    }

    /// A code span is verbatim by definition — even a REAL tag inside one is
    /// text the reader asked to see literally.
    #[test]
    fn nothing_inside_a_code_span_is_markup() {
        assert_eq!(
            strip_inline_html("Run `a <br> b` and `<img src=x>` now."),
            "Run `a <br> b` and `<img src=x>` now."
        );
    }

    /// …and real inline HTML OUTSIDE code is still folded, or the fix would
    /// have traded a corruption for a regression.
    #[test]
    fn real_inline_html_outside_code_is_still_folded() {
        let got =
            strip_inline_html("A <br> line with <span>kept text</span> and <img src=x> here.");
        assert!(!got.contains("<br>"), "a void tag survived: {got}");
        assert!(!got.contains("<span>"), "a span tag survived: {got}");
        assert!(!got.contains("<img"), "an img tag survived: {got}");
        assert!(got.contains("kept text"), "inner text was lost: {got}");
    }

    /// Tags with attributes and closing forms are still recognised.
    #[test]
    fn attributes_and_closing_forms_are_still_tags() {
        assert!(!strip_inline_html("<div class=\"x\">t</div>").contains("<div"));
        assert!(!strip_inline_html("a<br/>b").contains("<br"));
        assert!(is_html_element("DIV"), "tag names are case-insensitive");
        assert!(!is_html_element("hash"));
        assert!(!is_html_element(""));
    }
}
// The `mod composition_probe` — "what is a markdown document made of, in
// bytes?", `#[ignore = "measurement"]` — moved to
// `tests/probes/markdown_composition.rs`: it is a MEASUREMENT, not a guard, and
// the `probes` target exists for that since the 87 targets were regrouped. It
// queried `disclose::threshold_tokens`, which stays in `tokenade-core`.

#[cfg(test)]
mod table_padding {
    use super::*;

    #[test]
    fn alignment_padding_goes_and_the_cells_do_not() {
        let t = "| Formatter             | Extensions        |\n\
                 | --------------------- | ----------------- |\n\
                 | gofmt                 | .go               |\n\
                 | prettier              | .js, .ts, .css    |\n";
        let out = unpad_tables(t);
        assert_eq!(
            out,
            "| Formatter | Extensions |\n| --- | --- |\n| gofmt | .go |\n| prettier | .js, .ts, .css |\n"
        );
    }

    /// The separator row carries ALIGNMENT. Losing a `:` silently re-renders
    /// the whole column, which is a change to the document, not to its padding.
    #[test]
    fn the_separator_keeps_its_alignment_markers() {
        let t =
            "| a | b | c | d |\n| :-------- | --------: | :------: | ------ |\n| 1 | 2 | 3 | 4 |\n";
        let out = unpad_tables(t);
        assert!(out.contains("| :--- | ---: | :---: | --- |"), "{out}");
    }

    /// A fenced block may contain pipe-delimited lines, and its bytes are code.
    #[test]
    fn a_code_fence_is_not_touched() {
        let t = "```sh\n| a        | b        |\n```\n| x        | y        |\n";
        let out = unpad_tables(t);
        assert!(
            out.contains("| a        | b        |"),
            "the fence was unpadded:\n{out}"
        );
        assert!(
            out.contains("| x | y |"),
            "the table was not unpadded:\n{out}"
        );
    }

    /// An escaped pipe is part of the cell, not a separator.
    #[test]
    fn an_escaped_pipe_stays_inside_its_cell() {
        let t = "| pattern    | note        |\n| ---------- | ----------- |\n| a \\| b     | alternation |\n";
        let out = unpad_tables(t);
        assert!(out.contains("| a \\| b | alternation |"), "{out}");
    }

    /// Prose that merely contains a pipe is not a table row.
    #[test]
    fn prose_with_a_pipe_is_left_alone() {
        for line in [
            "Run `foo | bar` to see it.",
            "|",
            "||",
            "The result | is unclear",
        ] {
            let t = format!("{line}\n");
            assert_eq!(
                unpad_tables(&t),
                t,
                "line was taken for a table row: {line}"
            );
        }
    }

    /// A document with no pipe at all pays nothing.
    #[test]
    fn a_document_without_tables_is_returned_as_is() {
        let t = "# Title\n\nSome prose that goes on for a while.\n";
        assert_eq!(unpad_tables(t), t);
    }

    /// The end of the file must not gain or lose a newline.
    #[test]
    fn the_trailing_newline_is_preserved_either_way() {
        assert_eq!(unpad_tables("| a  | b  |\n"), "| a | b |\n");
        assert_eq!(unpad_tables("| a  | b  |"), "| a | b |");
    }

    /// The gain, on the shape that motivated it: a translated docs page whose
    /// tables are padded to the width of their longest cell.
    #[test]
    fn a_padded_table_page_halves() {
        let mut t = String::from("# Formatters\n\nProse about formatters.\n\n");
        t.push_str(
            "| Formatter                  | Extensions                                   |\n",
        );
        t.push_str(
            "| -------------------------- | -------------------------------------------- |\n",
        );
        for i in 0..40 {
            t.push_str(&format!("| tool{i:<22} | .ext{i:<41} |\n"));
        }
        let out = unpad_tables(&t);
        assert!(
            out.len() * 2 < t.len(),
            "expected about half: {} of {}",
            out.len(),
            t.len()
        );
        assert!(
            out.contains("| tool7 | .ext7 |"),
            "a cell was altered:\n{out:.200}"
        );
    }
}
#[cfg(test)]
mod marker_window {
    use super::*;

    /// The window, not the rule, is what made this module disagree with the
    /// detector that routes into it. MEASURED 2026-08-26 over 9 001 documents
    /// of 4-40 KB: 381 were routed here as `Kind::Markdown` and handed back by
    /// the first line, 4.99 MB of them, and 377 of the 381 carry the marker
    /// further down. Widening to the bound `looks_strongly_like_markdown`
    /// already uses takes the rejections from 381 to 4 and the corpus from
    /// 79 654 264 delivered bytes to 78 312 670 — 292 documents that used to
    /// come back untouched no longer do.
    #[test]
    fn a_marker_below_the_first_forty_lines_still_counts() {
        let mut t = String::from("---\ntitle: Formatters\ndescription: A list\n---\n\n");
        t.push_str("import { Tabs } from '@astrojs/starlight/components';\n\n");
        t.push_str("# Formatters\n\n");
        for i in 0..40 {
            t.push_str(&format!(
                "Line {i} of an introduction that says nothing structural.\n"
            ));
        }
        t.push_str("\n## The list\n\n- one\n- two\n");
        assert!(
            looks_like_markdown(&t),
            "a document whose strong marker sits below line 40 was rejected"
        );
    }

    /// …and the shape the narrow window was protecting against must still be
    /// rejected. `# ` alone is the marker that collides with shell, Python,
    /// .gitignore and requirements.txt comments, and a wider window sees MORE
    /// of them — so the strong/weak rule is what has to hold, and it does.
    #[test]
    fn a_comment_dominated_config_file_is_still_not_markdown() {
        let mut t = String::from("# Production requirements\n# Pinned by the platform team\n\n");
        for i in 0..300 {
            t.push_str(&format!("package-{i}==1.{i}.0\n"));
            if i % 20 == 0 {
                t.push_str(&format!("# group {i}\n"));
            }
        }
        assert!(
            !looks_like_markdown(&t),
            "a requirements.txt with many `# ` comments was taken for markdown"
        );
    }

    /// A shell script is the same collision with a body that has `- ` in it —
    /// a usage message, a heredoc. It reaches this function through
    /// `compress.rs`, which asks the question directly.
    #[test]
    fn a_shell_script_with_a_dash_line_is_still_not_markdown() {
        let mut t = String::from("#!/usr/bin/env bash\n# Deploy helper\nset -euo pipefail\n\n");
        for i in 0..80 {
            t.push_str(&format!("step_{i}() {{ echo running {i}; }}\n"));
        }
        t.push_str("usage() {\n  cat <<'EOF'\nOptions:\n  -v  verbose\nEOF\n}\n");
        assert!(
            !looks_like_markdown(&t),
            "a shell script was taken for markdown"
        );
    }

    /// The early return is what keeps the wider bound free: a document that
    /// qualifies on line two must not be scanned further.
    #[test]
    fn a_document_that_qualifies_early_is_not_scanned_further() {
        let mut t = String::from("# Title\n\n## Section\n");
        // A tail long enough that scanning it would be visible if it happened.
        for i in 0..5000 {
            t.push_str(&format!("prose line {i}\n"));
        }
        assert!(looks_like_markdown(&t));
    }
}
#[cfg(test)]
mod a_cut_never_lands_inside_a_construct {
    use super::short_preview;

    /// And the caller: a truncated preview no longer ends inside a construct.
    #[test]
    fn short_preview_never_ends_inside_a_construct() {
        let long = format!("Titre de section (3 lines · \"[image: {}\")", "x".repeat(400));
        let out = short_preview(&long, 160);
        assert!(!out.contains("[image:"), "{out}");
        assert!(out.starts_with("Titre de section"), "{out}");
    }
}
#[cfg(test)]
mod a_heading_with_nothing_under_it_carries_no_count {
    use super::outline;

    /// `(0 lines)` reads as "this section is empty", which is a
    /// conclusion and not a fact. Seen on the page boundary of a
    /// `web a b c` batch, where the target's heading immediately precedes the page's
    /// own heading.
    #[test]
    fn an_empty_section_gets_no_suffix() {
        let md = "# la cible\n# Le titre de la page\nune ligne\nune autre\n";
        let out = outline(md);
        assert!(!out.contains("0 lines"), "{out}");
        assert!(out.contains("# la cible\n"), "{out}");
        // And the section that REALLY carries lines keeps its count.
        assert!(out.contains("2 lines"), "{out}");
    }
}
#[cfg(test)]
mod the_outline_counts_in_grammatical_english {
    use super::{compact, counted};

    #[test]
    fn one_is_singular_and_the_rest_are_not() {
        assert_eq!(counted(1, "line"), "1 line");
        assert_eq!(counted(0, "line"), "0 lines");
        assert_eq!(counted(2, "line"), "2 lines");
        assert_eq!(counted(1, "section"), "1 section");
        assert_eq!(counted(7, "section"), "7 sections");
    }

    /// The bound on both sides on the OUTLINE itself, not only on
    /// the helper: a ONE-line section and a multi-line one, in the same
    /// document, so that the fixture reaches both branches.
    #[test]
    fn the_outline_says_one_line_not_one_lines() {
        let mut doc = String::from("# Titre\n\n");
        doc.push_str("## Section courte\n\nune seule ligne de contenu ici\n\n");
        // A section whose ONLY line cannot serve as a preview — a
        // table rule is made only of formatting characters. It
        // reaches the "no preview" branch, the only one that still wrote its
        // plural by hand, and without it the mutation that puts it back SURVIVES
        // (as the harness said).
        doc.push_str("## Section sans aperçu\n\n| --- | --- |\n\n");
        doc.push_str("## Section longue\n\n");
        for i in 0..40 {
            doc.push_str(&format!("paragraphe {i} avec assez de texte pour peser\n\n"));
        }
        let out = compact(&doc);
        assert!(
            !out.contains("(1 lines"),
            "the outline still writes \"1 lines\":\n{out}"
        );
        assert!(
            !out.contains("(1 sections"),
            "the outline still writes \"1 sections\":\n{out}"
        );
    }
}
#[cfg(test)]
mod outline_preview_skips_figure_captions {
    /// The preview is what an agent reads to decide which section to expand,
    /// so a figure caption there costs tokens and says nothing about the
    /// section. Measured on britannica.com/animal/lion, where all three
    /// sections previewed as `"[1 of 2](/media/1/342664/159005)"` and
    /// `"[lions chasing a warthog](/media/1/342664/159006)(more)"`.
    #[test]
    fn a_caption_yields_to_the_prose_beneath_it() {
        let md = "## Hunting\n\n[lions chasing a warthog](/media/1/342664/159006)(more)\n\nLions prey on a large variety of animals ranging in size from rodents to hippopotamus.\n";
        let out = super::outline(md);
        assert!(
            out.contains("Lions prey on a large variety"),
            "the prose under the caption must be previewed: {out}"
        );
        assert!(
            !out.contains("lions chasing a warthog"),
            "the figure caption must not be the preview: {out}"
        );
    }

    /// A preference, never a rejection: dropping the caption outright emptied
    /// every preview, which thinned the outline enough to trip the
    /// "too small to still be about what it summarises" refusal — and the
    /// whole document got served instead. A section that has nothing else
    /// keeps its caption.
    #[test]
    fn a_section_with_only_a_figure_keeps_it_rather_than_losing_its_preview() {
        let md = "## Gallery\n\n![a plate of the specimen](/media/9/1/2.jpg)\n";
        let out = super::outline(md);
        assert!(
            out.contains("a plate of the specimen"),
            "a figure-only section must still carry a preview: {out}"
        );
    }

    /// Judged on the OPENING only. A sentence that merely links to something
    /// mid-way is prose and must be previewed unchanged.
    #[test]
    fn a_sentence_that_merely_contains_a_link_is_still_prose() {
        let md = "## Diet\n\nLions [prey](https://example.com/science/predation) on animals of many sizes.\n";
        let out = super::outline(md);
        assert!(out.contains("Lions [prey]"), "prose was skipped: {out}");
    }

    /// Markdown IMAGE syntax is a figure wherever it points. Without this the
    /// rule would lean entirely on the URL, and an image served from a path
    /// that looks like nothing in particular would preview as the caption.
    #[test]
    fn image_syntax_is_a_figure_even_when_the_url_looks_ordinary() {
        let md = "## Pipeline\n\n![schema of the stages](/assets/d)\n\nEach stage runs independently of the others.\n";
        let out = super::outline(md);
        assert!(
            out.contains("Each stage runs independently"),
            "the prose under the image must be previewed: {out}"
        );
        assert!(
            !out.contains("schema of the stages"),
            "an image is a figure whatever its url: {out}"
        );
    }

    /// The test is on what the line OPENS with. A sentence that happens to
    /// carry a media link in its middle is prose, and must win over a caption
    /// line beneath it.
    #[test]
    fn a_media_link_inside_a_sentence_does_not_make_it_a_caption() {
        // A PLAIN sentence follows, so a wrong verdict on the first line is
        // visible: the fallback cannot mask it by re-choosing the same line.
        let md = "## Throughput\n\nSee the chart ![chart](/media/c.png) for the measured numbers.\n\nThe numbers were collected over a week.\n";
        let out = super::outline(md);
        assert!(
            out.contains("See the chart"),
            "a sentence carrying a media link is still prose: {out}"
        );
        assert!(
            !out.contains("The numbers were collected"),
            "the first prose line must win, not the one after it: {out}"
        );
    }

    /// The form that actually reaches this code. The first version keyed on markdown image
    /// syntax, but tokenade's own HTML→markdown renders `<img>` as
    /// `[image: <alt>]`, so every extracted web page carries that instead —
    /// which is why the fix did not reach real pages.
    #[test]
    fn tokenades_own_image_rendering_is_a_figure() {
        let md = "## Section\n\n[image: plate 4 alt]\n\nThe section explains how the plates were engraved.\n";
        let out = super::outline(md);
        assert!(
            out.contains("The section explains how"),
            "prose under the image must be previewed: {out}"
        );
        assert!(
            !out.contains("plate 4 alt"),
            "the image line is a figure: {out}"
        );
    }

    /// The bare anchor the web pipeline emits for a captioned figure whose
    /// image carried no `alt`. Same rule, without alt text to name.
    #[test]
    fn the_bare_image_anchor_is_a_figure() {
        let md = "## Education\n\n[image]\nSonnet titled The Rainbow in her own hand\n\nFrom 1832 her mathematical abilities began to emerge.\n";
        let out = super::outline(md);
        assert!(out.contains("From 1832"), "prose must win: {out}");
        assert!(
            !out.contains("Sonnet titled"),
            "caption must be skipped: {out}"
        );
    }

    /// Skipping the figure alone only PROMOTES its caption. On Wikipedia the
    /// `<figcaption>` arrives as ordinary prose with no markup left to
    /// recognise it by — its POSITION under the figure is the only signal,
    /// and it is enough.
    #[test]
    fn the_caption_under_a_figure_is_skipped_too() {
        let md = "## Adult years\n\n[image: portrait alt]\nWatercolour portrait of Ada King, c. 1840, by Alfred Edward Chalon\n\nLovelace became close friends with her tutor Mary Somerville.\n";
        let out = super::outline(md);
        assert!(
            out.contains("Lovelace became close friends"),
            "the prose must win over the caption: {out}"
        );
        assert!(
            !out.contains("Watercolour portrait"),
            "the caption under a figure must be skipped: {out}"
        );
    }

    /// Still a preference. A section holding nothing but a figure and its
    /// caption keeps one of them rather than losing its preview — the whole
    /// reason the image rule fell back in the first place.
    #[test]
    fn a_figure_and_its_caption_alone_still_leave_a_preview() {
        let md = "## Plate\n\n[image: the plate]\nEngraved in 1845 by an unknown hand\n";
        let out = super::outline(md);
        assert!(
            out.contains("the plate") || out.contains("Engraved in 1845"),
            "a figure-only section must keep a preview: {out}"
        );
    }

    /// Only the line DIRECTLY under a figure is its caption. Prose further
    /// down is prose, and a section whose figure sits after the opening
    /// sentence is previewed by that sentence.
    #[test]
    fn only_the_line_directly_under_a_figure_is_a_caption() {
        let md = "## Method\n\nThe method was refined over several seasons of field work.\n\n[image: a diagram]\nDiagram of the apparatus\n";
        let out = super::outline(md);
        assert!(
            out.contains("The method was refined"),
            "an opening sentence must win: {out}"
        );
    }

    /// An ordinary (non-media) link opening a line is a cross-reference, not a
    /// figure — the rule keys on the TARGET, not on the bracket.
    #[test]
    fn a_plain_link_opening_a_line_is_not_a_figure() {
        let md = "## See also\n\n[The ownership chapter](https://doc.rust-lang.org/book/ch04.html) explains the rules in full.\n";
        let out = super::outline(md);
        assert!(
            out.contains("The ownership chapter"),
            "a cross-reference is not a caption: {out}"
        );
    }
}
#[cfg(test)]
mod a_plan_replaces_the_text_only_when_it_maps_it {
    /// **A one-line outline is not a map.**
    ///
    /// Outline delivery stashes the whole page and delivers the outline of the
    /// clipped view. On `zhwiki` that is the right move — 59 headings, 2,939 characters
    /// against 18,523 for the view itself. On a page WITHOUT headings, the clipped
    /// view is 180 lines, under the threshold that would trigger the promotion
    /// of structural labels: its outline drops to the single `<h1>` line,
    /// and the delivery goes from ~11,000 characters of sentences to 14.
    #[test]
    fn one_heading_does_not_map_a_page_but_three_do() {
        let one = super::outline("# Titre\n\nUne phrase, puis une autre.\n");
        assert!(
            !super::outline_maps_the_page(&one),
            "a single heading maps nothing: {one}"
        );
        let three = super::outline("# Titre\n\ncorps\n\n## A\n\ncorps\n\n## B\n\ncorps\n");
        assert!(
            super::outline_maps_the_page(&three),
            "three headings make a map: {three}"
        );
    }

    /// The bound is counted on the OUTLINE, not on the source: the outline is what
    /// would be delivered, and `outline` may promote one where the source has
    /// no visible heading.
    #[test]
    fn a_heading_inside_a_fence_is_not_a_section() {
        let md = "# Titre\n\n```\n# pas un titre\n## pas un titre non plus\n```\n\ntexte\n";
        assert!(
            !super::outline_maps_the_page(&super::outline(md)),
            "three hashes of which two are inside a fence do not make three sections"
        );
    }
}
#[cfg(test)]
mod the_outline_keeps_what_precedes_the_first_heading {
    /// **A paper opens with its title, its authors and its abstract, and its
    /// first `##` arrives at section 3.**
    ///
    /// Those lines belonged to no section, so the outline threw them away.
    /// MEASURED on 2026-09-11 on `pdf_mid.pdf`: the delivered outline named four
    /// sections and carried NEITHER the title NOR the abstract — precisely what one reads
    /// of a paper one will not read.
    #[test]
    fn the_preamble_is_described_as_a_section() {
        let md = "Systematic Characterization of a Sequence Group\n\n                  Ada Example, ABC Lab\n\n                  Abstract: finding similarities in a group of sequences.\n\n                  ## Introduction\n\ncorps un\n\n## Method\n\ncorps deux\n";
        let out = super::outline(md);
        assert!(
            out.contains("Systematic Characterization"),
            "the paper's title is missing from the outline:\n{out}"
        );
        // And it stays a SIBLING of the sections, not their parent: otherwise the
        // "(N sections)" suffix replaces the preview, hence the title itself.
        let first = out.lines().next().unwrap_or_default();
        assert!(
            !first.contains("sections)"),
            "the preamble swallowed the sections as children:\n{out}"
        );
        assert!(
            out.contains("## Introduction") && out.contains("## Method"),
            "the document's own sections have disappeared:\n{out}"
        );
    }

    /// **Anti-vacuity.** A document that OPENS with a heading has no preamble,
    /// and must not gain an outline line that describes nothing.
    #[test]
    fn a_document_opening_on_a_heading_gains_nothing() {
        let md = "# Titre\n\ncorps\n\n## Section\n\ncorps\n";
        let out = super::outline(md);
        assert!(
            !out.contains("before the first heading"),
            "preamble invented on a document that has none:\n{out}"
        );
    }
}

#[cfg(test)]
mod a_details_block_states_its_title_and_size {
    #[test]
    fn title_on_the_opening_line_the_next_line_or_a_one_line_block() {
        let doc = "# Rapport\n\nIntro.\n\n<details><summary>Les lignes écartées (356)</summary>\n\n- a\n- b\n- c\n\n</details>\n\n<details>\n<summary>Annexe</summary>\n\n- x\n</details>\n\n<details><summary>Court</summary>un mot</details>\nApres le bloc court.\n";
        let out = super::compact(doc);
        assert!(out.contains("[details: Les lignes écartées (356) — 7 line(s) not shown]"), "{out}");
        assert!(out.contains("[details: Annexe — 5 line(s) not shown]"), "{out}");
        assert!(out.contains("[details: Court — 1 line(s) not shown]"), "{out}");
        assert!(out.contains("Apres le bloc court."), "a one-line block swallowed what follows:\n{out}");
    }
}

#[cfg(test)]
mod an_html_table_is_data {
    #[test]
    fn the_matrix_is_rendered_and_logos_stay_a_marker() {
        // Padding the compactor drops anyway, so the fold pays and the
        // table path is actually reached (a tiny document is left as is).
        let pad = "<!-- generated by the docs build; do not edit by hand, see CONTRIBUTING -->\n".repeat(40);
        let doc = pad + "# Pilotes\n\n## Matériel\n\n- une imprimante\n- un câble\n\n## Compatibilité\n\nVoir [la liste](https://example.org).\n\n<table>\n<tr><th>&nbsp;</th><th>Linux</th><th>Windows</th></tr>\n<tr><td>USB</td><td>Yes</td><td>Not tested</td></tr>\n<tr><td>Serial</td><td>Yes</td><td><b>No</b> (a|b)</td></tr>\n</table>\n\n<table><tr><td><img src=\"a.png\"></td><td><img src=\"b.png\"></td></tr></table>\n\nFin.\n";
        let out = super::compact(&doc);
        assert!(out.contains("| USB | Yes | Not tested |"), "{out}");
        assert!(out.contains("| Serial | Yes | No (a\\|b) |"), "{out}");
        assert!(out.contains("Linux") && out.contains("Windows"), "{out}");
        assert!(!out.contains("&nbsp;"), "an entity left in a cell:\n{out}");
        assert!(out.contains("[table: collapsed — 1 line(s) not shown]"), "a table of logos keeps its marker:\n{out}");
        assert!(out.contains("Fin."), "{out}");
    }
}
