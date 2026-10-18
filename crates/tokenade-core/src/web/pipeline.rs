//! The stages of `html_to_markdown`, in the order the page goes through
//! them. Each one is named after what it decides, so the entry point reads
//! as the pipeline it is instead of 324 lines of interleaved rationale.
//!
//! The comments are the reason these are stages and not one function: every
//! branch here was bought with a measurement, and they belong next to the
//! code they justify.

use super::*;

/// XML that is not a page: a feed, or a sitemap. `None` = keep converting.
pub(super) fn xml_document_shortcut(html: &str) -> Option<String> {
    // A feed is not a page, and this is the one function every caller reaches.
    // `distill_text` (hook) and `html_to_markdown{,_capped}` (`tokenade web`)
    // are separate entry points, so routing feeds in either one alone left the
    // other emitting tag soup — measured on americansongwriter.com/feed:
    // 237 KB of RSS became 109 910 bytes of dates, GUIDs and
    // `https://wordpress.org/?v=6.9.5`, one text node per line.
    //
    // Only the root-element signal is available here (no Content-Type), which
    // is the reliable half; `distill_text` still checks the header for feeds
    // that omit the XML declaration. `render` returns None on anything that is
    // not really a feed, so a mislabelled page falls through unchanged.
    if html.trim_start().starts_with("<?xml") {
        if let Some(md) =
            crate::site_readers::render(&crate::site_readers::Kind::FeedDocument, html)
        {
            return Some(format!("> source: feed\n\n{md}"));
        }
        // Not a feed — but a sitemap is the other XML an agent actually fetches,
        // and it was passing through byte for byte (23 MB on one.google.com).
        if let Some(md) = crate::xml::sitemap_to_markdown(html) {
            return Some(md);
        }
        // Any other XML: an export, an API response. The HTML pipeline only
        // returned its bare values, without the name of the field carrying
        // them.
        if let Some(md) = super::labelled_xml_records(html) {
            return Some(md);
        }
    }
    None
}

