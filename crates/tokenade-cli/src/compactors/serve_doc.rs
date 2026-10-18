//! **Serve a DOCUMENT, and say what was not answered.**
//!
//! Reading a file with a question (`--prompt`) is the product's cheapest path
//! in tokens: only the parts that answer come out. The price of that deal is
//! a promise — what was NOT answered must be announced, otherwise an absence
//! reads as a negative answer.

use super::*;

/// Smallest served text that earns the "this is the source, not an answer"
/// note.
///
/// The note is ~110 characters and exists to stop a fold being mistaken for an
/// answer. On a document short enough to be handed over whole it buys almost
/// nothing and costs proportionally a lot — measured on a 229-character source
/// where it made the reply 47 % LARGER than the plain read, inverting the one
/// promise the command makes. Same reasoning as the skeleton footer's
/// minimum: a note has to amortise its own size.
pub(super) const UNANSWERED_NOTE_MIN_CHARS: usize = 900;

/// The note itself, in ONE place: it is emitted before the delivery bound and
/// checked again after it, and two spellings would let the check pass on a
/// note that no longer matches.
pub(super) const UNANSWERED_NOTE: &str =
    "[tokenade] nothing in this source answers that question — what follows is the source itself, not an answer.";

/// `tokenade crawl <url> [--max N]` — discover same-domain URLs from a start
/// page (page anchors + `/sitemap.xml`). Net-new; distinct from the code-intel
/// `map`. Prints one URL per line.
pub(crate) fn cmd_crawl(rest: &[String]) -> i32 {
    let mut url: Option<&str> = None;
    let mut max: usize = 200;
    let mut iter = rest.iter();
    while let Some(a) = iter.next() {
        match a.as_str() {
            "--max" | "--limit" => match iter.next().and_then(|v| v.parse::<usize>().ok()) {
                Some(n) => max = n,
                None => {
                    eprintln!("tokenade crawl: --max expects an integer");
                    return 2;
                }
            },
            other if url.is_none() => url = Some(other),
            other => {
                eprintln!("tokenade crawl: unexpected argument '{other}'");
                return 2;
            }
        }
    }
    let Some(url) = url else {
        eprintln!("usage: tokenade crawl <url> [--max N]");
        return 2;
    };
    let urls = tokenade_core::web_map::discover_urls(url, max);
    if urls.is_empty() {
        eprintln!("tokenade crawl: no URLs discovered for {url}");
        return 1;
    }
    println!("# {} URLs discovered from {url}", urls.len());
    for u in &urls {
        println!("{u}");
    }
    0
}

