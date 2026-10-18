// The tests of `detectors`.
//
// Spliced by `include!` from `detectors.rs`: the same module, so the same
// private access and the same `super::` paths, without a single visibility
// line changed. It is the shape this repo already uses for 33 other modules.
//
// A `//!` is impossible here: a file spliced by `include!` is not a
// module, and an INNER doc comment has nothing to qualify.

#[cfg(test)]
mod a11y_fingerprint {
    use super::*;

    /// The tree that was being destroyed: 500 refs, not one `generic` node.
    #[test]
    fn a_snapshot_without_a_generic_node_is_still_a_snapshot() {
        let body: String = (0..8)
            .map(|i| {
                format!("  - listitem:\n    - link \"Item {i}\" [ref=e{i}]:\n      - /url: /items/{i}\n")
            })
            .collect();
        let tree = format!("- document \"Dashboard\":\n{body}");
        assert!(looks_like_a11y_snapshot(&tree));
        assert!(looks_like_a11y_snapshot(
            "- generic [ref=e1]:\n  - generic [ref=e2]:\n    - button [ref=e3]\n"
        ));
    }

    /// And the other direction, or the rule would claim any bullet list.
    #[test]
    fn prose_and_bullet_lists_are_not_snapshots() {
        assert!(!looks_like_a11y_snapshot(
            "- first item\n- second item\n- third item\n- fourth item\n"
        ));
        // A mention of a ref is not a tree of them.
        assert!(!looks_like_a11y_snapshot(
            "the snapshot uses [ref=e1] markers\n- see the docs\n"
        ));
        // Refs without the tree around them.
        assert!(!looks_like_a11y_snapshot("[ref=e1] [ref=e2] [ref=e3]\n"));
    }
}

#[cfg(test)]
mod a_bracketed_timestamp_is_not_a_json_array {
    use super::*;

    /// Every one of these opens with `[` followed by a digit — the shape a JSON
    /// array of numbers has — and not one of them is JSON. Real files, from the
    /// 19 the corpus sweep flipped: envoy, ninja, unreal, docker, storybook,
    /// playwright. All 19 failed `serde_json::from_str`.
    const NOT_JSON: &[&str] = &[
        "[14:25:48.462] [INFO] storybook v10.2.10\n[14:25:48.749] [DEBUG] more\n",
        "[2026-06-05T01:01:04.710Z] request served\n",
        "[1/842] Building CXX object src/main.cc.o\n",
        "[2026.06.05-01.01.04:710][ 42]LogInit: Display: engine start\n",
    ];

    /// Real JSON that opens on a number, in each shape the grammar allows.
    const IS_JSON: &[&str] = &[
        "[1, 2, 3]",
        "[1]",
        "[ 42 ]",
        "[-1, 2]",
        "[1.5e10, 2]",
        "[0]",
        "[\n  12,\n  13\n]",
    ];

    #[test]
    fn a_bracketed_timestamp_is_refused() {
        for t in NOT_JSON {
            assert!(
                !opens_a_json_value(t.trim_start()),
                "claimed as a JSON array: {:?}",
                &t[..t.len().min(48)]
            );
        }
    }

    #[test]
    fn an_array_of_numbers_is_still_json() {
        for t in IS_JSON {
            assert!(
                opens_a_json_value(t.trim_start()),
                "real JSON refused: {t:?}"
            );
            assert!(
                serde_json::from_str::<serde_json::Value>(t).is_ok(),
                "fixture is not valid JSON: {t:?}"
            );
        }
    }

    /// The other openers are characters prose does not put there, so they stay
    /// as loose as they were — the number is the only one that needed a rule.
    #[test]
    fn the_other_openers_are_untouched() {
        for t in [
            "{\"a\": 1}",
            "[\"x\"]",
            "[true]",
            "[null]",
            "[[1]]",
            "[{}]",
            "{}",
            "[]",
        ] {
            assert!(opens_a_json_value(t), "refused: {t}");
        }
    }

