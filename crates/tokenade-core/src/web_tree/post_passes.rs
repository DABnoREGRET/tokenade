//! **The AFTER passes: what is removed once the markdown is written.**
//!
//! A relative address without an origin is not clickable, a replacement text
//! that the neighbouring sentence repeats costs bytes for nothing. These
//! passes read the RENDERING, not the tree — and the "already said" rule had
//! to be tightened three times: it found 39 sites, 3 remain.

use super::*;

/// The heading of the reference-resolution table, spelled ONCE — the writer
/// below and the reader that annotates it must talk about the same one.
///
/// Not `## References`: that is the heading the page carries itself. A
/// Wikipedia page came out with TWO `## References` (its bibliography, then
/// our `⟨N⟩` targets), with nothing saying which is which (2026-10-03).
pub const REFERENCES_HEADING: &str = "## Link targets";

/// **A relative target without its origin cannot be followed.**
///
/// The references keep the URL as the page writes it, and a page writes
/// `/news/2026/7/21/…`. MEASURED 2026-09-09 on the 39 corpus pages: **248
/// relative targets delivered across 15 pages, and the page's origin appears
/// in NO output** — nothing, anywhere, says which host to resolve them under.
/// The table promises a resolution and delivers something unfollowable.
///
/// The origin is written on the HEADING rather than copied onto every entry:
/// once instead of 248 times, and above all the heading is what survives the
/// outline when the page is too big to be served whole — the entry, on the
/// other hand, disappears.
///
/// Silent when nothing is relative, and silent when the URL is not a real
/// origin: a local file is distilled under `https://example.invalid/`, and
/// announcing that origin would be worse than staying quiet.
pub fn note_the_origin_of_relative_references(md: String, url: &str) -> String {
    let Some(origin) = page_origin(url) else {
        return md;
    };
    let heading_line = format!("{REFERENCES_HEADING}\n");
    if md.contains(&heading_line) && md.lines().any(|l| l.starts_with('⟨') && l.contains("⟩ /"))
    {
        return md.replacen(
            &heading_line,
            &format!("{REFERENCES_HEADING} — relative entries are under {origin}\n"),
            1,
        );
    }
    // **The guarantee only held above a threshold nobody stated.**
    //
    // The references table only exists from six links up. Below that, the
    // addresses stay INLINE — and the origin was named nowhere. That is the
    // case where the agent needs it most: a small page is often a
    // documentation leaf or a redirect, where nothing else says which host to
    // resolve `../neighbour` under.
    //
    // Two shapes escape the table even on a LARGE page, because the
    // references pattern only accepts an address that is absolute or starts
    // with `/`: a target in `?tab=x` and one in `../` are never numbered.
    // Where the heading already carries the announcement, they benefit from
    // it; otherwise they were silent on both sides.
    // **And the table can lose its heading.** The hard cap keeps one line in
    // N without knowing anything about the structure: on `pages/zhwiki` it
    // takes away the line `## Link targets — relative entries are under …` and
    // leaves the entries, so 59 relative targets were still delivered without
    // a host. The first arm no longer finds its heading, the second finds
    // nothing INLINE, and the origin guarantee fell between the two.
    //
    // Both phrasings are tested, not only the heading one: the function is
    // called twice — once on the rendering, once on what is left after the
    // cap — and duplicating the announcement would be the same bug in
    // reverse.
    if md.contains("relative entries are under") || md.contains(ORIGIN_NOTE_OPENING) {
        return md;
    }
    if !carries_an_inline_relative_target(&md) && !carries_a_numbered_relative_entry(&md) {
        return md;
    }
    format!("{ORIGIN_NOTE_OPENING}{origin}]\n\n{md}")
}

/// The leading announcement, up to the origin excluded. A constant because it
/// is now written in one place and READ BACK in another — an idempotence
/// guard looking for a hand-copied sentence is a defect already paid for
/// once.
pub(super) const ORIGIN_NOTE_OPENING: &str = "[tokenade: relative links below resolve under ";

/// A NUMBERED entry of the references table whose target is relative.
///
/// The same question as `carries_an_inline_relative_target`, asked of the
/// other shape: `⟨12⟩ /wiki/Foo`. That is what a relative link becomes as
/// soon as the page carries enough of them to deserve a table.
pub(super) fn carries_a_numbered_relative_entry(md: &str) -> bool {
    md.lines().any(|l| l.starts_with('⟨') && l.contains("⟩ /"))
}

