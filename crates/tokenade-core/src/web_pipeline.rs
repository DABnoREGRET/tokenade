//! The ONE content pipeline: fetched bytes in, markdown out.
//!
//! What this module does NOT do is as important as what it does. It makes no
//! delivery decision, attaches no provenance, books no savings and prints
//! nothing. Those differ legitimately between the hook (a PreToolUse verdict on
//! stdout) and `tokenade web` (a stream on stdout), and forcing them together
//! would be the same mistake in the other direction.
//!
//! What it does own is every decision about WHAT the page says — and those had
//! silently diverged. Before this module, the hook had five content
//! capabilities `tokenade web` did not:
//!
//!   * feeds recognised by CONTENT (not URL shape);
//!   * feed DISCOVERY when a site index distils to nothing;
//!   * product facts prepended (price/availability the distiller drops as
//!     chrome — allbirds distils 213 735 tokens to 630, none of it the price);
//!   * the 50 k-token hard cap;
//!   * the server content-type feeding the distiller.
//!
//! and `tokenade web` had one the hook did not (an input cap). None of that was
//! a decision. Anything added here reaches both channels or neither.

use crate::Savings;

/// How the markdown was produced. The caller needs this because it decides the
/// savings assiette and the delivery policy — reader-grade output has already
/// beaten what a native fetch would have seen, so it should not then be
/// second-guessed by a gate designed for ordinary HTML.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Generic HTML/text extraction.
    Distilled,
    /// A JSON body rendered as an outline.
    JsonOutline,
    /// The page was a feed, recognised by its content type or root element.
    Feed,
    /// The page distilled to nothing and declared a feed; that feed was served
    /// instead. Carries the feed's URL, because the delivery MUST say that
    /// these are the site's latest items and not the requested page.
    FeedDiscovered(String),
    /// Structured product facts, prepended to the ordinary extraction.
    ProductFacts,
    /// Inline scripts were executed and their text harvested.
    JsHarvest,
}

impl Source {
    /// Has this output already beaten what the agent's own fetcher would have
    /// seen? Then serve it — do not run it past a gate whose job is to decide
    /// whether ordinary HTML is worth intercepting. A correctly rendered
    /// 30-entry Atom feed was declined by exactly that gate (MEASURED on
    /// simonwillison.net) before the distinction existed.
    pub fn is_reader_grade(&self) -> bool {
        matches!(
            self,
            Source::Feed | Source::FeedDiscovered(_) | Source::ProductFacts
        )
    }
}

/// The pipeline's output.
pub struct Distilled {
    /// Markdown, already redacted. NO provenance, NO banner — the channel adds
    /// those, and adds them BEFORE it books anything.
    pub md: String,
    pub savings: Savings,
    pub source: Source,
    /// The INPUT was cut before extraction because it exceeded the caller's
    /// `max_input_tokens`. The caller may want to stash the full rendering and
    /// point at it; `md` alone cannot say "there was more".
    pub truncated_input: bool,
    /// The markdown BEFORE the hard cap, when the cap has bitten.
    ///
    /// The counterpart of `truncated_input` for the OUTPUT cut. `md` is then a
    /// sampled view — one line in N — and delivering a recovery pointer to
    /// that view means stashing the cut rather than the page.
    ///
    /// MEASURED 2026-09-10: `pages/zhwiki` only named a 24,695-byte stash, the
    /// clipped view, while the full text — 277 KB — was written to the cache
    /// on every render without anything naming it. An earlier change already
    /// put it there and pointed to it from the FOLD FOOTER, which never
    /// reaches the reader of a large page: `disclose` replaces the body with
    /// an outline and takes the footer along. The stash is content-addressed,
    /// so stashing twice writes only one file; what was missing was for the
    /// DELIVERY to name it.
    pub full_md: Option<String>,
}

/// Per-call knobs. Everything else about the pipeline is not a per-call
/// decision — a page must not distil differently depending on who asked.
#[derive(Default, Clone, Copy)]
pub struct PageOpts {
    /// Cut the HTML at this many tokens BEFORE extraction. 0 = no cap.
    /// `tokenade web --max-tokens` is the only caller that sets it; the hook
    /// bounds the OUTPUT instead (see `HARD_CAP_TOKENS`), and both bounds now
    /// exist on both channels.
    pub max_input_tokens: usize,
}

// Markdown beyond this is truncated head/tail. Mirrors the Read+Bash caps: a
// 10 MB article page can still produce 1 MB of markdown after extraction, and
// the agent's context cannot survive that.

/// One extra fetch, only for feed discovery, only on a page that already
/// distilled to nothing.
const FEED_FETCH_CAP: usize = 4 * 1024 * 1024;