    #[test]
    fn the_whole_detector_agrees() {
        let log = "[14:25:48.462] [INFO] storybook v10.2.10\n\
                   [14:25:48.749] [DEBUG] Getting package.json info\n\
                   [14:25:48.997] [INFO] Starting...\n";
        assert_ne!(
            crate::auto::detect(log),
            crate::auto::Kind::Json,
            "a bracketed-timestamp log still routes to the JSON compactor"
        );
        // Long enough for `detect` to look at all, and carrying a quote: the
        // JSON arm has always required one (or a `": "`), so a bare array of
        // digits is `Unknown` both before and after this change and would prove
        // nothing here.
        let arr = format!(
            "[{}]",
            (0..400)
                .map(|i| format!("{i}, \"item_{i}\""))
                .collect::<Vec<_>>()
                .join(", ")
        );
        assert_eq!(
            crate::auto::detect(&arr),
            crate::auto::Kind::Json,
            "an array of numbers must still reach the JSON compactor"
        );
    }
}

#[cfg(test)]
mod a_bracketed_banner_is_not_a_json_array {
    use super::*;

    /// Every one opens with `[` followed by `t`, `f` or `n` — the three
    /// letters that begin `true`, `false` and `null` — and every one is
    /// prose. The first is **tokenade's own banner**, so a document carrying
    /// a fold notice was typed as JSON by the tool that wrote the notice.
    ///
    /// Found from the outside: headroom shipped the same defect on
    /// 2026-08-27 (a harness banner typed `JSON_ARRAY` on bracket balance,
    /// then destroyed by a lossy fallback). Ours cannot destroy — the JSON
    /// compactor returns its input when it cannot parse — but it costs the
    /// same thing: the compactor that WOULD have folded the document never
    /// runs.
    const NOT_JSON: &[&str] = &[
        "[tokenade folded 70 line(s) total: recover the verbatim bytes]\nkey: val\n",
        "[note: the run finished, 3 of 4 shards green]\nkey: val\n",
        "[failed: could not reach the registry]\nkey: val\n",
        "[none of the four probes answered]\nkey: val\n",
        "[truncated at 4096 bytes]\nkey: val\n",
        "[falling back to the cached manifest]\nkey: val\n",
    ];

    /// Real JSON opening on each literal, in the shapes the grammar allows.
    const IS_JSON: &[&str] = &[
        "[true]",
        "[true, false]",
        "[ null ]",
        "[\n  true,\n  null\n]",
        "[false]",
        "[null]",
    ];

    #[test]
    fn a_bracketed_banner_is_refused() {
        for t in NOT_JSON {
            assert!(
                !opens_a_json_value(t.trim_start()),
                "claimed as a JSON array: {:?}",
                &t[..t.len().min(52)]
            );
        }
    }

    #[test]
    fn a_literal_array_is_still_json() {
        for t in IS_JSON {
            assert!(
                opens_a_json_value(t.trim_start()),
                "refused real JSON: {t:?}"
            );
            assert!(
                serde_json::from_str::<serde_json::Value>(t).is_ok(),
                "the fixture itself is not JSON: {t:?}"
            );
        }
    }

    /// `truncated` starts with `tr`, `falling` with `fal`, `none` with `n` —
    /// a prefix check that stopped short of the whole literal would accept
    /// all three. The boundary is what separates them, and it is the SAME
    /// boundary the number opener uses.
    #[test]
    fn the_boundary_is_what_separates_them() {
        assert!(literal_is_followed_by_json("true]"));
        assert!(literal_is_followed_by_json("null, 1]"));
        assert!(!literal_is_followed_by_json("truncated at 4096 bytes]"));
        assert!(!literal_is_followed_by_json("falling back]"));
        assert!(!literal_is_followed_by_json("nullable column]"));
        // The shared rule, reached from the other opener.
        assert!(number_is_followed_by_json("42, 1]"));
        assert!(!number_is_followed_by_json("14:25:48.462]"));
    }
}

#[cfg(test)]
mod a_clock_without_a_date_is_still_a_log {
    use super::*;

