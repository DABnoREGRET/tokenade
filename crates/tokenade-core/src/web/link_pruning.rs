//! Link-dense blocks: menus, tag clouds, related-article rails.
//!
//! Kept when there is prose between the links, dropped when there is not —
//! the difference between a paragraph that cites and a navigation bar.

use super::*;

/// Rewrite a Markdown string by replacing every `[text](url)` inline
/// link with `text[N]` and emit a numbered `## Link targets` block at the
/// end. Same URL → same number (deduplicated). Useful for long-form
/// articles where the same source is cited 5+ times.
///
/// Ported from `crawl4ai/markdown_generator/markdown_generation_strategy.py:82-146`.
/// We use `[N]` instead of crawl4ai's `⟨N⟩` because Markdown viewers
/// render brackets unambiguously while angle-quotes get mangled by
/// some renderers.
///
/// No-op when the input has fewer than 3 inline links — the citation
/// scaffold would cost more tokens than it saves on short articles.
/// **DECLARED RESERVE.** Ported then never wired in: a measurement showed
/// that moving the URLs out of the running text saved −302 bytes and dropped
/// the richness ratchet from 1.0000 to 0.7122. Kept WITH its measurement, so
/// that nobody ports it again.
#[allow(dead_code)]
pub(crate) fn linkify_to_citations(md: &str) -> String {
    let link_re = re(r"\[([^\]]+)\]\(([^)]+)\)");
    let matches: Vec<_> = link_re.captures_iter(md).collect();
    if matches.len() < 3 {
        return md.to_string();
    }
    // Build (url → number) dedup map in order of first appearance.
    let mut url_to_num: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut urls_in_order: Vec<String> = Vec::new();
    for cap in &matches {
        let url = cap
            .get(2)
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();
        url_to_num.entry(url.clone()).or_insert_with(|| {
            urls_in_order.push(url.clone());
            urls_in_order.len()
        });
    }
    // Rewrite the body. Use replace_all with a closure.
    let body = link_re.replace_all(md, |caps: &regex::Captures| {
        let text = caps.get(1).map_or("", |m| m.as_str());
        let url = caps.get(2).map_or("", |m| m.as_str());
        let num = url_to_num.get(url).copied().unwrap_or(0);
        format!("{text}[{num}]")
    });
    let mut out = body.into_owned();
    out.push_str(&format!("\n\n{}\n\n", crate::web_tree::REFERENCES_HEADING));
    for (i, url) in urls_in_order.iter().enumerate() {
        let _ = writeln!(out, "[{}] {}", i + 1, url);
    }
    out
}

