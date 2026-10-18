//! **Several queries in one call**, and what we get out of them.
//!
//! A results page is not an answer: the answer is what the results allow us
//! to say, and that is a different operation.
//!
//! The diagnosis lives here because a search that returns nothing is the
//! case most costly to debug blind — an absence asserts nothing, so the
//! degraded path must SAY it in the body.

use super::*;

/// Run several queries CONCURRENTLY in one invocation (D11).
///
/// Decide the wave and run the queries through it. Separate from
/// [`cmd_search_batch`] only so the decision is OBSERVABLE: with the search
/// itself injected, a test can watch how many run at once. Wired straight into
/// the command, a revert to "all at once" changes nothing any test can see —
/// which is how the defect stood.
pub fn search_batch_results(
    queries: &[String],
    run: impl Fn(&str) -> tokenade_core::search::Outcome + Sync,
) -> Vec<(String, tokenade_core::search::Outcome)> {
    let wave = tokenade_core::search::batch_concurrency(queries.len());
    in_waves(queries, wave, |q| (q.clone(), run(q)))
}

/// The agent that wants three questions answered gets them in one round-trip's
/// wall-clock instead of three.
///
/// In WAVES, not all at once. Each query cross-process-reserves up to `FANOUT`
/// engines for the whole of its fetch and the selector never queues behind a
/// held engine, so a batch wider than the engine pool starves its own tail: the
/// losers came back `all-engines: rate-limited`, a message about a fleet-wide
/// wall, when what held every engine was this same batch. Measured over 30 days
/// of real use, a query in position 1 of a batch was refused 14 % of the time
/// and one in position 4, 87 %. `search::batch_concurrency` sizes the wave from
/// the engines actually usable right now.
///
/// `now` is read PER WAVE. It gates the per-window budget reset, and a later
/// wave that reused the first wave's timestamp would keep looking at a window
/// that has since rolled over.
pub fn cmd_search_batch(queries: &[String], json: bool, answer: bool) -> i32 {
    let results = search_batch_results(queries, |q| {
        tokenade_core::search::run_search(q, tokenade_core::platform::now_unix_secs())
    });

    if json {
        let arr: Vec<_> = results
            .iter()
            .map(|(q, o)| {
                serde_json::json!({
                    "query": q,
                    "engines": o.used,
                    "refused": o.refused.iter()
                        .map(|(e, r)| serde_json::json!({"engine": e, "why": r.as_str()}))
                        .collect::<Vec<_>>(),
                    "results": o.results.iter()
                        .map(|r| serde_json::json!({"title": r.title, "url": r.url, "snippet": r.snippet}))
                        .collect::<Vec<_>>(),
                })
            })
            .collect();
        let sizes = usable_sizes(arr.iter().zip(results.iter()).map(|(v, (_q, o))| {
            (
                tokenade_core::tokenizer::estimate(&v.to_string()),
                o.is_usable(),
            )
        }));
        println!("{}", serde_json::json!({ "batch": arr }));
        record_batch_cache_saving("search", &sizes);
        return if results.iter().any(|(_, o)| o.is_usable()) {
            0
        } else {
            1
        };
    }

    let mut any = false;
    let mut sizes: Vec<usize> = Vec::new();
    for (q, o) in &results {
        let via = if o.used.is_empty() {
            "no engine".to_string()
        } else {
            o.used.join("+")
        };
        let body = if answer && o.is_usable() {
            answer_from_results(q, o)
        } else {
            tokenade_core::redact::redact_for_model(&tokenade_core::search::render(q, o))
        };
        let sz = tokenade_core::tokenizer::estimate(&body);
        sizes.extend(usable_sizes([(sz, o.is_usable())]));
        println!("[tokenade:search via {via}] {q:?}:\n\n{body}");
        if o.is_usable() {
            any = true;
            // ONE ROW PER QUERY, under the SAME op and the same assiette the
            // single-query path books. It booked one flat turn tax for the
            // WHOLE batch before, so three questions asked in one call
            // reported a third of what the same three asked separately do —
            // and asking them in one call is what this product's own guidance
            // tells agents to do. What a search is worth cannot depend on how
            // the agent packaged it.
            log_gain(
                "search",
                if answer {
                    "web-search-answer"
                } else {
                    "web-search"
                },
                // The SEARCH assiette, like the single-query path and like the
                // hook. `credit_binary` is the document turn tax, and for
                // `claude-code` it is `NativePdf` — zero by design.
                tokenade_core::coefficients::web_search_credit_savings(sz),
            );
        }
    }
    record_batch_cache_saving("search", &sizes);
    if any {
        0
    } else {
        1
    }
}

