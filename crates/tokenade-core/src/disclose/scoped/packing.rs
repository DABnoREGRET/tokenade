//! **What FITS in the budget.**
//!
//! An answer is made of chosen pieces, not of a continuous excerpt. Two
//! packing shapes, because two shapes of material: a list entry
//! is a unit taken whole or not at all, a paragraph is a
//! window centred on its best passage.
//!
//! Cutting a list entry in the middle returns a fragment that nothing can
//! complete — whereas a truncated paragraph stays readable.

use super::*;

/// The greedy packer's running state.
///
/// `spans` maps a start line to (last line covered, emitted text); `covered`
/// tracks every line already paid for so overlapping picks never double-bill.
/// These were three loose locals that BOTH packing strategies mutated — naming
/// them together is what lets either strategy be read on its own.
#[derive(Default)]
pub struct Packed {
    spans: std::collections::BTreeMap<usize, (usize, String)>,
    covered: std::collections::BTreeSet<usize>,
    used: usize,
}

/// Pack one enumeration entry: the unit's lines collapsed into a single trimmed
/// line, plus its nearest heading for orientation. No ±context — items cost
/// ~10× less than a window, so greedy packing reaches far deeper into the
/// ranking before the budget bites.
///
/// `false` = the budget is spent, stop the walk.
pub fn pack_item_unit(
    p: &mut Packed,
    i: usize,
    lines: &[&str],
    item_head: &[Option<usize>],
    last_heading: &[Option<usize>],
    last: usize,
    budget_tokens: usize,
) -> bool {
    // One-liner item: the unit's lines collapsed into a single
    // trimmed entry. No score floor beyond the >0 the window
    // path already uses — a scattered relevant entry with a
    // modest BM25 score still gets in because items cost ~10×
    // less than a window, so greedy packing reaches far deeper
    // into the ranking before the budget bites.
    let mut end = i;
    while end < last && item_head[end + 1] == Some(i) {
        end += 1;
    }
    let mut text = String::new();
    for j in i..=end {
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(lines[j].trim());
    }
    if text.chars().count() > SCOPED_ITEM_MAX_CHARS {
        let cut = text
            .char_indices()
            .nth(SCOPED_ITEM_MAX_CHARS)
            .map(|(b, _)| b)
            .unwrap_or(text.len());
        text.truncate(cut);
        text.push('…');
    }
    let mut new_tokens = 0usize;
    if !p.covered.contains(&i) {
        new_tokens += crate::tokenizer::estimate(&text) + 1;
    }
    #[allow(unused_assignments)]
    let heading = last_heading[i].filter(|h| !p.covered.contains(h));
    if let Some(h) = heading {
        new_tokens += crate::tokenizer::estimate(lines[h]) + 1;
    }
    if p.used + new_tokens > budget_tokens {
        if !p.covered.is_empty() {
            return false;
        }
        // First pick, over budget: keep the "never return nothing"
        // guarantee but honour the ceiling the caller asked for.
        text = truncate_to_tokens(&text, budget_tokens);
        new_tokens = crate::tokenizer::estimate(&text) + 1;
    }
    if let Some(h) = heading {
        p.spans.insert(h, (h, lines[h].to_string()));
        p.covered.insert(h);
    }
    if !p.covered.contains(&i) {
        p.spans.insert(i, (end, text));
        for j in i..=end {
            p.covered.insert(j);
        }
    }
    p.used += new_tokens;
    true
}

/// Pack one prose window: the matched line and ±[`SCOPED_CONTEXT_LINES`],
/// rendered verbatim.
///
/// `false` = the budget is spent, stop the walk.
pub fn pack_prose_window(
    p: &mut Packed,
    i: usize,
    lines: &[&str],
    last: usize,
    budget_tokens: usize,
) -> bool {
    let lo = i.saturating_sub(SCOPED_CONTEXT_LINES);
    let hi = (i + SCOPED_CONTEXT_LINES).min(last);
    let mut new_tokens = 0usize;
    for j in lo..=hi {
        if !p.covered.contains(&j) {
            new_tokens += crate::tokenizer::estimate(lines[j]) + 1;
        }
    }
    if p.used + new_tokens > budget_tokens {
        if !p.covered.is_empty() {
            return false;
        }
        // First pick, over budget: spend the ceiling on this window,
        // ANCHOR LINE FIRST and then outward. Order matters and getting
        // it wrong is silent: filling from `lo` spent the whole budget
        // on leading context and cut the extract off before the line
        // that actually matched, so the served passage no longer
        // contained the answer it was retrieved for.
        let mut order = vec![i];
        for d in 1..=SCOPED_CONTEXT_LINES {
            if let Some(p) = i.checked_sub(d) {
                if p >= lo {
                    order.push(p);
                }
            }
            if i + d <= hi {
                order.push(i + d);
            }
        }
        let mut spent = 0usize;
        for j in order {
            let t = crate::tokenizer::estimate(lines[j]) + 1;
            if spent + t <= budget_tokens {
                if p.covered.insert(j) {
                    p.spans.insert(j, (j, lines[j].to_string()));
                }
                spent += t;
            } else if p.covered.is_empty() {
                // The anchor alone exceeds the budget: serve as much of
                // IT as fits rather than nothing.
                let cut = truncate_to_tokens(lines[j], budget_tokens);
                spent += crate::tokenizer::estimate(&cut) + 1;
                p.covered.insert(j);
                p.spans.insert(j, (j, cut));
            }
        }
        // `p.used` is not updated: this branch always breaks out of the
        // packing loop, so nothing reads it again. Keeping the write
        // would be a lie the compiler correctly calls dead.
        return false;
    }
    for j in lo..=hi {
        if p.covered.insert(j) {
            p.spans.insert(j, (j, lines[j].to_string()));
        }
    }
    p.used += new_tokens;
    true
}

