//! **What is not text: formula, diagram, video, code.**
//!
//! Each has a DECLARED form that must be preferred over what a name suggests —
//! and the source sometimes writes the same fact three times (hidden MathML,
//! TeX annotation, replacement text), in which case delivering all three is a
//! fabrication, not fidelity.

use super::*;

/// Collapse inline markdown links/images to numbered citations + a `## Link targets`
/// block (crawl4ai). `[text](url)` → `text⟨N⟩`, deduped by URL, so a page with
/// many repeated/long URLs (citation-heavy articles, link farms) costs far fewer
/// tokens with no information lost. No-op below a link threshold, where the
/// References overhead would not pay for itself.
pub fn compact_citations(md: &str) -> String {
    let re = match regex::Regex::new(
        // **A BARE bracket is not a label, an ESCAPED bracket is.**
        //
        // The class forbids `[` as much as `]`, and that is the half that was
        // missing the longest. `[Submitted on 12 Jun 2017 ([v1](url)), …]` —
        // the dateline arXiv puts in brackets on EVERY article — saw its label
        // start at the literal bracket at the beginning: the link swallowed
        // twenty-eight characters of prose, the opening `[` vanished from the
        // page (it is eaten by the pattern, not re-emitted) and the closing `]`
        // ended up alone at the end of the line. Delivered: `([v1⟨1⟩), …v7)]`,
        // brackets that no longer pair up and one source character lost.
        //
        // **The label may contain an ESCAPED bracket.** `[^\]\n]+` refuses any
        // `]`, the backslash in front changes nothing — so a link whose label
        // is an image mark, `[\[image\]](url)`, did not match and its address
        // stayed IN THE CLEAR while every other one on the page became `⟨N⟩`.
        // On the `latercera.com` home page: 73 addresses, 9,287 bytes, 20% of
        // what that page renders — and each one is already delivered by its
        // card's title, which carries the same numbered link just below.
        r#"(!?)\[((?:[^\[\]\n\\]|\\.)+)\]\((https?://[^)\s]+|/[^)\s]*)(?:\s+"[^"]*")?\)"#,
    ) {
        Ok(r) => r,
        Err(_) => return md.to_string(),
    };
    if re.find_iter(md).count() < 6 {
        return md.to_string();
    }
    let mut urls: Vec<String> = Vec::new();
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let body = re
        .replace_all(md, |c: &regex::Captures| {
            let img = &c[1];
            let text = c[2].trim();
            let url = c[3].to_string();
            let n = *seen.entry(url.clone()).or_insert_with(|| {
                urls.push(url.clone());
                urls.len()
            });
            format!("{img}{text}⟨{n}⟩")
        })
        .into_owned();
    if urls.is_empty() {
        return md.to_string();
    }
    let mut out = body.trim_end().to_string();
    out.push_str(&format!("\n\n{REFERENCES_HEADING}\n"));
    for (i, u) in urls.iter().enumerate() {
        out.push_str(&format!("⟨{}⟩ {}\n", i + 1, u));
    }
    out
}

/// `<sup>` / `<sub>` content with a boundary marker — but only where losing the
/// boundary would change what the text SAYS.
///
/// Both tags used to fall through to the generic "concatenate the children"
/// arm, which turns `4<sup>2</sup>` into `42` and `H<sub>2</sub>O` into `H2O`.
/// That is not a formatting nit: the agent reads a different number than the
/// page shows, with nothing to indicate it. A footnote marker glues to the word
/// it annotates for the same reason (`fact<sup>1</sup>` → `fact1`).
///
/// Purely ALPHABETIC content keeps the old gluing on purpose: `1<sup>st</sup>`
/// is `1st` in prose and `1^st` would be the noisier, less faithful rendering.
/// So the marker is emitted exactly when the content contains something other
/// than letters — which is where the ambiguity lives.
pub(super) fn raised_or_lowered(is_sup: bool, s: &str) -> String {
    let t = s.trim();
    if t.is_empty() {
        return String::new();
    }
    if t.chars().all(|c| c.is_alphabetic()) {
        return t.to_string();
    }
    let marker = if is_sup { '^' } else { '_' };
    format!("{marker}{t}")
}

