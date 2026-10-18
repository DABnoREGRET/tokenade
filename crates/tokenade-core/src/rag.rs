//! Structural + dense retrieval over any text (lever #1 v2).
//!
//! Named `web_rag` until 2026-07 because WebFetch was the first caller. It was
//! never web-specific: the engine takes markdown and a question, so a document
//! off disk goes through the same call (`compactors::cmd_web --prompt`). What
//! it needs is a QUERY — which is why the stash path, having none, cannot use
//! it.
//!
//! The flat baseline [`crate::disclose::prompt_scoped_extract`] scores the
//! distilled markdown *line by line* with BM25 and returns the top lines
//! plus a fixed ±4-line context window. That works, but it hands the model
//! arbitrary line slices that straddle section boundaries and drop the
//! heading a passage lives under.
//!
//! This module segments the markdown into typed **sections** by heading
//! hierarchy, scores each section with a **hybrid** signal (the shared
//! BM25 tokenizer + IDF over sections, blended with dense cosine from
//! [`crate::embedding`]), boosts sections whose heading /
//! breadcrumb match the query and whose body carries answer-bearing shapes
//! (code / params / tables), and returns a *coherent section subtree*
//! prefixed with its breadcrumb.
//!
//! It is **on by default** since 0.8.13 (`TOKENADE_RAG=0` opts out; dispatched
//! by [`prompt_scoped_extract_dispatch`], which has lived in this module since
//! 2026-09-13 — the choice of engine belongs to whoever knows both) and degrades to the
//! flat baseline on any embedding error, empty / headingless input, or when
//! no section carries a lexical anchor — never a regression, never a panic,
//! fully deterministic (no RNG, no clock).

mod dispatch;
mod split;
pub use dispatch::*;
pub(crate) use split::*;

use crate::disclose::{bm25_tokenize, prompt_scoped_extract};

/// Default hybrid blend weight (matches `search_hybrid`): `alpha` on the
/// normalised BM25 signal, `1-alpha` on the normalised dense signal.
const DEFAULT_ALPHA: f64 = 0.5;
/// Type bonus multiplier for a section whose body carries an answer-bearing
/// shape (code fence / indented code / parameter list / table).
const TYPE_BONUS: f64 = 1.2;
/// Field-weight multiplier when a query term matches inside the section's
/// own heading.
const HEADING_MULT: f64 = 3.0;
/// Field-weight multiplier when a query term matches inside an ancestor
/// (breadcrumb) heading but not the section's own heading.
const BREADCRUMB_MULT: f64 = 2.0;
/// Truncate the *embedding input* for a section to this many bytes (the
/// full text is still emitted). ~2 KB ≈ the first ~512 tokens — enough to
/// characterise a section without paying to embed a whole page of prose.
const EMBED_TRUNC_BYTES: usize = 2048;

/// Sub-split granularity (block-level retrieval): a section whose body
/// exceeds MAX chars is split at paragraph boundaries into blocks of
/// [MIN, MAX] chars, small paragraphs aggregated. Measured cause of the v2
/// regression: packing whole sections was too coarse — one fat section ate
/// the budget or was skipped entirely. Field values ~200-500 chars are the
/// sweet spot for prose block retrieval; the ceiling is a bit higher here
/// because doc pages carry code blocks (kept unsplit).
const SPLIT_MIN_CHARS: usize = 200;
const SPLIT_MAX_CHARS: usize = 600;

/// Max distinct anchor regions packed into one scoped extract. Measured
/// recall@K plateaus at 4 (a 5th region adds ~0pp). See the packing loop.
const RAG_MAX_REGIONS_DEFAULT: usize = 4;

/// Read an `f64` hyperparameter from `key`, else `default`. Lets the tuning
/// harness sweep every knob without recompiling; production reads the
/// baked-in defaults (no env set).
fn hp(key: &str, default: f64) -> f64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|x| x.is_finite())
        .unwrap_or(default)
}

fn type_bonus() -> f64 {
    hp("TOKENADE_RAG_TYPE_BONUS", TYPE_BONUS)
}

fn heading_mult() -> f64 {
    hp("TOKENADE_RAG_HEADING_MULT", HEADING_MULT)
}

fn breadcrumb_mult() -> f64 {
    hp("TOKENADE_RAG_BREADCRUMB_MULT", BREADCRUMB_MULT)
}

fn qa_answer_mult() -> f64 {
    hp("TOKENADE_RAG_QA_MULT", QA_ANSWER_MULT)
}

