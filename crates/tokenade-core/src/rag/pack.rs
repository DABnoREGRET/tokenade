//! **Choosing what FITS in the budget.**
//!
//! The score says what is worth it; packing says what fits. A capped region,
//! a group that must stay whole, a cut that lands on a block boundary:
//! returning half an answer costs the full price and does not answer.

use super::*;

/// The budget as it is spent: what has been taken, and what taking more would
/// cost. Holds the two caps so no call site can apply one and forget the other.
pub(super) struct Packer<'a> {
    sec_tokens: &'a [usize],
    block_score: &'a std::collections::HashMap<usize, f64>,
    budget_tokens: usize,
    window_cap: usize,
    selected: std::collections::BTreeSet<usize>,
    used: usize,
}

impl Packer<'_> {
    fn take(&mut self, i: usize) {
        self.selected.insert(i);
        self.used += self.sec_tokens[i];
    }

    fn fits(&self, i: usize, win_used: usize) -> bool {
        self.used + self.sec_tokens[i] <= self.budget_tokens
            && win_used + self.sec_tokens[i] <= self.window_cap
    }

    fn score(&self, i: usize) -> f64 {
        self.block_score.get(&i).copied().unwrap_or(0.0)
    }

    /// Grow a contiguous same-group window around `anchor`, higher-scoring
    /// neighbour first: the answer is often the sentence NEXT to the block that
    /// matched, but coherence must not starve the other anchors.
    fn expand_window(&mut self, ms: &[usize], anchor: usize) {
        let mut win_used = self.sec_tokens[anchor];
        let apos = ms.iter().position(|&i| i == anchor).unwrap_or(0);
        let (mut lo, mut hi) = (apos, apos);
        loop {
            let prev = lo
                .checked_sub(1)
                .map(|p| ms[p])
                .filter(|i| !self.selected.contains(i));
            let next = (hi + 1 < ms.len())
                .then(|| ms[hi + 1])
                .filter(|i| !self.selected.contains(i));
            let pick = match (
                prev.filter(|&i| self.fits(i, win_used)),
                next.filter(|&i| self.fits(i, win_used)),
            ) {
                (Some(p), Some(nx)) => Some(if self.score(nx) > self.score(p) {
                    nx
                } else {
                    p
                }),
                (Some(p), None) => Some(p),
                (None, Some(nx)) => Some(nx),
                (None, None) => None,
            };
            let Some(i) = pick else { break };
            win_used += self.sec_tokens[i];
            self.take(i);
            if Some(i) == prev {
                lo -= 1;
            } else {
                hi += 1;
            }
        }
    }
}

/// ---- Greedy pack by GROUP (the pre-split section). Blocks carry the
/// scores; groups carry the coherence. A group is pulled in at its best
/// block's score. If the whole group fits the remaining budget it is
/// emitted intact — the deciding block is often one definition while the
/// answer sits in a sibling (measured on MDN's Parameters list) — else
/// its blocks join best-first until the budget is full.
pub(super) fn pack_regions(
    sections: &[Section],
    sec_toks: &[Vec<String>],
    terms: &[String],
    scored: &[(usize, f64)],
    budget_tokens: usize,
) -> std::collections::BTreeSet<usize> {
    let sec_tokens = section_token_costs(sections);
    let block_score: std::collections::HashMap<usize, f64> = scored.iter().copied().collect();
    let members = group_members(sections);
    // Iterate ANCHORS in global block-score order — that guarantees every
    // strong hit gets budget before any single region can swallow it
    // (measured: group-first packing let the top page section eat the whole
    // budget while the marker block sat in the 4th-best group). Around each
    // anchor, expand a contiguous same-group window (higher-scoring
    // neighbour first) capped to a fraction of the budget: the answer is
    // often the sentence NEXT to the block that matched, but coherence must
    // not starve the other anchors.
    let region_cap = region_cap();
    let mut p = Packer {
        sec_tokens: &sec_tokens,
        block_score: &block_score,
        budget_tokens,
        window_cap: ((budget_tokens as f64 / window_div()) as usize).max(120),
        selected: Default::default(),
        used: 0,
    };
    let mut region_count = 0usize;
    // Term-coverage diversity: each next anchor is chosen by score × a bonus
    // for covering query terms the selection hasn't touched yet. "Compare X
    // and Y" prompts need budget on BOTH entities; plain score order spends
    // it all on the stronger one. Single-topic prompts are unaffected — the
    // first anchor covers the terms and the bonus collapses to 1.
    let block_terms = blocks_query_terms(sec_toks, terms);
    let mut covered: std::collections::HashSet<&String> = Default::default();
    while p.used < budget_tokens {
        let pick = scored
            .iter()
            .filter(|(i, _)| !p.selected.contains(i) && p.used + sec_tokens[*i] <= budget_tokens)
            .map(|&(i, s)| {
                let uncovered = block_terms[i]
                    .iter()
                    .filter(|t| !covered.contains(**t))
                    .count();
                let bonus = 1.0 + diversity_bonus() * uncovered as f64 / terms.len().max(1) as f64;
                (i, s * bonus)
            })
            .max_by(|x, y| {
                x.1.partial_cmp(&y.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(y.0.cmp(&x.0))
            });
        let Some((idx, _)) = pick else { break };
        covered.extend(block_terms[idx].iter().copied());
        p.take(idx);
        region_count += 1;
        p.expand_window(&members[&sections[idx].group], idx);
        if region_count >= region_cap {
            break;
        }
    }
    p.selected
}

/// Emit in document order with breadcrumbs + `…` gap markers.
pub(super) fn emit_selected(
    sections: &[Section],
    selected: &std::collections::BTreeSet<usize>,
) -> String {
    let mut out = String::new();
    let mut prev: Option<usize> = None;
    // A heading with no body, emitted without the section that follows it,
    // is a title and nothing under it: `## Fourth Edition` (a title page) came
    // first in the answer to "how do I auto convert an e-book" on calibre's
    // `eng.epub` (2026-10-03). Kept when its next section is emitted too —
    // then it is the heading of what follows.
    let says_something = |i: usize| {
        sections[i].body_lines.iter().any(|l| !l.trim().is_empty()) || selected.contains(&(i + 1))
    };
    for &i in selected.iter().filter(|&&i| says_something(i)) {
        if let Some(p) = prev {
            if i > p + 1 {
                out.push_str("…\n\n");
            }
        }
        // Prefix a breadcrumb only when this section starts a new contiguous
        // run (its parent above wasn't just emitted) — keeps a subtree from
        // repeating the same ancestor path on every child.
        let new_run = prev.map(|p| i > p + 1).unwrap_or(true);
        if new_run {
            out.push_str(&sections[i].breadcrumb_label());
            out.push('\n');
        }
        out.push_str(&section_block(&sections[i]));
        if !out.ends_with('\n') {
            out.push('\n');
        }
        prev = Some(i);
    }
    out
}
