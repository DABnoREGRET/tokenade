//! **The OUTLINE of a page, and when it is worth more than the page.**
//!
//! An outline only replaces the body if it says enough: a page without headings
//! gives an empty outline, and delivering it instead of the text loses everything. Hence the
//! promotion of an implicit structure (numbered sections, bold headings)
//! and the measurement that says whether the outline really MAPS the page.

use super::*;

/// L66 — render `md` as an outline: just the heading hierarchy plus
/// a one-line preview per section and a `(N lines)` count, so the
/// agent sees the SHAPE of a doc without paying for every paragraph.
/// Used by [`crate::disclose::maybe_stash`] when a markdown body is
/// past the 5 k-token disclosure threshold — the outline becomes the
/// summary, the full markdown lives in the stash for grep / Read.
///
/// Output shape:
///
/// ```text
/// # Getting Started (3 sections)
/// ## Install (15 lines · "Install via npm or yarn. Requires Node 18+.")
/// ## Configuration (43 lines · "Configure via config.yaml or env vars.")
/// ## Quick start (28 lines · "Run `npm run dev` to launch the local server.")
/// # API Reference (2 sections)
/// …
/// ```
///
/// Picks h1-h4 (drops h5/h6, which are rarely structural). Preview is
/// the first non-empty paragraph line of the section, trimmed to 160
/// chars.
/// Headings the outline walk will actually use — i.e. not inside a fence.
/// Must stay in step with the walk in [`outline`]; they disagreed once and the
/// disagreement was invisible in the output.
pub(super) fn visible_headings(md: &str) -> usize {
    let mut in_fence = false;
    let mut n = 0;
    for line in md.lines() {
        let t = line.trim_start();
        if crate::fences::fence_marker(t).is_some() {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence && parse_heading(line).is_some_and(|(d, _)| d <= 4) {
            n += 1;
        }
    }
    n
}

/// The document with every fence delimiter removed.
///
/// The heading walk skips fenced content, and correctly: experience taught it
/// that a `#` inside a fence is a code comment, not a section. But two real
/// shapes put genuine headings inside a fence:
///
///   * govinfo.gov serves every federal bill as one ``` block — preformatted
///     text, not code;
///   * `datatracker.ietf.org/doc/html/rfc2616` is worse, and it is OUR doing.
///     The converter turns the RFC's `<h4>` anchors into `#### 3.2.1 General
///     Syntax` and then wraps the surrounding `<pre>` in a fence, so the
///     pipeline emits twenty headings inside fences it created itself and the
///     outline ignores every one of them. RFC 2616 outlined as a single
///     section of 9 343 lines.
pub(super) fn without_fence_delimiters(md: &str) -> String {
    md.lines()
        .map(|l| {
            let t = l.trim_start();
            if crate::fences::fence_marker(t).is_some() {
                ""
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Headings at depth ≥2 — the test for whether opening the fences revealed a
/// DOCUMENT or just source code.
///
/// This distinction is the whole safety of opening fences, and it is measured:
/// `raw.githubusercontent.com/torvalds/linux/…/checkpatch.pl` and git's
/// `Makefile` are full of `# comment` lines, and the first version of this
/// outlined them as sections — `# SPDX-License-Identifier: GPL-2.0`,
/// `# (c) 2001, Dave Jones`. A comment in Perl, shell, Python, Make or YAML is
/// always ONE `#`; nothing in those languages writes `###`. A real document
/// nests: rfc2616 revealed `##`, `###` and `####` together.
pub(super) fn nested_headings(md: &str) -> usize {
    md.lines()
        .filter(|l| parse_heading(l).is_some_and(|(d, _)| (2..=4).contains(&d)))
        .count()
}

/// Structure carried as a bare NUMBER: `1. Introduction`, `8.1. Character
/// Encoding`. RFCs published as plain text do this, and so do most standards
/// bodies; `rfc8259.txt` (JSON) came back as one undifferentiated block.
///
/// Riskier than the labelled form — `1. Install the package` is a numbered
/// LIST, not a section — so it carries an extra test that a list cannot pass:
/// **the numbering must nest**. A specification has `1`, `1.1`, `8.3`; a list
/// is flat. Requiring three multi-level hits is what separates them.
///
/// Also refuses indented lines and lines carrying dot leaders, which is how a
/// TABLE OF CONTENTS is written (` 1.3. Introduction . . . . . . 5`). Without
/// that, every document would outline its own contents page twice.
pub(super) fn numbered_sections(lines: &[&str]) -> Option<String> {
    const MIN_HITS: usize = 5;
    const MIN_NESTED: usize = 3;
    const MAX_LABEL_CHARS: usize = 80;
    static NUM: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let num = NUM
        .get_or_init(|| regex::Regex::new(r"^(\d+(?:\.\d+)*)\.?\s+(\S.*)$").expect("static regex"));
    let mut hits: Vec<(usize, usize)> = Vec::new(); // (line, nesting level)
    for (i, l) in lines.iter().enumerate() {
        if l.starts_with(' ') || l.starts_with('\t') || l.chars().count() > MAX_LABEL_CHARS {
            continue;
        }
        if l.contains(" . ") || l.contains("...") {
            continue; // table of contents
        }
        if let Some(c) = num.captures(l) {
            hits.push((i, c[1].matches('.').count() + 1));
        }
    }
    if hits.len() < MIN_HITS || hits.iter().filter(|(_, lv)| *lv >= 2).count() < MIN_NESTED {
        return None;
    }
    let depth_of: std::collections::HashMap<usize, usize> =
        hits.iter().map(|(i, lv)| (*i, (*lv + 1).min(4))).collect();
    Some(
        lines
            .iter()
            .enumerate()
            .map(|(i, l)| match depth_of.get(&i) {
                Some(d) => format!("{} {}", "#".repeat(*d), l.trim()),
                None => (*l).to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// Promote the structural labels of a document that carries NO html headings.
///
/// Legal texts, standards and public-domain books are the class: their
/// structure is `Article 17` / `SEC. 101.` / `CHAPTER I` on a line of its own,
/// not `<h2>`. Measured on eight long documents, five outlined uselessly —
/// eur-lex's GDPR (88 907 tokens, 3 692 lines, **zero** headings) fell through
/// to the "no headings → show the first 20 lines" path and delivered EUR-Lex's
/// navigation and its *search tips*, while Article 17 sat at line 1 423 of the
/// stash. govinfo bills and Gutenberg plain text do the same.
///
/// STRICTLY a fallback: it runs only when the document is long and the outline
/// it would otherwise produce is nearly empty, so a page that already outlines
/// well is never touched. And it demands REPETITION — a label has to appear at
/// least `MIN_REPEATS` times on its own short line before it counts as
/// structure, which is what stops a sentence beginning "Article 5 states…"
/// from becoming a section.
pub(super) fn promote_unheaded_structure(md: &str) -> Option<String> {
    const MIN_REPEATS: usize = 5;
    const MAX_LABEL_CHARS: usize = 90;
    // Deliberately multilingual: these documents are as often French, German,
    // Spanish or Italian as English, and the corpus that motivated this
    // includes legifrance and eur-lex.
    static LABEL: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let label = LABEL.get_or_init(|| {
        regex::Regex::new(concat!(
            r"^\s*(?i:art\.|article|artikel|articolo|art[ií]culo|sec\.|section|",
            r"chapter|chapitre|kapitel|cap[ií]tulo|capitolo|part|partie|teil|",
            r"titre|t[ií]tulo|titolo|livre|annex|annexe|anhang|appendix|schedule|",
            r"r[eè]gle|rule|clause)\s+[0-9IVXLCDM]+",
        ))
        .expect("static regex")
    });
    let lines: Vec<&str> = md.lines().collect();
    let hits: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| {
            let t = l.trim();
            // Dot leaders mean this is the TABLE OF CONTENTS, not the section
            // itself. `rfc5322.txt` promoted `Appendix C. Acknowledgements
            // . . . . . . 42` — the contents entry, complete with its page
            // number — because `Appendix C` matches the label pattern and
            // nothing was checking for leaders. The numeric promoter had this
            // guard from the start; the labelled one did not.
            // Dot leaders only. Indentation is NOT a usable signal here: this
            // promoter runs on markdown derived from HTML, where leading
            // whitespace is incidental — testing it deleted every `CHAPTER`
            // heading from the GDPR, which is indented in eur-lex's markup.
            // The numeric promoter, which works on plain-text specs where
            // column 0 is meaningful, keeps its indent test.
            if t.contains(" . ") || t.contains("...") {
                return false;
            }
            if t.is_empty() || t.chars().count() > MAX_LABEL_CHARS {
                return false;
            }
            let Some(m) = label.find(t) else { return false };
            // The label word itself must be capitalised. A heading writes
            // `Section 4` or `SECTION 4`; a CROSS-REFERENCE inside prose
            // writes `section 3.2.4`, and rfc5322 wraps one to the start of a
            // line — "section 3.2.4. Comments may nest." passed every other
            // test, capitalised word and all, because the capital belonged to
            // the next sentence rather than to a title.
            if !t[m.start()..]
                .chars()
                .next()
                .is_some_and(char::is_uppercase)
            {
                return false;
            }
            // What follows the number decides whether this is a HEADING or a
            // SENTENCE that happens to start with one. `rfc5322.txt` contains
            // "Section 2 lays out the general description of a message and
            // its…" at the head of a paragraph — a blank line before it, a
            // plausible length, and every other test passed. But a heading
            // capitalises what comes next (`Sec. 10001. Short title.`,
            // `Appendix C. Acknowledgements`, `SECTION 1. SHORT TITLE`) or has
            // nothing after the number at all (`CHAPTER I`, `Article 17`,
            // whose titles sit on the following line); prose carries on in
            // lowercase, because it is the middle of a sentence.
            let rest = t[m.end()..].trim_start_matches([':', '.', '-', '—', ' ']);
            !rest.chars().next().is_some_and(|c| c.is_lowercase())
        })
        .map(|(i, _)| i)
        .collect();
    if hits.len() < MIN_REPEATS {
        return numbered_sections(&lines);
    }
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut next = hits.iter().peekable();
    for (i, l) in lines.iter().enumerate() {
        if next.peek() == Some(&&i) {
            next.next();
            out.push(format!("## {}", l.trim()));
        } else {
            out.push((*l).to_string());
        }
    }
    Some(out.join("\n"))
}

/// Below this number of headings, a document has no structure of its own:
/// it is what `outline` tests before PROMOTING structural
/// labels, and it is the same threshold that decides whether an outline deserves to
/// REPLACE the text (see [`outline_maps_the_page`]).
pub const NEARLY_UNSTRUCTURED: usize = 3;

/// Is this outline a MAP of the page, or only its title?
///
/// A caller that delivers an outline INSTEAD of the text is making a bet: that the
/// reader will find enough in it to navigate. The bet is lost when the outline names
/// only one section — the agent then gets the page's title and nothing else,
/// where the clipped sample gave it 180 lines of real text.
///
/// MEASURED on 2026-09-11 on the synthetic page of
/// `e2e_a_body_cut_at_the_cap_says_so` (600 KB, one `<h1>`, 9,000 paragraphs):
/// the delivery had dropped to the single line `# MARKER-START`, 14 characters,
/// against ~11,000 characters of sentences before outline delivery. The outline of the clipped
/// VIEW is degenerate precisely because the view is short: it is 180
/// lines, under the `LONG_ENOUGH_TO_NEED_AN_OUTLINE` that would trigger
/// promotion. The earlier measurement of the right choice was on `zhwiki` (59 headings) and
/// had no page without headings at hand.
///
/// The threshold is not new: it is [`NEARLY_UNSTRUCTURED`], the number that
/// `outline` already uses to say "this document has no structure".
pub fn outline_maps_the_page(outline: &str) -> bool {
    visible_headings(outline) >= NEARLY_UNSTRUCTURED
}

pub fn outline(md: &str) -> String {
    // Walk lines and split into sections each time we hit a heading
    // line (`# ...` through `#### ...`). We track depth so we can
    // count "(N sections)" per ancestor.
    #[derive(Debug, Clone)]
    struct Section {
        depth: usize,
        title: String,
        lines: Vec<String>,
    }
    // A long document with (almost) no headings gets its structural labels
    // promoted first — see `promote_unheaded_structure`. Cheap to decide: two
    // counts over the lines, and it bails on anything already well-structured.
    const LONG_ENOUGH_TO_NEED_AN_OUTLINE: usize = 200;
    let promoted;
    let md = {
        // Count what the WALK will see, not what the text contains. These two
        // disagreed and the fallback never fired on rfc2616: the document has
        // 21 heading LINES, all of them inside fences, so a fence-unaware count
        // said "well structured" about an outline that came out as one section
        // of 9 343 lines.
        let heads = visible_headings(md);
        if heads < NEARLY_UNSTRUCTURED && md.lines().count() >= LONG_ENOUGH_TO_NEED_AN_OUTLINE {
            // Headings we can already see but are skipping come first: they are
            // the document's own, and inventing structure on top of them would
            // be worse than using them.
            // Open the fences first, then decide what the result is worth:
            // its own nested headings if it has them, otherwise promoted
            // structural labels, otherwise nothing and we leave the document
            // exactly as it was.
            let opened = without_fence_delimiters(md);
            const REVEALED_ENOUGH: usize = 3;
            let choice = if nested_headings(&opened) >= REVEALED_ENOUGH {
                Some(opened)
            } else {
                promote_unheaded_structure(&opened)
            };
            match choice {
                Some(p) => {
                    promoted = p;
                    promoted.as_str()
                }
                None => md,
            }
        } else {
            md
        }
    };
    let mut sections: Vec<Section> = Vec::new();
    // **What precedes the first heading is CONTENT, not emptiness.**
    //
    // A scientific paper opens with its title, its authors and its abstract,
    // and its first `##` only arrives at section 3. Those lines belonged
    // to no section, so the outline threw them away: MEASURED on 2026-09-11 on
    // `pdf_mid.pdf`, the delivered outline named four sections and **carried neither the
    // title nor the abstract** — that is, precisely what one reads of a paper
    // one will not read. The preamble is therefore a section without a heading, described
    // like the others.
    // Described, not invented: the label says WHERE this text sits, it does not
    // lend it a title the document does not write.
    const PREAMBLE: &str = "(before the first heading)";
    let mut current: Option<Section> = Some(Section {
        depth: 1,
        title: PREAMBLE.to_string(),
        lines: Vec::new(),
    });
    // A `#` inside a fenced code block is a COMMENT, not a heading. Without
    // this, technical documentation outlines itself out of its own examples:
    // docs.python.org/3/library/asyncio-task produced sections titled
    // `# Wait until both tasks are completed (should take` and
    // `# Nothing happens if we just call "nested()".` — Python comments,
    // promoted to document structure. Shell, Ruby, YAML and TOML samples all
    // carry the same hazard.
    let mut in_fence = false;
    for line in md.lines() {
        if crate::fences::fence_marker(line).is_some() {
            in_fence = !in_fence;
        }
        if let Some((depth, title)) = parse_heading(line).filter(|_| !in_fence) {
            if depth <= 4 {
                if let Some(s) = current.take() {
                    // An EMPTY preamble is not a preamble: a document that
                    // opens with its heading must not gain an outline line
                    // that describes nothing.
                    if s.title != PREAMBLE || s.lines.iter().any(|l| !l.trim().is_empty()) {
                        sections.push(s);
                    }
                }
                current = Some(Section {
                    depth,
                    title,
                    lines: Vec::new(),
                });
                continue;
            }
        }
        if let Some(ref mut s) = current {
            s.lines.push(line.to_string());
        }
    }
    if let Some(s) = current.take() {
        if s.title != PREAMBLE || s.lines.iter().any(|l| !l.trim().is_empty()) {
            sections.push(s);
        }
    }
    // **The preamble is the SIBLING of the sections, not their parent.** Its depth
    // is only known once the document has been walked: set to 1 in advance, it
    // makes all the sections its children, and the "(N sections)" suffix
    // then replaces the preview — that is, the abstract, the only thing one reads
    // of a paper one will not read.
    if sections.first().is_some_and(|s| s.title == PREAMBLE) {
        let shallowest = sections.iter().skip(1).map(|s| s.depth).min().unwrap_or(1);
        sections[0].depth = shallowest;
    }
    // Render. For each section pick the first non-empty, non-list-marker
    // line as the preview and crop to 160 chars.
    let mut out = String::new();
    // Pre-compute "sections under each h1/h2" for the parent counts.
    let mut counts: Vec<usize> = vec![0; sections.len()];
    for (i, sec) in sections.iter().enumerate() {
        // Count how many later sections have depth > sec.depth, before
        // hitting another section at depth ≤ sec.depth.
        let mut n = 0usize;
        for next in &sections[i + 1..] {
            if next.depth <= sec.depth {
                break;
            }
            n += 1;
        }
        counts[i] = n;
    }
    // A heading label that repeats many times in one document is a UI WIDGET,
    // not document structure. stackoverflow.com/q/11227809 has 31 headings, of
    // which 26 are `N Comments` / `Comments`: the outline described the comment
    // widgets and said nothing about the 26 answers, which are the page. The
    // same shape on kubernetes.io (`Note:` ×11, callout boxes promoted by the
    // `<dt>` rule) and on a phpBB forum (`Antworten` / `Zugriffe` ×20, table
    // column labels).
    //
    // COLLAPSED, not deleted: the first occurrence is kept and carries the
    // count, so the outline still says a comment thread is there and the full
    // text is still in the stash for `expand-ref`. Deleting would trade one
    // kind of blindness for another.
    //
    // Measured on the 147-page corpus: 3 pages have any label repeated ≥5
    // times, and all three are widgets. Real documents do not repeat a heading
    // five times — `Syntax`, `Examples`, `References` appear once.
    const REPEATED_LABEL_IS_WIDGET: usize = 5;
    let label_key = |t: &str| {
        // `23 Comments` and `4 Comments` are the same widget.
        t.trim_start_matches(|c: char| c.is_ascii_digit() || c == ' ')
            .trim()
            .to_ascii_lowercase()
    };
    let mut label_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for sec in &sections {
        *label_counts.entry(label_key(&sec.title)).or_default() += 1;
    }
    let mut widget_seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for (i, sec) in sections.iter().enumerate() {
        let key = label_key(&sec.title);
        let repeats = label_counts.get(&key).copied().unwrap_or(1);
        if repeats >= REPEATED_LABEL_IS_WIDGET && !widget_seen.insert(key) {
            continue; // already summarised at its first occurrence
        }
        let prefix = "#".repeat(sec.depth);
        // The preview must carry information. A section that OPENS with a code
        // block used to be previewed as "```" — on man7.org/…/openat.2.html
        // that was all thirteen sections, every one of them described by the
        // same three characters. Skip the delimiter and preview the first line
        // of the code itself (`int openat(int dirfd, …)` for SYNOPSIS), which
        // is what the section is actually about. Table rules and horizontal
        // rules are dropped for the same reason: pure decoration.
        // Same failure as the man-page one above, with a different source of
        // noise: a section that OPENS WITH A FIGURE is previewed by the
        // figure's caption. On britannica.com/animal/lion all three sections
        // read `"[1 of 2](/media/1/342664/159005)"`,
        // `"[lions chasing a warthog](/media/1/342664/159006)(more)"` — the
        // outline is what an agent reads to decide what to expand, and a media
        // caption tells it nothing about the section while costing its tokens.
        // (The same page's prose previews fine once the figure line is passed:
        // doc.rust-lang.org previews are already real sentences, which is what
        // this should look like everywhere.)
        //
        // The rule is about IMAGES, not about links in general: a line whose
        // only content is a picture is not the section's prose. Markdown image
        // syntax and media-asset URLs are both removed, then the line has to
        // carry enough letters left over to be a sentence rather than an
        // affordance like `(more)`.
        // A line that BEGINS with a figure is that figure's caption, however
        // much text trails it — the caption belongs to the picture, not to the
        // section's argument. Judged on the opening only, so a sentence that
        // merely mentions an image later (`Lions [prey](…) on a large variety
        // of animals…`) is still prose and still previewed.
        let opens_with_a_figure = |l: &str| -> bool {
            let t = l.trim_start();
            // The form that actually occurs. The first version keyed on markdown image
            // syntax and on media URLs, which is what a page AUTHORED in
            // markdown carries — but tokenade's own HTML→markdown renders an
            // `<img>` as `[image: <alt>]`, and that is what every extracted
            // web page contains. Measured on a figure-per-section fixture,
            // all eight sections previewed as `"[image: plate N alt]"`; on
            // en.wikipedia.org/wiki/Ada_Lovelace three of four sections
            // previewed as the caption beneath one.
            if t.starts_with("[image:") || t == "[image]" {
                return true;
            }
            let (is_img, open) = if let Some(r) = t.strip_prefix('!') {
                (true, r)
            } else {
                (false, t)
            };
            if !open.starts_with('[') {
                return false;
            }
            let Some(close) = open.find("](") else {
                return false;
            };
            let Some(paren) = open[close + 2..].find(')') else {
                return false;
            };
            if is_img {
                return true; // `![alt](…)` is an image wherever it points
            }
            let url = open[close + 2..close + 2 + paren].to_ascii_lowercase();
            url.contains("/media/")
                || url.contains("/image")
                || [
                    ".jpg", ".jpeg", ".png", ".gif", ".webp", ".svg", ".mp4", ".webm", ".avif",
                ]
                .iter()
                .any(|e| url.contains(e))
        };
        let uninformative = |l: &str| {
            l.is_empty()
                || l.starts_with('#')
                || crate::fences::fence_marker(l).is_some()
                || l.chars()
                    .all(|c| matches!(c, '|' | '-' | ':' | ' ' | '=' | '*' | '_'))
        };
        // **A preview that quotes OUR marker says nothing about the section.**
        //
        // The preview exists so that an agent decides what to expand. Two forms
        // empty it without being empty: the elision mark we have just
        // set ourselves (`… [21 lines elided by tokenade] …`), and the line that
        // is only a service link — `[编辑⟨104⟩]`, `[edit⟨9⟩]`, the pencil
        // Wikipedia puts under each heading.
        //
        // MEASURED 2026-09-10 on the 43 pages: **85 previews out of 384**, and this
        // is not a long tail — `wikipedia_art` has **29 out of 30** and
        // `zhwiki` **46 out of 46**. On the two biggest encyclopedias in the
        // corpus, the outline one reads to choose says "edit" on every line.
        //
        // A PREFERENCE, never a rejection, for the reason in the block above:
        // emptying the preview of every section of a page makes the whole outline
        // be refused and serves the article instead. A section that has only that
        // keeps it.
        let says_nothing_of_its_own = |l: &str| {
            // A caption is better than a pencil — but a bare `[image]`
            // is not a caption. The page gave no alt text,
            // so the mark says "there is an illustration here" and nothing
            // more, which is exactly what a service link says.
            // MEASURED 2026-09-10: the TEN hollow previews of `yahoofin_quote`
            // are this shape, protected as if they were captions.
            if opens_with_a_figure(l) && l.trim_start() != "[image]" {
                return false;
            }
            // What remains once the CONSTRUCTS are removed: links, labels,
            // `⟨N⟩` references, `(×N)` template suffix. If not a single
            // word character remains, the line said nothing of its own.
            //
            // The rule is STRUCTURAL and not a length, because
            // "twelve characters" is a threshold calibrated on one script
            // and blind to the other — `[编辑]` is two and `[edit]` four,
            // and a six-character Chinese sentence is a sentence.
            //
            // **It also covers our elision marker, in its TWO
            // forms**, and an explicit clause that named it was removed
            // because a mutant survived it. Expanded, `… [21 lines
            // elided by tokenade] …` has only ellipses outside the
            // brackets; folded by the "said-once" fold, `… [21 «1»` leaves
            // the bracket OPEN, so everything after it counts as inside.
            // One rule is better than two that overlap.
            let mut depth = 0usize;
            let mut carries_a_word = false;
            for c in l.chars() {
                match c {
                    '[' | '(' | '⟨' => depth += 1,
                    ']' | ')' | '⟩' => depth = depth.saturating_sub(1),
                    _ if depth == 0 && c.is_alphanumeric() => carries_a_word = true,
                    _ => {}
                }
            }
            !carries_a_word
        };
        // A PREFERENCE, never a rejection. Dropping caption lines outright
        // emptied the preview of every section on britannica.com/animal/lion,
        // which made the outline so thin that the "a summary too small to
        // still be about what it summarises" guard refused it — and the whole
        // article was served instead. Improving a preview must never cost the
        // outline itself, so the figure line is still used when it is all the
        // section has.
        let trimmed: Vec<&str> = sec.lines.iter().map(|l| l.trim()).collect();
        // A caption is the line UNDER its figure. Skipping the figure alone
        // just promotes the caption into the preview — which is what
        // en.wikipedia.org does, where the `<figcaption>` arrives as ordinary
        // prose (`"Watercolour portrait of Ada King, c. 1840, possibly by
        // Alfred Edward Chalon"`) with no markup left to recognise it by. Its
        // POSITION is the only reliable signal, and it is enough.
        // DIRECTLY under: the blank line that separates two paragraphs also
        // separates a figure from prose that is not its caption. Skipping
        // blanks here claimed the first sentence of the section whenever the
        // figure stood in its own paragraph, which is the common case.
        let follows_a_figure = |i: usize| -> bool { i > 0 && opens_with_a_figure(trimmed[i - 1]) };
        let pick = |skip_figures: bool, skip_service_lines: bool| {
            trimmed
                .iter()
                .enumerate()
                .find(|(i, l)| {
                    !uninformative(l)
                        && !(skip_service_lines && says_nothing_of_its_own(l))
                        && !(skip_figures && (opens_with_a_figure(l) || follows_a_figure(*i)))
                })
                .map(|(_, l)| *l)
        };
        // Prose first; failing that a figure, which at least says what
        // the section is about; failing that whatever remains, even a pencil.
        let preview = pick(true, true)
            .or_else(|| pick(false, true))
            .or_else(|| pick(false, false))
            .map(|s| short_preview(s, 90))
            .unwrap_or_default();
        let line_count = sec.lines.len();
        // **`(0 lines)` teaches nothing and reads as "empty page".**
        //
        // A heading that carries no line and no sub-section is just
        // a heading. The suffix exists to say what a section CONTAINS;
        // on this one there is nothing to say, and the parenthesis invites the conclusion
        // that it is empty. Seen on the page boundary of a `web a b c` batch, where the
        // target's heading is followed by the page's own heading.
        let suffix = if counts[i] == 0 && line_count == 0 {
            String::new()
        } else if counts[i] > 0 {
            format!(" ({})", counted(counts[i], "section"))
        } else if let Some(gone) = only_our_elision_marker(&sec.lines) {
            // **A section reduced to our marker is not QUOTED, it is
            // COUNTED.** `(1 lines · "… [9 lines elided by tokenade] …")` says
            // two false things in one: the section did not carry one line
            // but nine, and the quote repeats a text we have just
            // written. The marker carries its own number; that is what is
            // worth saying. MEASURED 2026-09-10: **36 sections** of
            // `pages/zhwiki`, the page that hits the hard ceiling.
            format!(" ({} elided)", counted(gone, "line"))
        } else if !preview.is_empty() {
            format!(" ({} · \"{preview}\")", counted(line_count, "line"))
        } else {
            format!(" ({})", counted(line_count, "line"))
        };
        let suffix = if repeats >= REPEATED_LABEL_IS_WIDGET {
            format!("{suffix} — ×{repeats} on this page, collapsed")
        } else {
            suffix
        };
        let _ = writeln!(out, "{prefix} {}{}", sec.title, suffix);
    }
    if out.is_empty() {
        // No headings at all → fall back to a head-preview so caller
        // still sees something useful.
        let head: Vec<&str> = md.lines().take(20).collect();
        return head.join("\n");
    }
    out.trim_end().to_string()
}

/// Parse `# Title` / `## Title` / etc. Returns the depth (1-6) and
/// the title text on match.
///
/// A tab, or four-or-more spaces before a single `# `, is indented code
/// (`# comment` in a sample) — the same hole the fence walk already
/// closed for ``` / `~~~`. Depth ≥2 still matches when indented: PDF
/// extractors park `## Introduction` eighteen spaces in
/// (`the_preamble_is_described_as_a_section`).
pub(super) fn parse_heading(line: &str) -> Option<(usize, String)> {
    let trimmed = line.trim_start();
    let mut depth = 0;
    let bytes = trimmed.as_bytes();
    while depth < bytes.len() && bytes[depth] == b'#' && depth < 6 {
        depth += 1;
    }
    if depth == 0 || depth >= bytes.len() {
        return None;
    }
    if bytes[depth] != b' ' && bytes[depth] != b'\t' {
        return None;
    }
    // `# comment` in indented code (4 spaces / tab). Nested ATX (`##`)
    // still matches when indented: PDF extractors park `## Introduction`
    // eighteen spaces in.
    if depth == 1 && (line.starts_with('\t') || line.bytes().take_while(|&b| b == b' ').count() > 3)
    {
        return None;
    }
    Some((depth, trimmed[depth + 1..].trim().to_string()))
}

/// **`1 lines` is a mistake the agent reads on every folded section.**
///
/// A page's outline announces what each section contains, and it wrote the
/// plural unconditionally: MEASURED 2026-09-09 on the 39 pages of the corpus,
/// **19 `(1 lines)` on 6 pages and 13 `(1 sections)`**. It is the most
/// travelled route of the product, and this string goes out on every section.
///
/// A single declarer, so that the next count added to the outline does not raise
/// the question again. Nothing RE-READS this form — verified: the `N lines` the
/// product reparses (`elision::weight_of`) come from another writer and
/// keep their hinge.
pub fn counted(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// How many lines a section lost, when it has NOTHING left but the marker
/// that says so.
///
/// `None` as soon as a non-empty line is not a marker: the section still carries
/// something, and that something is what must be quoted.
///
/// Both forms count. Expanded, `… [9 lines elided by tokenade] …`;
/// folded by the "said-once" fold, `… [9 «1»`, where the common sentence was
/// output once at the top of the document. The number is in the same place in
/// both, right after `… [`, and it is the only thing asked of it.
pub(super) fn only_our_elision_marker(lines: &[String]) -> Option<usize> {
    let mut total = 0usize;
    let mut seen = false;
    for l in lines {
        let t = l.trim();
        if t.is_empty() {
            continue;
        }
        let rest = t.strip_prefix("… [")?;
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            return None;
        }
        total += digits.parse::<usize>().ok()?;
        seen = true;
    }
    seen.then_some(total)
}

pub(super) fn short_preview(s: &str, cap: usize) -> String {
    if s.chars().count() <= cap {
        return s.replace('\"', "'");
    }
    let truncated: String = s.chars().take(cap).collect();
    let truncated = crate::textshape::cut_before_an_unclosed_bracket(&truncated);
    format!("{}…", truncated.replace('\"', "'"))
}
