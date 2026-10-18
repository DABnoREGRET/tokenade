//! **Cutting a document into chunks that stand on their own.**
//!
//! Sentences first, subsections next — and both bounds matter: a chunk that
//! is too short loses its context, a chunk that is too long dilutes relevance
//! over what does not answer.
//!
//! A sentence ending is not a period: `e.g.`, `v1.2`, `Dr.` and an address
//! carry one without ending anything. The table of terminators and closers is
//! what keeps us from cutting in the middle of a version number.

use super::*;

/// Sentence-terminating characters OUTSIDE ASCII. The embedder is
/// `potion-multilingual-128M`, but every boundary this module knows about is
/// ASCII-shaped (a blank line, a `- ` item, a `.` before a space). Scripts
/// that terminate sentences with their own punctuation AND do not put a space
/// after it therefore have no boundary at all: a Chinese, Japanese, Hindi or
/// Amharic paragraph arrives as ONE line, `u.len() < 4` sends it through
/// untouched, and the section is never split — in exactly the languages the
/// multilingual embedder was chosen to serve.
///
/// Only characters that END a sentence in their script are listed. Clause
/// separators are deliberately absent even though they look terminal to a
/// reader of another script: `؛` (Arabic semicolon), `،` (Arabic comma) and
/// `၊` (Myanmar little section) separate clauses, not sentences, and cutting
/// there splits mid-sentence. `՞` (Armenian question mark) is written OVER
/// the stressed vowel inside the questioning word, not at the end, so it is
/// absent too — Armenian ends on `։` alone. `…` is left out because it is
/// more often intra-sentential (a trailing-off pause) than terminal.
pub(crate) const SENTENCE_TERMINATORS: &[char] = &[
    '。', // U+3002 ideographic full stop (Chinese, Japanese)
    '．', // U+FF0E fullwidth full stop
    '！', // U+FF01 fullwidth exclamation mark
    '？', // U+FF1F fullwidth question mark
    '｡',  // U+FF61 halfwidth ideographic full stop
    '।',  // U+0964 devanagari danda (Hindi, Marathi, Nepali, Bengali…)
    '॥',  // U+0965 devanagari double danda (verse / section end)
    '؟',  // U+061F Arabic question mark
    '۔',  // U+06D4 Arabic full stop (Urdu)
    '።',  // U+1362 Ethiopic full stop (Amharic, Tigrinya)
    '፧',  // U+1367 Ethiopic question mark
    '։',  // U+0589 Armenian full stop
    '჻',  // U+10FB Georgian paragraph separator
    '။',  // U+104B Myanmar sign section (sentence end)
    '៕',  // U+17D4 Khmer sign khan (sentence end)
];

/// Marks that CLOSE the sentence they follow, so they belong to the piece
/// being cut rather than opening the next one. Without this a quoted line
/// ends up as `…said.` / `」 next…`, orphaning the closing bracket.
pub(crate) const SENTENCE_CLOSERS: &[char] = &[
    '"', '\'', ')', ']', '»', '”', '’', '）', '］', '」', '』', '》', '〉',
];

/// Shortest piece a sentence cut may produce, in CHARACTERS.
///
/// ASCII `.` is not only a full stop: `e.g.`, `M. Dupont`, `Fig. 3`, `U.S. `
/// and enumerations all put one before a space, and cutting there yields
/// two- and four-character shards that carry no retrievable meaning and cost
/// a chunk boundary each. A cut that would leave a piece shorter than this is
/// simply not taken — the text accumulates until the next candidate. The
/// value sits above every such abbreviation shard while staying below a
/// typical sentence in the dense scripts this exists for (CJK writes a lot in
/// few characters), so the guard never merges two real sentences there.
pub(crate) const SENTENCE_MIN_CHARS: usize = 16;

/// Cut `line` after sentence terminators, never producing a piece shorter
/// than `min_chars`. Concatenating the result reproduces `line` exactly
/// (whitespace after a terminator stays on the piece it closed), so this only
/// ever moves a line break — it never edits text.
pub(crate) fn split_sentences(line: &str, min_chars: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut piece = String::new();
    let mut len = 0usize; // chars in `piece`, for the minimum
    let mut it = line.chars().peekable();
    while let Some(c) = it.next() {
        piece.push(c);
        len += 1;
        let ascii = matches!(c, '.' | '!' | '?');
        if !ascii && !SENTENCE_TERMINATORS.contains(&c) {
            continue;
        }
        // ASCII `.` is also a decimal point, a path separator and a domain
        // separator; only whitespace (or end of line) after it makes it
        // plausibly terminal. The wide terminators are unambiguous, and the
        // scripts using them do NOT write a space afterwards — requiring one
        // would disable the whole table.
        if ascii && it.peek().is_some_and(|n| !n.is_whitespace()) {
            continue;
        }
        // Absorb the rest of the terminal run (`?!`, `。」`) and any trailing
        // space, so the next piece starts on the next sentence's first word.
        while it.peek().is_some_and(|n| {
            matches!(n, '.' | '!' | '?')
                || *n == ' '
                || *n == '\t'
                || SENTENCE_CLOSERS.contains(n)
                || SENTENCE_TERMINATORS.contains(n)
        }) {
            piece.push(it.next().expect("peeked"));
            len += 1;
        }
        if len < min_chars {
            continue; // abbreviation shard — keep accumulating.
        }
        out.push(std::mem::take(&mut piece));
        len = 0;
    }
    if !piece.is_empty() {
        // A tail below the minimum is not a sentence of its own.
        match out.last_mut() {
            Some(last) if len < min_chars => last.push_str(&piece),
            _ => out.push(piece),
        }
    }
    out
}

