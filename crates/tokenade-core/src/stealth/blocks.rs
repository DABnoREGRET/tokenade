//! **The block that carries no marker.**
//!
//! Some pages say nothing: no title, no widget, just a STRUCTURE that does
//! not look like content. The signal is statistical, so it must be cautious —
//! declaring a genuinely thin page blocked is a dead loss.
//!
//! `looks_like_source_or_doc` is the safety rail: code and documentation
//! naturally have the shape one mistakes for a block.

use super::*;

/// Structural integrity check — Tier 3 from crawl4ai's
/// `antibot_detector.py:138-188`. Returns [`ChallengeKind::GenericBlock`]
/// when the page is small AND structurally hollow — i.e., the agent
/// received an empty shell rather than article content.
///
/// Triggers only when **two of three** signals fire:
///   1. No `<body>` tag anywhere.
///   2. Fewer than 50 visible characters after removing `<script>` /
///      `<style>` blocks.
///   3. Zero semantic content elements (`<p>`, `<h1..6>`, `<article>`,
///      `<section>`, `<li>`, `<td>`, `<a>`, `<pre>`).
///
/// Only applied to HTML pages under 50 KB — large pages have enough
/// real content to outweigh missing structure (and false-positive
/// risk grows with the page).
pub fn structural_block_signal(html: &str) -> Option<ChallengeKind> {
    // Skip JSON/XML — they legitimately lack body/semantic tags.
    let trimmed = html.trim_start();
    if trimmed.starts_with('{')
        || trimmed.starts_with('[')
        || trimmed.starts_with("<?xml")
        || trimmed.starts_with("<rss")
    {
        return None;
    }
    if html.len() > 50 * 1024 {
        return None;
    }
    // Require evidence this is an HTML DOCUMENT that wanted to run scripts.
    // Without it the three hollowness signals fire on any short non-HTML
    // input — plain prose has no `<body>` and no semantic tags, so it scores
    // 2/3 and every `tokenade web` on a text fragment would claim an
    // anti-bot block. Both things this tier targets (an unnamed wall, a
    // JS-only page) are script-bearing HTML documents, so the gate costs
    // nothing and is what makes the signal a signal.
    let head: String = html
        .chars()
        .take(2_048)
        .collect::<String>()
        .to_ascii_lowercase();
    let is_html_doc = head.contains("<html") || head.contains("<!doctype html");
    if !is_html_doc || !html.to_ascii_lowercase().contains("<script") {
        return None;
    }
    let mut signals = 0;
    if !html.contains("<body") {
        signals += 1;
    }
    // Strip script/style (cheap, regex-free, accepts case variants).
    fn eq_ci(bytes: &[u8], at: usize, needle: &str) -> bool {
        bytes.len() >= at + needle.len()
            && bytes[at..at + needle.len()].eq_ignore_ascii_case(needle.as_bytes())
    }
    // Bytes, decoded once at the end: pushing `bytes[i] as char` turned every
    // non-ASCII character into as many Latin-1 codepoints as it has bytes, and
    // this count is compared against a ceiling below.
    let mut visible: Vec<u8> = Vec::with_capacity(html.len());
    let mut in_skip = false;
    // Markup depth. The original counted every byte that was not a literal
    // `<` or `>`, which meant tag names and attributes — `meta charset=utf-8`,
    // `div id=challenge-container` — were tallied as visible TEXT. A real
    // challenge shell is nothing but markup, so it always cleared the 50-char
    // floor and signal 2 could effectively never fire: the tier was far less
    // sensitive than its docstring claims. Excluding tag interiors is what
    // the comment always said this did.
    let mut in_tag = false;
    let mut i = 0;
    let bytes = html.as_bytes();
    while i < bytes.len() {
        if !in_skip {
            // Look for <script or <style opener.
            if bytes[i] == b'<' && (eq_ci(bytes, i + 1, "script") || eq_ci(bytes, i + 1, "style")) {
                in_skip = true;
                i += 1;
                continue;
            }
            if bytes[i] == b'<' {
                in_tag = true;
            } else if bytes[i] == b'>' {
                in_tag = false;
            } else if !in_tag {
                visible.push(bytes[i]);
            }
            i += 1;
        } else {
            // Skip until `</script>` or `</style>` (any case).
            if eq_ci(bytes, i, "</script>") {
                in_skip = false;
                i += 9;
            } else if eq_ci(bytes, i, "</style>") {
                in_skip = false;
                i += 8;
            } else {
                i += 1;
            }
        }
    }
    let visible_chars = crate::text_encoding::utf8_owned(visible)
        .chars()
        .filter(|c| !c.is_whitespace())
        .count();
    if visible_chars < 50 {
        signals += 1;
    }
    let semantic_markers = [
        "<p", "<h1", "<h2", "<h3", "<h4", "<h5", "<h6", "<article", "<section", "<li", "<td",
        "<a ", "<pre",
    ];
    let has_semantic = semantic_markers.iter().any(|m| html.contains(m));
    if !has_semantic {
        signals += 1;
    }
    if signals >= 2 {
        Some(ChallengeKind::GenericBlock)
    } else {
        None
    }
}

