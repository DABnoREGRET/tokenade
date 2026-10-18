//! **The crossing: from HTML to markdown.**
//!
//! The normal path. What decides WHAT to convert lives in the neighbouring
//! modules; here we convert, measure what it saved, and hold a token cap —
//! because an endless page is still an endless page after conversion.

use super::*;

/// Convert a small trusted HTML fragment (a comment body, a post) to
/// markdown — the conversion steps of the main pipeline WITHOUT the
/// noise/boilerplate strips, for site readers that already isolated the
/// content structurally.
pub(crate) fn fragment_to_markdown(fragment: &str) -> String {
    // Code-fence sanctuary (essence, MIT — see module doc), same reasoning as
    // `html_to_markdown`: protect `<pre>`/`<code>` from the noise strip below
    // before it can eat a documented tag example. Restored at the tail, after
    // `collapse_whitespace` — so the real code's own indentation never passes
    // through the whitespace squeeze.
    let (stashed, code_stash) = sanctuary_code_blocks(fragment);
    let mut s = stashed;
    // Kill noise blocks FIRST, exactly as `html_to_markdown` does. Without
    // this, `strip_all_tags` below removes the `<script>` TAGS and keeps their
    // SOURCE as prose: IGN embeds a `application/ld+json` Product blob next to
    // every card in its article container, and the reader emitted 30 raw JSON
    // objects inline — more tokens than the generic path it was meant to beat.
    // Any container-anchored reader is exposed to this, not just IGN.
    for tag in ["script", "style", "noscript", "svg", "iframe", "template"] {
        s = strip_block(&s, tag);
        s = strip_unclosed_block(&s, tag);
    }
    // AFTER the script/style strip, deliberately. Inside a script body a
    // browser does not open a comment at all, so a JS string containing
    // `"<!--"` (or the legacy `<script><!-- … //--></script>` idiom) would
    // otherwise swallow the rest of the document. Removing script bodies
    // first makes the remaining `<!--` markers unambiguously markup.
    s = strip_html_comments(&s);
    s = strip_permalink_anchors(&s);
    s = headings_to_md(&s);
    s = list_to_md(&s);
    s = definition_list_to_md(&s);
    s = pre_code_to_md(&s);
    s = inline_code_to_md(&s);
    s = links_to_md(&s);
    s = blockquote_to_md(&s);
    s = paragraph_breaks(&s);
    s = strip_all_tags(&s);
    s = decode_entities(&s);
    let out = collapse_whitespace(&s).trim().to_string();
    restore_code_blocks(&out, &code_stash)
}

/// Does a structured-data value carry BLOCK HTML?
///
/// The question is not "is there an angle bracket" — a title can contain
/// `a < b` — but "is this string a document fragment". A closed or open block
/// element name answers on its own; an `<em>` in the middle of a sentence is
/// not enough and does not need converting.
pub(super) fn carries_markup(s: &str) -> bool {
    re(r"(?i)<(p|ul|ol|li|div|table|br|h[1-6])\b[^>]*>").is_match(s)
}

/// Generator class names that mark a permalink/self-link widget.
pub(super) const NAV_CLASSES: [&str; 4] = ["headerlink", "hash-link", "top-link", "permalink"];

/// Minimum median words-per-link-label for a link cluster to be CONTENT
/// (a headline list, a result list) rather than navigation.
///
/// Measured, not chosen: over every run the two link pruners cut across the
/// 147-page corpus, navigation sits at a median of 1-3 words per label
/// (`[Cache]`, `[UIEvent]`, `[1000Mbps]`, `[pics]`) and tops out at 6
/// (Wikipedia's maintenance categories, `Art6 categoriesView allView all`).
/// Content starts at 7 and runs to 14 — the Guardian's front page
/// (`Slowdown in Arctic ice melt ends after largest winter decline on record`)
/// and a page of Arabic exam-result titles. Nothing sits in between.
pub(super) const MIN_CONTENT_LABEL_WORDS: usize = 7;

/// Steps 2-5 of the pipeline on noise-stripped HTML. `aggressive` gates the
/// two lossy heuristics — `<main>/<article>` isolation and negative-class
/// subtree removal; the relaxed pass keeps the whole `<body>` and only strips
/// semantic chrome tags, as the recovery tier when the aggressive pass
/// over-pruned.
pub(super) fn convert_body(noise_stripped: &str, aggressive: bool) -> String {
    convert_body_with(noise_stripped, aggressive, None)
}

