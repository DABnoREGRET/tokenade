//! **Is the layout readable as is?**
//!
//! A table rendered inline becomes a sequence of numbers without
//! columns, and a page of leader dots (`....... 42`) is pure noise.
//!
//! Hence the explicit REFUSAL: when the layout pass cannot render
//! something honest, it says so instead of returning an approximation
//! that reads like a reading. Right-to-left writing is the
//! extreme case — the other engine would return reversed lines there.

use super::*;

/// Characters that form a LEADER: the dotted rule joining a table-of-contents
/// entry to its page number, or the underscore rule of a form field.
pub(crate) const LEADER_CHARS: [char; 5] = ['.', '-', '_', '·', '•'];
/// Shortest run of leader characters that is a rule rather than punctuation.
/// Three keeps the ellipsis "..." out of prose statistics by collapsing it too,
/// which is harmless here — see [`whitespace_padding_ratio`].
pub(crate) const LEADER_MIN_RUN: usize = 3;

/// Extract each page independently, so one bad page cannot cost the document.
/// A page that panics or errors comes back empty; every other page survives.
pub(crate) fn pdf_pages_isolated(bytes: &[u8]) -> Vec<String> {
    let Ok(doc) = lopdf::Document::load_mem(bytes) else {
        return Vec::new();
    };
    let mut nums: Vec<u32> = doc.get_pages().keys().copied().collect();
    nums.sort_unstable();
    nums.iter()
        .map(|n| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut s = String::new();
                {
                    let mut out = pdf_extract::PlainTextOutput::new(&mut s);
                    if pdf_extract::output_doc_page(&doc, &mut out, *n).is_err() {
                        return String::new();
                    }
                }
                s
            }))
            .unwrap_or_default()
        })
        .collect()
}

/// Collapse leader runs (`....`, `- - -`, `· · ·`, `___`) to a single space.
///
/// Written for [`whitespace_padding_ratio`] and not for the extracted text
/// itself: a leader is typographic furniture whose whole job is to span
/// horizontal space, and it does so with characters that are half whitespace
/// when spaced out (`. . . . . .`). Counting those spaces makes any document
/// carrying a table of contents look like it is drowning in padding, which is
/// the difference between a metric that catches a regression and a metric that
/// is switched off after its third false alarm.
///
/// Runs separated by SINGLE spaces are one run: `. . . .` is a leader, and its
/// spaces are part of the rule, not of the prose around it.
pub(crate) fn collapse_leader_runs(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if !LEADER_CHARS.contains(&chars[i]) {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        // Walk the run: leader chars, optionally single-spaced.
        let mut j = i;
        let mut leaders = 0;
        while j < chars.len() {
            if LEADER_CHARS.contains(&chars[j]) {
                leaders += 1;
                j += 1;
            } else if chars[j] == ' ' && chars.get(j + 1).is_some_and(|c| LEADER_CHARS.contains(c))
            {
                j += 1;
            } else {
                break;
            }
        }
        if leaders >= LEADER_MIN_RUN {
            // The rule REPLACES the horizontal space it spanned, so it absorbs
            // the whitespace on both sides of it too. Leaving those behind
            // ("Chapter One" + rule + "12" → three spaces) would hand the
            // metric back most of the padding the collapse just removed.
            while out.ends_with(' ') {
                out.pop();
            }
            out.push(' ');
            while chars.get(j) == Some(&' ') {
                j += 1;
            }
            i = j;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// Share of an extracted document that is whitespace, leaders discounted.
///
/// A REGRESSION GUARD, not a fix for a known defect — the distinction matters
/// for anyone tempted to "improve" the number. An earlier audit claimed our PDF
/// extraction was 54 % padding spaces; RE-MEASURED 2026-07-22 through the
/// current pipeline it is 0.1701 / 0.1796 / 0.1789 on `pdf_small` / `pdf_mid` /
/// `pdf_large`, which is simply the rate at which English prose contains spaces
/// and newlines. There is nothing to fix. What this exists for is the day an
/// extraction change starts padding cells to a column width or emitting
/// positioning runs again: that shows up here as a step change long before
/// anyone notices it in a token bill.
///
/// The leader collapse moves those three by at most 0.0003 (`pdf_mid`: 0.1793
/// raw against 0.1796 collapsed — it can tick UP, because collapsing a
/// `|---|---|` separator removes non-whitespace). It is in the metric for the
/// documents that DO carry a table of contents, where the raw figure is a
/// multiple of this: MEASURED, one dotted ToC line runs 0.40, over the ceiling
/// on its own.
pub fn whitespace_padding_ratio(text: &str) -> f64 {
    let collapsed = collapse_leader_runs(text);
    let total = collapsed.chars().count();
    if total == 0 {
        return 0.0;
    }
    collapsed.chars().filter(|c| c.is_whitespace()).count() as f64 / total as f64
}

/// Why this page must NOT keep the layout pass's output — or `None` to keep it.
///
/// The reasons were two inline conditions inside the `match` arm, added
/// a year apart, each one a page shape the layout analysis reads wrongly. A
/// third would have been a third `&&` in a guard nobody can read, and none of
/// them could say WHICH rule fired. One predicate, one place, and it NAMES its
/// reason so a future measurement can count them instead of guessing.
pub(crate) fn layout_pass_refused(md: &str) -> Option<&'static str> {
    if md.trim().is_empty() {
        return Some("empty");
    }
    // A COVER page is a layout grid, and layout analysis reads a grid as a
    // table: `example-long.pdf`'s ECMA-262 cover came back as `|  |  | ECMA-262 |`
    // / `|  | 15 Edition | / June 2024 |` where the plain path read it as one
    // clean line. That was a regression the `to_markdown` switch introduced, so
    // it is guarded rather than lived with.
    if looks_like_layout_grid(md) {
        return Some("layout-grid");
    }
    // An RTL page: the converter is not bidi-aware by design (see the call
    // site), and its grouping cuts Arabic words into single letters.
    if rtl_dominant(md) {
        return Some("rtl");
    }
    None
}

/// Is this text mostly right-to-left?
///
/// Counted on LETTERS, not on characters: digits, punctuation and the markdown
/// scaffolding a converter adds (`|`, `-`, `#`) belong to no script and would
/// otherwise let a table of numbers outvote the prose around it.
///
/// The range test is the library's own `is_rtl_text` rather than a second copy
/// of the Unicode blocks — its comment says the authoritative list "lives in
/// exactly one place", and a private copy here is the drift this repo has paid
/// for before.
pub(crate) fn rtl_dominant(text: &str) -> bool {
    let (mut rtl, mut ltr) = (0usize, 0usize);
    for c in text.chars() {
        if !c.is_alphabetic() {
            continue;
        }
        if pdf_oxide::text::rtl_detector::is_rtl_text(c as u32) {
            rtl += 1;
        } else {
            ltr += 1;
        }
    }
    rtl > ltr
}