fn nav_link_penalty() -> f64 {
    hp("TOKENADE_RAG_NAV_PENALTY", NAV_LINK_PENALTY)
}

fn diversity_bonus() -> f64 {
    hp("TOKENADE_RAG_DIVERSITY", DIVERSITY_BONUS)
}

fn nav_density_thresh() -> f64 {
    hp("TOKENADE_RAG_NAV_DENSITY", 0.5)
}

fn split_min() -> usize {
    hp("TOKENADE_RAG_SPLIT_MIN", SPLIT_MIN_CHARS as f64) as usize
}

fn split_max() -> usize {
    hp("TOKENADE_RAG_SPLIT_MAX", SPLIT_MAX_CHARS as f64) as usize
}

/// Window cap = budget / this divisor (coherence-vs-breadth). Default 3.
fn window_div() -> f64 {
    hp("TOKENADE_RAG_WINDOW_DIV", 3.0).max(1.0)
}

/// Fusion budget = budget / this divisor. Default 2.
fn fusion_div() -> f64 {
    hp("TOKENADE_RAG_FUSION_DIV", 2.0).max(1.0)
}

/// Counter-boost for answer sections on Q&A-shaped pages (both "Question"
/// and "Answer…" headings present): the question echoes the query and wins
/// raw similarity, but the answers are the retrieval target.
const QA_ANSWER_MULT: f64 = 1.8;

/// Soft demotion for blocks that are mostly links (TOC / nav / link-box
/// survivors) — they match lexically but carry no answer text.
const NAV_LINK_PENALTY: f64 = 0.5;

/// Anchor-selection bonus per fraction of query terms the candidate covers
/// that the selection so far doesn't (multi-entity "compare X and Y"
/// prompts; collapses to no-op once terms are covered).
const DIVERSITY_BONUS: f64 = 0.8;

/// Fraction of a block's chars covered by markdown links.
fn link_density(text: &str) -> f64 {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"\[[^\]]*\]\([^)]*\)").expect("static regex"));
    let total = text.trim().chars().count().max(1);
    let link: usize = re.find_iter(text).map(|m| m.as_str().chars().count()).sum();
    link as f64 / total as f64
}

/// One markdown section keyed by heading hierarchy.
pub(crate) struct Section {
    /// Heading level 1..=6, or 0 for the synthetic pre-heading intro.
    level: usize,
    /// The full heading markdown line (with leading `#`s), or empty for the
    /// intro. Reconstructed verbatim into the output.
    heading_line: String,
    /// Heading text with the `#`s and surrounding whitespace stripped.
    heading: String,
    /// Ancestor heading texts, outermost first (breadcrumb, excludes self).
    breadcrumb: Vec<String>,
    /// Body lines between this heading and the next (heading excluded).
    body_lines: Vec<String>,
    /// Which pre-split section this block came from ([`subsplit`] keeps the
    /// id across the blocks it cuts) — the coherence unit for emission.
    group: usize,
    has_code: bool,
    has_table: bool,
    has_param: bool,
}

impl Section {
    /// Heading + body, for BM25 / dense scoring and output token estimates.
    fn text(&self) -> String {
        let mut s = String::new();
        if !self.heading.is_empty() {
            s.push_str(&self.heading);
            s.push('\n');
        }
        for l in &self.body_lines {
            s.push_str(l);
            s.push('\n');
        }
        s
    }

    /// `> Vec > Methods`-style breadcrumb (ancestors + own heading).
    ///
    /// Bounded, because "heading" is whatever [`segment`] decided a heading is.
    /// On a page with no real hierarchy — an `llms.txt`, a wall of prose where
    /// full sentences parse as headings — the ancestor chain is paragraphs long,
    /// and three of them cost 5 648 tokens (MEASURED 2026-08-01,
    /// `tokenade.net/llms.txt`). A label exists to say WHERE a passage came
    /// from; past a couple of lines it stops doing that and starts being the
    /// payload.
    fn breadcrumb_label(&self) -> String {
        const MAX_LABEL_CHARS: usize = 160;
        let mut parts: Vec<&str> = self.breadcrumb.iter().map(|s| s.as_str()).collect();
        if !self.heading.is_empty() {
            parts.push(self.heading.as_str());
        }
        if parts.is_empty() {
            // A document with no headings yields one synthetic section, which
            // `subsplit` then cuts into blocks — and every block used to be
            // labelled `(intro)`. Measured on a 1 500-line blob, a two-passage
            // answer labelled BOTH passages that way: one opened at line 128,
            // the other at line 902. A label exists to say where a passage came
            // from, and for the second one that label was simply false.
            //
            // `subsplit` gives the first block the section's own level and
            // every later one `level + 1`, so the synthetic intro's opening
            // block is the only one still at 0. That is exactly the block the
            // word "intro" is true of; the rest say they continue, which
            // claims no position and is true of all of them.
            return if self.level == 0 {
                "> (intro)".to_string()
            } else {
                "> (continued)".to_string()
            };
        }
        let full = format!("> {}", parts.join(" > "));
        if full.chars().count() <= MAX_LABEL_CHARS {
            return full;
        }
        let cut: String = full.chars().take(MAX_LABEL_CHARS).collect();
        format!("{}…", cut.trim_end())
    }
}