/// Combined detect: try named providers first, then structural Tier 3.
pub fn detect_any_block(html: &str) -> Option<ChallengeKind> {
    detect_challenge(html).or_else(|| structural_block_signal(html))
}

/// Cheap guard: returns true when `text` looks like source code,
/// documentation, or shell command output rather than a fetched HTML
/// response. We use this to gate the challenge detector, which has
/// to test for several brand-name literals (cloudflare/akamai/etc.)
/// that legitimately appear in our own codebase, in CHANGELOGs, and
/// in any grep/sed/cat output that touches those files.
///
/// Heuristics (cheap, all O(head only)):
///   - Rust line markers: `pub fn`, `use crate::`, `//!`, `///`, `fn `
///   - Python markers: `def `, `class `, `import `
///   - Markdown headers/lists: `# `, `## `, `- ` at line start
///   - Shell-output prefixes: `==> ` / `=== ` / `[INFO]` (test harness)
///   - File-system path lines (`./foo/bar.rs:`)
///
/// Returns true if ≥2 source signals fire in the first 4 KB. Two
/// signals required so a single keyword in real HTML doesn't trip us.
/// Drop a leading line number so the source signals below can be seen through
/// it. MEASURED (2026-08-04, stash blobs `fc76ad88160a` / `02278f8e173e`):
/// an excerpt of our OWN `stealth.rs` — the file that lists the vendor
/// literals — was classified `challenge=reblaze` / `cloudflare-interstitial`
/// and its content REPLACED by stealth remediation, growing 391 B → 985 B.
/// Every signal was there (`///`, `// `), just behind `50\t` / `105\t`:
/// `trim_start()` does not eat digits.
///
/// This is the shape source code most often arrives in — `Read` and `cat -n`
/// emit `<num>\t`, `grep -n` emits `<num>:` — so the guard was blind on its
/// most common input. The `path:line` form was already handled; the bare
/// numeric one was not.
///
/// Deliberately tight, because a false positive HERE suppresses a real
/// challenge detection: the digits must be immediately followed by a tab,
/// colon or pipe, and the remainder must still fire a source signal on its
/// own and be corroborated by a second line.
pub(crate) fn strip_line_number(l: &str) -> &str {
    let digits = l.len() - l.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if digits == 0 || digits > 7 {
        return l;
    }
    let rest = &l[digits..];
    match rest.as_bytes().first() {
        Some(b'\t') | Some(b':') | Some(b'|') => rest[1..].trim_start(),
        _ => l,
    }
}