/// Longest run of consecutive backticks in `s` (0 if none).
pub(super) fn longest_backtick_run(s: &str) -> usize {
    let mut max = 0;
    let mut cur = 0;
    for c in s.chars() {
        if c == '`' {
            cur += 1;
            max = max.max(cur);
        } else {
            cur = 0;
        }
    }
    max
}

/// Inline code with CommonMark-correct backtick escalation (htmd's rule): the
/// delimiter must be longer than any backtick run inside, and content that
/// starts/ends with a backtick is space-padded so it can't merge with the fence.
pub(super) fn inline_code(s: &str) -> String {
    let s = s.trim();
    if s.is_empty() {
        return String::new();
    }
    let ticks = "`".repeat(longest_backtick_run(s) + 1);
    let pad = if s.starts_with('`') || s.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{ticks}{pad}{s}{pad}{ticks}")
}

/// A fenced code block whose fence is one backtick longer than the longest run
/// inside (≥3), so example code containing a ``` fence doesn't close early.
pub(super) fn code_fence(code: &str, lang: &str) -> String {
    let fence = "`".repeat(longest_backtick_run(code).max(2) + 1);
    format!("\n\n{fence}{lang}\n{}\n{fence}\n\n", code.trim_end())
}

/// `<svg id="mermaid-*">` / `class="mermaid"` — recover the diagram's label text
/// as a ```mermaid fence before the SVG is dropped as noise (crawl4ai's rescue).
pub(super) fn mermaid_fence(n: NodeRef<Node>) -> Option<String> {
    let id = attr(n, "id").unwrap_or("");
    let class = attr(n, "class").unwrap_or("");
    if !id.contains("mermaid") && !class.contains("mermaid") {
        return None;
    }
    let mut text = String::new();
    for d in n.descendants() {
        if let Node::Text(t) = d.value() {
            let s = t.text.trim();
            if !s.is_empty() {
                text.push_str(s);
                text.push('\n');
            }
        }
    }
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    Some(format!("\n\n```mermaid\n{text}\n```\n\n"))
}

/// The TeX of a formula read from a FRAGMENT — the same rule as the
/// rendering, exposed for the fidelity judge.
///
/// The source writes the same formula THREE times (presentation MathML, TeX
/// `<annotation>`, the fallback image's `alt`) and the rendering delivers it
/// once. `bag_recall` counts OCCURRENCES: the reference must therefore count
/// only one, otherwise a correct rendering reads as a loss. Two spellings of
/// "the TeX the page declares" would make the rule and its judge diverge —
/// the same reason that already made [`is_machine_only_markup`] public.
pub fn tex_of_math_fragment(fragment: &str) -> Option<String> {
    let doc = Html::parse_fragment(fragment);
    let math = doc
        .tree
        .root()
        .descendants()
        .find(|d| el_name(*d) == Some("math"))?;
    math_tex(math)
}

/// Does this image's `alt` repeat a formula already rendered?
///
/// The general `alt`-duplicate rule cannot answer: it requires three words of
/// at least four letters to judge, and a TeX string often has only ONE word in
/// the whitespace sense — `{\displaystyle a^{2}+b^{2}=c^{2}.}`. Out of the 99
/// formulas of `pages/wikipedia_math` it caught only one.
///
/// The question asked here is narrower and exact: does a `<math>` that
/// PRECEDES this image declare exactly this string? No threshold, no word
/// count, and the bound is the siblings — a formula from another paragraph
/// cannot silence an image.
pub(super) fn alt_repeats_a_formula(img: NodeRef<Node>, alt: &str) -> bool {
    for prev in img.prev_siblings() {
        let math = if el_name(prev) == Some("math") {
            Some(prev)
        } else {
            prev.descendants().find(|d| el_name(*d) == Some("math"))
        };
        if let Some(m) = math {
            if math_tex(m).as_deref() == Some(alt) {
                return true;
            }
        }
    }
    false
}