/// Parse `# ..######` heading level from a line, returning the level and the
/// trimmed heading text. Requires a space after the `#`s (ATX headings), so
/// a bare `#foo` or a `####` rule is not misread as a heading.
///
/// A tab, or four-or-more spaces before a single `# `, is indented code
/// — same gate as `markdown::outline::parse_heading` (round/2026-09-19-11).
/// Depth ≥2 still matches when indented (PDF extractors).
fn parse_heading(line: &str) -> Option<(usize, String)> {
    let t = line.trim_start();
    if !t.starts_with('#') {
        return None;
    }
    let hashes = t.chars().take_while(|&c| c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &t[hashes..];
    if !rest.starts_with(' ') && !rest.starts_with('\t') {
        return None;
    }
    if hashes == 1
        && (line.starts_with('\t') || line.bytes().take_while(|&b| b == b' ').count() > 3)
    {
        return None;
    }
    Some((hashes, rest.trim().to_string()))
}

/// Segment markdown into sections by heading hierarchy, maintaining an
/// ancestor breadcrumb stack. Content before the first heading becomes a
/// synthetic level-0 intro section (only when it holds non-blank text).
fn segment(markdown: &str) -> Vec<Section> {
    let mut sections: Vec<Section> = Vec::new();
    // Ancestor stack of (level, heading) currently in scope.
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut cur: Option<Section> = None;
    let mut intro: Vec<String> = Vec::new();
    let mut seen_heading = false;

    // Same fence guard as `markdown::outline`: a `#` inside a code block is a
    // comment. Here it decides SECTION BOUNDARIES, so a code comment used to
    // split a retrievable passage in two and title it with the comment —
    // degrading exactly the `expand-ref --prompt` lookups that documentation
    // pages depend on once they are stashed.
    let mut in_fence = false;
    for line in markdown.lines() {
        if line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~") {
            in_fence = !in_fence;
        }
        if let Some((level, heading)) = parse_heading(line).filter(|_| !in_fence) {
            seen_heading = true;
            if let Some(sec) = cur.take() {
                sections.push(sec);
            } else if !intro.is_empty() {
                // Flush the pre-heading intro exactly once.
                let mut sec = Section {
                    level: 0,
                    heading_line: String::new(),
                    heading: String::new(),
                    breadcrumb: Vec::new(),
                    body_lines: std::mem::take(&mut intro),
                    group: 0,
                    has_code: false,
                    has_table: false,
                    has_param: false,
                };
                classify(&mut sec);
                if sec.body_lines.iter().any(|l| !l.trim().is_empty()) {
                    sections.push(sec);
                }
            }
            // Pop ancestors at the same or deeper level, then this heading's
            // breadcrumb is whatever remains in scope.
            while stack.last().map(|(l, _)| *l >= level).unwrap_or(false) {
                stack.pop();
            }
            let breadcrumb = stack.iter().map(|(_, h)| h.clone()).collect();
            stack.push((level, heading.clone()));
            cur = Some(Section {
                level,
                heading_line: line.to_string(),
                heading,
                breadcrumb,
                body_lines: Vec::new(),
                group: 0,
                has_code: false,
                has_table: false,
                has_param: false,
            });
        } else if let Some(sec) = cur.as_mut() {
            sec.body_lines.push(line.to_string());
        } else if !seen_heading {
            intro.push(line.to_string());
        }
    }
    if let Some(mut sec) = cur.take() {
        classify(&mut sec);
        sections.push(sec);
    } else if !seen_heading && intro.iter().any(|l| !l.trim().is_empty()) {
        // Whole doc had no headings → single intro section (caller falls back).
        let mut sec = Section {
            level: 0,
            heading_line: String::new(),
            heading: String::new(),
            breadcrumb: Vec::new(),
            body_lines: intro,
            group: 0,
            has_code: false,
            has_table: false,
            has_param: false,
        };
        classify(&mut sec);
        sections.push(sec);
    }
    sections
}

