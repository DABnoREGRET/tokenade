//! What the page SHOWS to nobody.
//!
//! A subject distinct from the neighbour's: `prune.rs` discards what is
//! painted but is not what the page is about (nav, footer, consent banner,
//! "read also"); here, nothing is painted for the reader's eye — a
//! presentation duplicate already removed from the bytes by `tree_dedup`, an
//! element hidden by CSS, an `aria-hidden` twin whose text is already
//! delivered elsewhere, microdata written for robots, a tooltip that only
//! opens on hover.
//!
//! The boundary matters, because the two criteria are wrong in different
//! ways: `sr-only` is INVISIBLE to the eye and yet delivered (a screen reader
//! reads it to someone), and discarding `aria-hidden` wholesale destroyed
//! 35,154 characters out of 46,876. Every rule here carries its measurement.

use super::*;

/// A presentational duplicate of an earlier sibling run (`tree_dedup`): removed
/// from the walk exactly as the tree-sitter pre-pass removed it from the source
/// bytes. Consulted from every entry point that can render an element —
/// `block`, `inline_into` AND `list_item`, since the list arm calls the latter
/// directly and would otherwise bypass the guard.
pub(super) fn is_dup_drop(n: NodeRef<Node>) -> bool {
    DUP_DROPS.with(|d| {
        let d = d.borrow();
        !d.is_empty() && d.contains(&n.id())
    })
}

/// Classes that hide an element from the EYE while keeping it for assistive
/// tech — the standard visually-hidden idiom under its several names.
pub(super) fn is_visually_hidden(n: NodeRef<Node>) -> bool {
    let Node::Element(e) = n.value() else {
        return false;
    };
    e.attr("class").is_some_and(|c| {
        c.split_whitespace().any(|w| {
            let w = w.to_ascii_lowercase();
            HIDDEN_STEMS.iter().any(|stem| w.contains(stem))
                || w == "sr"
                || w.starts_with("sr-")
                || w.ends_with("-sr")
        })
    })
}

/// The ROOTS of the "read by a screen reader, invisible to the eye" classes.
///
/// The list was an EQUALITY on seven exact names, and stackoverflow.com writes
/// its own `v-visible-sr`: the eighth went by silently, and with it the twin
/// it announces. MEASURED 2026-09-09 on the corpus's frozen question, the
/// delivered line said `36.4k2626 gold badges7373 silver badges8080 bronze
/// badges` — **every number doubled**, exactly what the twin rule exists to
/// prevent. A root plus an `sr` affix also covers the common utility prefixes
/// (`u-sr-only`, `is-sr-only`) that an equality could not see.
pub(super) const HIDDEN_STEMS: &[&str] = &[
    "sr-only",
    "visually-hidden",
    "visuallyhidden",
    "screen-reader",
    "screenreader",
    "a11y-hidden",
    "hidden-visually",
];

/// Text of a subtree, concatenated the way a browser lays inline content out:
/// NO separator between nodes, so `<span>W</span>hile` is `While` and not
/// `W hile`. Inserting one here would invent a space the page never renders —
/// and would hide the very twins this exists to find.
pub(super) fn subtree_text(n: NodeRef<Node>) -> String {
    let mut s = String::new();
    for d in n.descendants() {
        if let Node::Text(t) = d.value() {
            s.push_str(&t.text);
        }
    }
    collapse_ws(&s).trim().to_string()
}

/// **This node carried ONLY whitespace.**
///
/// [`subtree_text`] trims its edges, so it does not tell an empty element
/// from an element whose whole content is a space — and that space is the
/// only thing the element contributed. MEASURED on ikea.com/gb/en/cat:
/// `matching<a>\u{a0}</a><a>desks, </a>` rendered `matchingdesks`, a word the
/// page does not contain, because the link holding the non-breaking space
/// yields an empty string and disappears along with its only contribution.
/// Same family as the `WhileWhile` / `DeDe` that got the twin rule written
/// just below.
pub(super) fn holds_only_whitespace(n: NodeRef<Node>) -> bool {
    let mut seen = false;
    for d in n.descendants() {
        if let Node::Text(t) = d.value() {
            if !t.text.is_empty() {
                seen = true;
            }
            if t.text.chars().any(|c| !c.is_whitespace()) {
                return false;
            }
        }
    }
    seen
}