/// A DOCUMENT rather than a page — pdf, docx, xlsx, pptx, odt, epub …
///
/// `Ok(rc)` = answered. `Err(bytes)` hands the buffer back untouched so the
/// caller can keep reading it as markup; returning it beats cloning a
/// multi-megabyte document to satisfy ownership.
///
/// Signature first, then look INSIDE the zip. A sniffer keys off the first
/// zip entry, whose order no spec fixes: a LibreOffice .docx sniffs as plain
/// "zip" and would otherwise reach the HTML path below and print raw
/// compressed bytes at the agent.
pub(super) fn serve_document(
    bytes: Vec<u8>,
    detected: Option<&str>,
    path_arg: Option<&str>,
    prompt: Option<&String>,
    provenance: &str,
) -> Result<i32, Vec<u8>> {
    let Some(fmt) = detected
        .and_then(|ext| tokenade_core::docs::detect_format(Some(&format!("x.{ext}")), &bytes))
        .or_else(|| tokenade_core::docs::detect_format(path_arg, &bytes))
    else {
        return Err(bytes);
    };
    let (text, sav) = tokenade_core::media::extract_document_with_savings(&bytes, fmt);
    // Redact the extract at the SOURCE, before any downstream path can emit
    // it. A document can hold secrets (an .env pasted into a wiki export, an
    // AWS key in a runbook PDF), and the scoped-extract path
    // (`prompt_scoped_extract_dispatch`) returns raw matching lines with no
    // redaction of its own — so `--prompt "aws credentials"` on a PDF
    // holding `AWS_SECRET_ACCESS_KEY=…` printed it verbatim, while the same
    // read WITHOUT a prompt (which stashes) redacted it. Redacting here
    // closes that asymmetry for every branch at once (skeleton, scoped
    // extract, stash); `redact` is idempotent, so the paths that already
    // redact are unaffected. `cmd_auto` does the same at its own source.
    let text = tokenade_core::redact::redact_for_model(&text);
    if !text.trim().is_empty() {
        // BOUND the extract. Printing it whole was unbounded, and real
        // documents are not small: a 5.8 MB spreadsheet (2574 rows, 23k
        // cells of post text) extracted to 27.6 MB — 6.9 MILLION tokens,
        // i.e. many times any context window, and 4.7x MORE than the 1.45M
        // an agent would have paid just cat-ing the raw bytes. We printed
        // that and logged it as a saving.
        //
        // `maybe_stash` is the mechanism the rest of the product already
        // uses for exactly this: under the threshold it returns the text
        // untouched (a 37-token document is unaffected), over it the full
        // extract goes to the stash and the agent gets a summary plus an
        // `expand-ref` handle — so nothing is lost, it is just not billed
        // up front. Extraction stays faithful; only DELIVERY is bounded.
        //
        // SPREADSHEETS get a skeleton, not retrieval. Measured: asked
        // "which people have a job title mentioning coach", prompt-scoped
        // extraction returned 5 of 296 matching rows — RAG answers "here are
        // passages that resemble your question", a table question is "give
        // me EVERY row where", and no 1500-token budget returns 296 rows.
        // The map + a script beats an approximation of the data: exact
        // answer, and the 6.9M-token file never enters the context.
        let is_sheet = matches!(
            fmt,
            tokenade_core::docs::Format::Xlsx | tokenade_core::docs::Format::Ods
        );
        if is_sheet && sav.tokens_after > tokenade_core::disclose::threshold_tokens() {
            // Stash FIRST and keep the handle. The map alone is a dead end:
            // it tells the agent where the data is and gives it no way to
            // reach it, so it re-reads the file. `stash_for_reference`
            // returns the hash and path the recipe is built from — the
            // earlier `let _ = maybe_stash(..)` threw exactly that away.
            // Hint `.md`, do not let the extension be sniffed from content.
            // What we stash is the markdown table this module rendered; a
            // sheet holding JSON blobs sniffs as JSON and the stash lands as
            // `<hash>.api.rest.json` — observed. An agent that trusts that
            // name calls `json.load` on a markdown table and gets a parse
            // error (same bug class, other channel).
            let hint =
                std::path::PathBuf::from(format!("{}.md", path_arg.unwrap_or("spreadsheet")));
            let data = tokenade_core::disclose::stash_for_reference(
                &text,
                &tokenade_core::disclose::SourceHint::File(hint),
            );
            let data_ref = data
                .as_ref()
                .map(|(h, p)| (h.as_str(), p.to_string_lossy().into_owned()));
            if let Some(skel) = tokenade_core::docs::spreadsheet_skeleton(
                &bytes,
                fmt,
                path_arg.unwrap_or("<file>"),
                data_ref.as_ref().map(|(h, p)| (*h, p.as_str())),
            ) {
                log_gain(
                    "web",
                    &format!("skeleton-{}", docs_format_label(fmt)),
                    credit_binary_for(fmt, tokenade_core::tokenizer::estimate(&skel)),
                );
                println!(
                    "{}",
                    crate::hooks::webfetch_pretool::attach_provenance(provenance, &skel)
                );
                return Ok(0);
            }
        }
        // A QUESTION changes everything, so try it first. A web page is a
        // document: WebFetch carries the agent's `prompt` and serves a
        // prompt-scoped extract rather than the whole page, and that same
        // retrieval (dense signal + flat line packing) won its bench. The
        // engine takes any text, so a document off disk gets it too — this
        // is the same call the WebFetch hook makes.
        let delivered = prompt
            .filter(|p| !p.trim().is_empty())
            .and_then(|p| {
                nudge_batch_questions_if_repeated(
                    path_arg.unwrap_or_default(),
                    p,
                );
                let budget = tokenade_core::disclose::webfetch_scoped_budget_for(
                    sav.tokens_after,
                );
                // A sheet is searched row for row, not through its profile.
                let can_hold_sheets = matches!(
                    fmt,
                    tokenade_core::docs::Format::Xlsx
                        | tokenade_core::docs::Format::Ods
                        | tokenade_core::docs::Format::Xls
                        | tokenade_core::docs::Format::FlatOdf
                );
                let searched = if can_hold_sheets {
                    let rows = tokenade_core::docs::with_verbatim_tables(|| {
                        tokenade_core::media::extract_document_with_savings(&bytes, fmt).0
                    });
                    std::borrow::Cow::Owned(tokenade_core::redact::redact_for_model(&rows))
                } else {
                    std::borrow::Cow::Borrowed(text.as_str())
                };
                tokenade_core::gain::prompt_scoped_extract_asked(&searched, p, budget, "read")
            })
            .unwrap_or_else(|| {
                let (bounded, bound_bit) = tokenade_core::disclose::maybe_stash_reporting(
                    text,
                    tokenade_core::disclose::SourceHint::web(),
                );
                // Two different situations reach this fallback and they
                // need opposite words. A question that found nothing must
                // NOT be told to "re-run with a question" — that advice is
                // for work already done, and worse, it lets the passages
                // below read as the answer they are not.
                if prompt.is_some_and(|p| !p.trim().is_empty())
                    && bounded.len() >= UNANSWERED_NOTE_MIN_CHARS
                {
                    return format!("{UNANSWERED_NOTE}\n{bounded}");
                }
                // No question → we cannot retrieve, only truncate. Say so:
                // an agent that knows it can ASK gets a targeted answer for
                // ~1.5k tokens instead of paging a summary of a document it
                // never needed whole. Only when the bound actually bit —
                // a small document is served entire and needs no advice.
                // On the FACT reported by the stash, not on a search of the
                // document. A document that quotes this tool's output — a
                // patchnote, a bug report, a transcript — carries `[disclose:`
                // in its own text, and a short one was then told it was "a
                // fold, not the document" and advised to re-run with a
                // question it did not need.
                if bound_bit {
                    format!(
                        "{bounded}\n[tokenade] This document is large, so the above is a \
                         fold, not the document. If you are looking for something specific, \
                         re-run with a question and get just the relevant passages: \
                         `tokenade read <file> --prompt \"<what you need>\"` — ask several \
                         at once with commas, `--prompt \"q1, q2\"` — the CHEAPEST option in tokens spent, \
                         each answered and labelled, one round-trip instead of N."
                    )
                } else {
                    bounded
                }
            });
        log_gain(
            "web",
            &format!("extract-{}", docs_format_label(fmt)),
            credit_binary_for(
                fmt,
                sav.tokens_after
                    .min(tokenade_core::tokenizer::estimate(&delivered)),
            ),
        );
        println!(
            "{}",
            crate::hooks::webfetch_pretool::attach_provenance(provenance, &delivered)
        );
        return Ok(0);
    }
    // Extractable format but NO text layer — a scanned / image-only /
    // encrypted PDF, etc. (pdf-extract and poppler both return empty
    // here; only OCR/vision recovers it). Running html_to_markdown on
    // the raw bytes would just vomit binary, so stash the bytes and
    // emit a metadata-only hint instead.
    let summary = tokenade_core::disclose::maybe_stash_bytes(
        bytes,
        tokenade_core::disclose::SourceHint::web(),
    );
    println!(
        "{}",
        crate::hooks::webfetch_pretool::attach_provenance(provenance, &summary)
    );
    Ok(0)
}