mod pack;
mod score;
use pack::*;
use score::*;

pub fn prompt_scoped_extract_rag(
    markdown: &str,
    prompt: &str,
    budget_tokens: usize,
) -> Option<String> {
    let terms = query_terms(prompt);
    if terms.is_empty() {
        // **Say WHICH gate refuses.** The flat fallback is correct, but it is
        // silent: measured on 93 real agent questions taken from transcripts,
        // **33 do not take this path** and nothing said why. A debug log
        // names the gate; it costs nothing when `TOKENADE_DEBUG_LOG` is
        // empty.
        crate::debug_log::log(
            "rag",
            "fallback",
            serde_json::json!({ "why": "terms_empty", "prompt": prompt.chars().take(80).collect::<String>() }),
        );
        // Same contract as baseline: vague/empty prompt → whole-page serve.
        return None;
    }
    let mut sections = segment(markdown);
    for (i, s) in sections.iter_mut().enumerate() {
        s.group = i;
    }
    // Block granularity: fat sections split into retrievable blocks (the
    // measured v2 failure was section-level packing being too coarse). This
    // also carries headingless pages: paragraph blocks are retrievable units
    // even without a heading hierarchy (measured on kernel-parameters-style
    // definition pages, where the old headingless bailout forced the flat
    // baseline and both missed). Only a page too small to split at all
    // still defers to the flat baseline.
    let sections = subsplit(sections);
    if sections.len() <= 1 {
        crate::debug_log::log(
            "rag",
            "fallback",
            serde_json::json!({ "why": "too_few_blocks" }),
        );
        return prompt_scoped_extract(markdown, prompt, budget_tokens);
    }

    let n = sections.len();
    let sec_texts: Vec<String> = sections.iter().map(|s| s.text()).collect();
    let sec_toks: Vec<Vec<String>> = sec_texts.iter().map(|t| bm25_tokenize(t)).collect();

    // ---- BM25 over sections-as-documents (df = #sections containing term).
    // Same BM25 as the flat extractor, over SECTIONS instead of lines — the
    // formula was written out here a second time until the two were diffed.
    let avg_len = crate::disclose::bm25_avg_len(&sec_toks);
    let idf = crate::disclose::bm25_idf(&terms, &sec_toks);
    let bm25: Vec<f64> = sec_toks
        .iter()
        .map(|toks| crate::disclose::bm25_score_one(&terms, toks, &idf, avg_len))
        .collect();
    let bm25_max = bm25.iter().cloned().fold(0.0_f64, f64::max);
    // Lexical anchor gate: at least one query term must be DISCRIMINATING —
    // the same rule, and the same constant, the flat baseline has always
    // applied. "Shares a word with the query" is not that rule: a function
    // word like `of` or `an` sits in every section, so its IDF is near zero
    // yet strictly positive, and the old `bm25_max > 0` test passed on it.
    // A question the document cannot answer ("airspeed velocity of an
    // unladen swallow") then came back as a confident set of passages,
    // indistinguishable from an answer, ranked on nothing but `of` and `an`.
    // Falling through to the flat baseline lets it re-apply the gate and
    // serve the page whole rather than a pseudo-excerpt.
    let max_idf = idf
        .iter()
        .filter(|(t, _)| !crate::disclose::is_function_word(t))
        .map(|(_, v)| *v)
        .fold(0.0_f64, f64::max);
    if bm25_max <= 0.0 || max_idf < crate::disclose::SCOPED_MIN_ANCHOR_IDF {
        crate::debug_log::log(
            "rag",
            "fallback",
            serde_json::json!({ "why": "no_anchor", "bm25_max": bm25_max, "max_idf": max_idf, "prompt": prompt.chars().take(80).collect::<String>() }),
        );
        return prompt_scoped_extract(markdown, prompt, budget_tokens);
    }

    // ---- Dense cosine + zero-LLM intent labels, batched in ONE embed call
    // (query + all blocks + the label descriptions — labels add a dozen tiny
    // inputs to a call we make anyway).
    let cand = embed_candidates(&bm25, n);
    let Some(dense) = dense_scores(&terms, prompt, &sec_texts, &cand, n) else {
        crate::debug_log::log(
            "rag",
            "fallback",
            serde_json::json!({ "why": "embed_failed" }),
        );
        return prompt_scoped_extract(markdown, prompt, budget_tokens);
    };
    let dense_max = dense.iter().cloned().fold(0.0_f64, f64::max);

    let scored = score_sections(&BlockSignals {
        sections: &sections,
        sec_texts: &sec_texts,
        terms: &terms,
        bm25: &bm25,
        bm25_max,
        dense: &dense,
        dense_max,
    });
    if scored.is_empty() {
        crate::debug_log::log("rag", "fallback", serde_json::json!({ "why": "no_score" }));
        return prompt_scoped_extract(markdown, prompt, budget_tokens);
    }

    let selected = pack_regions(&sections, &sec_toks, &terms, &scored, budget_tokens);
    if selected.is_empty() {
        crate::debug_log::log(
            "rag",
            "fallback",
            serde_json::json!({ "why": "empty_pack" }),
        );
        return prompt_scoped_extract(markdown, prompt, budget_tokens);
    }

    let mut out = emit_selected(&sections, &selected);
    // ---- Flat-fusion safety net. Line-granular BM25 excels exactly where
    // block granularity dilutes a needle (definition walls, single-term
    // queries — measured: `nosmt` in kernel-parameters, a tagline under a
    // README's own name). Append the flat extractor's top lines that the
    // structural pick didn't already cover, capped to a half budget (the
    // needle lines rank deep in flat's ordering — a quarter budget measured
    // short of them): the union is never much worse than either extractor
    // alone, and recall is worth more than the extra tokens here.
    let fusion_budget = ((budget_tokens as f64 / fusion_div()) as usize).max(150);
    merge_flat_baseline(&mut out, markdown, prompt, fusion_budget);
    let trimmed = out.trim();
    if trimmed.is_empty() {
        prompt_scoped_extract(markdown, prompt, budget_tokens)
    } else {
        // Generated documents (changelogs, journals, status reports rendered
        // as prose) answer a question with the same sentence fifteen times
        // over. Fold those, keeping every value; leave real prose alone — and
        // never a code fence: the passages that answer a question about a
        // library are largely code samples, and folding one into a template
        // plus a value list hands back something the caller cannot run.
        Some(crate::template_fold::fold_if_it_pays_outside_code(trimmed))
    }
}

