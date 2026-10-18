// The tests of `web_batch`.
//
// Spliced in by `include!` from `web_batch.rs`: the same module, hence the same
// private access and the same `super::` paths, without a single visibility line
// changed. It is the shape this repository already uses for 33 other modules.
//
// A `//!` is impossible here: a file spliced in by `include!` is not a module,
// and an INNER doc comment has nothing to qualify.

#[cfg(test)]
mod fair_share_tests {
    use super::fair_share_body;

    fn page(title: &str, n: usize) -> String {
        let mut md = format!("# {title}\n");
        for i in 0..n {
            md.push_str(&format!("## Section {i}\nune ligne de contenu numéro {i}\n"));
        }
        md
    }

    /// **A big page no longer eats the others' share.**
    ///
    /// The batch used to be summarised as a single document and the outline cut
    /// at head and tail: MEASURED on four pages, the MIDDLE ones vanished
    /// entirely. Here the first weighs fifty times the last.
    #[test]
    fn every_page_is_represented_however_big_its_neighbour() {
        let pages: Vec<(String, String)> = vec![
            ("a.html".into(), page("Enorme", 400)),
            ("b.html".into(), page("Moyenne", 20)),
            ("c.html".into(), page("Minuscule", 2)),
        ];
        let body = fair_share_body(&pages);
        for t in ["a.html", "b.html", "c.html"] {
            assert!(body.contains(&format!("# {t}")), "{t} missing:\n{body}");
        }
        // And the total budget holds: each share is bounded to its share, so
        // the sum is too. The bound is the SHARE and not the total, so that a
        // page that does not use its own does not hand it to the next one —
        // otherwise the result would depend on the order of the targets.
        let budget = tokenade_core::disclose::SUMMARY_MAX_BODY_CHARS;
        assert!(
            body.chars().count() <= budget + pages.len(),
            "the batch exceeds a single page's budget: {} > {budget}",
            body.chars().count()
        );
        // The big one is indeed CUT: without sharing it took everything.
        assert!(body.contains("preview clamped"), "{body}");
    }

    /// A single page takes the whole budget: the split does not punish it.
    #[test]
    fn a_lone_page_keeps_the_whole_budget() {
        let one = vec![("a.html".to_string(), page("Seule", 400))];
        let body = fair_share_body(&one);
        assert!(body.chars().count() > tokenade_core::disclose::SUMMARY_MAX_BODY_CHARS / 2);
    }
}

#[cfg(test)]
mod absent_page_tests {
    use super::name_the_pages_the_summary_left_out;

    fn targets() -> Vec<String> {
        ["a.html", "b.html", "c.html"].iter().map(|s| s.to_string()).collect()
    }

    /// The measured case: four pages requested, two in the delivered outline,
    /// and nothing saying so.
    #[test]
    fn a_page_clamped_out_of_the_summary_is_named() {
        let summary = "[disclose:summary kind=md tokens=9 mode=md-outline lines=9]\n\
                       # a.html\n# Un titre (3 lines)\n# c.html\n"
            .to_string();
        let out = name_the_pages_the_summary_left_out(summary, &targets());
        assert!(out.starts_with("[tokenade:web batch] 1 of the 3 pages"), "{out}");
        assert!(out.contains("absent: b.html"), "{out}");
        // And the summary itself follows, intact.
        assert!(out.contains("[disclose:summary"), "{out}");
    }

    /// Nothing to say in the three cases where there is nothing to say: all
    /// targets are there, nothing was summarised, or there was only one page.
    #[test]
    fn nothing_is_said_when_nothing_is_missing() {
        let whole = "[disclose:summary kind=md tokens=9 mode=md-outline lines=9]\n\
                     # a.html (2 sections)\n# b.html\n# c.html (1 line)\n"
            .to_string();
        assert_eq!(
            name_the_pages_the_summary_left_out(whole.clone(), &targets()),
            whole
        );
        // No summary at all: the batch is delivered whole, every title is there.
        let raw = "\n# a.html\ndu texte\n".to_string();
        assert_eq!(name_the_pages_the_summary_left_out(raw.clone(), &targets()), raw);
        // A single target: the question does not arise.
        let one = "[disclose:summary kind=md]\n# z.html\n".to_string();
        assert_eq!(
            name_the_pages_the_summary_left_out(one.clone(), &["a.html".to_string()]),
            one
        );
    }