    /// The real shape: a storybook debug log, every line a clock reading and a
    /// level, and diagnostics that read as markdown bullets.
    fn storybook() -> String {
        let mut s = String::new();
        s.push_str("[14:25:48.462] [INFO] storybook v10.2.10\n");
        s.push_str("[14:25:48.749] [DEBUG] Getting package.json info for /Users/dev/proj/package.json...\n");
        s.push_str("[14:25:48.997] [INFO] Starting...\n");
        s.push_str("[14:25:49.095] [DEBUG] Starting preview..\n");
        s.push_str("[14:25:49.098] [WARN] Unable to index files:\n");
        for i in 0..40 {
            s.push_str(&format!(
                "- ./../ui/src/components/widget_{i}.stories.tsx: CSF: default export must be an object\n"
            ));
        }
        s
    }

    /// Three shapes, and the DATE-ONLY one is here because relaxing this
    /// predicate to accept a clock first broke it: requiring a `:` refused
    /// `2026-08-02 INFO worker=7`, which carries a date and no time at all.
    /// A widening that narrows somewhere else is the easiest regression to
    /// ship, and the only thing that caught it was a test with a real fixture.
    #[test]
    fn a_stamp_with_a_level_is_a_log_line() {
        for l in [
            // clock, no date
            "[14:25:48.462] [INFO] storybook v10.2.10",
            "14:25:48.462 WARN something",
            // date, no clock
            "2026-08-02 INFO worker=7 status=ok path=/var/run/app/7",
            "[2026-08-02] ERROR upstream timeout",
            // both
            "[2026-05-28T10:00:01Z] INFO still works",
            "2026-05-28 10:00:01 ERROR still works",
        ] {
            assert!(looks_like_log_line(l), "refused: {l}");
        }
    }

    /// The level word carries the weight. A bare clock proves nothing, and
    /// prose that merely contains a level word is not a record either.
    #[test]
    fn a_clock_alone_and_a_level_alone_are_both_refused() {
        for l in [
            "14:25:48.462 transformed module_7.js",
            "[1/842] Building CXX object src/main.cc.o",
            "The INFO level is the default for this logger.",
            "- ./src/x.tsx: CSF: default export must be an object",
        ] {
            assert!(!looks_like_log_line(l), "claimed: {l}");
        }
    }

    /// BOTH halves of the markdown veto are load-bearing, and the corpus said
    /// so: vetoing on the timestamps alone costs six real `.md` files
    /// (`kubectl-logs.md`, `docker/logs.md` — documentation ABOUT logs, which
    /// quotes them); vetoing on "no heading and no fence" alone costs 174,
    /// README files that open on a badge block and reach their first heading
    /// past line 40.
    #[test]
    fn a_document_that_quotes_logs_keeps_its_structure() {
        let doc = "# kubectl logs\n\nUse it like this:\n\n\
                   [14:25:48.462] [INFO] pod started\n\
                   [14:25:49.462] [WARN] restart\n\
                   [14:25:50.462] [ERROR] crash\n\n\
                   - the first flag\n- the second flag\n";
        assert!(
            looks_like_markdown_doc(doc),
            "a heading means this is prose that quotes a log, not a log"
        );
    }

    /// **Two detectors competing for a document read the same text.**
    ///
    /// `command_chain::detect` reads the WHOLE text and its `is_section_header`
    /// accepts `## X` and `### X` — ordinary markdown headings. This detector
    /// only read `head`, 16 384 bytes. On a scientific paper, whose
    /// abstract fits on a single very long line and whose section headings
    /// are rare and late, the first saw ONE and the second
    /// FOUR: the loser was chosen by a window, not by evidence.
    ///
    /// MEASURED on 2026-09-11 on the corpus's `pdf_mid.pdf`: headings on lines 188,
    /// 434, 471 and 757, first heading at byte 14 451, and the paper went to the
    /// stash in a file named `…api.rest.chain.txt` — the retrieval footer
    /// choosing its advice from that extension.
    #[test]
    fn a_document_with_rare_late_headings_stays_a_document() {
        // A preamble longer than `head`: 16 384 bytes, then the headings.
        let mut doc = String::from("Systematic Characterization of a Sequence Group\n\n");
        while doc.len() < 17_000 {
            doc.push_str(
                "Finding similarities in a group of sequences often involves studying their \
                 common subsequences or their common substrings, which is the problem this \
                 article states and then solves in the general case. ",
            );
        }
        doc.push_str("\n\n### THE EMBEDDING ANTICHAIN\n\ncorps de section\n");
        doc.push_str("\n### OF N SEQUENCES\n\ncorps de section\n");
        assert_eq!(
            crate::auto::detect(&doc),
            crate::auto::Kind::Markdown,
            "a paper whose headings arrive after 16 KB is a document, \
             not a call chain"
        );
    }