/// The TeX a formula DECLARES — the `alttext` attribute of `<math>`, failing
/// that the `<annotation encoding="application/x-tex">` of its `<semantics>`.
///
/// Both are in the MathML specification and both carry the same string on
/// the 99 formulas of `pages/wikipedia_math`. The attribute is read first
/// because it is on the element itself: an `<annotation>` of a NESTED
/// `<math>` then cannot be mistaken for the parent's.
pub(super) fn math_tex(n: NodeRef<Node>) -> Option<String> {
    if let Some(t) = attr(n, "alttext") {
        let t = one_line(t.trim());
        if !t.is_empty() {
            return Some(t);
        }
    }
    for d in n.descendants() {
        if el_name(d) != Some("annotation") {
            continue;
        }
        // `encoding` says what the annotation CONTAINS. Without this bound, a
        // content-MathML annotation (`application/mathml-content+xml`) would
        // go out as if it were TeX.
        let enc = attr(d, "encoding").unwrap_or("");
        if !enc.contains("tex") {
            continue;
        }
        let t = one_line(subtree_text(d).trim());
        if !t.is_empty() {
            return Some(t);
        }
    }
    None
}

/// The accessible name of a vector ILLUSTRATION — or nothing at all.
///
/// An `<svg>` is discarded by [`is_dropped`] because that is how a page draws
/// its ICONS. Measured 2026-09-10 on the 39 corpus pages: **85 `<svg>` carry a
/// non-empty `<title>`, across 14 pages**, and they are all scenery labels —
/// "play", "search", "caret-left", "facebook", "rss", "GOV.UK", "Logo La
/// Tercera". Rendering their title would add 85 scenery marks to the corpus,
/// and a threshold on the word count would decide by coin toss ("British
/// Broadcasting Corporation" is three words, "Schema du pipeline de
/// compaction" four).
///
/// Two forms from the specification separate the DIAGRAM from the icon, and
/// **none of the 85 carries them**:
///  * a `<desc>`, the long description, which an icon never writes;
///  * a `<text>`, that is, a graphic that DRAWS words — which a labelled
///    diagram does, and no glyph does.
///
/// The rendered name is the `<title>`, failing that the `<desc>`: that is the
/// order in which a screen reader announces them, and it is an illustration's
/// `alt`.
pub(super) fn svg_illustration(n: NodeRef<Node>) -> Option<String> {
    let mut title: Option<String> = None;
    let mut desc: Option<String> = None;
    let mut draws_words = false;
    for d in n.descendants() {
        match el_name(d) {
            Some("title") if title.is_none() => {
                let t = one_line(subtree_text(d).trim());
                if !t.is_empty() {
                    title = Some(t);
                }
            }
            Some("desc") if desc.is_none() => {
                let t = one_line(subtree_text(d).trim());
                if !t.is_empty() {
                    desc = Some(t);
                }
            }
            Some("text") => draws_words = true,
            _ => {}
        }
    }
    if desc.is_none() && !draws_words {
        return None;
    }
    Some(crate::markdown::image_marker(Some(
        title.or(desc)?.as_str(),
    )))
}

/// Known video-embed iframe hosts → a canonical markdown link, so an `<iframe>`
/// that would otherwise be dropped becomes a meaningful reference (jina's table).
pub(super) fn video_link(n: NodeRef<Node>) -> Option<String> {
    const HOSTS: [(&str, &str); 7] = [
        ("youtube.com", "YouTube"),
        ("youtube-nocookie.com", "YouTube"),
        ("youtu.be", "YouTube"),
        ("vimeo.com", "Vimeo"),
        ("dailymotion.com", "Dailymotion"),
        ("bilibili.com", "Bilibili"),
        ("twitch.tv", "Twitch"),
    ];
    let src = attr(n, "src")?.trim();
    if src.is_empty() {
        return None;
    }
    for (host, label) in HOSTS {
        if src.contains(host) {
            let url = if let Some(rest) = src.strip_prefix("//") {
                format!("https://{rest}")
            } else {
                src.to_string()
            };
            return Some(format!("\n\n[{label} video]({url})\n\n"));
        }
    }
    None
}