/// `convert_body`, optionally reusing a document already parsed from
/// `noise_stripped` — the ladder's two tiers differ only by root, so parsing
/// twice is pure waste (F4). `doc` MUST come from this same input.
pub(super) fn convert_body_with(
    noise_stripped: &str,
    aggressive: bool,
    doc: Option<&crate::web_tree::Doc>,
) -> String {
    // On the tree path the selection below is dead work: `select_and_convert`
    // re-does isolation AND boilerplate stripping on its own parse of
    // `noise_stripped`, and overwrites `s` wholesale further down. Computing
    // it anyway cost every distilled page a full regex main-isolation +
    // boilerplate pass whose result was thrown away.
    let tree = crate::web_tree::web_tree_enabled();
    let mut s = if tree {
        String::new()
    } else if aggressive {
        // Kill chrome blocks but keep the content of <header> inside
        // <article> intact by stripping chrome AFTER isolating main content.
        // Isolation that keeps almost nothing FAILED — `<main>` was a
        // client-rendered shell, not the content. miro.com isolates 298 471
        // bytes down to 409; hashnode.com to 3 246. The aggressive tier then
        // produced zero characters and the ladder fell back to the RELAXED
        // pass, which does no boilerplate stripping at all — so a shell
        // `<main>` cost the page every bit of pruning.
        //
        // Threshold from the corpus, not from taste: across 124 pages where
        // the aggressive tier wins, isolation keeps at least 4.5 % of the
        // input (p5 26.5 %, median 90 %); the pages that collapse keep
        // 0.1-3.4 %. The gap is where this sits.
        // essence's 10%-isolation floor (MIT — see module doc), scoped to
        // `<article>` only: the failure mode it names is a listing page
        // where ONE `<article>` product-card gets isolated out of many
        // (books.toscrape.com) — `<article>` has no fallback-region rescue
        // the way `<main>` does (`outside_region_if_main_is_a_section`
        // only fires when `isolate_main_span` picked `<main>`), so a bad
        // article pick has no second chance downstream. `<main>` keeps its
        // own corpus-calibrated 4 % floor above unchanged: essence's 10 % is
        // measured against THEIR corpus, not this one, and the comment above
        // already shows winning `<main>` pages as low as 4.5 %.
        // ONE `<main>`/`<article>` scan, reused by the sibling rescues below.
        let found = find_main(noise_stripped);
        let main = match &found {
            Some((tag, _whole, inner)) => {
                let m = noise_stripped[inner.clone()].to_string();
                let floor = if *tag == "article" {
                    ARTICLE_ISOLATED_PERCENT
                } else {
                    MIN_ISOLATED_PERCENT
                };
                if m.len() * 100 >= noise_stripped.len() * floor {
                    m
                } else {
                    // essence's selector-priority list (MIT — see module
                    // doc + `isolate_by_priority_selectors`), consulted only
                    // as a RESCUE here: `<main>`/`<article>` existed but
                    // failed its floor. A page can still name its content
                    // precisely (`#readme`, `.markdown-body`, …) without
                    // wrapping it in `<main>` at all.
                    isolate_by_priority_selectors(noise_stripped)
                        .unwrap_or_else(|| noise_stripped.to_string())
                }
            }
            None => isolate_by_priority_selectors(noise_stripped)
                .unwrap_or_else(|| noise_stripped.to_string()),
        };
        // Boilerplate subtrees by negative class/id keywords (nav/share/
        // cookie/related/…) — the single highest-value quality+economy signal
        // in the extraction literature (trafilatura/readability/defuddle all
        // converge on it).
        // Same reasoning one step later, cumulatively. The per-block span
        // guard stops ONE match from eating the page, but amazon.de is
        // stripped by many separate blocks that together remove 96 % of it,
        // leaving 126 characters. When the keyword set removes that much, it
        // has misfired on this page's markup — keep the document.
        //
        // Corpus again: pages where the aggressive tier wins keep at least
        // 40.8 % after stripping (p5 62 %); the collapsing ones keep 3.6-5.5 %.
        let keep_or_restore = |region: String| {
            let stripped = strip_negative_class_blocks(&region);
            if stripped.len() * 100 >= region.len() * MIN_KEPT_AFTER_STRIP_PERCENT {
                stripped
            } else {
                region
            }
        };
        let kept = keep_or_restore(main);
        // `<main>` is the page's content on an ARTICLE page. On a FRONT page it
        // is often one section of several — aljazeera.com wraps its featured
        // block in `<main id="featured-news-container">` and puts every story
        // card after it, latercera keeps 56 of its 149 headings inside `<main>`
        // — so isolating discards the rest of the page below the fold.
        //
        // The siblings are stripped INDEPENDENTLY and joined afterwards, not
        // appended to the isolated region first. An earlier version did it the other
        // way and four unrelated pages lost real content: `MAX_BOILERPLATE_PERCENT`
        // judges each cut as a share of the region being stripped, so growing
        // that region let through cuts it had previously refused. Two regions,
        // two denominators, no coupling.
        // Two reasons to keep what isolation discarded, and they are not the
        // same reason. `<article>` siblings are kept ALWAYS: the element says
        // "self-contained content" in the standard, so leaving one out is a
        // loss whatever the rest of the page looks like. The whole outside
        // region is kept only when `<main>` turns out to hold a MINORITY of the
        // page's headings — the signature of a `<main>` that is one section
        // rather than the content, and the shape that `<div>`-based story grids
        // take (latercera keeps 56 of 147 headings inside `<main>`; its cards
        // are plain `<div>`s, so the `<article>` rule cannot see them).
        let whole_span = found.as_ref().map(|(_, w, _)| w.clone());
        let outside = whole_span
            .clone()
            .and_then(|sp| outside_region_if_main_is_a_section(noise_stripped, sp))
            .or_else(|| whole_span.and_then(|sp| article_siblings(noise_stripped, sp)));
        match outside {
            Some(sib) => format!("{kept}\n{}", keep_or_restore(sib)),
            None => kept,
        }
    } else {
        noise_stripped.to_string()
    };
    if !tree {
        // The strip below is by TAG NAME, which is wrong for header/footer: see
        // `web_tree::dropped_at` for the measurement (huggingface.co/models lost
        // all 30 model ids). The tree path consults ancestry; here there is no
        // tree, so re-tag the item-scoped ones first and let the strip see only
        // page chrome. Fixing one path and leaving its twin is THE recurring
        // defect in this module.
        s = descope_item_chrome(&s);
        for tag in [
            "header", "footer", "nav", "aside", "form", "button", "input", "select",
        ] {
            s = strip_block(&s, tag);
        }
    }

    // 3. Convert structural tags to markdown. The tree walk (scraper/html5ever)
    // replaces the whole regex conversion chain when TOKENADE_WEB_TREE=1; it
    // already handles tag stripping and entity decoding, so steps 3-4 collapse
    // into one call. Selection above (main isolation, boilerplate strip) still
    // runs on the regex path for now — this swaps CONVERSION only, so the two
    // paths can be A/B'd against each other. See web_tree.rs.
    if tree {
        // Full tree path: selection (main isolation + boilerplate skip) AND
        // conversion happen inside one parse of the ORIGINAL noise-stripped
        // input — the regex selection above is skipped entirely (F1).
        s = match doc {
            Some(d) => crate::web_tree::select_and_convert_parsed(d, aggressive),
            None => crate::web_tree::select_and_convert(noise_stripped, aggressive),
        };
    } else {
        s = headings_to_md(&s);
        s = list_to_md(&s);
        s = definition_list_to_md(&s);
        s = pre_code_to_md(&s);
        s = inline_code_to_md(&s);
        s = links_to_md(&s);
        s = blockquote_to_md(&s);
        s = unwrap_inline_source_newlines(&s);
        s = tables_to_md(&s);
        s = paragraph_breaks(&s);
        s = images_to_md_alt(&s);

        // 4. Strip remaining tags then decode entities.
        s = strip_all_tags(&s);
        s = decode_entities(&s);
    }

    // 4b. Link-dense block prune (markdown-side link-density test) +
    // link-only list runs (sidebars/sitemaps) + "Skip to content" artifacts.
    s = hoist_headings_out_of_links(&s);
    s = drop_empty_list_items(&s);
    s = split_butted_link_runs(&s);
    s = prune_link_list_runs(&s);
    s = prune_link_dense_blocks(&s);
    // essence's link-text dedup (MIT — see module doc): an icon/sr-only span
    // plus a visible label both landing in the same `<a>` produces
    // `[Apple Apple](url)` — halve it when the label is the same phrase
    // repeated back to back.
    s = dedup_repeated_link_text(&s);
    // essence's repeated-list-item collapse (MIT — see module doc): drop the
    // 3rd+ consecutive `- ` bullet whose text is byte-identical to the one
    // before it (a placeholder/skeleton row repeated by a JS list widget).
    s = collapse_repeated_list_items(&s);
    let skip_re = re(r"(?i)^\s*\[?skip to (main )?content\]?(\([^)]*\))?\s*$");
    // essence's UI_NOISE literal alternation (MIT — see module doc): chrome
    // strings that name a WIDGET rather than say anything about the page
    // ("Copy for LLM", a language-picker count, MediaWiki's `[[edit]]`…).
    let ui_noise_re = re(
        r"(?i)^\s*(?:Ask about this section|Copy for LLM|View as Markdown|Was this page helpful\??|Table of Contents|In this article|On this page|Show more|Read more|Load more|Scroll to top|Toggle the table of contents|\d+ languages?|Edit this page on GitHub|Suggest (?:changes|edits?))\s*$",
    );
    let mediawiki_edit_re = re(r"\s*\[\[edit\]\([^)]*\)\]");
    s = s
        .lines()
        .filter(|l| !skip_re.is_match(l) && !ui_noise_re.is_match(l))
        .map(|l| mediawiki_edit_re.replace_all(l, "").into_owned())
        .collect::<Vec<_>>()
        .join("\n");
    // 5. Whitespace squeeze.
    s = collapse_whitespace(&s);
    s.trim().to_string()
}