/// Lifted out of the error branch so `--help` and a bad invocation cannot
/// drift apart — and so there is ONE place to keep honest.
pub(super) const WEB_USAGE: &str = "usage: tokenade web <url|file>… [--prompt \"…\"] [--max-tokens N]\n  \
  --max-tokens N bounds the INPUT that gets parsed, not the reply: the page is\n  \
  still converted whole so its tail is not lost, so the answer is the same size\n  \
  with it as without. To bound the REPLY, ask a question with --prompt.\n  \
     compact HTML page → markdown (real web content)\n  \
     from url:      `tokenade web https://example.com`\n  \
     SEVERAL:       `tokenade web https://a.com https://b.com` (fetched concurrently — one round-trip, not N)\n  \
     pipe input:    `curl -s https://example.com | tokenade web -`\n  \
     from file:     `tokenade web saved.html`";

/// What a distilled page books, and under what op name: the SINGLE declarer,
/// shared by the one-target path and by every page of a batch.
///
/// Reader-grade output books the flat reliability credit, not the web model: a
/// rendered feed compacted nothing the agent would otherwise have carried, and
/// crediting the payload would invent a saving. Same rule as
/// `readers::reader_savings` — one assiette, decided in one place.
///
/// It lived inline in the single-target path alone, and the batch path could
/// not reach it: it booked one flat document turn tax for the whole batch
/// instead. Two spellings of "what is a page worth" is one too many, and the
/// second one was never written.
pub(super) fn book_distilled_page(
    d: &tokenade_core::web_pipeline::Distilled,
) -> (&'static str, tokenade_core::web_credit::WebBasis) {
    use tokenade_core::web_pipeline::Source;
    let op = match d.source {
        Source::JsonOutline => "json-to-markdown",
        Source::Feed | Source::FeedDiscovered(_) => "feed",
        Source::ProductFacts => "product-facts",
        Source::JsHarvest => "js-harvest",
        Source::Distilled if d.truncated_input => "html-to-markdown(truncated)",
        Source::Distilled => "html-to-markdown",
    };
    // A basis, not a figure: the credit is taken on what is finally delivered.
    let basis = if d.source.is_reader_grade() {
        tokenade_core::web_credit::WebBasis::Flat(
            tokenade_core::coefficients::reader_credit_savings(d.savings.tokens_after),
        )
    } else {
        tokenade_core::web_credit::WebBasis::Page {
            raw: d.savings.tokens_before,
        }
    };
    (op, basis)
}