/// Remove the markup that must never reach the converter, in the one order
/// proven safe. Returns the stripped source.
pub(super) fn strip_markup_noise(mut s: String) -> String {
    // 1. Kill noise blocks (everything between open and close tag, including
    // the tags). Some real-world HTML is malformed (unclosed `<script>`); we
    // do a second sweep with a fallback that grabs from the opening tag to
    // EOF if no closer is found (surfaced by `adv:web:broken-html`).
    // `title` joins the noise list: its text is served through the meta header
    // (`harvest_meta_header` reads the ORIGINAL html, before this strip), and
    // left in place it becomes body text on any page where the relaxed tier
    // keeps the whole document — the page's name said twice, the second time
    // as a stray line of prose. SVG `<title>` elements go with the `svg` strip
    // that precedes them in this list.
    // On the tree path the walk drops all seven tags itself
    // (`web_tree::is_dropped`) and the rescue sources above were harvested from
    // the ORIGINAL html, so this loop is not what protects the OUTPUT — F6
    // measured it byte-identical with the whole thing skipped. Two tags stay and
    // two are deliberately gone:
    //
    //  * `script`/`style` are kept because the regexes still running BELOW must
    //    not scan live JS. `strip_permalink_anchors`' quote-aware attribute
    //    group can run past an unbalanced quote inside
    //    `document.write('<a href="#' + id + '">')` and delete every byte up to
    //    the next real `</a>`. Cheap insurance, and `strip_block` before
    //    `strip_unclosed_block` is the proven-safe order: a properly closed
    //    script is removed whole, so the unclosed pass only meets real strays.
    //  * `svg`/`iframe` are NOT stripped here any more. Scanning them was a
    //    hazard: with script bodies present, a JS string holding
    //    `'<iframe src=…>'` closed the JS-safe way (`<\/iframe>`) leaves an
    //    opener with no closer, and `strip_unclosed_block` then truncates from
    //    there to EOF — silently deleting the rest of the page. html5ever
    //    recovers from unclosed elements the way a browser does. Letting it
    //    also revives the walk's `mermaid_fence`/`video_link` recovery, which
    //    this strip had kept dead: 77neko5 regains its `[YouTube video](…)`.
    let tree_path = crate::web_tree::web_tree_enabled();
    let noise_tags: &[&str] = if tree_path {
        &["script", "style"]
    } else {
        &[
            "script", "style", "noscript", "svg", "iframe", "template", "title",
        ]
    };
    for tag in noise_tags {
        s = strip_block(&s, tag);
        s = strip_unclosed_block(&s, tag);
    }
    // AFTER the script/style strip, deliberately. Inside a script body a
    // browser does not open a comment at all, so a JS string containing
    // `"<!--"` (or the legacy `<script><!-- … //--></script>` idiom) would
    // otherwise swallow the rest of the document. Removing script bodies
    // first makes the remaining `<!--` markers unambiguously markup.
    // Regex path only. html5ever tokenises comments itself, and inside a
    // rawtext element it never treats `<!--` as markup at all — so on the tree
    // path, where the script strip above no longer runs, applying this regex
    // would EAT the page instead of protecting it.
    if !tree_path {
        s = strip_html_comments(&s);
    }
    // BOTH paths. The corpus called this dead on the tree path — 17 pages carry
    // the exact generator classes it targets and none changed without it — and
    // the corpus was wrong: those pages wrap the glyph in an `<svg>` icon that
    // another pass happened to remove. A bare `¶` anchor (Sphinx, godoc) still
    // leaks into the heading, which
    // `permalink_anchors_do_not_leak_into_headings` asserts and which no corpus
    // page exercises. Kept, with `script`/`style` stripped above so it never
    // scans JS.
    s = strip_permalink_anchors(&s);
    // A `>` inside a quoted attribute value is legal HTML5 (only `<`, `&` and
    // the quote itself must be escaped), and modern class strings put one there:
    // Tailwind/shadcn arbitrary variants like `class="max-md:[&>a]:hidden"` or
    // `data-[state=open]>svg`. Every tag matcher below is `<tag\b[^>]*>`, whose
    // `[^>]*` ends at that stray `>` and spills the attribute tail into the body
    // as text — huggingface.co blog titles came out as a spurious `# a]:hidden">`
    // heading. Normalise once here, at the choke point AFTER the script/comment
    // strips (so every `<…>` left is an element tag), rather than teaching nine
    // sibling regexes to skip quotes.
    // Regex path only, for the same reason: this normalisation exists BECAUSE
    // `<tag\b[^>]*>` matchers mis-slice on a quoted `>`, which html5ever does
    // not — it is dead work on the tree path (0/147 corpus pages change without
    // it) and now actively risky there, since script bodies are no longer
    // stripped before it and it would rewrite `>` inside them.
    if !tree_path {
        s = escape_stray_gt_in_tags(&s);
    }
    s
}