/// Read ONE result page for an answer: a dedicated reader first, the generic
/// HTML pipeline only when no reader claims the URL or the one that does
/// misses.
///
/// The reader is a PARAMETER so the composition is observable without a
/// network: the readers key on real hosts over https, so offline there is no
/// way to watch a real one serve, and a call site that asked and then dropped
/// the answer looked exactly like one that used it. Same reason
/// `search_batch_results` takes its search — see its note.
pub fn answer_page(
    url: &str,
    title: &str,
    via_reader: impl Fn(&str) -> Option<String>,
) -> Option<(String, String, String)> {
    const PAGE_CAP: usize = 4 * 1024 * 1024;
    // Nothing extra is booked for a reader here: what this answer saved is
    // priced once, on the whole body, by the caller.
    if let Some(md) = via_reader(url) {
        return Some((title.to_string(), url.to_string(), md));
    }
    let fetched = tokenade_core::web_fetch::fetch(
        url,
        &tokenade_core::web_fetch::FetchOpts::page("15", PAGE_CAP),
    )?;
    let body = tokenade_core::text_encoding::decode(&fetched.bytes)
        .map(|(t, _)| t)
        .unwrap_or_else(|| String::from_utf8_lossy(&fetched.bytes).into_owned());
    let d = tokenade_core::web_pipeline::distill_page(&body, url, fetched.content_type.as_deref());
    Some((title.to_string(), url.to_string(), d.md))
}

/// Read the top results and return WHAT THEY SAY.
///
/// A list of links is only half an answer: the caller then fetches three or
/// four of them, and that is where the tokens actually go. Everything needed
/// was already built — `web_fetch` for the page, `web_pipeline` for the
/// distillation, the prompt-scoped extract for the question — so this is the
/// composition of the two channels this refactor unified, not new machinery.
pub fn answer_from_results(query: &str, outcome: &tokenade_core::search::Outcome) -> String {
    /// Pages read per answer. Three is the point where a second source
    /// corroborates the first without paying for a fourth fetch.
    const READ_TOP_N: usize = 3;
    /// Tokens of distilled page allowed per source before the scoped extract
    /// takes over.
    const PER_SOURCE_BUDGET: usize = 1200;

    let picks: Vec<_> = outcome.results.iter().take(READ_TOP_N).collect();
    // Fetch concurrently: three sequential page fetches is the difference
    // between an answer and a pause.
    // No cap: three page fetches share nothing.
    let fetched_pages = in_waves(&picks, picks.len(), |r| {
        answer_page(&r.url, &r.title, |url| {
            site_reader_read("search-answer", url).map(|read| read.md)
        })
    });

    let mut out = String::new();
    let mut got = 0usize;
    let mut unanchored: Vec<(String, String, String)> = Vec::new();
    for page in fetched_pages {
        let Some((title, url, md)) = page else {
            continue;
        };
        // Ask the page the question instead of pasting it. This is the whole
        // point: a scoped extract is a fraction of the page.
        match tokenade_core::rag::prompt_scoped_extract_dispatch(&md, query, PER_SOURCE_BUDGET) {
            Some(passage) if !passage.trim().is_empty() => {
                got += 1;
                out.push_str(&format!("## {title}\n{url}\n\n{}\n\n", passage.trim()));
            }
            // The page had nothing to say about the query. It used to be
            // pasted anyway — the first 2 400 characters, under the same
            // `## Title` as a page that DID answer, so the caller could not
            // tell an answer from a page opening. Hold it back and let the
            // sources that actually answered stand alone.
            _ => unanchored.push((title, url, md)),
        }
    }
    if got == 0 {
        // Nothing anchored anywhere. The openings are still better than
        // nothing — but they are labelled for what they are, and the links
        // come along so the caller can go further.
        for (title, url, md) in &unanchored {
            let head = tokenade_core::caps::truncate_at_char_boundary(md, 2400).trim();
            if head.is_empty() {
                continue;
            }
            out.push_str(&format!("## {title}\n{url}\n\n{head}\n\n"));
        }
        let body = if out.trim().is_empty() {
            String::new()
        } else {
            format!("\n\n{out}")
        };
        // Say so and hand back the links, rather than pretend the search
        // failed — or worse, pretend these openings answered the question.
        return format!(
            "[tokenade] no passage in the top results answers {query:?}; what follows are page \
             openings, not answers.\n\n{}{body}",
            tokenade_core::search::render(query, outcome)
        );
    }
    let rest: Vec<&str> = outcome
        .results
        .iter()
        .skip(READ_TOP_N)
        .map(|r| r.url.as_str())
        .collect();
    if !rest.is_empty() {
        out.push_str(&format!("Other results not read: {}\n", rest.join(" · ")));
    }
    tokenade_core::redact::redact_for_model(&out)
}