/// Token savings of running `html_to_markdown` on `html`.
pub fn html_to_markdown_with_savings(html: &str) -> (String, crate::Savings) {
    let before = tokenizer::estimate(html);
    let md = html_to_markdown(html);
    let after = tokenizer::estimate(&md);
    (md, crate::Savings::new(before, after))
}

/// Jina-Reader-style upfront token budget. Truncates the input HTML to
/// approximately `max_input_tokens` BEFORE running the (regex-heavy)
/// markdown extraction. Lower peak memory + earlier termination on
/// pathological pages (10 MB single-page apps, comment threads with
/// 50 K replies).
///
/// We cut on a `>` or `\n` boundary so we never split mid-attribute and
/// confuse the downstream regexes. The truncation is approximate — the
/// final token count after markdown extraction is normally well under
/// `max_input_tokens` because extraction removes 60–80 % of the input.
///
/// `max_input_tokens = 0` is treated as no cap (forwarded as-is).
pub fn truncate_to_max_tokens(html: &str, max_input_tokens: usize) -> &str {
    if max_input_tokens == 0 {
        return html;
    }
    let est = tokenizer::estimate(html);
    if est <= max_input_tokens || est == 0 {
        return html;
    }
    // Proportional cut: bytes-per-token is roughly stable across HTML.
    let target_bytes = (html.len() as u128 * max_input_tokens as u128 / est as u128) as usize;
    // Char-boundary-safe: a proportional BYTE offset lands mid-codepoint on
    // any non-ASCII page (CJK, accents) — `&html[..cut]` panicked there.
    let bounded = crate::caps::truncate_at_char_boundary(html, html.len().min(target_bytes));
    let cut = bounded.len();
    // Walk back to the previous `>` or newline so we don't slice mid-tag.
    let safe = html[..cut].rfind(['>', '\n']).map(|i| i + 1).unwrap_or(cut);
    &html[..safe]
}

/// Run [`html_to_markdown`] with an upfront token budget. See
/// [`truncate_to_max_tokens`] for the bounding semantics.
///
/// Returns `(markdown, savings, was_truncated)` so the caller can warn
/// the agent when content was dropped.
/// **DECLARED RESERVE.** Bounding now happens UPSTREAM, on the fetched bytes,
/// so nobody calls the budgeted variant any more. It stays because it is the
/// only one that tells the rendering it was TRUNCATED.
#[allow(dead_code)]
pub(crate) fn html_to_markdown_capped(
    html: &str,
    max_input_tokens: usize,
) -> (String, crate::Savings, bool) {
    let trimmed = truncate_to_max_tokens(html, max_input_tokens);
    let was_truncated = trimmed.len() < html.len();
    let (md, _sav) = html_to_markdown_with_savings(trimmed);
    // Report savings against the **full** input — the cap is supposed to
    // be invisible from a "how much did STS save" point of view.
    let before = tokenizer::estimate(html);
    let after = tokenizer::estimate(&md);
    (md, crate::Savings::new(before, after), was_truncated)
}