/// Recover folded page lines without advertising `--raw`.
///
/// `--raw` is NOT here. `disclose::recovery_menu` and webfetch recover
/// already stopped promoting it (107/111 dumps nus ; rounds 2026-09-23 /
/// 2026-09-23-2). Pasting `--raw` on both fold-footer arms (markdown stash
/// and HTML fallback) sent the model back to the dump. It stays in `--help`
/// and the `--prompt` miss refusal.
fn page_fold_recovery(hash: &str, path: &std::path::Path, full_md: bool) -> String {
    let where_ = if full_md {
        "the FULL distilled page"
    } else {
        "the page as fetched"
    };
    format!(
        "every folded line is in {where_}: ASK it → \
         `tokenade expand-ref {hash} --prompt \"q1, q2\"` ({})",
        path.display()
    )
}

/// Turn a decoded response body into deliverable markdown.
///
/// `url` is the page's own URL — used for JS-render provenance and to decide
/// whether feed discovery is even appropriate. `content_type` is the server's,
/// normalized or raw; it is the authoritative signal for feeds and for the
/// distiller's own format choice, and passing `None` merely costs accuracy.
pub fn distill_page(body: &str, url: &str, content_type: Option<&str>) -> Distilled {
    distill_page_with(body, url, content_type, PageOpts::default())
}