    /// A prefix is not a presence: `# a.html` proves nothing about
    /// `# a.html.bak`, and the comparison is made on the LINE.
    #[test]
    fn a_prefix_is_not_a_presence() {
        let summary = "[disclose:summary kind=md]\n# a.html.bak (2 lines)\n".to_string();
        let out = name_the_pages_the_summary_left_out(
            summary,
            &["a.html".to_string(), "a.html.bak".to_string()],
        );
        assert!(out.contains("absent: a.html"), "{out}");
        assert!(!out.contains("absent: a.html, a.html.bak"), "{out}");
    }
}

#[cfg(test)]
mod batch_booking_tests {
    use super::*;

    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

    /// The wave bounds how many run AT ONCE. Two halves, and the second is why
    /// the helper exists at all: it must still be concurrent inside a wave, or
    /// the cap has quietly turned the batch back into N sequential calls.
    #[test]
    fn a_wave_bounds_what_runs_at_once_without_serialising_the_batch() {
        let items: Vec<usize> = (0..7).collect();
        let (live, peak) = (AtomicUsize::new(0), AtomicUsize::new(0));
        let out = in_waves(&items, 2, |i| {
            let n = live.fetch_add(1, SeqCst) + 1;
            peak.fetch_max(n, SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(40));
            live.fetch_sub(1, SeqCst);
            i * 10
        });
        assert_eq!(
            out,
            vec![0, 10, 20, 30, 40, 50, 60],
            "the caller reads ONE list in the input's order — no wave is visible in it"
        );
        let peak = peak.load(SeqCst);
        assert!(
            peak <= 2,
            "a wave of 2 must never run more than 2, ran {peak}"
        );
        assert!(
            peak > 1,
            "a wave of 2 that only ever ran 1 is N sequential calls"
        );
    }

    /// A wave of 0 divides a batch into no waves at all. Every item would be
    /// dropped in silence, which on `search` is every query answered by nothing.
    #[test]
    fn a_wave_of_zero_still_runs_the_whole_batch() {
        let items: Vec<usize> = (0..5).collect();
        assert_eq!(in_waves(&items, 0, |i| *i), items);
        assert_eq!(
            in_waves(&items, 99, |i| *i),
            items,
            "a wave wider than the batch is no cap"
        );
        assert!(in_waves::<usize, usize>(&[], 3, |i| *i).is_empty());
    }

    /// The command's OWN path, with only the network swapped out: it must ask
    /// for the bound and honour it. Asserting `batch_concurrency` alone leaves
    /// the wiring free to go back to firing every query at once.
    #[test]
    fn the_search_batch_runs_its_queries_through_the_bound() {
        let queries: Vec<String> = (0..9).map(|i| format!("q{i}")).collect();
        let (live, peak) = (AtomicUsize::new(0), AtomicUsize::new(0));
        let out = search_batch_results(&queries, |q| {
            let n = live.fetch_add(1, SeqCst) + 1;
            peak.fetch_max(n, SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(40));
            live.fetch_sub(1, SeqCst);
            assert!(q.starts_with('q'));
            tokenade_core::search::Outcome {
                results: Vec::new(),
                used: Vec::new(),
                refused: Vec::new(),
            }
        });
        assert_eq!(out.len(), queries.len(), "every query is still run");
        assert_eq!(
            out.iter().map(|(q, _)| q.clone()).collect::<Vec<_>>(),
            queries,
            "and answered in the order asked"
        );
        let peak = peak.load(SeqCst);
        assert!(
            peak < queries.len(),
            "9 queries fired at once is the defect: {peak} ran together"
        );
        // Not compared against a second `batch_concurrency` call: that reads
        // live engine health, so the two reads can disagree and the assertion
        // would measure the machine. The bound is at most `ENGINES / FANOUT`
        // whatever the health, which is what `peak < queries.len()` pins.
        assert!(peak >= 1, "the batch still ran");
    }