/// A link address that needs an origin to be followed.
///
/// The three shapes the specification makes UNambiguous: `/from-the-root`,
/// `?query-only` and `../` or `./`. A `#anchor` fragment resolves within the
/// page itself and needs nothing; a bare address (`neighbour/page`) is
/// discarded because it cannot be told apart from text in parentheses, and a
/// false note would cost more than silence.
pub(super) fn carries_an_inline_relative_target(md: &str) -> bool {
    let mut rest = md;
    while let Some(at) = rest.find("](") {
        let after = &rest[at + 2..];
        if after.starts_with('/')
            || after.starts_with('?')
            || after.starts_with("../")
            || after.starts_with("./")
        {
            return true;
        }
        rest = after;
    }
    false
}

/// How many meaningful words are needed before judging an alt redundant.
/// Below that, coincidence is possible: two common words do not make a
/// duplicate.
pub(super) const ALT_WORDS_TO_JUDGE: usize = 3;

/// A word long enough to carry meaning. Articles and prepositions repeat
/// everywhere and say nothing about resemblance.
pub(super) const ALT_WORD_MIN_CHARS: usize = 4;

/// **An image description that its neighbourhood already says word for
/// word.**
///
/// On a news home page, an article's thumbnail carries the article's TITLE as
/// its `alt`, and the title is right next to it, spelled out. The output then
/// delivers the same sentence twice: `[image: Funcionaria PDI enfrenta a
/// asaltantes…]` followed by the link that says exactly that. MEASURED
/// 2026-09-09 on the 39 corpus pages: **28 descriptions, 2,485 bytes, across
/// 6 pages.**
///
/// **And the example above was not one of them.** A news card is written
/// `link` / EMPTY line / `## title`: the neighbour below was the blank, and
/// the title two lines away. Re-measured 2026-09-10 counting only non-empty
/// lines: **78 more descriptions across 5 pages, 8,400 bytes**, 73 of them on
/// the `latercera.com` home page alone — 14.6% of what it delivered, each
/// title served twice. The rule had been citing its own target without ever
/// reaching it.
///
/// Lossless by CONSTRUCTION, like the twin rule: the removal only happens if
/// the description is found **as-is**, as a contiguous phrase, in the line
/// before, the line after or the rest of its own.
///
/// Three successive tightenings, each paid for by reading the diff, and that
/// is the point of this entry:
/// 1. "80% of the words" dropped one word in five;
/// 2. "all the words" counted those that were ONLY in a URL —
///    `[\[image: See Deals from Intel\]](…/Intel/BrandStore/…)` passed as
///    redundant, and emptying a link's only label leaves a mute link;
/// 3. "all the words" was still wrong even in prose: on slack.com, four
///    DIFFERENT award badges follow one another, and the fourth lost its name
///    because its words were lying around, scattered, in the neighbourhood.
///    Scattered words are not a sentence said.
///
/// The MARK stays: `[image]` says there is an illustration there, which the
/// neighbouring text does not say. Only the repetition goes.
pub fn drop_alts_their_neighbourhood_already_says(md: String) -> String {
    if !md.contains("[image: ") {
        return md;
    }
    let lines: Vec<&str> = md.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        let mut rebuilt = String::with_capacity(line.len());
        let mut rest = *line;
        while let Some(at) = rest.find("[image: ") {
            // **The bracket may be ESCAPED, and the backslash belongs to the
            // delimiter, not to the description.** The link references write
            // `[\[image: …\]](url)`: taking the alt up to the `]` glued a
            // backslash onto it, which broke the comparison AND the
            // sole-label guard. Measured while writing their test.
            let from = at + "[image: ".len();
            let Some(close) = rest[from..].find(']') else {
                break;
            };
            let close = from + close;
            let escaped = rest[..close].ends_with('\\');
            let alt_end = if escaped { close - 1 } else { close };
            let end = close + 1;
            let alt = &rest[from..alt_end];
            let delimiter = &rest[alt_end..end];
            let phrase = normalise_for_comparison(alt);
            let carrying = phrase
                .split(' ')
                .filter(|w| w.chars().count() >= ALT_WORD_MIN_CHARS)
                .count();
            // **EMPTY lines are not neighbours.** A news card is written
            // `link` / empty line / `## title`, so the neighbour below was the
            // blank and the title two lines away. This function's comment
            // cites `Funcionaria PDI enfrenta a asaltantes…` as its example,
            // and that example did not reach it.
            let above = nearest_prose_line(&lines, i, -1);
            let below = nearest_prose_line(&lines, i, 1);
            let neighbourhood = prose_only(&[above, below, &rest[..at], &rest[end..]].join(" "));
            // Emptying a link's only label leaves a mute link — except when a
            // neighbouring HEADING says the same sentence: there, what the
            // link leads to is named one line below, spelled out.
            let named_by_a_heading = [above, below]
                .iter()
                .any(|l| is_a_heading(l) && normalise_for_comparison(l).contains(&phrase));
            let redundant = carrying >= ALT_WORDS_TO_JUDGE
                && (named_by_a_heading || !is_the_whole_label_of_a_link(&rest[..at], &rest[end..]))
                && normalise_for_comparison(&neighbourhood).contains(&phrase);
            rebuilt.push_str(&rest[..at]);
            if redundant {
                rebuilt.push_str("[image");
                rebuilt.push_str(delimiter);
            } else {
                rebuilt.push_str(&rest[at..end]);
            }
            rest = &rest[end..];
        }
        rebuilt.push_str(rest);
        out.push(rebuilt);
    }
    let mut joined = out.join("\n");
    if md.ends_with('\n') {
        joined.push('\n');
    }
    joined
}