/// Compact a SAVED results page — the job `tokenade serp` used to be, now
/// reached through the one search command instead of a second one.
pub fn compact_saved_serp(path: &str) -> i32 {
    use std::io::Read;
    let mut html = String::new();
    if path == "-" || path.is_empty() {
        // Capped: `yes | tokenade search --html -` reached 49 GB and was
        // OOM-killed before this.
        if crate::bounded_input::read_stdin_lossy(&mut html).is_err() {
            eprintln!("tokenade search: failed to read stdin");
            return 1;
        }
    } else {
        // Through `read_path_bounded`, like every other entry point that
        // slurps a path an agent chose: `search --html /dev/zero` served the
        // byte-reading path and never returned (2.6 GB resident in 5 s).
        match crate::bounded_input::read_path_bounded(std::path::Path::new(path)) {
            Ok(b) => {
                html = tokenade_core::text_encoding::decode(&b)
                    .map(|(t, _)| t)
                    .unwrap_or_else(|| String::from_utf8_lossy(&b).into_owned())
            }
            Err(e) => {
                eprintln!("tokenade search: {path}: {e}");
                return 1;
            }
        }
    }
    // Use the ENGINE'S OWN extractor when we can tell whose page this is — the
    // generic anchor walk is the weak parse that returns navigation.
    let (hits, engine) = tokenade_core::search::extract_saved_page(&html);
    let found = hits.len();
    let before = tokenade_core::tokenizer::estimate(&html);
    let outcome = tokenade_core::search::Outcome {
        results: hits,
        used: engine.map(|e| vec![e.name()]).unwrap_or_default(),
        refused: Vec::new(),
    };
    // **Name the page we hold, not a query nobody typed.**
    // `render` receives the query, and this route passed it the constant
    // "saved page": an empty result answered `no usable results for "saved page"`,
    // which names something that exists nowhere and hides the only useful
    // fact — which file we are talking about.
    let source = if path == "-" || path.is_empty() {
        "the page on stdin".to_string()
    } else {
        path.to_string()
    };
    let mut body =
        tokenade_core::redact::redact_for_model(&tokenade_core::search::render(&source, &outcome));
    // A list of anchors is not a list of results, and only the ABSENCE of the
    // "parsed as a X results page" line said so.
    if let Some(note) = tokenade_core::search::unrecognised_saved_page_note(&source, engine, found)
    {
        body = format!("{note}\n{body}");
    }
    let sav = tokenade_core::Savings::new(before, tokenade_core::tokenizer::estimate(&body));
    log_gain("search", "serp_compact", credit_web(&sav));
    if let Some(e) = engine {
        eprintln!("[tokenade] parsed as a {} results page", e.name());
    }
    // A results page folds harder than anything else we handle — 700 KB of
    // markup down to the handful of hits that answer the query. When the page
    // arrived on STDIN that fold was the only copy: there is no path to go back
    // to, and the page cannot be re-fetched identically. Reading a FILE keeps
    // its own recovery, so only the piped case needs a pointer.
    let body = if path == "-" || path.is_empty() {
        crate::wrap::attach_drop_recovery(
            body,
            &html,
            sav.tokens_before.saturating_sub(sav.tokens_after),
            sav.tokens_before,
            &tokenade_core::disclose::SourceHint::web(),
            "web-batch",
        )
    } else {
        body
    };
    println!("{body}");
    0
}

/// Probe every engine and report what each one actually returns.
///
/// Written because I hand-rolled this probe three times in one session while
/// diagnosing engines that were serving navigation bars. A capability the
/// maintainer needs every time an engine drifts belongs in the product.
pub fn search_diagnose() -> i32 {
    const PROBE: &str = "python asyncio vs threading";
    println!(
        "probing {} engines with {PROBE:?}\n",
        tokenade_core::search::ENGINES.len()
    );
    println!(
        "{:<12} {:>8} {:>7} {:>10}  verdict",
        "engine", "bytes", "hits", "ms"
    );
    let mut healthy = 0usize;
    for e in tokenade_core::search::ENGINES {
        let t = std::time::Instant::now();
        let body = tokenade_core::web_fetch::fetch_search(&e.url(PROBE), e.cookie());
        let ms = t.elapsed().as_millis();
        let (bytes, hits, verdict) = match &body {
            None => (0, 0, "unreachable".to_string()),
            Some(b) if tokenade_core::serp::looks_like_challenge(b) => {
                (b.len(), 0, "bot-challenge".to_string())
            }
            Some(b) => {
                let h = e.parse(b);
                let v = if h.len() < tokenade_core::search::MIN_HITS {
                    "thin".to_string()
                } else if !tokenade_core::serp::plausible_for_query(&h, PROBE) {
                    "off-topic".to_string()
                } else {
                    healthy += 1;
                    "ok".to_string()
                };
                (b.len(), h.len(), v)
            }
        };
        println!("{:<12} {bytes:>8} {hits:>7} {ms:>10}  {verdict}", e.name());
    }
    println!(
        "\n{healthy}/{} engines healthy",
        tokenade_core::search::ENGINES.len()
    );
    // Non-zero when the fleet cannot serve a search at all, so a monitor can
    // watch this.
    i32::from(healthy == 0)
}