/// One wording for a parameter that no longer exists, in both its spellings.
///
/// Says what the command does INSTEAD, which is the only thing the caller
/// needs: `web` reads the page it is given and recognises a search-results
/// page on its own, so the parameter has no replacement to reach for — it has
/// nothing to do at all. A refusal is the one message that cannot say too
/// much, because it is the door it closes.
pub(super) fn kind_is_retired(value: &str) -> String {
    format!(
        "tokenade web: `kind={value}` was an MCP tool parameter and no longer exists. \
         Drop it — `web` tells a search-results page from an article by itself: \
         `tokenade web <url|file>…`"
    )
}

/// What `tokenade web` answers when the page carried no readable text.
///
/// Measured 2026-09-04 on five shapes — an empty body, a whitespace-only one,
/// one holding a comment, one holding only empty tags, one holding only a
/// `<style>` — `tokenade web <file>` answered with a single newline, exit 0,
/// and nothing on stderr. One byte is indistinguishable from a crash, a
/// truncated fetch, and a page that really is blank, and the agent has no way
/// to tell which it got. The BATCH route already refuses the empty one by name
/// (`the body decoded to no readable text`), so the two spellings of the same
/// command disagreed about whether this is worth saying.
///
/// The document path settled the same question years earlier — a `.pdf` that
/// yields no text says so rather than looking unsupported — and this is that
/// rule, on the other content entry point.
pub(crate) fn empty_page_note(source: &str) -> String {
    format!(
        "[tokenade: {source} carried no readable text — an empty body, or nothing outside \
         script/style/markup. That is the whole answer, not a truncation.]"
    )
}