/// The tiered extraction ladder (T1 aggressive → T4 embedded state).
pub(super) fn extraction_ladder(
    s: &str,
    html: &str,
    code_stash: &[String],
    noscript_inner: &str,
) -> String {
    // 2-5. Tiered extraction ladder — each tier only runs when the previous
    // one came back skeletal, and a tier is kept only when it recovers MORE:
    //   T1 aggressive (main isolation + boilerplate strip)
    //   T2 relaxed (whole body, chrome tags only)
    //   T3 noscript (crawler render served for JS-disabled clients)
    //   T4 embedded state (JSON-LD / __NEXT_DATA__ prose mining)
    // A genuinely tiny page stays tiny on every tier. The T1→T2 floor is the
    // readability/newspaper ~200-char validity convention: below it, the
    // isolation FAILED (SPA shell `<main>`, article subtree matching a
    // negative class keyword) — not "the page says nothing".
    // Restored immediately after each `convert_body` call (not deferred to
    // the function's end) so every downstream char-count heuristic — the
    // T1/T2 floor right here, the thin-result/challenge-banner checks below —
    // sees the real recovered length, not the placeholder's.
    // Parsed lazily and ONCE for both tiers on the tree path (F4): T2 only runs
    // when T1 came back skeletal, and it walks the very same tree from the body
    // root instead of re-parsing the document.
    let parsed = crate::web_tree::web_tree_enabled().then(|| crate::web_tree::parse_for_select(s));
    let mut body = restore_code_blocks(&convert_body_with(s, true, parsed.as_ref()), code_stash);
    if body.chars().count() < EXTRACT_VALID_MIN_CHARS {
        let relaxed =
            restore_code_blocks(&convert_body_with(s, false, parsed.as_ref()), code_stash);
        if relaxed.chars().count() > body.chars().count() {
            body = relaxed;
        }
    }
    if body.chars().count() < EXTRACT_VALID_MIN_CHARS && !noscript_inner.is_empty() {
        let ns = convert_body(noscript_inner, false);
        if ns.chars().count() > body.chars().count() {
            body = ns;
        }
    }
    {
        let mined = harvest_embedded_prose(html, &body);
        if !mined.is_empty() {
            if body.is_empty() {
                body = mined;
            } else {
                body = format!("{body}\n\n{mined}");
            }
        }
    }
    body
}

/// Prepend the page's `og:` header, minus whatever the body already says.
pub(super) fn prepend_meta_header(mut body: String, header: &str) -> String {
    // Drop only the LINE that repeats, not the whole header. This used to
    // be all-or-nothing, which was survivable while it fired rarely; once
    // the `<title>` fallback made the title match on a third of the
    // corpus, it took every one of those pages' descriptions down with it
    // — 689 words of real summary across 147 pages, for 13 duplicate
    // headings.
    let head_zone: String = body.chars().take(400).collect();
    let kept: Vec<&str> = header
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter(|l| {
            !(l.starts_with('#') && head_zone_repeats_title(&head_zone, l))
                && !(l.starts_with("> ") && head_zone.contains(l.trim_start_matches("> ")))
        })
        .collect();
    // The title survived the filter above, yet the body may STILL open with
    // its own copy — `head_zone_repeats_title` compares raw substrings, so
    // `Models` against `Models | Django documentation`, `Writing your docs`
    // against `Writing Your Docs - MkDocs`, or a smart-quoted headline
    // against its plain twin all read as different strings. Measured: 14 of
    // 147 corpus pages shipped their name twice.
    //
    // Drop the BODY's copy rather than the header: the meta title is the
    // more complete of the two (it carries the site name), and this
    // direction removes a line instead of degrading one — the mistake the
    // reverted attempt made when it kept ebay's double-encoded `&amp;`
    // heading and threw the clean title away.
    if let Some(title_line) = kept.iter().find(|l| l.starts_with('#')) {
        body = drop_body_heading_echoing_title(&body, title_line.trim_start_matches('#').trim());
    }
    if !kept.is_empty() {
        let kept = kept.join("\n\n");
        body = if body.is_empty() {
            kept
        } else {
            format!("{kept}\n\n{body}")
        };
    }
    body
}

/// The note that says our own extraction came back with nothing, and why.
/// `None` = the result is not thin, so nothing needs saying.
/// The one sentence about the HTTP status that a thin/blocked-page note may
/// carry, written once so the layer that KNOWS the status can recognise it and
/// answer it.
///
/// It used to read "Check the HTTP status to tell them apart" — an instruction
/// to the one party that cannot follow it. Measured over this machine's
/// transcripts: **37 real deliveries** carried that clause; in **26 no status
/// line was present anywhere in the message**, and in the other **11 the status
/// was stated three lines above** (404 x9, 403, 429). Not once did it lead
/// anywhere. tokenade made the request and holds the answer;
/// `webfetch_pretool::attach_provenance` substitutes it on every serve path.
pub const STATUS_QUESTION: &str =
    "Only the HTTP status separates them, and tokenade states it above whenever the server \
     answered anything but 2xx";