pub(super) fn strip_negative_class_blocks(html: &str) -> String {
    // One compiled regex, cached like every other `re()` pattern.
    // Two keyword groups, differing only in whether a LEADING word boundary is
    // required.
    //
    // Long, unambiguous words are matched as a suffix too, because generators
    // prefix their own name: Sphinx ships `class="sphinxsidebar"`, and
    // `\bsidebar\b` does not match it — there is no boundary between `x` and
    // `s`. Sphinx documents Python, Django, NumPy, SciPy and pandas, and on
    // docs.python.org/3/glossary the surviving sidebar became the page's whole
    // outline: "Previous topic / Next topic / This page", with the actual
    // glossary nowhere in the preview.
    //
    // Short or ambiguous words keep both boundaries: `ad`/`ads`/`nav` as bare
    // substrings match `download`, `header-adjacent`, `navigable`.
    //
    // `dl` is in the tag list because catalogue facets are DEFINITION LISTS:
    // `<dl class="filter-box"><dt class="filter-box-title">Wake On LAN</dt>`.
    // Promoting `<dt>` to a heading made every surviving
    // boilerplate `<dl>` a section in the outline — the gap was there before,
    // that change just made it impossible to miss.
    //
    // `filter`/`facet`/`refine` are the universal catalogue-UI words. Without
    // them newegg.com's Network Interface Cards category outlined itself as
    // its own filter panel — `Brands (103 lines)`, `Price`, `Speed`,
    // `Connectors`, `Availability` — with not one product in the summary. A
    // hyphen is a word boundary, so `\bfilter\b` still matches
    // `filter-box-label`.
    let open = re(
        r#"(?is)<(div|section|ul|ol|dl|aside|span|p)\b[^>]*\b(?:class|id)\s*=\s*["'][^"']*(?:[-_a-z]*(?:sidebar|navbar|sharing|social|newsletter|cookie|consent|breadcrumbs?|footer|related|widget|advert|sponsor|outbrain|taboola|paywall|subscribe|signup|popup|modal|pagination|(?-i:promo))|\b(?:nav|menu|share|banner|byline|ads?|comment-form|filters?|facets?|refine))\b[^"']*["'][^>]*>"#,
    );
    // ONE pass: collect every boilerplate block's byte range, then splice
    // them all out together.
    //
    // The previous version removed the FIRST match, re-lowercased the whole
    // page, and repeated — capped at six. So a page with more than six
    // boilerplate blocks kept the rest, and the cap was invisible: newegg's
    // Network Interface Cards category has 31 filter boxes, of which 25
    // survived and became the page's entire outline. Collecting ranges also
    // drops the per-iteration full-string lowercase, which was the reason a
    // cap felt necessary at all.
    let lower = html.to_ascii_lowercase();
    let mut cuts: Vec<(usize, usize)> = Vec::new();
    let mut pos = 0usize;
    while let Some(m) = open.find_at(html, pos) {
        let tag_end = html[m.start() + 1..]
            .find(|c: char| !c.is_ascii_alphanumeric())
            .map(|i| m.start() + 1 + i)
            .unwrap_or(m.end());
        let tag = html[m.start() + 1..tag_end].to_ascii_lowercase();
        let open_pat = format!("<{tag}");
        let close_pat = format!("</{tag}");
        let mut depth = 1i32;
        let mut i = m.end();
        while depth > 0 && i < html.len() {
            let next_open = lower[i..].find(&open_pat).map(|x| i + x);
            let next_close = lower[i..].find(&close_pat).map(|x| i + x);
            match (next_open, next_close) {
                (Some(o), Some(c)) if o < c => {
                    depth += 1;
                    i = o + open_pat.len();
                }
                (_, Some(c)) => {
                    depth -= 1;
                    i = lower[c..]
                        .find('>')
                        .map(|x| c + x + 1)
                        .unwrap_or(html.len());
                }
                _ => i = html.len(),
            }
        }
        // A boilerplate block is PERIPHERAL by definition. When a match would
        // remove most of the document it is not boilerplate — the keyword hit
        // a layout container. newegg.com wraps its entire catalogue in
        // `<div class="row has-side-banner">`: `banner` matched the layout
        // MODIFIER ("this row has a side banner"), and the cut took 109 926 of
        // 192 707 bytes — the product grid and every facet with it. The
        // aggressive tier then had too little left to win, so the ladder fell
        // back to the relaxed pass and shipped the page unpruned, which is why
        // the filter panel became the whole outline.
        //
        // This guards every keyword, present and future, instead of trying to
        // spell `banner` in a way that excludes `has-side-banner`.
        // A Sphinx SECTION id is never boilerplate. Sphinx emits
        // `id="s-<slugified heading>"` for every section, and a heading that
        // happens to name an API concept collides with the keyword list:
        // django's `<section id="s-be-careful-with-related-name-and-related-
        // query-name">` matched `related` and took the whole `related_name`
        // documentation with it — the paragraph and the list explaining
        // `'%(app_label)s'` and `'%(class)s'` were absent from a 59 KB stash.
        //
        // Measured: the corpus has 53 `id="s-…"` elements and exactly TWO
        // match a keyword — both of them this false positive. Sphinx documents
        // Python, Django, NumPy, SciPy and pandas, so the rule pays for itself
        // beyond the one page.
        if html[m.start()..m.end()].contains("id=\"s-") {
            pos = m.end();
            continue;
        }
        // A class that TURNS AN AD OFF is not an ad. vogue.com wraps its story
        // grid in `class="SummaryCollectionGridItems-… hide-read-more-ad"`, and
        // `ad` matched the suffix — so 20 970 bytes of headlines went out as
        // advertising. Measured: 285 distinct ad-matching class tokens across
        // the corpus, exactly ONE of them a negation.
        if negates_the_keyword(&html[m.start()..m.end()]) {
            pos = m.end();
            continue;
        }
        let span = i.saturating_sub(m.start());
        if span * 100 > html.len() * MAX_BOILERPLATE_PERCENT {
            pos = m.end();
            continue;
        }
        cuts.push((m.start(), i));
        // Resume AFTER the block: anything nested inside it is already gone.
        pos = i.max(m.end());
        if cuts.len() >= MAX_BOILERPLATE_BLOCKS {
            break;
        }
    }
    if cuts.is_empty() {
        return html.to_string();
    }
    let mut out = String::with_capacity(html.len());
    let mut cursor = 0usize;
    for (a, b) in cuts {
        if a >= cursor && b <= html.len() {
            out.push_str(&html[cursor..a]);
            cursor = b;
        }
    }
    out.push_str(&html[cursor..]);
    out
}