    /// Anti-vacuity of the previous one: a REAL command chain must stay
    /// a chain. Widening what markdown claims is only worth it if what it must
    /// not claim still escapes it.
    #[test]
    fn a_real_command_chain_stays_a_chain() {
        let chain = "=== git status ===\n\nOn branch main\nnothing to commit\n\n\
                     === docker ps ===\n\nCONTAINER ID   IMAGE\nabc123         nginx\n";
        assert_eq!(
            crate::auto::detect(chain),
            crate::auto::Kind::CommandChain,
            "the `=== cmd ===` banners carry no markdown marker and \
             must stay with the chain compactor"
        );
    }

    /// **The CENSUS that decides the window, on real documents.**
    ///
    /// Two detectors compete for a markdown document: this one, and
    /// `command_chain`, whose `is_section_header` accepts `## X` and `### X`,
    /// that is, ordinary markdown headings. The first sees
    /// `head.lines().take(40)`, the second sees the WHOLE text. The loser is
    /// therefore chosen by a window and not by evidence.
    ///
    /// Ignored: it is a MEASUREMENT, not a ratchet, and it reads thousands of
    /// files. It reports, over the tree's `.md` files, how many are claimed by
    /// each detector — to run BEFORE and AFTER any change to the window.
    ///
    /// `cargo test -p tokenade-core --lib -- --ignored --nocapture which_markdown_documents`
    #[test]
    #[ignore = "measurement census, not a ratchet"]
    fn which_markdown_documents_are_claimed_and_by_whom() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>, cap: usize) {
            if out.len() >= cap {
                return;
            }
            let Ok(rd) = std::fs::read_dir(dir) else { return };
            let mut entries: Vec<_> = rd.filter_map(Result::ok).map(|e| e.path()).collect();
            // Deterministic order: a census whose sample changes between
            // two passes does not measure a change, it measures its sample.
            entries.sort();
            for p in entries {
                if out.len() >= cap {
                    return;
                }
                let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
                if p.is_dir() {
                    if name == "target" || name == "node_modules" || name == ".git" {
                        continue;
                    }
                    walk(&p, out, cap);
                } else if name.ends_with(".md") {
                    out.push(p);
                }
            }
        }
        const CAP: usize = 4000;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut files = Vec::new();
        walk(&root, &mut files, CAP);
        assert!(
            files.len() >= 500,
            "only {} .md file(s) reached: the census measures nothing",
            files.len()
        );
        let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
        let mut chain_but_not_markdown = Vec::new();
        for f in &files {
            let Ok(text) = std::fs::read_to_string(f) else { continue };
            let kind = crate::auto::detect(&text);
            *counts.entry(format!("{kind:?}")).or_default() += 1;
            if format!("{kind:?}") == "CommandChain" {
                chain_but_not_markdown.push(f.clone());
            }
        }
        println!("{} .md file(s) counted", files.len());
        for (k, n) in &counts {
            println!("  {k:<16} {n}");
        }
        println!("--- claimed by command_chain (first 10):");
        for f in chain_but_not_markdown.iter().take(10) {
            println!("  {}", f.display());
        }
    }

    #[test]
    fn a_structureless_timestamped_stream_is_not_markdown() {
        let s = storybook();
        assert!(
            !looks_like_markdown_doc(&s),
            "its own diagnostics supply the two bullet markers"
        );
        assert_eq!(
            crate::auto::detect(&s),
            crate::auto::Kind::Logs,
            "and refusing markdown must hand it to the log compactor, not to \
             Unknown — an unclaimed document is served WHOLE"
        );
    }

    /// One home: the veto asks the same predicate `Kind::Logs` is claimed on.
    #[test]
    fn the_veto_and_the_claim_read_the_same_predicate() {
        // Both predicates moved down into `detectors/shapes.rs`: the
        // family, not the trunk, and the visibility they carry is no longer the
        // same — we bound on the NAME, not on the qualifier.
        let src = crate::source_guard::module_family(
            &crate::source_guard::crates_dir()
                .join("tokenade-core")
                .join("src")
                .join("auto"),
            "detectors",
        );
        let at = src.find("fn is_a_timestamped_stream").expect("gone");
        let end = src[at..].find("fn looks_like_markdown_doc").expect("gone") + at;
        assert!(
            src[at..end].contains("looks_like_log_line"),
            "the veto re-decides from its own stamp counter — the two answers \
             will drift, and this is the pair that decides who gets the document"
        );
    }
}