/// The last explanation for a page's thinness: **a guess about OUR
/// processing**, and that is what makes it quotable.
///
/// The core does not know the request status, let alone the refusal a
/// dedicated READER met one layer above. When that refusal happened, this
/// sentence blames isolation for a failure that comes from the site:
/// `webfetch_pretool` then replaces it, as it already replaces
/// [`STATUS_QUESTION`]. A constant rather than a copied string — two spellings
/// would have drifted apart at the first changed word.
pub const ISOLATION_GUESS: &str = "isolation likely picked the wrong element (a shell \
     `<main>`/`<article>`) rather than the page having no text";

pub(super) fn thin_result_note(html: &str, recovered: usize, body: &str) -> Option<String> {
    // The size exemption ("a genuinely tiny page is not a failure") is right
    // for a small page that DID yield some text, and wrong for one that
    // yielded NONE: MEASURED 2026-07-22 on archive.org/details/nasa_techdocs,
    // a 1 872-byte JS bootstrap — under the 2 048-byte bar — whose whole
    // delivery was the site's generic `<title>`, "Internet Archive: Digital
    // Library of Free & Borrowable Texts…", with nothing saying the item
    // page had not been reached. A shell is not a small page.
    //
    // Zero recovered characters is a fact about OUR RESULT, not a judgement
    // about the page, which is exactly what this banner is for — so it does
    // not need the document to be large to be worth stating.
    // The `<script>` requirement was an AND on this whole gate until
    // 2026-08-26, so a page with no JavaScript at all that recovered nothing
    // got no note and no body: the empty string, silently. Measured on three
    // real files — an Angular render with custom elements (8 361 bytes,
    // 41 characters of visible text), `mdka`'s 149 KB depth-5003 nesting
    // benchmark (293 characters), and a 241-byte fixture whose unclosed
    // `<title>` swallows the document (44 characters). None carries a single
    // `<script`. What is JS-specific is the ADVICE, not the fact, so the fact
    // is stated either way and the causes branch below.
    let has_script = html.to_ascii_lowercase().contains("<script");
    if recovered < EXTRACT_VALID_MIN_CHARS
        && (html.len() >= EMPTY_RESULT_MIN_HTML_BYTES || recovered == 0)
    {
        // States the MEASURED fact and stops there. Reusing the
        // `generic-block` wording here would have claimed "no body text,
        // no semantic elements" over pages that plainly had some — a 404,
        // a parked domain, a login shell. Asserting an emptiness we did
        // not observe is the same error as asserting a completeness we do
        // not have, and it points the agent at a bot wall that may not
        // exist.
        //
        // The nav/main split (essence, MIT — see module doc) adds WHY,
        // approximated with regex over the ORIGINAL html since there is
        // no DOM here: a page that is mostly chrome (`nav_ratio` high)
        // failed differently than one with genuinely little visible text
        // at all, and differently again from one where isolation likely
        // grabbed the wrong element.
        let (total_chars, nav_chars) = nav_main_char_split(html);
        let nav_ratio = if total_chars == 0 {
            0.0
        } else {
            nav_chars as f64 / total_chars as f64
        };
        let why = if total_chars < EXTRACT_VALID_MIN_CHARS * 2 {
            "the page's own visible text is thin even before isolating navigation".to_string()
        } else if nav_ratio >= 0.7 {
            format!(
                "{:.0}% of the page's visible text sits in nav/header/footer/aside, so \
                     isolating main content left little",
                nav_ratio * 100.0
            )
        } else {
            ISOLATION_GUESS.to_string()
        };
        // Naming client-side rendering on a page that ships no script would
        // point the agent at a cause the document rules out.
        let causes = if has_script {
            format!(
                "the text is rendered client-side (or arrives by XHR after load, which no \
                 session-less fetch reaches); an unnamed anti-bot block; or the page really \
                 is a stub, error or landing page. {STATUS_QUESTION}"
            )
        } else {
            "this page ships no script, so client-side rendering is not the explanation: \
             the markup may be malformed in a way that swallowed the document (an \
             unclosed tag), the text may sit inside custom elements the extractor does \
             not treat as content, or the page really is a stub. `tokenade expand-ref` \
             on the banner hash returns the raw bytes"
                .to_string()
        };
        let note = format!(
            "[tokenade: thin-result] recovered {recovered} characters from {} KB of HTML \
                 ({why}). \
                 Possible causes, in the order worth checking: {causes}.\n\n---\n\n{body}",
            html.len() / 1024,
        );
        // A `<script>` tag is itself the evidence that this is a shell worth
        // explaining, however small — archive.org's 1 872-byte JS bootstrap
        // is the case this branch was written for, and it must keep its note.
        //
        // Without one there is no such evidence, and an explanation longer
        // than the document it explains is not a saving but inflation with a
        // banner on it. Widening this note to every scriptless page,
        // degenerate ones included, broke it; `proptest` found it on the empty string,
        // where a 400-byte note replaced 0 bytes.
        if !has_script && note.len() >= html.len() {
            return None;
        }
        return Some(note);
    }
    None
}