/// Join the picked spans in document order, marking every gap with `…` so the
/// reader can never mistake an excerpt for the whole.
pub fn render_picked(spans: &std::collections::BTreeMap<usize, (usize, String)>) -> Option<String> {
    if spans.is_empty() {
        return None;
    }
    let mut out = String::new();
    let mut prev_end: Option<usize> = None;
    for (&i, (end, text)) in spans {
        if let Some(p) = prev_end {
            if i > p + 1 {
                out.push_str("\n…\n");
            }
        }
        out.push_str(text);
        out.push('\n');
        prev_end = Some(*end);
    }
    let trimmed = out.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub fn scoped_extract_inner(
    text: &str,
    prompt: &str,
    budget_tokens: usize,
    enum_aware: bool,
) -> Option<String> {
    let query_terms = scoped_query_terms(prompt);
    if query_terms.is_empty() {
        return None;
    }
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return None;
    }
    // Tokenise every line once — reused for df, avg length, and scoring.
    let line_toks: Vec<Vec<String>> = lines.iter().map(|l| bm25_tokenize(l)).collect();
    let idf = bm25_idf(&query_terms, &line_toks);
    // Anchor gate: at least one surviving query term must be
    // discriminating (present on < ~half the lines). Otherwise the
    // prompt has no lexical foothold in this page → bail so the caller
    // serves the page whole rather than a misleading pseudo-excerpt.
    let max_idf = idf
        .iter()
        .filter(|(t, _)| !is_function_word(t))
        .map(|(_, v)| *v)
        .fold(0.0_f64, f64::max);
    if max_idf < SCOPED_MIN_ANCHOR_IDF {
        return None;
    }
    let mut scored = scoped_line_scores(&query_terms, &line_toks, &idf);
    if scored.is_empty() {
        return None;
    }
    // ---- Enumeration awareness. On pages dominated by list/table/link
    // entries the ±context window is the wrong packing unit: every hit
    // drags in 2·SCOPED_CONTEXT_LINES neighbour entries that scored
    // nothing, so the budget dies after a handful of windows while
    // relevant entries scattered across a long list never surface
    // (MEASURED vs native WebFetch on curated link-list READMEs: we
    // surfaced 0 where native kept 4). Entries are self-contained
    // one-liners — pack them as such and the same budget covers ~10×
    // more of the list. Gated on document-level item density so prose
    // pages keep the window path byte-for-byte.
    let last = lines.len() - 1;
    // A RECORD stream (a log, `grep -r` output) is a list whose entries are
    // its lines: a hit's neighbours are other requests, not context. Packed
    // as ±windows, 10 log lines matching `items/777 … status=500` came back
    // as 2, each inside eight unrelated lines, with the budget spent
    // (2026-10-03).
    let record_stream = crate::disclose::is_record_stream(text);
    let item_head = if record_stream {
        lines
            .iter()
            .enumerate()
            .map(|(i, l)| (!l.trim().is_empty()).then_some(i))
            .collect()
    } else {
        scoped_item_heads(&lines)
    };
    let non_empty = lines.iter().filter(|l| !l.trim().is_empty()).count();
    let item_lines = item_head.iter().filter(|h| h.is_some()).count();
    let enum_mode = enum_aware
        && non_empty >= SCOPED_ENUM_MIN_LINES
        && item_lines as f64 >= SCOPED_ENUM_DENSE_FRACTION * non_empty as f64;
    if enum_mode {
        // Score a wrapped item as ONE unit: continuation-line scores
        // collapse onto their head so a unit competes exactly once.
        let mut best: std::collections::HashMap<usize, f64> = std::collections::HashMap::new();
        for (i, s) in &scored {
            let key = item_head[*i].unwrap_or(*i);
            let e = best.entry(key).or_insert(0.0);
            if *s > *e {
                *e = *s;
            }
        }
        scored = best.into_iter().collect();
    }
    // Highest score first; ties broken by document order (stable sort
    // over the doc-ordered input did this implicitly before — the
    // explicit tie-break keeps it deterministic after the HashMap fold).
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    let last_heading = scoped_last_headings(&lines);
    // Greedily pack, highest score first, until the token budget is
    // exhausted. `rendered` maps a start line to (covered end line,
    // emitted text); `covered` tracks every line already paid for so
    // overlapping picks don't double-bill. On the window path each line
    // renders verbatim (byte-identical to the historic output); on the
    // item path a unit renders as one trimmed line with no ±context.
    let mut p = Packed::default();
    for (i, _) in &scored {
        let i = *i;
        let ok = if enum_mode && item_head[i] == Some(i) {
            pack_item_unit(
                &mut p,
                i,
                &lines,
                &item_head,
                &last_heading,
                last,
                budget_tokens,
            )
        } else {
            pack_prose_window(&mut p, i, &lines, last, budget_tokens)
        };
        if !ok || p.used >= budget_tokens {
            break;
        }
    }
    render_picked(&p.spans)
}