/// Cut `piece` into windows of at most `max` chars, each ending after a
/// whitespace run when one exists in the window (a hard cut otherwise).
/// Concatenation reproduces `piece`. The sentence cut has nothing to cut on
/// in an unpunctuated line: a 250 KB paragraph stayed ONE block, the right
/// block was retrieved, and the window shown was its head — the answer, at
/// 120 KB, never reached the reader (headroom #2518, 2026-10-03).
pub(crate) fn split_at_spaces(piece: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = piece;
    while rest.chars().count() > max {
        let end = rest.char_indices().nth(max).map_or(rest.len(), |(i, _)| i);
        let cut = rest[..end]
            .rfind(char::is_whitespace)
            .map(|i| i + rest[i..].chars().next().map_or(1, char::len_utf8))
            .filter(|&i| i > 0)
            .unwrap_or(end);
        out.push(rest[..cut].to_string());
        rest = &rest[cut..];
    }
    if !rest.is_empty() {
        out.push(rest.to_string());
    }
    out
}

/// Block-granularity sub-split: any section whose body exceeds
/// [`SPLIT_MAX_CHARS`] is split at paragraph boundaries into blocks of
/// roughly [`SPLIT_MIN_CHARS`]..[`SPLIT_MAX_CHARS`] chars. Fenced code
/// blocks and contiguous table rows are atomic units (never split inside);
/// an oversized single unit becomes its own block. Continuation blocks keep
/// the section's heading/breadcrumb for scoring but an empty `heading_line`
/// (the heading is only emitted once) and `level + 1` so the parent's
/// subtree still covers them in document order.
pub(crate) fn subsplit(sections: Vec<Section>) -> Vec<Section> {
    let split_min = split_min();
    let split_max = split_max();
    let mut out: Vec<Section> = Vec::with_capacity(sections.len());
    for sec in sections {
        let body_chars: usize = sec.body_lines.iter().map(|l| l.chars().count() + 1).sum();
        if body_chars <= split_max {
            out.push(sec);
            continue;
        }
        // ---- Atomic units: paragraphs, whole code fences, indented
        // CommonMark code (4 spaces / tab), table runs. A blank used to
        // flush, so a 20-line indented sample with one empty line became
        // two units and then the line chopper; `classify` already flags
        // those lines `has_code`. Same hole round/2026-09-19-7 closed
        // for markdown elision.
        // Each unit with whether a BLANK line preceded it in the source: the
        // accumulator below restores a paragraph break only where there was
        // one. Units are also cut WITHOUT a blank — at a fence, a list item,
        // and where indented code starts — and a source file trips the last
        // one on every function: `fn f() {` then its 4-space body. Rejoined
        // with a blank, `tokenade read x.rs --prompt` handed back a line the
        // file does not have, between a signature and its body (2026-10-04).
        let mut units: Vec<Vec<String>> = Vec::new();
        let mut cur: Vec<String> = Vec::new();
        // Whether every line of `cur` is blank or indented code, kept as `cur`
        // grows: re-scanning `cur` on each line was quadratic, and a 24 000-
        // line pretty-printed JSON took 155 s here in a debug build
        // (2026-10-03). Only ever read while `cur` is non-empty.
        let mut cur_code = true;
        let code_or_blank = |l: &str| l.trim().is_empty() || is_indented_code_line(l);
        let mut in_fence = false;
        for (i, line) in sec.body_lines.iter().enumerate() {
            if cur.is_empty() {
                cur_code = true;
            }
            let t = line.trim_start();
            let fence = t.starts_with("```") || t.starts_with("~~~");
            if fence {
                if !in_fence && !cur.is_empty() && cur.iter().any(|l| !l.trim().is_empty()) {
                    // Fence opens a new unit; flush pending prose first.
                    units.push(std::mem::take(&mut cur));
                }
                in_fence = !in_fence;
                cur.push(line.clone());
                if !in_fence {
                    units.push(std::mem::take(&mut cur));
                }
                continue;
            }
            if in_fence {
                cur.push(line.clone());
                continue;
            }
            if line.trim().is_empty() {
                let indented_run = !cur.is_empty()
                    && cur_code
                    && sec.body_lines[i + 1..]
                        .iter()
                        .find(|x| !x.trim().is_empty())
                        .is_some_and(|n| is_indented_code_line(n));
                if indented_run {
                    cur.push(line.clone());
                    continue;
                }
                if !cur.is_empty() {
                    units.push(std::mem::take(&mut cur));
                }
                continue;
            }
            if is_indented_code_line(line) {
                if !cur.is_empty() && !cur_code {
                    units.push(std::mem::take(&mut cur));
                    cur_code = true;
                }
                cur.push(line.clone());
                continue;
            }
            // List items are unit boundaries too — long lists (comment
            // threads, definition pages) often have NO blank lines, which
            // used to collapse a whole section into one unsplittable unit
            // (→ single block → flat fallback, measured on HN threads).
            let is_item = t.starts_with("- ")
                || t.starts_with("* ")
                || (t.chars().next().is_some_and(|c| c.is_ascii_digit()) && t.contains(". "));
            if is_item && !cur.is_empty() {
                units.push(std::mem::take(&mut cur));
                cur_code = true;
            }
            cur_code &= code_or_blank(line);
            cur.push(line.clone());
        }
        if !cur.is_empty() {
            units.push(cur);
        }
        // Units are runs of consecutive source lines, in order, with only
        // blank lines skipped between them: walk the source once to learn
        // which ones a blank really preceded.
        let units: Vec<(Vec<String>, bool)> = {
            let mut at = 0usize;
            units
                .into_iter()
                .map(|u| {
                    let start = at;
                    while at < sec.body_lines.len()
                        && sec.body_lines[at].trim().is_empty()
                        && u.first().is_some_and(|f| !f.trim().is_empty())
                    {
                        at += 1;
                    }
                    let blank = at > start;
                    at += u.len();
                    (u, blank)
                })
                .collect()
        };
        // A single fence-free unit far beyond MAX (a wall of lines with no
        // blank separators — kernel-parameters-style definition pages) is
        // chunked at line boundaries so it stays retrievable.
        let units: Vec<(Vec<String>, bool)> = units
            .into_iter()
            .flat_map(|(u, blank)| -> Vec<(Vec<String>, bool)> {
                let uc: usize = u.iter().map(|l| l.chars().count() + 1).sum();
                let fenced = u.iter().any(|l| {
                    let t = l.trim_start();
                    t.starts_with("```") || t.starts_with("~~~")
                });
                let indented = u.iter().any(|l| is_indented_code_line(l))
                    && u.iter()
                        .all(|l| l.trim().is_empty() || is_indented_code_line(l));
                if uc <= 2 * split_max || fenced || indented {
                    return vec![(u, blank)];
                }
                // Too few lines to cut BETWEEN lines. That is the normal shape
                // of a paragraph in a script whose sentences end on their own
                // punctuation with no following space (see
                // [`SENTENCE_TERMINATORS`]): the whole paragraph is one line,
                // so before this the unit went out whole and the section never
                // split. Cutting the fat lines into sentences gives the line
                // chopper below something to work with; if there are still no
                // boundaries (one unbroken run of characters) the unit passes
                // through untouched exactly as it used to.
                let orig_lines = u.len();
                let u = if orig_lines < 4 {
                    u.into_iter()
                        .flat_map(|l| {
                            if l.chars().count() > split_max {
                                split_sentences(&l, SENTENCE_MIN_CHARS)
                                    .into_iter()
                                    .flat_map(|p| split_at_spaces(&p, split_max))
                                    .collect()
                            } else {
                                vec![l]
                            }
                        })
                        .collect::<Vec<String>>()
                } else {
                    u
                };
                // A unit that was already line-rich reaches here; a short one
                // only if sentence cutting actually produced lines to work
                // with. Unchanged short units go out whole, as before.
                if u.len() < 4 && u.len() == orig_lines {
                    return vec![(u, blank)];
                }
                // This is the ONLY place a table run is ever cut: table rows
                // carry no blank lines, so a whole table arrives here as one
                // unit and is chopped at line boundaries. Chunk 2 onward was
                // therefore a run of ANONYMOUS rows — `| 42 | 17 | 3.1 |` with
                // no column name anywhere in it. Unretrievable (no query term
                // can match) and unreadable once retrieved (nothing says which
                // column is which). Replicate header + delimiter onto every
                // continuation chunk; chunk 0 already opens with them.
                //
                // Bounded two ways: only when the header fits in a quarter of
                // the block budget (a 40-column header would cost more than
                // the rows it labels, and blocks here are <= split_max), and
                // the replicated chars are charged to `cc` so a chunk carrying
                // a header still respects split_max instead of overflowing by
                // the header's width.
                let header: Vec<String> = table_header_prefix(&u)
                    .map(<[String]>::to_vec)
                    .filter(|h| {
                        h.iter().map(|l| l.chars().count() + 1).sum::<usize>() * 4 <= split_max
                    })
                    .unwrap_or_default();
                let header_chars: usize = header.iter().map(|l| l.chars().count() + 1).sum();
                let mut chunks: Vec<Vec<String>> = Vec::new();
                let mut cur: Vec<String> = Vec::new();
                let mut cc = 0usize;
                for l in u {
                    let lc = l.chars().count() + 1;
                    if cc + lc > split_max && !cur.is_empty() {
                        chunks.push(std::mem::take(&mut cur));
                        cc = 0;
                    }
                    if cur.is_empty() && !chunks.is_empty() && !header.is_empty() {
                        cur.extend(header.iter().cloned());
                        cc += header_chars;
                    }
                    cc += lc;
                    cur.push(l);
                }
                if !cur.is_empty() {
                    chunks.push(cur);
                }
                // A chunk cut out of one unit was not preceded by a blank.
                chunks
                    .into_iter()
                    .enumerate()
                    .map(|(k, c)| (c, blank && k == 0))
                    .collect()
            })
            .collect();
        // ---- Accumulate units into MIN..MAX blocks.
        let unit_chars = |u: &[String]| -> usize { u.iter().map(|l| l.chars().count() + 1).sum() };
        let mut blocks: Vec<Vec<String>> = Vec::new();
        let mut acc: Vec<String> = Vec::new();
        let mut acc_chars = 0usize;
        let mut acc_blank = false;
        for (u, blank) in units {
            let uc = unit_chars(&u);
            if acc_chars + uc > split_max && acc_chars >= split_min {
                blocks.push(std::mem::take(&mut acc));
                acc_chars = 0;
            }
            if acc.is_empty() {
                acc_blank = blank;
            } else if blank {
                acc.push(String::new()); // restore the paragraph break
                acc_chars += 1;
            }
            acc_chars += uc;
            acc.extend(u);
        }
        if !acc.is_empty() {
            // A tiny trailing block reads better merged into its predecessor.
            if acc_chars < split_min {
                if let Some(last) = blocks.last_mut() {
                    if acc_blank {
                        last.push(String::new());
                    }
                    last.extend(acc);
                } else {
                    blocks.push(acc);
                }
            } else {
                blocks.push(acc);
            }
        }
        if blocks.len() <= 1 {
            out.push(sec);
            continue;
        }
        for (bi, body) in blocks.into_iter().enumerate() {
            let mut block = Section {
                level: if bi == 0 { sec.level } else { sec.level + 1 },
                heading_line: if bi == 0 {
                    sec.heading_line.clone()
                } else {
                    String::new()
                },
                heading: sec.heading.clone(),
                breadcrumb: sec.breadcrumb.clone(),
                body_lines: body,
                group: sec.group,
                has_code: false,
                has_table: false,
                has_param: false,
            };
            classify(&mut block);
            out.push(block);
        }
    }
    out
}