#[cfg(test)]
mod a_page_with_no_script_still_says_it_recovered_nothing {
    //! The thin-result note required the page to carry a `<script` tag, so a
    //! static page that yielded nothing was delivered as the empty string
    //! with nothing said. Three real files, none with a single script.

    fn note(html: &str) -> Option<String> {
        super::thin_result_note(html, 0, "")
    }

    #[test]
    fn a_scriptless_page_that_recovered_nothing_gets_a_note() {
        let html = format!(
            "<html lang=\"en\"><head><title>Malformed</head><body>\n{}\n</body></html>\n",
            "  <div class=\"container\"><section id=\"ok\"></section></div>\n".repeat(20)
        );
        let n = note(&html).expect("a page that gave up nothing must say so");
        assert!(n.contains("[tokenade: thin-result]"));
    }

    #[test]
    fn the_note_does_not_blame_client_side_rendering_when_there_is_no_script() {
        let html = format!(
            "<html><body>{}</body></html>\n",
            "<app-root data-ng=\"x\"><app-nav></app-nav></app-root>".repeat(20)
        );
        let n = note(&html).expect("note");
        assert!(
            !n.contains("rendered client-side"),
            "a page with no script cannot be blamed on client-side rendering: {n}"
        );
        assert!(n.contains("ships no script"));
    }

    #[test]
    fn a_scripted_page_keeps_the_client_side_advice() {
        let html =
            "<html><body><div id=\"root\"></div><script src=\"/app.js\"></script></body></html>\n";
        let n = note(html).expect("note");
        assert!(n.contains("rendered client-side"), "{n}");
    }

    #[test]
    fn a_page_that_recovered_plenty_gets_no_note() {
        let html = "<html><body><p>plenty of real prose here</p></body></html>\n";
        assert!(super::thin_result_note(html, 4_000, "plenty of real prose here").is_none());
    }
}

#[cfg(test)]
mod the_note_never_costs_more_than_it_explains {
    //! Widening the note to scriptless pages reached the degenerate
    //! ones too. `proptest` found it on the empty string: a 400-byte
    //! explanation in place of nothing at all.

    #[test]
    fn an_empty_document_gets_no_note() {
        assert!(super::thin_result_note("", 0, "").is_none());
    }

    #[test]
    fn a_tiny_document_gets_no_note() {
        assert!(super::thin_result_note("<html></html>", 0, "").is_none());
    }

    #[test]
    fn a_document_large_enough_to_explain_still_gets_one() {
        let html = format!(
            "<html><body>{}</body></html>",
            "<div class=\"x\"></div>".repeat(200)
        );
        let n = super::thin_result_note(&html, 0, "").expect("note");
        assert!(n.len() < html.len(), "{} vs {}", n.len(), html.len());
    }
}