/// A drop cap and its screen-reader twin: the same word marked up TWICE, once
/// `aria-hidden="true"` for the eye (the oversized initial plus the rest of the
/// word) and once `sr-only` for assistive tech. A browser renders one, a screen
/// reader reads the other; we were emitting both, which FABRICATED tokens —
/// `WhileWhile`, `DeDe` on theverge.com — that then went on to be indexed and
/// embedded as if they were words the page contained.
///
/// Deliberately NOT "honour `aria-hidden`": measured over the 147-page corpus,
/// 35 154 of 46 876 characters inside `aria-hidden` subtrees appear NOWHERE
/// else (supabase marketing copy, react API names, healthline nav). Dropping
/// the attribute wholesale would destroy them. This rule fires only when an
/// adjacent sibling already carries the text, so it is lossless by
/// construction rather than by measurement.
///
/// **The twin is not always identical: it can be more COMPLETE.**
/// stackoverflow.com marks up a badge counter as `<span aria-hidden><span
/// class="badgecount">196</span></span>` then, right after,
/// `<span class="v-visible-sr">196 silver badges</span>` — the accessible copy
/// carries the number AND its label. Strict equality did not see that twin,
/// and the delivered line said `8080 gold badges196196 silver
/// badges254254 bronze badges`: every number DOUBLED, exactly the
/// `WhileWhile` / `DeDe` this rule exists to prevent.
pub(super) fn is_aria_hidden_twin(n: NodeRef<Node>) -> bool {
    let Node::Element(e) = n.value() else {
        return false;
    };
    if e.attr("aria-hidden").map(str::trim) != Some("true") {
        return false;
    }
    let mine = subtree_text(n);
    if mine.is_empty() {
        return false;
    }
    // The twin sits on either side depending on whether the markup leads with
    // the visual copy or the accessible one.
    let mut neighbours = n
        .next_siblings()
        .find(|s| matches!(s.value(), Node::Element(_)))
        .into_iter()
        .chain(
            n.prev_siblings()
                .find(|s| matches!(s.value(), Node::Element(_))),
        );
    neighbours.any(|s| is_visually_hidden(s) && twin_carries(&subtree_text(s), &mine))
}

/// Does the twin's text already carry mine?
///
/// Identical, or mine followed by a space — "196" in "196 silver badges". The
/// prefix-plus-space is what keeps the rule lossless by CONSTRUCTION: free
/// containment would remove `1` because a neighbour says `2100`.
pub(super) fn twin_carries(twin: &str, mine: &str) -> bool {
    if twin == mine {
        return true;
    }
    twin.strip_prefix(mine)
        .is_some_and(|rest| rest.starts_with(char::is_whitespace))
}

/// schema.org microdata stored in an element the browser paints FOR NOBODY:
/// markup for MACHINES, next to the text the page actually displays.
///
/// Found on `stackexch_q`. Stack Exchange writes the author and date of each
/// comment TWICE — once for the reader, once for robots:
///
/// ```html
/// <div class="d-none" itemprop="name">Jane Doe</div>
/// &ndash;&nbsp;<a class="comment-user">Jane Doe</a>
/// <div class="d-none" itemprop="datePublished">2021-07-19 17:04:42 +00:00</div>
/// <span class="comment-date">Commented <span>Jul 19, 2021 at 17:04</span></span>
/// ```
///
/// We rendered both, so every comment reached the agent with its author
/// doubled and its date doubled: "Jane Doe – Jane Doe⟨17⟩ 2021-07-19
/// 17:04:42 +00:00 Commented Jul 19, 2021 at 17:04". Same fabrication as the
/// drop caps' `WhileWhile`, on a kind of page an agent reads every day.
///
/// **Both conditions are necessary, and the measurement says so.** Written on
/// `display:none` alone, the rule took real content away: IKEA's "Product
/// details" tab (`<div hidden role="tabpanel">`, 489 bytes of dimensions and
/// materials) and Newegg's seller panel (`style="…display:none"`, 2,718 bytes
/// of ratings and shipping costs). Those two are painted on click or hover. A
/// hidden `itemprop`, on the other hand, is never displayed: the format
/// exists to be read by a program, and the value it carries is the one the
/// page shows next to it.
///
/// To be distinguished from the two neighbouring rules, which say something
/// else. `sr-only` is VISIBLE to a screen reader, hence delivered to someone;
/// `aria-hidden` is painted for the eye, and `is_aria_hidden_twin` measures
/// why dropping it wholesale would destroy 35,154 characters out of 46,876.
pub(super) fn is_machine_only_microdata(n: NodeRef<Node>) -> bool {
    let Node::Element(e) = n.value() else {
        return false;
    };
    is_machine_only_markup(
        e.attr("class"),
        e.attr("style"),
        e.attr("hidden").is_some(),
        e.attr("itemprop").is_some(),
    )
}