#[cfg(test)]
mod table_versus_listing {
    use super::*;

    fn urls_csv(rows: usize) -> String {
        let mut t = String::from("article,embed_url,title\n");
        for i in 0..rows {
            t.push_str(&format!(
                "https://example.com/en/articles/piece-{i},https://api.podcache.net/embedded-player/sh/5f3df491-b095-465c-9d23-791b4fa9f7eb/ep/{i:08x},Titre numero {i}\n"
            ));
        }
        t
    }

    /// Every row of this file contains slashes and no spaces-in-pairs, so
    /// the plain path-listing branch counted all 300 of them as paths. Measured
    /// on the real file: 63 818 bytes delivered as
    /// `[tokenade:archive] plain-listing — 299 entries`, 191 bytes, every title
    /// gone. It is a table, and a table compactor exists.
    #[test]
    fn a_csv_of_url_columns_is_not_a_path_listing() {
        let t = urls_csv(300);
        // The precondition here used to assert that the fixture DOES reach the
        // weak branch, so the CSV win below could not be vacuous. Now
        // the archive branch counts only lines that END on their path, and a
        // row ending on `Titre numero 12` does not — so the CSV-as-paths defect is now
        // impossible at two layers instead of one. The assertion is kept, with
        // its sense flipped and its reason written down, rather than deleted:
        // if the weak branch ever claims this shape again, that is worth
        // knowing here.
        assert!(
            !matches!(
                crate::archive::detect_format(&t),
                crate::archive::Format::Plain
            ),
            "the archive branch claims a CSV of URL columns again"
        );
        assert_eq!(crate::auto::detect(&t), Kind::Csv);
    }

    /// The narrowing, stated as a test: only the branch that reads a row's
    /// CONTENT yields. A tar listing is anchored on its permissions column, and
    /// an archive whose member names carry two commas apiece is a delimited
    /// table by the comma test and an archive in fact.
    #[test]
    fn a_tar_listing_of_comma_named_members_is_still_an_archive() {
        let mut t = String::new();
        for i in 0..8 {
            t.push_str(&format!(
                "-rw-r--r-- root/root 1024 2026-01-01 12:00 dir/a,b,c-{i}.txt\n"
            ));
        }
        assert!(
            detect_csv_or_tsv(&t).is_some(),
            "fixture no longer looks like a table, so it proves nothing"
        );
        assert_eq!(crate::auto::detect(&t), Kind::ArchiveListing);
    }

    /// `grep -rn x dir | head` whose few hits include one line of a bundle is
    /// all long-line, code-shaped bytes: the minified detector took it for a
    /// bundle and the source hits were never delivered.
    #[test]
    fn search_hits_with_one_bundle_line_are_search_output() {
        let mut bundle = String::from("dist/assets/index-9c1f.js:1:");
        for i in 0..2000 {
            bundle.push_str(&format!("var a{i}=function(e){{return e.revived?{i}:null}};"));
        }
        let t = format!(
            "src/inbox.ts:12:  if (row.revived) markRevived(row.id);\n{bundle}\nsrc/lib/revive.ts:4:export const TTL = 14;\n"
        );
        assert_eq!(crate::auto::detect(&t), Kind::SearchOut);
        let (_, out, _) = crate::auto::compact(&t);
        assert!(out.contains("src/inbox.ts:12:  if (row.revived) markRevived(row.id);"), "{out}");
        assert!(out.contains("src/lib/revive.ts:4:export const TTL = 14;"), "{out}");
    }
}
