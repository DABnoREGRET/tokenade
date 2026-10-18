//! Telling a page's navigation apart from its content.
//!
//! The measurement (how many characters sit inside nav-ish containers versus
//! the main region), the sufficiency rule built on it, and the note the
//! converter prepends when what it converted is mostly menu.

use super::*;

/// A result that clears the thin floor can still be nothing but the site's
/// menus. The `[thin-result]` banner only fires below [`EXTRACT_VALID_MIN_CHARS`],
/// so it measures VOLUME — and a genre list clears any volume bar.
///
/// MEASURED 2026-07-22 on `bandcamp.com/discover`: 506 delivered characters,
/// every one of them a filter label ("electronic", "vinyl", "cassettes"), no
/// album whatsoever, and nothing in the output saying the listing was missing.
/// The agent reads a menu as the page.
///
/// The signal is STRUCTURAL, never the emitted text: the share of the ORIGINAL
/// page's visible text sitting in `nav/header/footer/aside`. It separates
/// cleanly — bandcamp 0.89, an Eventbrite listing 0.22, a Wikipedia article
/// 0.03 — and reuses the ratio and the 0.7 threshold the thin-result branch
/// already trusts for its WHY clause.
///
/// The body is PREPENDED to, never replaced: what we extracted is still the
/// page, the note only says what it mostly consists of.
pub(super) fn prepend_mostly_navigation_note(html: &str, body: &str) -> String {
    /// Past this, whatever the chrome share, enough real text came back that
    /// the agent can judge for itself.
    const MAX_BODY_CHARS: usize = EXTRACT_VALID_MIN_CHARS * 5;
    const MIN_NAV_RATIO: f64 = 0.7;
    if body.chars().count() > MAX_BODY_CHARS || html.len() < EMPTY_RESULT_MIN_HTML_BYTES {
        return body.to_string();
    }
    let (total_chars, nav_chars) = nav_main_char_split(html);
    if total_chars == 0 {
        return body.to_string();
    }
    let ratio = nav_chars as f64 / total_chars as f64;
    if ratio < MIN_NAV_RATIO {
        return body.to_string();
    }
    format!(
        "[tokenade: mostly-navigation] {:.0}% of this page's visible text sits in \
         nav/header/footer/aside, and what follows is short — so it is likely the site's \
         MENUS rather than the listing or article you asked for. The items themselves are \
         probably fetched client-side after load, which no session-less fetch reaches.\n\n\
         ---\n\n{body}",
        ratio * 100.0
    )
}

/// Coarse text-node classification approximating essence's DOM walk
/// (`racer.rs::extract_content_quality`, MIT — adapted): every character of
/// visible text sitting inside a `<nav|header|footer|aside>` element, or a
/// container carrying `role="navigation|banner|contentinfo"`, is charged to
/// `nav_chars`; `total_chars` is the whole document's visible text. No real
/// DOM here (see module doc) — a scan over the same tag list `convert_body`
/// already treats as chrome is the string-based approximation, run on the
/// ORIGINAL html so nothing has been stripped yet.
///
/// `nav_chars` can double-count nested chrome (a `<nav>` inside a
/// `role="banner"` header, say) — acceptable for a ratio used only as a
/// diagnostic, not as an extraction gate.
pub(super) fn nav_main_char_split(html: &str) -> (usize, usize) {
    let mut nav_chars = 0usize;
    for tag in ["nav", "header", "footer", "aside"] {
        let r = re(&format!(r"(?is)<{tag}\b[^>]*>(.*?)</{tag}>"));
        for c in r.captures_iter(html) {
            nav_chars += decode_entities(&strip_all_tags(c.get(1).unwrap().as_str()))
                .chars()
                .count();
        }
    }
    // ARIA landmark roles land on ordinary containers most often — `regex`
    // has no backreferences (see module doc), so the closing tag can't be
    // tied to whichever opened it and the common container names are
    // enumerated instead of matched generically.
    for tag in ["div", "section", "ul"] {
        let r = re(&format!(
            r#"(?is)<{tag}\b[^>]*\brole\s*=\s*["'](?:navigation|banner|contentinfo)["'][^>]*>(.*?)</{tag}>"#
        ));
        for c in r.captures_iter(html) {
            nav_chars += decode_entities(&strip_all_tags(c.get(1).unwrap().as_str()))
                .chars()
                .count();
        }
    }
    let total_chars = decode_entities(&strip_all_tags(html)).chars().count();
    (total_chars, nav_chars.min(total_chars))
}

/// essence's post-isolation accept ladder (`CONTENT_SUFFICIENT_CHARS =
/// 1000`, MIT — adapted), for callers that want a sufficiency verdict rather
/// than the raw split. `1000 / 5 = 200` is exactly tokenade's own
/// [`EXTRACT_VALID_MIN_CHARS`] floor, which is why the bottom rung reuses it
/// instead of a fresh constant.
///
/// essence escalates a `needs_js` case to a JS-rendered re-fetch; web.rs has
/// no transport hook to escalate through (that lives in `web_pipeline.rs`,
/// out of scope here), so that rung folds into "insufficient" — the caller
/// still has `nav_ratio` to explain why.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn content_sufficiency(main_chars: usize, total_chars: usize, nav_chars: usize) -> bool {
    const CONTENT_SUFFICIENT_CHARS: usize = 1000;
    if main_chars > CONTENT_SUFFICIENT_CHARS {
        return true;
    }
    let nav_ratio = if total_chars == 0 {
        0.0
    } else {
        nav_chars as f64 / total_chars as f64
    };
    if total_chars > CONTENT_SUFFICIENT_CHARS && nav_ratio < 0.7 {
        return true;
    }
    main_chars > CONTENT_SUFFICIENT_CHARS / 5
}