/// A `grep -rn` locator: `path/to/file.rs:82:` at the start of a line.
///
/// The rule beside this one recognises a path line only when it opens with
/// `./` or `/`, and `grep -rn <dir>` run from a repository root — the form
/// this project's own instructions hand out — emits a RELATIVE path with
/// neither. MEASURED 2026-09-07: every one of the seven lines that
/// `grep -rn <vendor> --include=*.rs crates/` returns scored zero source
/// signals, so an excerpt of the detector's own source was classified a bot
/// wall and all seven answers were replaced by stealth remediation.
///
/// Deliberately tight, because a false positive HERE suppresses a real
/// challenge detection: the field before the first colon must contain a
/// directory separator and none of the characters that mark up a document,
/// and it must be followed by a line number and a second colon. A minified
/// script line cannot reach it — `<script src="https://…` opens with `<`,
/// and the text before its first colon (`<script src="https`) holds no `/`.
pub(crate) fn grep_locator(l: &str) -> bool {
    let Some(colon) = l.find(':') else {
        return false;
    };
    let path = &l[..colon];
    if !path.contains('/') || path.is_empty() {
        return false;
    }
    if path.contains(['<', '>', '"', ' ', '\t']) {
        return false;
    }
    let rest = &l[colon + 1..];
    let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    (1..=7).contains(&digits) && rest.as_bytes().get(digits) == Some(&b':')
}

pub(crate) fn looks_like_source_or_doc(text: &str) -> bool {
    // Real HTML almost always opens with `<` (after optional BOM / DOCTYPE).
    // Cheap fast-path: if we DO see HTML structure, fall through and let
    // the regular detector run.
    let head = text.get(..text.len().min(4096)).unwrap_or(text);
    let trimmed = head.trim_start();
    if trimmed.starts_with("<!DOCTYPE")
        || trimmed.starts_with("<!doctype")
        || trimmed.starts_with("<html")
        || trimmed.starts_with("<HTML")
        || trimmed.starts_with("<?xml")
    {
        return false;
    }
    let mut hits = 0;
    for line in head.lines().take(80) {
        let l = strip_line_number(line.trim_start());
        // Collapsed from 5 if/else-if branches all incrementing `hits`
        // (clippy: identical_if_else_blocks). Same semantics, less noise.
        // #298 — membership from `source_signals`, policy kept here. The list
        // this replaces knew `use std::` and `pub struct` and not `#include`'s
        // companions or `func `; the shared one is the union of the three that
        // existed. What stays local is the CONTRACT: this function answers "is
        // this an anti-bot page?", not "is this source" — beware the trap of
        // reusing it on the strength of its name.
        // Only the SOURCE half is shared. The three others are this function's
        // own contract — it answers "is this a doc or tool output rather than a
        // CHALLENGE PAGE?" — and the first version of this edit replaced the
        // whole predicate with the source half alone, which deleted them. The
        // same trap, committed in the very edit that claimed to avoid it: the
        // name says "source or doc" and the doc half was three categories.
        let is_source_signal = crate::source_signals::line_opens_source(l)
            // Markdown headings.
            || l.starts_with("# ")
            || l.starts_with("## ")
            || l.starts_with("### ")
            // Tool output banners.
            || l.starts_with("==> ")
            || l.starts_with("=== ")
            || l.starts_with("[INFO]")
            || l.starts_with("[WARN]")
            || l.starts_with("[ERROR]")
            // grep-like `file:line` output, relative OR absolute — production
            // data showed challenge false-positives on `grep -rn … crates/`
            // because absolute paths were excluded.
            || ((l.starts_with("./") || l.starts_with('/')) && l.contains(':'))
            || grep_locator(l);
        if is_source_signal {
            hits += 1;
        }
        if hits >= 2 {
            return true;
        }
    }
    // Single-header markdown (e.g. `cat README.md | head -5` where the
    // head only contains `# Title` + intro prose): one h1 plus no
    // other source markers is still PROBABLY a doc, not an HTML
    // challenge page. Real challenge HTML always starts with `<!DOCTYPE`
    // / `<html` which the early-return at top already short-circuits.
    hits == 1
        && head
            .lines()
            .next()
            .is_some_and(|l| l.trim_start().starts_with("# "))
}