/// The heading line + body of a section, reconstructed verbatim.
fn section_block(sec: &Section) -> String {
    let mut s = String::new();
    if !sec.heading_line.is_empty() {
        s.push_str(&sec.heading_line);
        s.push('\n');
    }
    for l in &sec.body_lines {
        s.push_str(l);
        s.push('\n');
    }
    // Trim trailing blank lines so packed sections don't accrue whitespace.
    s.trim_end().to_string()
}

/// Index one past the last section in `idx`'s subtree: the first later
/// section whose level is `<=` this section's level (a sibling or ancestor),
/// or `n`. A level-0 intro owns nothing (its "subtree" is just itself).
/// (Packing moved to group granularity; kept for the structural tests.)
#[cfg_attr(not(test), allow(dead_code))]
fn subtree_end(sections: &[Section], idx: usize) -> usize {
    let level = sections[idx].level;
    if level == 0 {
        return idx + 1;
    }
    let mut end = idx + 1;
    while end < sections.len() && sections[end].level > level {
        end += 1;
    }
    end
}

/// Field-weight multiplier: a query term in the section's own heading is
/// worth [`HEADING_MULT`], a term only in an ancestor breadcrumb is worth
/// [`BREADCRUMB_MULT`], otherwise `1.0`.
fn field_mult(sec: &Section, terms: &[String]) -> f64 {
    let heading_toks = bm25_tokenize(&sec.heading);
    if terms.iter().any(|t| heading_toks.contains(t)) {
        return heading_mult();
    }
    let crumb_toks: Vec<String> = sec
        .breadcrumb
        .iter()
        .flat_map(|h| bm25_tokenize(h))
        .collect();
    if terms.iter().any(|t| crumb_toks.contains(t)) {
        return breadcrumb_mult();
    }
    1.0
}

/// Truncate to at most `max` bytes on a UTF-8 char boundary (never panics).
fn truncate_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
include!("rag_tests.rs");