/// As [`distill_page`], with the caller's knobs.
pub fn distill_page_with(
    body: &str,
    url: &str,
    content_type: Option<&str>,
    opts: PageOpts,
) -> Distilled {
    let ct = crate::text_encoding::normalized_content_type(content_type);
    let ct = ct.as_deref();

    // Cut the INPUT before any of it is parsed, when the caller asked for a
    // bound. Savings are still reported against the FULL input below: the cap
    // is a memory bound, not a discount, and booking against the trimmed body
    // would credit us for bytes we chose never to look at.
    let full_tokens = crate::tokenizer::estimate(body);
    let body = if opts.max_input_tokens > 0 {
        crate::web::truncate_to_max_tokens(body, opts.max_input_tokens)
    } else {
        body
    };
    // Only a real truncation can change the count, and `truncate_to_max_tokens`
    // is a no-op unless a bound was asked for — so without one this second full
    // tokenisation of the SAME bytes was pure cost.
    let body_tokens = if opts.max_input_tokens > 0 {
        crate::tokenizer::estimate(body)
    } else {
        full_tokens
    };
    let truncated_input = body_tokens < full_tokens;

    // Feeds are recognised by CONTENT, not by URL shape. `site_readers::plan`
    // matches the conventional suffixes (`.rss`, `/feed`), which misses every
    // feed served from a plain path or a query string — and the authoritative
    // signals, the content type and the root element, only exist once the bytes
    // are in hand. `render` returns None on anything that is not really a feed,
    // so a mislabelled page just continues down the generic path.
    if crate::web::looks_like_feed(body, ct) {
        if let Some(md) =
            crate::site_readers::render(&crate::site_readers::Kind::FeedDocument, body)
        {
            let md = redact(&format!("> source: feed\n\n{md}"));
            return finish(md, full_tokens, Source::Feed, truncated_input, None, url);
        }
    }

    let (mut md, mut sav) = crate::web::distill_text_with_savings_from(body, ct, body_tokens);
    // **Relative references finally say which host they live under.** This
    // is where the page URL is known; `html_to_markdown`, which writes the
    // table, does not receive it. See `note_the_origin_of_relative_references`.
    // `<base href>` wins over the page address: that is the HTML rule, and
    // ignoring it would announce an origin the document contradicts.
    let base = crate::web_tree::declared_base_href(body, url);
    md =
        crate::web_tree::note_the_origin_of_relative_references(md, base.as_deref().unwrap_or(url));
    // When there is almost nothing to render, say what the page says about
    // ITSELF: the frame where the document lives is a FACT, the three causes
    // listed by the diagnostic are hypotheses.
    if md.contains(crate::web_tree::THIN_RESULT_MARK) {
        if let Some(frame) = crate::web_tree::declared_frame_url(body, url) {
            md = md.replacen(
                crate::web_tree::THIN_RESULT_MARK,
                &format!(
                    "{} the document may be in a FRAME — fetch it directly: {frame} —",
                    crate::web_tree::THIN_RESULT_MARK
                ),
                1,
            );
        }
    } else if nothing_but_a_title(&md) {
        // A FRAMESET page does not trigger the "thin page" diagnostic: it
        // renders its title, so it renders something. And yet it has nothing
        // else by construction — its document is elsewhere. Without this
        // branch, `web` on an old portal returns one line and stays silent on
        // the only useful thing the page declares.
        // ALL the frames, not the first: a `<frameset cols="20%,80%">` opens
        // with its MENU, and naming that one alone sends the reader into the
        // navigation. There are two or three, the list fits on one line.
        let frames = crate::web_tree::declared_frame_urls(body, url);
        if !frames.is_empty() {
            md.push_str(&format!(
                "\n[tokenade: the document is in a FRAME — fetch it directly: {}]\n",
                frames.join(" · ")
            ));
        }
    }
    let mut source = if body_took_the_json_outline(body, ct) {
        Source::JsonOutline
    } else {
        Source::Distilled
    };

    // `distill_text` sends JSON to `json_outline`, which on a uniform array
    // emits the item count and the fields of the FIRST record only. Everything
    // downstream then labels that "ALL text preserved", which is false: 99 of
    // 100 records are gone and nothing on this path holds the bytes that had
    // them. Stash the RAW body and point at it, so the outline stays the cheap
    // inline view and every dropped record is still reachable.
    if sav.tokens_before > sav.tokens_after && source == Source::JsonOutline {
        if let Some((hash, path)) =
            crate::disclose::stash_for_reference(body, &crate::disclose::SourceHint::web())
        {
            md.push_str(&format!(
                "\n[full JSON stashed: {} · ~{} tokens · the outline above gives the SHAPE \
                 and the FIRST record only — ASK it what you need → `tokenade expand-ref \
                 {hash} --prompt \"q1, q2\"`, or `jq` the stashed file directly]\n",
                path.display(),
                sav.tokens_before,
            ));
            sav = Savings::new(sav.tokens_before, crate::tokenizer::estimate(&md));
        }
    }

    // **The hard cap SAMPLES a page, and stashed nothing.**
    //
    // `filter::generic` keeps one line every N and puts a marker between
    // each: that is the right answer to a 400,000-line command output, and a
    // wrong answer to an ARTICLE. MEASURED 2026-09-10 on `pages/zhwiki`: **53
    // headings reduced to 6**, and a paragraph fidelity of **0.039** over
    // 1,599 trigrams where the corpus median is 0.950. The cap is not wrong to
    // trigger — the estimator counts CJK at ~1 token/character, so an
    // 86,000-character Chinese article exceeds 50,000 tokens where its
    // English equivalent is worth ~23,000.
    //
    // What was missing was the STASH. The fold footer pointed to the body as
    // it was fetched — HTML — that is, it left the agent to redo the
    // distillation itself. The JSON branch forty lines above already does the
    // right thing for the same reason; here we stash the FULL MARKDOWN, the
    // form the caller wanted, and the footer points to it. The copy is only
    // taken if the cap is really going to bite.
    let mut full_md_stash = None;
    let mut full_md_before_the_cap = None;
    let over_cap = sav.tokens_after > crate::filter::HARD_CAP_TOKENS;
    // **A kept heading must not cost a paragraph.** First version measured:
    // keeping headings within the SAME 180-line window returns 59 headings
    // instead of 6 — and drops paragraph fidelity from 0.039 to **0.019**,
    // because the 53 heading lines come out of the body's budget. The ratchet
    // refused it, rightly: the outline arrived in exchange for half of what
    // was left to read.
    //
    // But the line window is a PROXY. What is capped is the token, and this
    // page's headings weigh a few hundred tokens against the cap's 50,000. So
    // they are paid for ON TOP, without touching the body. The extension is
    // bounded so that a page with five thousand headings is still served
    // under the cap.
    let structural = md.lines().filter(|l| l.starts_with('#')).count();
    let lines_budget =
        crate::filter::HARD_CAP_LINES.saturating_add(structural.min(HEADINGS_PAID_ON_TOP));
    if let Some((truncated, trunc_tokens)) = crate::filter::apply_hard_cap_rescuing(
        &md,
        sav.tokens_after,
        "web",
        lines_budget,
        // What a PAGE holds dear in its middle: its headings. An error line is
        // the criterion for a command output, and it says nothing about an
        // article — sampling took away 47 of the 53 headings of
        // `pages/zhwiki`. The leading `#` is the form `html_to_markdown` gives
        // a heading, whatever its level.
        &|l: &str| l.starts_with('#'),
    ) {
        // **The pre-cut markdown is MOVED, not copied.** It weighed 233,794
        // bytes on the hooks page, and it was copied TWICE: once before
        // knowing whether the cap would bite, once to be both stashed and
        // returned. `mem::replace` takes the old body out by swapping it for
        // the new one — zero copies — and the stash reads it by reference
        // before it is returned.
        let before_cap = if over_cap {
            Some(std::mem::replace(&mut md, truncated))
        } else {
            md = truncated;
            None
        };
        sav = Savings::new(sav.tokens_before, trunc_tokens);
        if let Some(full) = before_cap {
            // The type is DECLARED, not sniffed: we are the ones who just
            // produced this markdown. The sniffer gets it wrong as soon as a
            // page is made of prose without headings or lists — it returned
            // `api.rest.txt`, a label that reaches the file NAME and tells the
            // next reader to read it as an API response.
            full_md_stash = crate::disclose::stash_for_reference(
                &full,
                &crate::disclose::SourceHint::web_with_content_type("text/markdown"),
            );
            // What the DELIVERY must stash: the page, not the cut. The fold
            // footer below stashes the same text, and the stash is
            // content-addressed — so both calls write only ONE file and name
            // the same hash.
            full_md_before_the_cap = Some(full);
        }
        // **The cap also takes away the NOTE that makes the rest followable.**
        //
        // `note_the_origin_of_relative_references` writes the origin on the
        // HEADING of the references table, and its comment says why: "the
        // heading is what survives the outline when the page is too big". That
        // is true of the outline, and false of the hard cap, which keeps one
        // line in N without knowing anything about the structure. MEASURED
        // 2026-09-10: `pages/zhwiki` delivers **59 relative targets and not a
        // word about the host**, because the line
        // `## Link targets — relative entries are under https://zh.wikipedia.org/`
        // fell with the others. The origin guarantee therefore only held below
        // the cap.
        //
        // Re-applied HERE, on the text actually delivered. The function is
        // silent when nothing is relative any more and never duplicates an
        // existing announcement, so re-applying it costs nothing to a page the
        // cap spared on this point.
        md = crate::web_tree::note_the_origin_of_relative_references(
            md,
            base.as_deref().unwrap_or(url),
        );
    }

    // **A regular fact, said once — on the web route too.**
    //
    // The hard cap SAMPLES: one line kept every N, and a marker between each,
    // all with the same number. MEASURED 2026-09-10 on a 3,000-row catalogue:
    // **109 markers, all "26 lines", 3,706 of the 11,795 bytes delivered —
    // 31%** of the page for one repeated sentence.
    //
    // `elision::fold_uniform_markers` already answered this and had only ONE
    // caller, `wrap` — the exact mirror of a recurring mistake, a general
    // mechanism wired onto a single route. The cap passed in is what THIS
    // process's passes actually removed: a marker we did not write is
    // content, and the fold would delete it while claiming its count.
    //
    // And it must not be what pushes a threshold DOWNWARDS: stashing and the
    // replay memory are decided on the size of this view, further on.
    if let Some(folded) = crate::elision::fold_uniform_unless_it_crosses(
        &md,
        crate::fold_count::folded_ceiling(),
        &[
            crate::disclose::threshold_tokens(),
            crate::web_replay::min_tokens_to_fold(),
        ],
    ) {
        md = folded;
        sav = Savings::new(sav.tokens_before, crate::tokenizer::estimate(&md));
    }

    // **A fold that promises a hash must have one.**
    //
    // Placed AFTER the hard cap, because IT is what folds: written before,
    // the replacement found nothing and the promise went out intact.
    //
    // The fold footer comes from the world of COMMANDS: it points to a
    // `full output:` banner and, failing that, to `tokenade raw <cmd>`. A page
    // has neither. MEASURED 2026-09-09 on a 3,000-row catalogue: 2,824 lines
    // folded, and the output points to a missing banner and then to a command
    // that does not exist. The reader has NO way to recover the missing rows.
    //
    // Same move as the JSON branch twelve lines above: we stash the source
    // and point to it, so the preview stays cheap and nothing that was
    // removed becomes unrecoverable.
    let promise = crate::elision::fold_footer_promise();
    if md.contains(&promise) {
        // The MARKDOWN stash first: it is the whole page in the form the
        // caller asked for, so `--prompt` answers from it directly. The HTML
        // body only comes as a fallback — it has to be re-distilled to be
        // used, which is better than nothing and worse than that.
        let replacement = match full_md_stash.as_ref() {
            Some((hash, path)) => page_fold_recovery(hash, path, true),
            None => match crate::disclose::stash_for_reference(
                body,
                &crate::disclose::SourceHint::web(),
            ) {
                Some((hash, path)) => page_fold_recovery(&hash, &path, false),
                // The stash could not be written: say so, rather than promise.
                None => "recover the folded lines by fetching the page again".to_string(),
            },
        };
        md = md.replace(&promise, &replacement);
    }

    // The page said nothing because there was nothing in the MARKUP: its text
    // sits in inline scripts that never ran (a Relay payload, a preloaded state
    // object). Run them in the embedded JS engine and harvest what they leave
    // behind. Gated on the shell check so a page that distilled fine never pays
    // for the engine — on the measured corpus this fires on 6 pages in 25 and
    // wins on 3, and the margin in `beats_extraction` is what keeps a marginal
    // harvest from displacing structured markup.
    if crate::web::looks_like_spa_shell(&md, sav.tokens_before, sav.tokens_after) {
        if let Some(js_text) = crate::js_render::render(body, url) {
            if crate::js_render::beats_extraction(&js_text, &md) {
                crate::debug_log::log(
                    "web-pipeline",
                    "js_rendered",
                    serde_json::json!({
                        "url": url,
                        "extracted_chars": md.len(),
                        "js_chars": js_text.len(),
                    }),
                );
                md = redact(&crate::js_render::label_as_js_harvest(url, &js_text));
                sav = Savings::new(sav.tokens_before, crate::tokenizer::estimate(&md));
                source = Source::JsHarvest;
            }
        }
    }

    // The page said nothing: ask it where its content is, via the feed it
    // declares itself. Two admissible cases, and the difference is what the
    // substitution CLAIMS:
    //   * a site index — the feed is the site's latest, which is the honest
    //     answer to "what is on this site";
    //   * a page-scoped feed (`/@user` → `/@user.rss`) — the feed IS this
    //     page's own content, so it answers the question that was asked.
    // Anything else (an article URL declaring the site-wide feed) would answer
    // a different question, which is why the guard is not simply dropped.
    if crate::web::looks_like_spa_shell(&md, sav.tokens_before, sav.tokens_after) {
        if let Some(feed_url) = crate::site_readers::declared_feed_url(body, url) {
            let page_scoped = crate::site_readers::feed_is_page_scoped(&feed_url, url);
            if crate::site_readers::is_site_index(url) || page_scoped {
                if let Some(f) = crate::web_fetch::fetch(
                    &feed_url,
                    &crate::web_fetch::FetchOpts::api("12", FEED_FETCH_CAP),
                ) {
                    if let Some(feed_md) = crate::site_readers::render(
                        &crate::site_readers::Kind::FeedDocument,
                        &String::from_utf8_lossy(&f.bytes),
                    ) {
                        // The note must say which of the two substitutions this is.
                        // A page-scoped feed IS this page's content; the site-index
                        // one is not, and calling both "the site's latest" would
                        // understate the first and overstate the second.
                        let provenance = if page_scoped {
                            "this page's own items, from the feed it declares — the page itself \
                         renders them client-side"
                        } else {
                            "these are the site's LATEST items, not the requested page's own content"
                        };
                        let feed_md = redact(&format!(
                            "> source: {feed_url} (this page distilled to nothing; \
                         {provenance})\n\n{feed_md}"
                        ));
                        crate::debug_log::log(
                            "web-pipeline",
                            "feed_discovered",
                            serde_json::json!({
                                "url": url,
                                "feed_url": feed_url,
                                "page_scoped": page_scoped,
                                "shell_tokens": sav.tokens_after,
                            }),
                        );
                        return finish(
                            feed_md,
                            full_tokens,
                            Source::FeedDiscovered(feed_url),
                            truncated_input,
                            None,
                            url,
                        );
                    }
                }
            }
        }
    }

    // A product page states its price and availability in metadata the
    // distiller correctly drops as chrome (allbirds distils 213 735 tokens of
    // HTML to 630, none of it the price). So the facts are PREPENDED, not
    // substituted: the prose is still the page, the facts are what it was for.
    if let Some(facts) = crate::site_readers::product_facts(body) {
        let merged = redact(&format!("{facts}\n---\n\n{md}"));
        return finish(
            merged,
            full_tokens,
            Source::ProductFacts,
            truncated_input,
            None,
            url,
        );
    }

    // Redact at the single exit rather than per branch. `disclose` only stashes
    // when the body is large, so the inline path used to escape redaction
    // entirely — an `Authorization: Bearer …` in API docs or an `?api_key=` in
    // a tutorial URL went straight to the agent. Idempotent, so the branches
    // that already redacted are unaffected.
    finish(
        redact(&md),
        full_tokens,
        source,
        truncated_input,
        full_md_before_the_cap.map(|f| redact(&f)),
        url,
    )
}