/// Post-conversion prune of LINK-DENSE markdown blocks — the trafilatura
/// link-density test transposed to the markdown side (regex pipeline has
/// no DOM to score). A blank-line-separated block is dropped when >60% of
/// its characters are link syntax/labels AND it has ≥2 links AND fewer
/// than 3 commas (commas ≈ prose; nav clusters have none) AND it is short
/// (<400 chars). Long comma-rich paragraphs are never touched.
/// Is there real prose BETWEEN the links, as opposed to separators?
///
/// Link density alone cannot tell a nav cluster from a sentence that happens to
/// cite two documents, because a markdown link spends most of its characters on
/// a URL nobody reads. django's QuerySet reference opens with
///
/// > Throughout this reference we'll use the [example blog models](…) presented
/// > in the [database query guide](…).
///
/// which is 76 % link characters, comma-free and under 400 — a nav cluster by
/// every test we had, and it was deleted from a page an agent reads to learn
/// the API. Its labels are three words each, so the headline test does
/// not save it either.
///
/// A navigation cluster puts nothing between its links but separators — `·`,
/// `|`, `/`, whitespace, a bullet. Five real words outside the link syntax mean
/// somebody wrote a sentence.
pub(super) fn prose_between_the_links(block: &str, link_re: &Regex) -> bool {
    const MIN_PROSE_WORDS: usize = 5;
    let outside = link_re.replace_all(block, " ");
    let words = outside
        .split_whitespace()
        .filter(|w| w.chars().any(|c| c.is_alphanumeric()) && w.chars().count() > 1)
        .count();
    words >= MIN_PROSE_WORDS
}

pub(super) fn prune_link_dense_blocks(md: &str) -> String {
    let link_re = re(r"\[([^\]]*)\]\(([^)]*)\)|\[([^\]]*)\]\[\d+\]");
    let is_item = |l: &str| l.trim_start().starts_with("- ") || l.trim_start().starts_with("* ");
    let mut out: Vec<String> = Vec::new();
    for block in md.split("\n\n") {
        // A few lines and THEN a list — Wikipedia's `[[edit]]` link, or a
        // heading, joined to the list under it by a line of SPACES rather than
        // an empty one — is two things. Judged as one block it stopped being
        // "all list lines", the list exemption below no longer held, and the
        // density test took `External links`, the project's official site,
        // with the edit link (2026-10-03). The lines above are judged alone;
        // the list stays with the list rule.
        let lines: Vec<&str> = block.lines().collect();
        let split = lines.iter().position(|l| is_item(l)).filter(|&k| {
            k > 0
                && lines[k..]
                    .iter()
                    .filter(|l| !l.trim().is_empty())
                    .all(|l| is_item(l))
        });
        match split {
            Some(k) => {
                let above = lines[..k].join("\n");
                let list = lines[k..].join("\n");
                out.push(if keeps(&above, link_re) {
                    format!("{above}\n{list}")
                } else {
                    list
                });
            }
            None if keeps(block, link_re) => out.push(block.to_string()),
            None => {}
        }
    }
    out.join("\n\n")
}

/// Is this blank-line-separated block worth keeping, or a navigation cluster?
fn keeps(block: &str, link_re: &Regex) -> bool {
    // In CHARACTERS, not bytes: a reader counts characters, and the `·`
    // separator of a navigation bar weighs two of them in UTF-8 bytes.
    // Measured: `[Home](/) · [About](/about) · [Jobs](/jobs) · [Press](/press)`
    // fell to 60.0% instead of 66% because of those three extra bytes, just
    // under the threshold, and the bar got through again.
    let total = block.trim().chars().count();
    if total == 0 || total >= 400 {
        return true;
    }
    let commas = block.matches(',').count();
    if commas >= 3 {
        return true;
    }
    // Bulleted link lists belong to `prune_link_list_runs`, which has
    // ALREADY decided about them — it keeps any run shorter than its
    // MIN_RUN because a short run of links is content, not a sitemap.
    // Re-judging them here silently overrode that: a catalogue is
    // link-dense, comma-free and (per letter group) under 400 chars, so
    // developer.mozilla.org/en-US/docs/Web/API lost its entire index —
    // every `### A … ### X` heading survived with nothing underneath.
    // Two rules disagreeing about the same lines is the defect; the list
    // rule owns lists.
    if block
        .lines()
        .filter(|l| !l.trim().is_empty())
        .all(|l| l.trim_start().starts_with("- ") || l.trim_start().starts_with("* "))
    {
        return true;
    }
    // **Density is measured on what a reader SEES.** It used to count
    // `[label](url)` whole — address included — against the block's length,
    // addresses included as well. A long address was therefore enough to
    // make "dense" a block whose TEXT is not: measured on a Newegg product
    // page, a seller's rating ("4.2", "87 total ratings") wrapped in a card
    // link went away entirely, 369 characters, because the URL weighed more
    // than the words. The label against the visible text is the same
    // question asked of what the page shows.
    let mut nlinks = 0usize;
    let mut label_chars = 0usize;
    let mut syntax_chars = 0usize;
    for c in link_re.captures_iter(block) {
        nlinks += 1;
        let label = c
            .get(1)
            .or_else(|| c.get(3))
            .map(|m| m.as_str().chars().count())
            .unwrap_or(0);
        label_chars += label;
        syntax_chars += c.get(0).map(|m| m.as_str().chars().count()).unwrap_or(0) - label;
    }
    let visible = total.saturating_sub(syntax_chars);
    if nlinks >= 2
        && label_chars * 100 / visible.max(1) > 60
        && !link_labels_read_like_content(block)
        && !prose_between_the_links(block, link_re)
    {
        return false; // nav/related/share cluster — dropped
    }
    true
}