/// Answer a `--prompt` against one fetched page, keeping its way back.
///
/// Extracted 2026-09-06 so the BATCH can keep the same contract. It printed
/// "--prompt applies to a single target; ignoring it for a batch" and threw the
/// question away: measured on two local pages, a prompted single fetch answered
/// in 1 038 bytes and the same question over the pair returned 9 644 — the
/// caller paid 9.3x and did the finding themselves, on the call shape this
/// product's own guidance tells agents to prefer.
///
/// Split in TWO on 2026-09-14, and the reason is memory. The stash CONSUMES
/// the page's markdown, so answering after it forced the caller to keep a
/// copy — 233 794 bytes on the hooks page, one per page of a batch. The
/// extraction is a pure computation: it happens BEFORE, by reference, and
/// only the answer (a few kilobytes) waits for the stash. The contract does
/// not change: the answer still carries the pointer.
pub(crate) fn scope_extract(page_markdown: &str, q: &str) -> Option<String> {
    let budget = tokenade_core::disclose::webfetch_scoped_budget_for(
        tokenade_core::tokenizer::estimate(page_markdown),
    );
    tokenade_core::gain::prompt_scoped_extract_asked(page_markdown, q, budget, "web")
}

/// Glues the answer from [`scope_extract`] back onto the delivery that carries
/// the pointer.
pub(crate) fn attach_scoped(delivered: String, scoped: Option<String>) -> String {
    match scoped {
        // Carry the stash pointer over. The scoped extract REPLACES a body that
        // was already folded and already had its way back written on it, so
        // keeping only the answer is exactly the defect of a fold that names no
        // way back.
        Some(scoped) => match tokenade_core::disclose::stash_pointer_line(&delivered) {
            // Anti-duplicate on THIS pointer, not on the marker: a page is free
            // to quote a `[stashed: …]` line of ours in its own body, and a
            // stale hash reads exactly like a live one.
            Some(p) if !scoped.contains(p) => format!("{scoped}\n{p}"),
            _ => scoped,
        },
        // Nothing anchored. Serve the page, and say that it is the page — an
        // outline of section headings reads like a chosen answer otherwise.
        None if delivered.len() >= UNANSWERED_NOTE_MIN_CHARS
            && !delivered.contains(UNANSWERED_NOTE) =>
        {
            format!("{UNANSWERED_NOTE}\n{delivered}")
        }
        None => delivered,
    }
}

/// A page this session already delivered, unchanged, replaced by a pointer to
/// it — or, when it is new, remembered so the NEXT fetch can fold it.
///
/// One spelling for both routes, because this lived on the single-target path
/// alone. `tokenade web u1 u2` neither folded a page it had already delivered
/// nor recorded the ones it delivered, and the second half poisoned the single
/// route too: after a batch, fetching the same URL on its own came back whole.
/// Measured 2026-09-06 on one page — 9 259 bytes then 526 through the single
/// route, 18 601 then 18 601 through the batch — on the call shape this
/// product's own guidance tells agents to prefer ("several URLs in ONE call").
/// Third time these two routes have differed on something the batch should
/// have had (the size gate, then `--prompt`, now this).
///
/// Local paths are excluded exactly as the single path excluded them: replay
/// keys on a URL, and a file on disk is re-read for free.
///
/// Returns `Some((note, saved))` when the body was replaced, `None` when it was
/// new and is now remembered.
///
/// KNOWN BOUND, stated rather than left to be rediscovered: what each route
/// hands in differs by the provenance prefix — the batch bakes it into the
/// rendered markdown, the single path attaches it at print time. Identical
/// bodies and only identical bodies fold, so a page whose fetch produced
/// provenance notes (a redirect, a non-2xx) folds within its own route but not
/// across the two. Measured on an ordinary 200, where provenance is empty, the
/// cross-route fold works: 18 601 bytes for a pair, then 235 for one of them
/// fetched alone.
pub(crate) fn replay_fold_or_remember(
    target: &str,
    delivered: &str,
) -> Option<(String, tokenade_core::Savings)> {
    if !(target.starts_with("http://") || target.starts_with("https://")) {
        return None;
    }
    let session = tokenade_core::session::session_key().unwrap_or_default();
    if let Some(note) =
        tokenade_core::web_replay::fold_if_already_delivered(&session, target, delivered)
    {
        // Book what was actually avoided, not the whole page: the note itself
        // is delivered and costs its own tokens.
        let before = tokenade_core::tokenizer::estimate(delivered);
        let after = tokenade_core::tokenizer::estimate(&note);
        return Some((note, tokenade_core::Savings::new(before, after)));
    }
    tokenade_core::web_replay::remember(&session, target, delivered);
    None
}