fn redact(s: &str) -> String {
    crate::redact::redact_for_model(s)
}

/// One exit. Savings ALWAYS describe the markdown actually returned — booking
/// a number an earlier stage held is how a channel comes to credit itself for
/// tokens it then goes on to spend.
fn finish(
    md: String,
    before: usize,
    source: Source,
    truncated_input: bool,
    full_md: Option<String>,
    page_url: &str,
) -> Distilled {
    // Links as the page wrote them are relative to IT: `](../x.html)` names
    // nothing the agent can open once it holds only the markdown. Measured
    // 2026-10-05: docs.python.org's `os.path` page came out with
    // `](../builtins/exceptions.html#OSError)`. Resolved against the page,
    // here, at the one exit every branch takes (ScrapeGraphAI 2.2.1 made the
    // same fix) — in the short form `web::resolve_links` documents.
    // A bare `voisin.html` is not counted as relative by the origin note (it
    // reads like text in parentheses); resolved, it becomes `/dir/voisin.html`
    // and needs the host — so the note is laid again, after (idempotent).
    let md = crate::web_tree::note_the_origin_of_relative_references(
        crate::web::resolve_links(&md, page_url),
        page_url,
    );
    let full_md = full_md.map(|f| {
        crate::web_tree::note_the_origin_of_relative_references(
            crate::web::resolve_links(&f, page_url),
            page_url,
        )
    });
    // Last thing before the count, so EVERY exit pays it — feed, product facts
    // and JS harvest carry the same invisible characters and the same
    // letter-spaced headings as ordinary extraction, and a pass applied per
    // branch is a pass that will be forgotten on the next branch added. Before
    // `estimate` because these bytes are the point: booking the count of the
    // dirty markdown would credit us for tokens we no longer deliver.
    let md = crate::text_hygiene::clean(&md);
    // **The proven folds are NOT applied here, and that is measured.**
    //
    // `template_fold` and `phrase_fold` were only offered to the COMMAND
    // route, while `html-to-markdown` is the #1 family in the usage ranking.
    // Wiring them here — the distiller's single exit, so the only point both
    // web routes go through — saves **−906 bytes on one page of the 39**, and
    // breaks two things worth more:
    //
    // * a FEED loses the findability of its headings. Eight entries
    //   `Post number 1..8` share `Post number`, which the fold declares once
    //   and replaces with a reference: each heading becomes `«1» 1`,
    //   reconstructible and unfindable. "Nothing is lost" is not "it can
    //   still be found" — on content whose LIST is the point.
    // * the fidelity ratchet turns red on `newegg_product` (6,485 characters
    //   versus 7,410), which is the intended reduction, but it has to be
    //   earned page by page and not wholesale.
    //
    // What would be needed before asking the question again: a fold that
    // knows a SHORT LINE in a list is a heading and not prose. The two missing
    // guards, on the other hand, are in place — `phrase_fold` no longer
    // rewrites the inside of a code block, and
    // `fold_gate::crosses_no_threshold_downwards` carries the threshold rule
    // for ALL thresholds, not only the stash one.
    let after = crate::tokenizer::estimate(&md);
    Distilled {
        md,
        savings: Savings::new(before, after),
        source,
        truncated_input,
        full_md,
    }
}