/// The same question, asked of the raw ATTRIBUTES.
///
/// Public because the fidelity battery must ask EXACTLY this one: it compares
/// the delivered text with the page's text, and its reference counted the
/// hidden microdata. A word written twice in the source and once in the
/// output then reads as a LOSS, and the ratchet refused the deduplication
/// that fixed it. Two spellings of "machine markup" would have made the rule
/// and its judge diverge.
pub fn is_machine_only_markup(
    class: Option<&str>,
    style: Option<&str>,
    has_hidden_attr: bool,
    has_itemprop: bool,
) -> bool {
    has_itemprop && is_display_none_markup(class, style, has_hidden_attr)
}

/// The browser paints nothing of this element, read from its raw attributes.
pub fn is_display_none_markup(
    class: Option<&str>,
    style: Option<&str>,
    has_hidden_attr: bool,
) -> bool {
    if has_hidden_attr {
        return true;
    }
    if style.is_some_and(|st| {
        let st: String = st.chars().filter(|c| !c.is_whitespace()).collect();
        st.to_ascii_lowercase().contains("display:none")
    }) {
        return true;
    }
    class.is_some_and(|class| {
        class.split_whitespace().any(|w| {
            matches!(
                w.to_ascii_lowercase().as_str(),
                "d-none" | "is-hidden" | "u-hidden" | "js-hidden" | "display-none" | "hide"
            )
        })
    })
}

/// **A hidden tooltip: text a browser only paints on hover.**
///
/// Two conditions, and BOTH are required. The element carries the HTML
/// `hidden` attribute — not a class, not a style, the attribute the
/// specification defines as "not yet, or no longer, directly relevant". And
/// another element points to it through `aria-describedby`: that is how a
/// page DECLARES "this is the description of that", hence text ABOUT a
/// neighbouring element, shown on hover or focus.
///
/// **`hidden` alone is not enough, and that is measured.** Discarding every
/// element carrying the attribute saves 232 bytes of scenery on three corpus
/// pages (GOV.UK's "was this page useful" survey, an Egyptian site's recent
/// searches, "Click to show internal directories" on pkg.go.dev) and costs
/// **1,137 bytes of product description** on the IKEA page: "The container is
/// made of durable plastic that keeps its shape even if you cram it into your
/// bag", the designer's name. A hidden tab is CONTENT that a click reveals.
/// The same lesson as for `display:none`, reproduced on the attribute alone.
///
/// What the `aria-describedby` reference adds, no threshold provides: the
/// IKEA tab is the description of nothing, the tooltip is one by declaration.
///
/// MEASURED 2026-09-10 on a PMC article (`pmc_article`), the shape that led to
/// the rule: six authors, each with a `<div hidden>` pointed to by their
/// link's `aria-describedby`, each repeating the same affiliation. The page
/// delivered that sentence **seven times** — six tooltips plus the single
/// visible copy — and its OUTLINE opened with six author names as headings
/// before the abstract. After: the affiliation once, the one the browser
/// paints, and the outline starting with "Abstract".
pub(super) fn is_a_hidden_tooltip(n: NodeRef<Node>) -> bool {
    let Node::Element(e) = n.value() else {
        return false;
    };
    if e.attr("hidden").is_none() {
        return false;
    }
    let Some(id) = e.attr("id") else {
        return false;
    };
    n.tree()
        .root()
        .descendants()
        .filter_map(|d| match d.value() {
            Node::Element(el) => el.attr("aria-describedby"),
            _ => None,
        })
        .any(|v| v.split_whitespace().any(|t| t == id))
}
