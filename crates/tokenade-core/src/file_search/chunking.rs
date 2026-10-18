//! **Cut a file into chunks that can be read on their own.**
//!
//! A chunk that starts in the middle of a function does not read: it has to be
//! extended up to a BALANCED boundary, otherwise search returns a syntactically
//! broken fragment nobody can do anything with.
//!
//! The overlap exists for the symmetric reason: an answer that straddles two
//! chunks would be missing from both.

use super::*;

/// Target chunk size in BYTES for AST chunking. Picked to roughly
/// match the line-chunk size (12 lines × ~80 chars ≈ 960). Ported from
/// Default desired chunk length.
pub const AST_CHUNK_BYTES: usize = 1024;

/// Minimum byte size below which an AST node is emitted as-is without
/// further splitting (min chunk size 50).
pub const AST_MIN_NODE_BYTES: usize = 50;

/// Slice `text` into overlapping line-windows. Fallback when no
/// tree-sitter parser is available for the language, or when the
/// caller doesn't know the language.
pub(crate) fn chunk_lines(text: &str) -> Vec<(usize, usize, String)> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return Vec::new();
    }
    let step = CHUNK_LINES.saturating_sub(CHUNK_OVERLAP).max(1);
    let mut out = Vec::new();
    let mut start = 0usize;
    while start < lines.len() {
        let end = (start + CHUNK_LINES).min(lines.len());
        let snippet = lines[start..end].join("\n");
        // line numbers are 1-indexed for the caller (matches editor UX).
        out.push((start + 1, end, snippet));
        if end == lines.len() {
            break;
        }
        start += step;
    }
    out
}

/// Best-effort stripping of string literals and `//` / `#` line comments
/// so brackets living inside them don't skew the balance counter. This is
/// deliberately a *simple* single-line heuristic (documented as such):
/// only `"` and `` ` `` open strings — Rust lifetimes (`&'a T`) and char
/// literals are left alone to avoid mis-detecting an unterminated string —
/// and multi-line block comments are not tracked. Worst case a miscount
/// just means we fall back to the fixed window.
/// **DECLARED RESERVE**: only `expand_to_balanced_region`, itself in
/// reserve, uses it.
#[allow(dead_code)]
pub(crate) fn strip_noise(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                if c == '\\' {
                    chars.next(); // skip the escaped char
                } else if c == q {
                    quote = None;
                }
            }
            None => {
                if c == '"' || c == '`' {
                    quote = Some(c);
                } else if c == '/' && chars.peek() == Some(&'/') {
                    break; // `//` line comment — rest is noise
                } else if c == '#' {
                    break; // `#` line comment (py/rb/sh) or Rust attribute
                } else {
                    out.push(c);
                }
            }
        }
    }
    out
}

/// Expand a fixed-window hit `[line_start, line_end]` (1-indexed,
/// inclusive) to the smallest bracket-balanced region of `content` that
/// fully encloses it — i.e. the complete `{ … }` / `( … )` / `[ … ]`
/// block the match sits inside, so the caller receives a whole
/// function/block rather than a slice cut mid-body.
///
/// Returns `Some((start, end, snippet))` **only** when a strictly larger
/// balanced region within `max_lines` is found; otherwise `None` and the
/// caller keeps the original window. That makes the change maximally
/// retro-compatible: a file with no brackets (prose, config), a block
/// that overflows the cap, or an already-complete window all leave the
/// hit untouched.
///
/// The counter is bracket-type-agnostic (any closer pops any opener) —
/// robust and cheap, matching FastContext's `extract_region` intent
/// while extending it to expand *upward* to the enclosing block, not just
/// downward. Because inner pairs always close before their parents, the
/// first completed pair that encloses the window is the innermost
/// enclosing block.
/// **DECLARED RESERVE**, like this whole module: no caller left since the
/// `file-search` verb was removed.
#[allow(dead_code)]
pub(crate) fn expand_to_balanced_region(
    content: &str,
    line_start: usize,
    line_end: usize,
    max_lines: usize,
) -> Option<(usize, usize, String)> {
    let lines: Vec<&str> = content.lines().collect();
    let n = lines.len();
    if n == 0 || line_start == 0 || line_start > n {
        return None;
    }
    let ws = line_start - 1; // 0-indexed window bounds
    let we = line_end.saturating_sub(1).min(n - 1);

    // Stack holds the line index of each unmatched opener.
    let mut stack: Vec<usize> = Vec::new();
    let mut best: Option<(usize, usize)> = None;
    'scan: for (i, raw) in lines.iter().enumerate() {
        for ch in strip_noise(raw).chars() {
            match ch {
                '{' | '(' | '[' => stack.push(i),
                '}' | ')' | ']' => {
                    if let Some(open_i) = stack.pop() {
                        if open_i <= ws && i >= we {
                            best = Some((open_i, i));
                            break 'scan;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    let (s, e) = best?;
    if e - s + 1 > max_lines {
        return None; // enclosing block too big → keep the window
    }
    if s + 1 == line_start && e + 1 == line_end {
        return None; // already exactly the window → nothing to do
    }
    Some((s + 1, e + 1, lines[s..=e].join("\n")))
}