/// Wrap the first heading line's text in a markdown link — used when a card
/// anchor wraps a heading, so `## Title` becomes `## [Title](href)`.
///
/// **And a card WITHOUT a heading kept its text while losing its address.**
/// The link wraps the whole block; if it lands on no line, it is written
/// nowhere. MEASURED 2026-09-12 on the franceinfo home page, whose cards put
/// their title in a `<p class="…title">` rather than an `<hN>`: **12,372
/// bytes** of addresses lost at once as soon as those cards were unfolded as
/// blocks. Failing a heading, the first line that says something carries the
/// link — never the `[image: …]` mark, which would render
/// `[\[image: …\]](url)` and leave the title bare, nor a line that is ALREADY
/// a link.
pub(super) fn linkify_first_heading(s: &str, href: &str) -> String {
    let mut done = false;
    let linkable = |t: &str| {
        !t.is_empty()
            && !t.starts_with("[image")
            && !t.contains("](")
            && t.chars().any(char::is_alphanumeric)
    };
    let out: Vec<String> = s
        .lines()
        .map(|line| {
            if done {
                return line.to_string();
            }
            let t = line.trim_start();
            if let Some(hashes) = atx_level(t) {
                let text = t[hashes..].trim();
                if !text.is_empty() && !text.starts_with('[') {
                    done = true;
                    return format!("{} [{}]({})", "#".repeat(hashes), text, href);
                }
            }
            line.to_string()
        })
        .collect();
    if done {
        return out.join("\n");
    }
    // **The price, measured and accepted.** On a Newegg product page, a
    // seller's rating ("4.2", "87 total ratings") is wrapped in an `<a>`
    // without a heading: it therefore takes the link, and a short block made
    // mostly of link is a navigation cluster for the density pruner, which
    // cut it — **369 characters on that page**, against 12,772 recovered on
    // ten others. A guard "put nothing on a card that already carries a link"
    // was written then REMOVED: measured on the corpus AND on a hand-written
    // page, it never fires, because a nested `<a>` is moved out of the
    // wrapper by the parser (a guard that cannot fire goes).
    let mut done = false;
    out.into_iter()
        .map(|line| {
            let t = line.trim();
            if done || !linkable(t) {
                return line;
            }
            done = true;
            let indent = &line[..line.len() - line.trim_start().len()];
            format!("{indent}[{t}]({href})")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// **A region's `heading` role says it is one.**
///
/// Same move as for landmark roles: the page DECLARES, we do not interpret.
/// An accordion component puts its label in a `<button>` — a discarded
/// element, because a button is a command and not text — wrapped in a
/// `<div role="heading" aria-level="3">`. MEASURED 2026-09-10 on the 39 pages:
/// on an IKEA product page, the labels "Materials and care" and "Good to
/// know" disappeared while their panels were delivered, so the material and
/// the care instructions arrived without saying what they were about.
///
/// A level outside [1, 6] is clamped rather than refused: ARIA allows
/// `aria-level="7"`, markdown does not, and a too-deep heading is better than
/// a lost one. Without `aria-level`, the level is 2 — what the specification
/// gives as the default.
///
/// Nothing is done when the region ALREADY carries a real `<hN>`: the heading
/// exists, and placing a second one would invent one.
pub(super) fn declares_itself_a_heading(n: NodeRef<Node>) -> Option<usize> {
    if attr(n, "role")?.split_whitespace().next()? != "heading" {
        return None;
    }
    if n.descendants()
        .any(|d| matches!(el_name(d), Some("h1" | "h2" | "h3" | "h4" | "h5" | "h6")))
    {
        return None;
    }
    let level = attr(n, "aria-level")
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(2);
    Some(level.clamp(1, 6))
}