use crate::fold_gate::is_indented_code_line;

/// If `unit` opens with a well-formed markdown table header — a pipe row
/// immediately followed by a `|---|:--:|` delimiter row — return those two
/// lines. `None` for anything else, so a header-less table (rows only, as
/// produced by some HTML→md converters) or a stretch of prose that merely
/// contains pipes never gets a fabricated header.
///
/// Deliberately strict on the delimiter: every cell must be non-empty and
/// made only of `-`, `:` and spaces, with at least one `-`. A row of real
/// data can otherwise pass for a delimiter (`| - | - |` as "n/a" cells is
/// common in changelog tables) and we would replicate data as a header.
pub(crate) fn table_header_prefix(unit: &[String]) -> Option<&[String]> {
    let is_row = |l: &String| {
        let t = l.trim();
        t.starts_with('|') && t.matches('|').count() >= 2
    };
    let (head, delim) = (unit.first()?, unit.get(1)?);
    if !is_row(head) || !is_row(delim) {
        return None;
    }
    let cells = delim.trim().trim_matches('|').split('|');
    let mut any = false;
    for c in cells {
        any = true;
        let c = c.trim();
        if c.is_empty()
            || !c.contains('-')
            || !c.chars().all(|ch| ch == '-' || ch == ':' || ch == ' ')
        {
            return None;
        }
    }
    if !any {
        return None;
    }
    Some(&unit[..2])
}
