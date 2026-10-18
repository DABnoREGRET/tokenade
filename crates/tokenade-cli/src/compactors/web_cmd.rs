//! **`tokenade web`: the command.**
//!
//! Compose the targets, serve a batch in parallel, fold, and return. What
//! READS a page lives in `tokenade_core::web`; here we decide what to ask
//! for, in what order, under what budget, and what to say when a target is
//! missing.

use super::*;

pub(crate) fn cmd_web(rest: &[String]) -> i32 {
    // This usage existed all along and `--help` could never reach it: `web` was
    // absent from `cmd_forwards_help`, so the global handler answered "this
    // command has no dedicated help yet" instead.
    if crate::help_routing::asks_for_help(rest) {
        println!("{WEB_USAGE}");
        return 0;
    }
    let mut max_input_tokens: usize = 0;
    let mut targets: Vec<&str> = Vec::new();
    let mut prompt: Option<String> = None;
    let mut iter = rest.iter().peekable();
    while let Some(a) = iter.next() {
        match a.as_str() {
            // What the caller wants FROM the document. WebFetch already carries
            // the agent's `prompt` and the web path uses it to serve a
            // prompt-scoped extract instead of the whole page; a document read
            // from disk had no way to say the same thing, so it could only ever
            // get "all of it". Same flag, same engine, same budget.
            "--prompt" | "--question" | "-q" => match iter.next() {
                Some(v) => prompt = Some(v.clone()),
                None => {
                    eprintln!("tokenade web: --prompt requires a value");
                    return 2;
                }
            },
            "--max-tokens" | "--max-input-tokens" => match iter.next() {
                Some(v) => match v.parse::<usize>() {
                    Ok(n) => max_input_tokens = n,
                    Err(_) => {
                        eprintln!("tokenade web: --max-tokens expects an integer, got '{v}'");
                        return 2;
                    }
                },
                None => {
                    eprintln!("tokenade web: --max-tokens requires a value");
                    return 2;
                }
            },
            // `kind=page` / `kind=serp` were MCP tool PARAMETERS, retired with
            // the zero-MCP pivot, and the retirement was only ever enforced on
            // the scaffolded docs. The command itself still met both spellings
            // the way it met any stranger: `--kind page` fell into the
            // unknown-flag arm below and got a shrug, while the bare
            // `kind=serp` fell into the POSITIONAL arm and became a second
            // fetch target — so `tokenade web page.html kind=serp` served the
            // page, printed `kind=serp: fetch/read failed` about a target
            // nobody named, and exited 1. A run that worked, reported as a
            // failure to anything reading the exit code.
            //
            // Measured 2026-08-31 across this machine's transcripts: 19 real
            // `web` calls in 10 sessions carry one of the two spellings. They
            // come from a doc block that taught `kind=page` for years, so the
            // caller is not guessing — it is remembering. The refusal says
            // what replaced the parameter, because "unknown flag" sends
            // someone to re-read the help for a word that is not in it.
            "--kind" => {
                let value = iter.next().map(|v| v.as_str()).unwrap_or("page");
                eprintln!("{}", kind_is_retired(value));
                return 2;
            }
            other if other.starts_with("kind=") => {
                eprintln!("{}", kind_is_retired(other.trim_start_matches("kind=")));
                return 2;
            }
            // Silent-accept sweep, same class as `execute`'s (#812): an
            // unrecognised flag fell into the positional arm below and became a
            // FETCH TARGET. `tokenade web --json page.html` answered
            // `--json: fetch/read failed`, served the real page anyway, and
            // exited 0 — and a mistyped `--max-token 5000 <url>` silently
            // dropped the budget while turning one fetch into a three-target
            // batch. A flag we do not know is a mistake, not a URL.
            other
                if other.starts_with("--")
                    || (other.starts_with('-') && other.len() > 1 && other != "-") =>
            {
                eprintln!("tokenade web: unknown flag '{other}'. Try `tokenade web --help`.");
                return 2;
            }
            // Positional: a URL or a file. MULTIPLE are a concurrent batch (D11).
            other => targets.push(other),
        }
    }
    // Nothing to fetch is a forgotten argument, not a job well done. `web`
    // was the one command of fifteen that answered a bare invocation with
    // silence and exit 0 — no output, nothing on stderr — so a typo read as
    // success. Every sibling (`read`, `search`, `query`, `impact`, `skeleton`,
    // `exec`, `expand-ref`, `wrap`, …) already answers usage and 2.
    if targets.is_empty() {
        eprintln!("{WEB_USAGE}");
        return 2;
    }
    // D11: several pages in one invocation, fetched concurrently — one
    // round-trip's wall-clock instead of one per page. Batch mode routes through
    // the same transport + content bricks.
    //
    // This used to end "the single-target path below keeps its extra
    // refinements (site readers, prompt-scoped extract)", and each of those
    // gaps was in turn found to be a defect rather than a design: the readers
    // were given to the batch, then `--prompt`, then the whole-answer size
    // gate, then the replay fold. A note saying the batch has less is how the
    // next gap goes unexamined, so it says nothing of the sort now.
    if targets.len() > 1 {
        // The question now travels with the batch, scoped per page by the same
        // helper the single-target path uses. It used to be announced as
        // ignored, which cost the caller 9.3x on a measured two-page fetch.
        return cmd_web_batch(&targets, max_input_tokens, prompt.as_deref());
    }
    let path_arg: Option<&str> = targets.first().copied();
    // D13: a single-target fetch counts toward the sequential-web-call nudge.
    // A URL — this path also serves local files, and reading a PDF off disk
    // was being counted as a web fetch and answered with "do them in ONE call:
    // `tokenade web url1 url2 …`", advice about a network the caller never
    // touched. Observed on `tokenade read <file>`.
    if path_arg.is_some_and(|p| tokenade_core::url::scheme_host(p).is_some()) {
        nudge_sequential_web_calls("cli-web", "fetch", false);
    }
    // A URL, not a path. Two shipped surfaces already TELL the agent to run
    // `tokenade web '<url>'` — the PostToolUse hint when a native WebFetch is
    // blocked (`recovery.rs`), and the install-time hint in
    // `install/claude_hooks.rs` — and both promised a fetch this command could
    // not perform: the URL went to `fs::read` and came back
    // "No such file or directory". The advertised recovery path from a 403 was
    // a dead end. Fetch it here with the same browser profile the readers use,
    // then hand the bytes to the pipeline below unchanged (binary sniff →
    // document extractor, else HTML → markdown), so a URL and a saved page
    // travel exactly the same road.
    let is_url = path_arg.is_some_and(|p| p.starts_with("http://") || p.starts_with("https://"));
    use std::io::IsTerminal;
    if !is_url && path_arg.is_none_or(|a| a == "-") && std::io::stdin().is_terminal() {
        eprintln!("{WEB_USAGE}");
        return 2;
    }
    // Read raw bytes (not lossy String): a "page" fed to `tokenade web`
    // can actually BE a binary document — e.g. `curl <url> | tokenade web -`
    // where the URL serves or redirects to a PDF. Decoding to UTF-8 first
    // would corrupt it before we could tell. Sniff magic bytes and, for any
    // format we can extract (PDF/DOCX/XLSX/…), route to the docs extractor
    // instead of html_to_markdown (which would mangle the binary).
    // Provenance the fetch knows and this command used to drop: the HTTP status
    // and the URL the redirects actually landed on. Round 25 gave the WebFetch
    // hook both; `cmd_web` went without because `reader_curl_get` returned only
    // bytes. Measured then: 23 % of real fetches are non-2xx and 11.6 % end on
    // a different host, all of it invisible here.
    // A domain with a dedicated reader has a known-better path — old.reddit for
    // new-reddit, the registry API for crates.io. Round 18: this command never
    // tried them, so `tokenade web https://www.reddit.com/r/rust/` served 37
    // characters of SPA shell while the WebFetch hook, on the same URL, served
    // the whole thread. Seventeen readers reachable from one channel of two.
    //
    // Fail-open exactly like the hook: any fetch or parse failure falls through
    // to the generic pipeline on the original URL, so a reader that goes stale
    // degrades to what this command did before instead of breaking it.
    if let Some(url) = path_arg.filter(|_| is_url) {
        if let Some(plan) = tokenade_core::site_readers::plan(url) {
            let started = std::time::Instant::now();
            if let Some(rendered) =
                crate::hooks::webfetch_pretool::readers::render_site_reader(&plan, url, started)
            {
                let basis = crate::hooks::webfetch_pretool::readers::reader_basis(&rendered);
                let out = tokenade_core::disclose::maybe_stash(
                    rendered.md,
                    tokenade_core::disclose::SourceHint::web_with_content_type("text/markdown"),
                );
                let sav = tokenade_core::web_credit::credit_on_delivery(
                    basis,
                    tokenade_core::tokenizer::estimate(&out),
                );
                log_gain("web", "site-reader", sav);
                println!("{out}");
                return 0;
            }
        }
    }
    let mut provenance = String::new();
    // The address that ANSWERED, when it differs from the one requested.
    let mut landing: Option<String> = None;
    // The server-declared media type, when there was a server. Held so the
    // format decisions below can consult it instead of guessing from bytes —
    // the hook has always had it and this command had not, which is why a
    // `text/html` page whose first 4 KB tripped the control-char heuristic
    // (inline SVG, base64 fonts, minified JS) was served here as a binary stash.
    let mut content_type: Option<String> = None;
    let bytes: Vec<u8> = match path_arg {
        Some(url) if is_url => {
            // Overridable like this product's other ceilings
            // (`TOKENADE_MAX_INPUT_BYTES`, `TOKENADE_MAX_ZIP_TOTAL_BYTES`,
            // `TOKENADE_RAW_BUDGET`) so the guard for "a cut body says so" can
            // drive it with a small page instead of a 17 MB one.
            let web_fetch_cap: usize = std::env::var("TOKENADE_WEB_FETCH_CAP")
                .ok()
                .and_then(|v| v.trim().parse::<usize>().ok())
                .filter(|v| *v > 0)
                .unwrap_or(16 * 1024 * 1024);
            // The same page fetch the hook makes: polite UA first, browser
            // profile on a refusal.
            match tokenade_core::web_fetch::fetch_detailed(
                url,
                &tokenade_core::web_fetch::FetchOpts::page("25", web_fetch_cap),
            ) {
                Ok(f) => {
                    provenance = crate::hooks::webfetch_pretool::provenance_notes(
                        f.status,
                        url,
                        f.effective_url.as_deref(),
                        f.truncated,
                    );
                    // **A relative URL resolves against the page that
                    // ANSWERED**, not against the one we asked for. After a
                    // redirect to another host, the requested origin is the
                    // wrong one, and everything the pipeline does with the
                    // address — feed discovery, JS rendering, announcing the
                    // origin of references — then targets a host that did not
                    // serve these bytes. `provenance_notes` already knew this
                    // address; the distiller did not.
                    landing = f.effective_url.clone();
                    content_type = f.content_type;
                    f.bytes
                }
                Err(why) => {
                    // The proxy contract asks that an absent capability
                    // "answer with a readable refusal naming the reason and the
                    // way around it. Never an empty or silently degraded
                    // result." Measured 2026-08-08: a closed port and a
                    // nonexistent host produced the SAME four words, so the
                    // message distinguished nothing a user could act on.
                    //
                    // `fetch_detailed` keeps the transport's own verdict,
                    // which is the difference between "try another URL" and
                    // "raise the timeout". The debug-log pointer stays: the
                    // cause names the LAST failure, the log shows every attempt.
                    eprintln!(
                        "tokenade web: {url}: {why}. Set TOKENADE_DEBUG_LOG=<path> and \
                         re-run to see what tokenade tried."
                    );
                    return 1;
                }
            }
        }
        Some("-") | None => {
            // Capped like the path branch two arms down, and for the same
            // reason: a pipe that never ends is not distinguishable from a
            // slow one until the machine is gone.
            match crate::bounded_input::read_stdin_bytes() {
                Ok(b) => b,
                Err(_) => {
                    eprintln!("tokenade web: failed to read stdin");
                    return 1;
                }
            }
        }
        // Through the bounded reader, whose own doc states the rule this site
        // was breaking: "every entry point that slurps a path an agent chose
        // must go through here". MEASURED on the published 1.1.8 and on this
        // build, `tokenade web /dev/urandom` grew to 2.6 GB of RSS in five
        // seconds and never returned, while `tokenade read /dev/urandom`
        // refused in 2 s with a sentence naming the cause and the override.
        // Same input, two commands, one guarded.
        Some(path) => match crate::bounded_input::read_path_bounded(std::path::Path::new(path)) {
            Ok(b) => {
                // A local file has no server to have labelled it, so the
                // pipeline saw `None` and took the HTML path — which parses a
                // markdown or text file as one giant text node and collapses
                // its newlines. The filename is the label here; see
                // `content_type_for_local_path`. `.html` and unknown
                // extensions still map to `None` and keep the HTML path.
                content_type = content_type.or_else(|| {
                    tokenade_core::web::content_type_for_local_file(path, &b).map(str::to_string)
                });
                b
            }
            Err(e) => {
                eprintln!("tokenade web: {path}: {e}");
                return 1;
            }
        },
    };
    // Same classifier as the hook. Before it was shared, this command had only
    // the byte heuristic, so a `text/html` page with enough inline SVG or
    // base64 to trip it was stashed as binary instead of read.
    let body_kind = tokenade_core::web_fetch::classify_body(&bytes, content_type.as_deref());
    let detected = body_kind.detected.clone();
    // A document answers here; anything else hands the bytes back.
    let bytes = match serve_document(
        bytes,
        detected.as_deref(),
        path_arg,
        prompt.as_ref(),
        &provenance,
    ) {
        Ok(rc) => return rc,
        Err(bytes) => bytes,
    };
    // Binary we recognise but cannot extract (an image, an archive…). The guard
    // above only covered formats we DO extract, so everything else fell straight
    // into `html_to_markdown` and got printed as mojibake — the exact "vomit
    // binary" outcome that branch's comment warns about, just one `if` further
    // out. Stash it and hand back a metadata line.
    //
    // `Some("bin")` is the sniffer's "unknown binary" verdict, and legacy-encoded
    // TEXT lands there: a UTF-16 CSV is dense in NUL bytes, so it sniffs binary
    // and the agent got a stash reference for a file it could have just read.
    // Give the decoder a shot at those before writing them off; it returns None
    // for anything that isn't really text, so a genuine unknown binary still
    // stashes.
    // `body_kind.binary` first: with a server that declared `text/*`, the byte
    // sniff is overruled and the page is read instead of stashed. With no
    // server (file / stdin) the classifier falls back to the sniff alone, so
    // this behaves exactly as it did.
    if body_kind.binary && detected.as_deref().is_some_and(|k| k != "bin") {
        let summary = tokenade_core::disclose::maybe_stash_bytes(
            bytes,
            tokenade_core::disclose::SourceHint::web(),
        );
        println!(
            "{}",
            crate::hooks::webfetch_pretool::attach_provenance(&provenance, &summary)
        );
        return 0;
    }
    // Decode with the file's OWN encoding rather than assuming UTF-8:
    // `from_utf8_lossy` turned a Windows-1252 `Région` into `R�gion` and a
    // Shift-JIS header into `�n��` — content destroyed, tokens still billed,
    // and the agent re-reads the file to try again.
    let Some((html, _enc)) = tokenade_core::text_encoding::decode(&bytes) else {
        let summary = tokenade_core::disclose::maybe_stash_bytes(
            bytes,
            tokenade_core::disclose::SourceHint::web(),
        );
        // Zero bytes stash to nothing, so this branch used to print the
        // provenance and a newline. See `empty_page_note`.
        let summary = if summary.trim().is_empty() {
            empty_page_note(path_arg.unwrap_or("this source"))
        } else {
            summary
        };
        println!(
            "{}",
            crate::hooks::webfetch_pretool::attach_provenance(&provenance, &summary)
        );
        return 0;
    };
    // The content pipeline — the same one the hook runs, in
    // `tokenade_core::web_pipeline`. This command used to have its own, and it
    // was missing five things the hook had: feeds by content, feed discovery,
    // product facts, the output hard cap, and the server content-type. None of
    // those absences was a decision.
    let d = tokenade_core::web_pipeline::distill_page_with(
        &html,
        landing
            .as_deref()
            .or_else(|| path_arg.filter(|_| is_url))
            .unwrap_or("https://example.invalid/"),
        content_type.as_deref(),
        tokenade_core::web_pipeline::PageOpts { max_input_tokens },
    );
    // Booked once the delivery is known (end of this function).
    let (op, basis) = book_distilled_page(&d);

    // Tell the stash what this payload IS instead of letting it sniff.
    // Content sniffing on markdown is wrong in a way that reaches the agent:
    // the detectors were written for TERMINAL output. `[text](url)` reads as an
    // INI `[section]` header (theguardian.com -> `api.rest.ini`) and `##`/`###`
    // headings read as multi-section shell-chain banners
    // (docs.python.org/3/glossary -> `api.rest.chain.txt`). The label names the
    // stash FILE, so it tells the next reader how to parse a document it never
    // saw.
    //
    // A JSON outline keeps sniffing — `api.rest.json` is the honest label for
    // it. And when the body carried no tags the conversion was a NO-OP and `md`
    // is the original bytes, so claiming markdown for it would replace a correct
    // label with a markdown outline of raw JSON. Hence: evidence the distiller
    // actually converted something.
    // **The question is asked of the PAGE, not of the cut — and by REFERENCE.**
    //
    // `d.md` is the view after the hard ceiling: on a large page, it is a
    // sample of lines that keeps the headings and drops the body. Asking the
    // question of it means asking what is left AFTER the answer was thrown
    // away. Measured on the Claude Code hooks page (2.8 MB of HTML, 233 795
    // characters of markdown) with `--prompt updatedToolOutput`: the term
    // appears SEVEN times in the page's markdown and ZERO times in the clipped
    // view, so the extractor found no anchor and delivered « nothing in this
    // source answers that question » followed by the whole outline. `full_md`
    // carries precisely the pre-cut markdown, for the stash; it also carries
    // the answer.
    //
    // The extraction is done HERE, before the stash consumes the markdown: it
    // is a pure computation, and the ordering avoids one page copy per page
    // read.
    let asked = prompt.as_deref().is_some_and(|p| !p.trim().is_empty());
    let scoped = prompt
        .as_deref()
        .filter(|p| !p.trim().is_empty())
        .and_then(|q| scope_extract(d.full_md.as_deref().unwrap_or(&d.md), q));
    let distilled_markdown =
        !matches!(d.source, tokenade_core::web_pipeline::Source::JsonOutline) && d.md != html;
    let stash_hint = if distilled_markdown {
        tokenade_core::disclose::SourceHint::web_with_content_type("text/markdown")
    } else {
        tokenade_core::disclose::SourceHint::web()
    };
    // Don't drop the tail silently when the INPUT cap cut the page. Convert the
    // FULL page once and stash it: the agent gets a structural head plus an
    // `expand-ref <hash>` hint, instead of a blind cut that loses everything
    // past the cap. Pages big enough to truncate but under the stash threshold
    // are shown in full.
    let delivered = if d.truncated_input {
        // The SAME routing the distiller above used. Converting the full page
        // with `html_to_markdown` directly re-introduced, on this branch alone,
        // the plain-text bug the distiller had just been taught to avoid.
        tokenade_core::disclose::maybe_stash(
            web::text_body_to_markdown(&html, content_type.as_deref()),
            stash_hint,
        )
    } else {
        // `maybe_stash` unconditionally, NOT only on the truncated branch:
        // `truncated_input` is a decision about the INPUT cap, not about the
        // size of what we are about to deliver. When the distiller does not
        // reduce, `md` is as large as the page — measured on a real npm registry
        // response (6.8 MB of minified JSON), where the conversion was a no-op,
        // nothing was truncated, and all 6.8 MB shipped with no stash pointer
        // and no way back.
        // **The stash holds the PAGE, not the cut.** When the hard ceiling
        // bit, `d.md` is a sampled view — one line in N — and stashing it gives
        // the agent an `expand-ref` to the cut. Same move as the
        // `truncated_input` branch just above, for the OUTPUT cut instead of
        // the input one. MEASURED 2026-09-10: `zhwiki` only named a 24 695-byte
        // stash while the full text, 277 KB, was already written to the cache
        // without being named.
        // **The stash holds the PAGE, the delivered outline comes from the
        // VIEW.**
        //
        // The two halves are chosen separately, and the measurement says why.
        // Stashing the page AND asking it for its outline gives **50 headings
        // in 4 637 characters**; the outline of the clipped view gives **59 in
        // 2 783** — richer and shorter at once, because that view was made
        // heading-dense on purpose. And delivering the view itself, as the
        // first version did, costs **18 523 characters**: six times the
        // outline, for the very page the product exists not to dump.
        // …but only if that outline is a MAP. On a page without headings, the
        // clipped view's outline fits on one line and the delivery drops to
        // nothing: the sample is then the best answer.
        match d.full_md {
            Some(full) => {
                let plan = tokenade_core::markdown::outline(&d.md);
                let body = if tokenade_core::markdown::outline_maps_the_page(&plan) {
                    plan
                } else {
                    d.md
                };
                tokenade_core::disclose::maybe_stash_with_body(full, stash_hint, body)
            }
            None => tokenade_core::disclose::maybe_stash(d.md, stash_hint),
        }
    };
    // A bound the caller ASKED for and cannot otherwise observe. It cuts the
    // parse — measured on a 133 637-byte page, `--max-tokens 50` takes the
    // distiller from 121 943 bytes of markdown to 93 — and then the branch
    // above deliberately converts the WHOLE page anyway so the tail is not
    // lost. The effect is real and invisible: identical stdout, empty stderr,
    // and a usage line that named the flag without saying what it bounds. An
    // agent setting it to shrink the reply had no way to learn otherwise.
    if d.truncated_input && max_input_tokens > 0 {
        eprintln!(
            "[tokenade] --max-tokens {max_input_tokens} bounded the parse, not the reply: \
             the page is still converted whole so its tail is not lost. Ask a question with \
             `--prompt \"…\"` to get only the passages you need."
        );
    }
    // `--prompt` on the page itself. It was parsed, advertised in the usage
    // line, handed to `serve_document` — and that route only answers for PDFs,
    // docx and friends; HTML fell straight past it. Measured on a 133 637-byte
    // page: the reply is byte-identical with and without a question, 4 714
    // bytes either way, while `read --prompt` on the same content returns a
    // 2 313-byte scoped extract. The batch route already says "--prompt
    // applies to a single target; ignoring it for a batch" — naming a contract
    // the single-target route did not keep.
    // **Only a question that was asked can go unanswered.**
    //
    // `attach_scoped(delivered, None)` does not tell « no question » from
    // « question asked, nothing anchored »: the latter deserves the note, the
    // former receives it wrongly. Measured on the 27 corpus pages when the
    // extraction was split in two: **27 pages out of 27** received « nothing
    // in this source answers that question » without any question being
    // asked, 108 characters each. So the question is asked again here.
    let delivered = if asked {
        attach_scoped(delivered, scoped)
    } else {
        delivered
    };
    // Already delivered, unchanged, in this session? Measured on a real ledger,
    // 12.8 % of everything this command delivers is a page it delivered before
    // — the same URL fetched three or four times while the first copy was still
    // in the agent's context. Lossless by construction: the page was fetched
    // and converted as usual, and only a body IDENTICAL to the previous one is
    // replaced by the pointer. See `web_replay` for the freshness and
    // compaction guards.
    let replay_url = path_arg.filter(|_| is_url).unwrap_or("");
    if let Some((note, saved)) = replay_fold_or_remember(replay_url, &delivered) {
        // A page: one row, credited on the pointer delivered. A fixed credit
        // (feed, product facts) keeps its row and the replay its own.
        match basis {
            tokenade_core::web_credit::WebBasis::Flat(s) => {
                log_gain("web", op, s);
                log_gain("web", "web-replay", saved);
            }
            b => log_gain(
                "web",
                "web-replay",
                tokenade_core::web_credit::credit_on_delivery(
                    b,
                    tokenade_core::tokenizer::estimate(&note),
                ),
            ),
        }
        println!(
            "{}",
            crate::hooks::webfetch_pretool::attach_provenance(&provenance, &note)
        );
        return 0;
    }
    let delivered = if delivered.trim().is_empty() {
        empty_page_note(path_arg.unwrap_or("this source"))
    } else {
        delivered
    };
    log_gain(
        "web",
        op,
        tokenade_core::web_credit::credit_on_delivery(
            basis,
            tokenade_core::tokenizer::estimate(&delivered),
        ),
    );
    println!(
        "{}",
        crate::hooks::webfetch_pretool::attach_provenance(&provenance, &delivered)
    );
    0
}

#[cfg(test)]
mod batch_accounting_tests {
    use super::batching_cache_saved;

    #[test]
    fn cache_saved_matches_the_sequential_rebill_model() {
        // 3 results of sizes 100, 50, 20 in emission order. Done one-at-a-time,
        // result 0 rides along in the 2 later turns, result 1 in the 1 later
        // turn, result 2 in none: 100*2 + 50*1 + 20*0 = 250 cache_read tokens.
        assert_eq!(batching_cache_saved(&[100, 50, 20]), 250);
        // A single target saves nothing (there is no "later turn" to avoid).
        assert_eq!(batching_cache_saved(&[999]), 0);
        assert_eq!(batching_cache_saved(&[]), 0);
        // Two equal results: s*1 + s*0 = s.
        assert_eq!(batching_cache_saved(&[40, 40]), 40);
    }
}