/// The nearest non-empty line in the given direction, without leaving the
/// document. An empty line says nothing; treating it as the neighbour amounts
/// to having no neighbour at all.
pub(super) fn nearest_prose_line<'a>(lines: &[&'a str], from: usize, step: isize) -> &'a str {
    let mut i = from as isize;
    loop {
        i += step;
        if i < 0 || i as usize >= lines.len() {
            return "";
        }
        let l = lines[i as usize];
        if !l.trim().is_empty() {
            return l;
        }
    }
}

/// An ATX heading, and nothing else: leading `#`s then a space.
pub(super) fn is_a_heading(line: &str) -> bool {
    let t = line.trim_start();
    let hashes = t.chars().take_while(|c| *c == '#').count();
    (1..=6).contains(&hashes) && t[hashes..].starts_with(' ')
}

/// **A mark must fit on ITS line.**
///
/// An `alt` attribute can contain line breaks — the Rust book and the
/// Guardian write some, because their source is markdown or hand-formatted
/// HTML. Copied as-is, `[image: …]` opened on one line and closed three lines
/// further down, and the lines in between read like prose: MEASURED
/// 2026-09-10 on the 39 pages, **7 marks across 2 pages** were in this case.
///
/// It is the rule code fences already hold ("a fence MUST own its line"):
/// everything line-oriented downstream — a large page's outline, the run
/// pruners, the truncated preview — otherwise sees a construct cut in two.
pub(crate) fn one_line(text: &str) -> String {
    if !text.contains(['\n', '\r', '\t']) {
        return text.to_string();
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Lowercased, and everything non-alphanumeric reduced to a space: two ways
/// of writing the same sentence then compare directly.
pub(super) fn normalise_for_comparison(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = true;
    for c in text.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
            space = false;
        } else if !space {
            out.push(' ');
            space = true;
        }
    }
    out.trim_end().to_string()
}

/// The neighbourhood without its ADDRESSES: a URL is not prose, and a word
/// that appears only there is not "already said".
///
/// Measured by reading the diff: `[\[image: See Deals from Intel\]](…/Intel/BrandStore/…)`
/// passed as redundant because "intel" is in its URL.
pub(super) fn prose_only(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("](") {
        out.push_str(&rest[..at]);
        match rest[at..].find(')') {
            Some(close) => rest = &rest[at + close + 1..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Is the description the ONLY label of its link?
///
/// `[\[image: See Deals from Intel\]](url)` has only that to say about the
/// link: emptying it would leave `[\[image]](url)`, a link of which nothing
/// says where it leads any more. The duplicate costs bytes; that one would
/// cost the meaning.
pub(super) fn is_the_whole_label_of_a_link(before: &str, after: &str) -> bool {
    before.trim_end().ends_with("[\\") && after.trim_start().starts_with("](")
}