    /// The number `cmd_search_batch` feeds the wave must actually be a cap:
    /// `search` is the one batch whose items contend, and a bound equal to the
    /// batch size is the defect this exists to close.
    #[test]
    fn the_search_batch_asks_for_a_bound_below_its_own_width() {
        let wide = tokenade_core::search::batch_concurrency(9);
        assert!(
            wide < 9,
            "a batch of 9 queries must not run 9 selections at once, got {wide}"
        );
        assert!(wide >= 1, "and never zero: every query is attempted");
    }

    /// A result nobody can use bought no round-trip. The saving is "you paid
    /// ONE context re-send instead of N"; a query that came back empty will be
    /// asked again, so counting it credits work that did not happen.
    #[test]
    fn an_unusable_result_buys_no_round_trip() {
        assert_eq!(
            usable_sizes([(100, true), (200, false), (300, true)]),
            vec![100, 300]
        );
        assert!(usable_sizes([(100, false), (200, false)]).is_empty());
        // Non-vacuity: with everything usable nothing is dropped, so the filter
        // is not silently emptying an honest batch.
        assert_eq!(usable_sizes([(100, true), (200, true)]), vec![100, 200]);
    }

    /// Every site that serves a DOCUMENT must pick its model with the format.
    ///
    /// The rule itself is unit-tested in `coefficients`, and that is not
    /// enough: a call site that keeps asking the agent-only table compiles and
    /// leaves every test green — which is exactly how `NativePdf` came to
    /// answer for `.xlsx` and `.docx`. Pinned on the source of the four files
    /// that serve documents, comment lines stripped first.
    #[test]
    fn every_document_site_picks_its_model_with_the_format() {
        // ALL the binary's files, not four named by hand. The comment above
        // already says why: "a call site that keeps asking the agent-only
        // table compiles and leaves every test green". A list of four files
        // has exactly the same flaw one notch higher — it says nothing about
        // the FIFTH site.
        let cli_src = tokenade_core::source_guard::crates_dir()
            .join("tokenade-cli")
            .join("src");
        let files = tokenade_core::source_guard::rust_sources_whole(&cli_src);
        assert!(
            files.len() > 50,
            "the walk only returned {} file(s)",
            files.len()
        );
        let mut all = String::new();
        for (path, src) in &files {
            if path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with("_tests.rs"))
            {
                continue;
            }
            let name = path.display();
            let code: String = src
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            // The agent-only table may still be asked for the WEB half, which
            // has no format; what it must not do is answer the binary one.
            for line in code.lines() {
                if line.contains("agent_web_binary_model") {
                    assert!(
                        !line.contains("let (_, "),
                        "{name} still takes the BINARY model from the agent-only \
                         table: {line}"
                    );
                }
            }
            all.push_str(&code);
        }
        // Non-vacuity: the replacement really is in use somewhere.
        assert!(
            all.matches("credit_binary_for(").count() + all.matches("binary_model_for(").count()
                >= 4,
            "the format-aware selector is not reaching the document sites"
        );
    }

    /// A search is booked through the SEARCH assiette, never the document one.
    ///
    /// `credit_binary` is the binary-document turn tax, and for `claude-code`
    /// that model is `NativePdf` — **zero by design**. Every search this CLI
    /// served was therefore worth nothing and fell under the ledger's noise
    /// threshold: measured 2026-08-28, not one `web-search`, `web-search-batch`
    /// or `serp_compact` row exists on a machine holding 603 search batches.
    ///
    /// Pinned on the source, because `run_search` reaches the network. Comment
    /// lines are stripped first, and the non-vacuity assertion checks the
    /// right function was sliced.
    #[test]
    fn a_search_is_never_booked_through_the_document_assiette() {
        // The two numbers the choice is between, so this test says WHY and not
        // merely WHICH function to call.
        let served = 600;
        let doc = tokenade_core::coefficients::binary_credit_savings(
            tokenade_core::coefficients::BinaryCreditModel::NativePdf,
            served,
        );
        assert_eq!(
            doc.tokens_before.saturating_sub(doc.tokens_after),
            0,
            "the document model credits a Claude search ZERO — that is the trap"
        );
        let sea = tokenade_core::coefficients::web_search_credit_savings(served);
        assert!(
            sea.tokens_before.saturating_sub(sea.tokens_after) > 0,
            "the search model must credit something"
        );

        // Both files are FOUND, not named: `cmd_search` moved down from
        // `compactors.rs` into `compactors/search_cmd.rs`, and this test would
        // have reported "function moved" — which reads as the product
        // disappearing when the test merely had the wrong address.
        let cli_src = tokenade_core::source_guard::crates_dir()
            .join("tokenade-cli")
            .join("src");
        let batch = tokenade_core::source_guard::the_one_source_under(
            &cli_src,
            concat!("fn ", "cmd_search_batch"),
        )
        .1;
        let single =
            tokenade_core::source_guard::the_one_source_under(&cli_src, concat!("fn ", "cmd_search("))
                .1;
        for (file, needle) in [
            (batch.as_str(), concat!("fn ", "cmd_search_batch")),
            (single.as_str(), concat!("fn ", "cmd_search(")),
        ] {
            let start = file.find(needle).expect("function moved");
            let end = file[start..]
                .find("\n/// ")
                .map(|i| start + i)
                .unwrap_or(file.len());
            let code: String = file[start..end]
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                code.contains("web_search_credit_savings"),
                "{needle} does not book through the search assiette"
            );
            assert!(
                !code.contains("credit_binary("),
                "{needle} still books a search as a document"
            );
            assert!(code.contains("run_search"), "sliced the wrong function");
        }
    }

    /// Both branches of `cmd_search_batch` must go through the filter.
    ///
    /// The pure rule above is not enough on its own: reverting either call site
    /// to a bare `sizes.push(sz)` compiles, books failures again, and leaves
    /// every test green. `cmd_search_batch` reaches the network, so the
    /// property is pinned on the SOURCE — comment lines stripped first, because
    /// a rule named in a comment is not a rule the code follows.
    #[test]
    fn both_search_branches_filter_before_they_book() {
        // The FAMILY: rendering and search moved down into `web_batch/`, and
        // the trunk now only carries the batch itself.
        let root = tokenade_core::source_guard::crates_dir()
            .join("tokenade-cli")
            .join("src")
            .join("compactors");
        let src = tokenade_core::source_guard::module_family(&root, "web_batch");
        let src = src.as_str();
        let start = src
            .find(concat!("fn ", "cmd_search_batch"))
            .expect("cmd_search_batch moved");
        let end = src[start..]
            .find("\n/// ")
            .map(|i| start + i)
            .unwrap_or(src.len());
        let code: String = src[start..end]
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !code.contains("sizes.push("),
            "a branch books every result again, usable or not"
        );
        assert_eq!(
            code.matches("usable_sizes(").count(),
            2,
            "both the --json branch and the text branch must filter"
        );
        // Non-vacuity: the slice really is the function body.
        assert!(code.contains("run_search"), "sliced the wrong function");
    }

    /// And a batch where nothing was usable books NOTHING — the saving formula
    /// needs at least two entries, so the filter reaches all the way through.
    #[test]
    fn a_batch_of_failures_books_nothing() {
        let none = usable_sizes([(4_000, false), (5_000, false), (6_000, false)]);
        assert_eq!(batching_cache_saved(&none), 0);
        // One survivor is still not a batch.
        let one = usable_sizes([(4_000, true), (5_000, false)]);
        assert_eq!(batching_cache_saved(&one), 0, "N=1 saves no re-send");
        // Two survivors do save one re-send: 4000·1 + 6000·0.
        let two = usable_sizes([(4_000, true), (5_000, false), (6_000, true)]);
        assert_eq!(batching_cache_saved(&two), 4_000);
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "tokenade-batchbook-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A page in a BATCH books the web assiette, not the document turn tax.
    ///
    /// The batch used to hand the caller a bare `String`, so the caller could
    /// only guess — and it guessed `credit_binary`, the flat 800-token document
    /// tax, ONCE for the whole batch. The single-target path books the
    /// per-agent WEB model, whose `before` is the raw page. The two must agree,
    /// because what a page is worth cannot depend on how it was packaged.
    #[test]
    fn a_batched_page_books_the_web_assiette_not_the_document_one() {
        let d = scratch("web");
        let body = "<p>alpha prose that goes on for a while.</p>".repeat(200);
        let page = format!(
            "<html><head><title>Alpha</title></head><body><h1>Alpha</h1>{body}</body></html>"
        );
        let f = d.join("a.page");
        std::fs::write(&f, &page).unwrap();

        let r = web_batch_render_one(f.to_str().unwrap(), 0, None).expect("the file renders");
        assert_eq!(r.op, "html-to-markdown", "op: {}", r.op);
        let tax = tokenade_core::coefficients::binary_turn_tax() as usize;
        assert_ne!(
            r.booked.tokens_before,
            r.booked.tokens_after + tax,
            "the batch booked the flat document turn tax for an ordinary page"
        );
        // Non-vacuity, and the shape of it matters. This used to assert
        // `before > after + tax`, which reads as "the page is worth more than
        // the flat tax" — a claim about the SIZE of one fixture under whichever
        // credit model the machine happens to select. It was green here and red
        // on CI the first time CI ever reached the test step: `before 547 after
        // 147 tax 800`, an assertion about the environment failing on a product
        // that was working. Asserting instead that the assiette SCALES with the
        // page is wrong for the same reason in the other direction: it holds
        // for `Transparent` and not for `ReaderClean`, so it would have been
        // green here and red under `gemini-cli`, `cursor` or `opencode`.
        //
        // What the test means has no size in it: whatever the active model
        // books for this page, it must not be what the DOCUMENT path would
        // book. Ask that path directly, with the same `after`, and require the
        // two to differ — exact, model-aware, and true on every agent.
        let as_document =
            crate::compactors::nudges::credit_binary_raw(false, r.booked.tokens_after);
        assert_ne!(
            (r.booked.tokens_before, r.booked.tokens_after),
            (as_document.tokens_before, as_document.tokens_after),
            "the batch booked what the document path books"
        );

        // And the same page, booked through the declarer the SINGLE path uses,
        // on what the batch delivered, lands on the same numbers.
        let (op, basis) = book_distilled_page(&tokenade_core::web_pipeline::distill_page_with(
            &page,
            f.to_str().unwrap(),
            Some("text/html"),
            tokenade_core::web_pipeline::PageOpts {
                max_input_tokens: 0,
            },
        ));
        assert_eq!(op, r.op);
        let booked = tokenade_core::web_credit::credit_on_delivery(basis, tokenade_core::tokenizer::estimate(&r.md));
        assert_eq!(booked.tokens_before, r.booked.tokens_before);
        assert_eq!(booked.tokens_after, r.booked.tokens_after);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// And the CALLER must book what the render hands it, once per page.
    ///
    /// The test above pins what `web_batch_render_one` RETURNS; on its own it
    /// stays green while `cmd_web_batch` throws the assiette away and books one
    /// flat credit for the batch — which is exactly the defect that stood.
    /// Reading (b) of a green mutation: the fixture did not reach the branch.
    ///
    /// The ledger root is redirected through `gain::set_root` rather than by
    /// moving `HOME`: `HOME` is process-wide and this suite runs in parallel,
    /// and moving it here broke an unrelated test two modules away. Rows are
    /// matched on THIS test's own file name, so a root another test already
    /// claimed is harmless.
    /// `search --answer` reads the top results of a live search, and it must
    /// read them the way every other page path does.
    ///
    /// That is where the readers matter most: the top results of a web search
    /// ARE reddit threads, GitHub issues and YouTube pages. This path scraped
    /// them instead — measured 2026-09-03 on the first hit of a live search,
    /// `github.com/BurntSushi/ripgrep/issues/176`: 18,965 bytes of thread
    /// through `tokenade web`, and through `search --answer` 832 bytes whose
    /// headings were `Description` and `References`, the latter a list of
    /// GitHub login and signup links.
    ///
    /// Two targets, because the question is asked per page: one URL a reader
    /// claims and one no reader does, and the log must tell them apart. The
    /// third assertion is the one that survives having the answer IGNORED —
    /// it needs the network, and says so rather than pretending otherwise:
    /// with none, the reader logs its own failure and that is what is checked.
    #[test]
    fn the_answer_path_asks_a_reader_for_every_page_it_reads() {
        let _env = tokenade_core::platform::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let d = scratch("answer-readers");
        let log = d.join("debug.log");

        struct Restore;
        impl Drop for Restore {
            fn drop(&mut self) {
                // SAFETY: still inside the ENV_LOCK guard's lifetime.
                unsafe {
                    std::env::remove_var(tokenade_core::debug_log::ENV_VAR);
                }
            }
        }
        // SAFETY: env vars are process-wide; ENV_LOCK is held above.
        unsafe {
            std::env::set_var(tokenade_core::debug_log::ENV_VAR, &log);
        }
        let _restore = Restore;

        let claimed = "https://github.com/BurntSushi/ripgrep/issues/176";
        let unclaimed = "https://example.com/no-reader-claims-this";
        let hit = |url: &str| tokenade_core::serp::Result {
            title: url.to_string(),
            url: url.to_string(),
            snippet: String::new(),
        };
        let outcome = tokenade_core::search::Outcome {
            results: vec![hit(claimed), hit(unclaimed)],
            used: vec!["fixture"],
            refused: Vec::new(),
        };
        let _body = answer_from_results("multiline search across lines", &outcome);
        let recorded = std::fs::read_to_string(&log).unwrap_or_default();

        let considered: Vec<serde_json::Value> = recorded
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter(|v| v["event"] == "site_reader_considered")
            .collect();
        // Read off the PARSED line rather than the file: the reader logs its
        // own events on the same channel, and a substring test for
        // `api.github.com` passes on one of those while this line says
        // nothing.
        let asked = |url: &str| -> Option<(String, String)> {
            considered
                .iter()
                .find(|v| v["details"]["url"] == url)
                .map(|v| {
                    (
                        v["component"].as_str().unwrap_or_default().to_string(),
                        v["details"]["via"].as_str().unwrap_or_default().to_string(),
                    )
                })
        };
        assert_eq!(
            asked(claimed),
            Some(("search-answer".into(), "api.github.com".into())),
            "the answer path must ask for a page a reader claims, and name it:\n{recorded:.800}"
        );
        assert_eq!(
            asked(unclaimed),
            Some(("search-answer".into(), "none".into())),
            "it must ask for EVERY page it reads, not only the ones that match:\n{recorded:.800}"
        );

        // And what the reader answers must be what the page contributes. The
        // assertion above holds for a call site that asks and drops the
        // answer; this one does not, and needs no network to say so.
        let served = answer_page("https://unreachable.invalid/x", "T", |u| {
            assert_eq!(u, "https://unreachable.invalid/x");
            Some("READER MARKDOWN".to_string())
        });
        assert_eq!(
            served,
            Some((
                "T".to_string(),
                "https://unreachable.invalid/x".to_string(),
                "READER MARKDOWN".to_string()
            )),
            "a reader that served was ignored and the page scraped instead"
        );
        assert_eq!(
            answer_page("https://unreachable.invalid/x", "T", |_| None),
            None,
            "no reader and no fetch must yield nothing, not an empty page"
        );
    }

    #[test]
    fn the_batch_books_one_row_per_page_through_that_assiette() {
        let _env = tokenade_core::platform::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let d = scratch("caller");
        let body = "<p>alpha prose that goes on for a while.</p>".repeat(200);
        let page = format!(
            "<html><head><title>Alpha</title></head><body><h1>Alpha</h1>{body}</body></html>"
        );
        let a = d.join("batchbook-a.page");
        let b = d.join("batchbook-b.page");
        std::fs::write(&a, &page).unwrap();
        // DIFFERENT content: two identical pages book two identical rows and
        // the ledger keeps one, which reads exactly like the defect.
        std::fs::write(
            &b,
            page.replace("alpha", "beta").replace("Alpha", "Beta") + "<p>tail</p>",
        )
        .unwrap();

        struct Restore;
        impl Drop for Restore {
            fn drop(&mut self) {
                // SAFETY: still inside the ENV_LOCK guard's lifetime.
                unsafe {
                    std::env::remove_var("TOKENADE_FORCE_GAIN");
                }
            }
        }
        // Redirect FIRST, so no write can reach the real ledger, and only then
        // lift the debug-build refusal.
        tokenade_core::gain::set_root(d.clone());
        let _restore = Restore;
        // SAFETY: env vars are process-wide; ENV_LOCK is held above. The
        // ledger refuses every write from a debug build without this.
        unsafe {
            std::env::set_var("TOKENADE_FORCE_GAIN", "1");
        }

        let want_a = web_batch_render_one(a.to_str().unwrap(), 0, None).expect("renders");
        let want_b = web_batch_render_one(b.to_str().unwrap(), 0, None).expect("renders");
        assert_ne!(
            want_a.booked.tokens_before, want_b.booked.tokens_before,
            "the two fixtures must book differently, or one row could pass for two"
        );
        let ledger = tokenade_core::gain::current_ledger_path();
        let before = std::fs::read_to_string(&ledger).unwrap_or_default();
        let code = cmd_web_batch(&[a.to_str().unwrap(), b.to_str().unwrap()], 0, None);
        assert_eq!(code, 0);

        let after = std::fs::read_to_string(&ledger).unwrap_or_default();
        let rows: Vec<serde_json::Value> = after[before.len().min(after.len())..]
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .filter(|v: &serde_json::Value| v["op"] == "html-to-markdown")
            .collect();
        assert_eq!(rows.len(), 2, "two pages must book two rows: {after}");
        let mut got: Vec<(u64, u64)> = rows
            .iter()
            .map(|r| {
                (
                    r["before"].as_u64().unwrap_or(0),
                    r["after"].as_u64().unwrap_or(0),
                )
            })
            .collect();
        let mut want = vec![
            (
                want_a.booked.tokens_before as u64,
                want_a.booked.tokens_after as u64,
            ),
            (
                want_b.booked.tokens_before as u64,
                want_b.booked.tokens_after as u64,
            ),
        ];
        got.sort();
        want.sort();
        assert_eq!(got, want, "the batch booked something else than each page");
        // The ledger root stays put for the rest of the process (`set_root` is
        // set-once), so only this test's own inputs are removed.
        let _ = std::fs::remove_file(&a);
        let _ = std::fs::remove_file(&b);
    }
}