/// How many heading lines the clipped view may carry ON TOP of its window.
///
/// An index-style documentation page can be nothing but a list of headings,
/// and the extension must stay an extension: beyond this, the view would
/// become the page again.
const HEADINGS_PAID_ON_TOP: usize = 220;

/// The page renders ONLY headings — so nothing to read.
///
/// A frameset document is exactly that: it carries a `<title>` and not one
/// line of body, because its content lives in another document. The "thin
/// page" diagnostic does not see it, since something was indeed rendered.
fn nothing_but_a_title(md: &str) -> bool {
    md.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .all(|l| l.starts_with('#'))
}

/// Did the distiller take the JSON-outline path, which keeps only the SHAPE and
/// the first record?
///
/// Content-type first, then the body's own first character: a JSON API that
/// answers `text/plain` (or nothing at all) is still JSON, and keying only on
/// the header meant the "99 records are gone" warning was skipped exactly where
/// it mattered most.
pub fn body_took_the_json_outline(body: &str, ct: Option<&str>) -> bool {
    let ct_json = ct.is_some_and(|c| c.contains("json"));
    let looks_json = body.trim_start().starts_with(['{', '[']);
    (ct_json || looks_json) && serde_json::from_str::<serde_json::Value>(body).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page long enough for the hard cap to bite: what it removes must stay
    /// recoverable AS MARKDOWN, not as HTML to re-distill.
    #[test]
    fn what_the_hard_cap_samples_away_stays_reachable_as_markdown() {
        // Each paragraph must be unique IN ITS SHAPE and not only in its
        // number: `Paragraph number {i} …` is a TEMPLATE, and the template
        // fold folds it before the cap has anything to cut. The first version
        // of this test stopped on its own anti-vacuity guard, which is exactly
        // what that guard is for.
        const WORDS: &[&str] = &[
            "harbour",
            "lantern",
            "quarry",
            "meridian",
            "tessellate",
            "bramble",
            "cormorant",
            "aqueduct",
            "palimpsest",
            "windlass",
            "saffron",
            "clerestory",
            "gantry",
            "fenland",
            "obsidian",
            "cartouche",
            "vellum",
            "sluice",
            "escarpment",
            "kestrel",
        ];
        let mut html = String::from("<html><body><h1>A very long article</h1>");
        let mut seed = 1u64;
        for i in 0..3_000 {
            let mut sentence = String::new();
            for _ in 0..24 {
                seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                sentence.push_str(WORDS[(seed >> 33) as usize % WORDS.len()]);
                sentence.push(' ');
            }
            html.push_str(&format!("<p>{sentence}marker{i}.</p>"));
        }
        html.push_str("</body></html>");
        let out = distill_page(&html, "https://example.test/long", Some("text/html"));

        // The cap did bite — and the probe is taken in the MIDDLE. The last
        // paragraph is the worst possible witness: `filter::generic` keeps the
        // head AND the tail, so `marker2999` survives a cut that nonetheless
        // removed nine tenths of the body.
        assert!(
            !out.md.contains("marker1500"),
            "the cap did not bite — this test measures nothing"
        );
        // And the footer names the stash of the FULL markdown.
        let line = out
            .md
            .lines()
            .find(|l| l.contains("FULL distilled page"))
            .unwrap_or_else(|| panic!("no footer names the stash:\n{}", tail(&out.md)));
        // The footer lives INSIDE a `[tokenade folded … (path)]`, so the line
        // ends with `)]` and not with `)`.
        let path = line
            .rsplit_once('(')
            .and_then(|(_, p)| p.split_once(')'))
            .map(|(p, _)| p)
            .unwrap_or_else(|| panic!("the footer carries no path: {line}"));
        assert!(
            path.ends_with(".md"),
            "the stash must be markdown, not the page to re-distill: {path}"
        );
        // The promise is TRUE: what the cap removed is in the file.
        let stashed = std::fs::read_to_string(path).expect("the stash must be readable");
        assert!(
            stashed.contains("marker1500") && stashed.contains("marker0"),
            "the stash does not carry what the cap removed"
        );

        // **And the DELIVERY must be able to name that page.** The fold footer
        // above is not enough: on a big enough page, `disclose` replaces the
        // body with an outline and takes the footer along. The pre-cut
        // markdown therefore travels in the struct, so that the delivering
        // route stashes the PAGE and not the CUT.
        let full = out
            .full_md
            .as_deref()
            .expect("the cap bit: the pre-cut markdown must travel along");
        assert!(
            full.contains("marker1500") && !out.md.contains("marker1500"),
            "`full_md` does not carry what `md` lost"
        );
    }

    /// The outline of a too-long page must SURVIVE clipping, and not be paid
    /// for with the body.
    #[test]
    fn the_outline_of_a_capped_page_survives_whole() {
        const WORDS: &[&str] = &[
            "harbour",
            "lantern",
            "quarry",
            "meridian",
            "tessellate",
            "bramble",
            "cormorant",
            "aqueduct",
            "palimpsest",
            "windlass",
            "saffron",
            "clerestory",
            "gantry",
            "fenland",
        ];
        let mut html = String::from("<html><body><h1>A book of many chapters</h1>");
        let mut seed = 7u64;
        for section in 0..150 {
            html.push_str(&format!("<h2>Chapter {section} of the survey</h2>"));
            for para in 0..20 {
                let mut sentence = String::new();
                for _ in 0..22 {
                    seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                    sentence.push_str(WORDS[(seed >> 33) as usize % WORDS.len()]);
                    sentence.push(' ');
                }
                html.push_str(&format!("<p>{sentence}s{section}p{para}.</p>"));
            }
        }
        html.push_str("</body></html>");
        let out = distill_page(&html, "https://example.test/book", Some("text/html"));

        // The rule is REACHED: clipping did bite in the middle.
        assert!(
            !out.md.contains("s75p10"),
            "clipping did not bite — this test measures nothing"
        );
        // All headings are there. It is the outline, and it is not sacrificed.
        let missing: Vec<usize> = (0..150)
            .filter(|i| !out.md.contains(&format!("Chapter {i} of the survey")))
            .collect();
        assert!(
            missing.is_empty(),
            "{} heading(s) lost to clipping, including {:?}",
            missing.len(),
            &missing[..missing.len().min(5)]
        );
        // And the extension does not push past the cap it serves.
        let delivered = crate::tokenizer::estimate(&out.md);
        assert!(
            delivered <= crate::filter::HARD_CAP_TOKENS,
            "the clipped view weighs {delivered} tokens, above the cap of {}",
            crate::filter::HARD_CAP_TOKENS
        );
    }

    fn tail(s: &str) -> String {
        s.lines().rev().take(4).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn a_feed_is_recognised_by_content_not_by_url() {
        // No `.rss`, no `/feed` — the URL gives nothing away.
        // Long enough to clear `READER_VALID_MIN_CHARS` — the reader treats a
        // short render as a failed parse, and a toy fixture below that floor
        // tests the floor rather than the routing.
        let entries: String = (1..=8)
            .map(|n| {
                format!(
                    "<entry><title>Post number {n} about something</title>\
                     <link href=\"https://ex.test/{n}\"/>\
                     <summary>A paragraph of summary text for entry {n}, long \
                     enough to look like a real feed item.</summary></entry>"
                )
            })
            .collect();
        let atom = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
             <feed xmlns=\"http://www.w3.org/2005/Atom\">\
             <title>Example weblog</title>{entries}</feed>"
        );
        let d = distill_page(&atom, "https://ex.test/x", Some("application/atom+xml"));
        assert_eq!(d.source, Source::Feed);
        assert!(d.md.contains("Post number 1"), "{}", d.md);
        assert!(
            d.source.is_reader_grade(),
            "a rendered feed must not be re-judged by the HTML gate"
        );
    }

    #[test]
    fn ordinary_html_is_distilled_and_not_reader_grade() {
        let html = format!(
            "<html><body><article><h1>Title</h1>{}</article></body></html>",
            "<p>Real sentences of body copy that carry the page.</p>".repeat(20)
        );
        let d = distill_page(&html, "https://ex.test/a", Some("text/html"));
        assert_eq!(d.source, Source::Distilled);
        assert!(d.md.contains("Title"));
        assert!(!d.source.is_reader_grade());
    }

    /// The warning exists because the outline drops every record after the
    /// first. A JSON API answering `text/plain` drops them just the same.
    #[test]
    fn json_is_detected_without_a_json_content_type() {
        let body = r#"[{"a":1},{"a":2},{"a":3}]"#;
        assert!(body_took_the_json_outline(body, None));
        assert!(body_took_the_json_outline(body, Some("text/plain")));
        assert!(!body_took_the_json_outline(
            "<html></html>",
            Some("text/html")
        ));
        // Looks like JSON, is not.
        assert!(!body_took_the_json_outline("{not json at all", None));
    }

    #[test]
    fn output_is_redacted_on_the_plain_path() {
        let _iso = crate::platform::StoreIsolation::new("output_is_redacted_on_th");
        // Small enough that nothing stashes — the path that used to escape
        // redaction entirely.
        let html = format!(
            "<html><body><article><h1>Setup</h1>{}\
             <p>Send Authorization: Bearer sk-live-abcdefghijklmnopqrstuvwxyz012345</p>\
             </article></body></html>",
            "<p>Configure the client before you begin the tutorial.</p>".repeat(12)
        );
        let d = distill_page(&html, "https://ex.test/docs", Some("text/html"));
        assert!(
            !d.md.contains("sk-live-abcdefghijklmnopqrstuvwxyz012345"),
            "a credential must never reach the agent inline: {}",
            d.md
        );
    }

    /// The hygiene pass must run on the delivered markdown, not on some earlier
    /// stage's copy — a heading set with CSS letter-spacing and a body seeded
    /// with zero-width spaces are both things real pages do.
    #[test]
    fn invisibles_and_letter_spacing_are_cleaned_before_delivery() {
        let html = format!(
            // ONE word only: HTML collapses a run of whitespace to a single
            // space long before we see it, so the two-space word boundary that
            // `rejoin_letter_spacing` honours does not survive an HTML source.
            // It is exercised at string level in `text_hygiene`, where sources
            // that DO preserve it (feeds, JSON outlines, plain text) live.
            "<html><body><article><h1>S t a r t H e r e</h1>{}</article></body></html>",
            "<p>Real\u{200B} sentences of bo\u{00AD}dy copy that carry the page.</p>".repeat(20)
        );
        let d = distill_page(&html, "https://ex.test/c", Some("text/html"));
        assert!(d.md.contains("StartHere"), "{}", d.md);
        assert!(
            !d.md.contains('\u{200B}') && !d.md.contains('\u{00AD}'),
            "{}",
            d.md
        );
        assert!(
            d.md.contains("body copy"),
            "soft hyphen must rejoin the word: {}",
            d.md
        );
    }

    #[test]
    fn savings_describe_the_markdown_actually_produced() {
        let html = format!(
            "<html><body><article>{}</article></body></html>",
            "<p>Body copy that survives extraction and is worth counting.</p>".repeat(40)
        );
        let d = distill_page(&html, "https://ex.test/b", Some("text/html"));
        assert_eq!(
            d.savings.tokens_after,
            crate::tokenizer::estimate(&d.md),
            "booking must describe what is delivered, not what an earlier stage held"
        );
        assert!(d.savings.tokens_before > d.savings.tokens_after);
    }
}
