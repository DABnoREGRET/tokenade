//! **What the HARNESS gathers** — without a model, and therefore without
//! invention.
//!
//! The scout is not an agent: everything below is deterministic Rust. File
//! ranking, lexical windows within each file, web pages, and packing under a
//! budget. The small local model then only does the language step — answer
//! and quote.
//!
//! That split is what makes verification possible: if the evidence were
//! produced by the model, checking the model against it would prove nothing.

use super::*;

// ---- web evidence (deterministic: search engines + page fetch) -------------

pub(crate) const WEB_PAGES: usize = 3;
pub(crate) const WEB_WINDOWS_PER_PAGE: usize = 2;
pub(crate) const WEB_WINDOW_LINES: usize = 40;

/// Search the live web (reusing the WebSearch gate's hardened engine fetchers)
/// and pack keyword-dense windows of the top pages. Same contract as code
/// evidence: numbered lines the model cites by (id, line); the "file" is the
/// page URL.
pub(crate) fn gather_web_evidence(question: &str) -> Result<Vec<Snippet>, String> {
    // Reuse the consolidated search core: it fans out across engines, parses,
    // dedups and ranks — with the cross-process-safe breaker — in one call.
    // (Was a hand-rolled loop over the old per-engine
    // `websearch_pretool::{ENGINES,fetch}`, removed when the search stack was
    // unified into `search::run_search`.)
    let now = tokenade_core::platform::now_unix_secs();
    let hits: Vec<tokenade_core::serp::Result> =
        tokenade_core::search::run_search(question, now).results;
    if hits.is_empty() {
        return Err("web search returned no results (all engines failed)".into());
    }

    let keywords = keywords(question);
    let mut pages: Vec<(String, String)> = Vec::new(); // (url, markdown)
    for h in hits.iter().take(WEB_PAGES) {
        if let Some(md) = fetch_page_markdown(&h.url) {
            pages.push((h.url.clone(), md));
        }
    }
    if pages.is_empty() {
        return Err("could not fetch any result page".into());
    }

    // Rarity weights across the fetched pages (same scheme as code evidence).
    let mut global_hits = vec![0usize; keywords.len()];
    for (_, md) in &pages {
        let low = md.to_lowercase();
        for (i, k) in keywords.iter().enumerate() {
            global_hits[i] += low.matches(k.as_str()).count();
        }
    }
    let weights: Vec<usize> = global_hits
        .iter()
        .map(|&h| {
            if h == 0 {
                0
            } else {
                (1000.0 / (h as f64).sqrt()) as usize
            }
        })
        .collect();

    let mut snippets = Vec::new();
    let mut budget = MAX_EVIDENCE_CHARS;
    for (url, md) in &pages {
        let mut taken = 0;
        for w in best_windows_sized(md, &keywords, &weights, WEB_WINDOW_LINES) {
            if taken >= WEB_WINDOWS_PER_PAGE || w.text.len() + 64 > budget {
                continue;
            }
            budget -= w.text.len() + 64;
            taken += 1;
            snippets.push(Snippet {
                file: PathBuf::from(url),
                start_line: w.start_line,
                text: w.text,
            });
        }
    }
    Ok(snippets)
}

/// Fetch one web page and reduce it to markdown (tokenade's own extractor).
pub(crate) fn fetch_page_markdown(url: &str) -> Option<String> {
    const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                      (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
    // `http_get` follows redirects + gunzips (was `curl -sSL --compressed`);
    // `--fail` (drop non-2xx bodies) maps to the `resp.ok()` check.
    let resp = tokenade_core::net::http_get(url, &[("User-Agent", UA)], 25)?;
    if !resp.ok() || resp.body.is_empty() {
        return None;
    }
    let html = String::from_utf8_lossy(&resp.body);
    let md = tokenade_core::web::html_to_markdown(&html);
    if md.trim().is_empty() {
        None
    } else {
        Some(md)
    }
}

// ---- evidence gathering (deterministic, zero SLM calls) --------------------

pub(crate) fn gather_evidence(question: &str) -> Result<Vec<Snippet>, String> {
    let intel = load_intel()?;
    Ok(evidence_for(&intel, question))
}

/// Deterministic evidence collection against pre-loaded indices (shared with
/// the explore loop, which reuses one Intel across turns).
pub(super) fn evidence_for(intel: &Intel, question: &str) -> Vec<Snippet> {
    let keywords = keywords(question);
    let mut files = intel.semantic_files(question);
    // Symbol-looking tokens (snake_case / :: / CamelCase) also go through the
    // AST index — exact symbol hits beat semantic file ranking for locate tasks.
    for tok in question.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':')) {
        if looks_like_symbol(tok) {
            for (f, _line) in intel.query_hits(tok) {
                if !files.contains(&f) {
                    files.push(f);
                }
            }
        }
    }
    files.truncate(top_files() + 2);

    // Two passes: first count how often each keyword hits across ALL candidate
    // files (its rarity), then score windows with rarity-weighted keywords. A
    // generic term ("llama", "local") hitting hundreds of lines must not outrank
    // one hit of a rare, specific term ("k-cache", "quantization").
    let contents: Vec<(usize, &PathBuf, String)> = files
        .iter()
        .enumerate()
        .filter_map(|(rank, f)| std::fs::read_to_string(f).ok().map(|c| (rank, f, c)))
        .collect();
    let mut global_hits = vec![0usize; keywords.len()];
    for (_, _, c) in &contents {
        let low = c.to_lowercase();
        for (i, k) in keywords.iter().enumerate() {
            global_hits[i] += low.matches(k.as_str()).count();
        }
    }
    // weight = 1000/sqrt(hits): 1 hit → 1000, 100 hits → 100, absent → 0.
    let weights: Vec<usize> = global_hits
        .iter()
        .map(|&h| {
            if h == 0 {
                0
            } else {
                (1000.0 / (h as f64).sqrt()) as usize
            }
        })
        .collect();

    // Collect windows from every ranked file, then rank GLOBALLY: weighted
    // keyword density first, semantic file rank as tie-break. A noisy repo
    // (vendored trees, fixtures) otherwise fills the pack with weak windows
    // from well-ranked-but-irrelevant files.
    let mut candidates: Vec<(usize, usize, Snippet)> = Vec::new(); // (score, file_rank, snip)
    for (rank, f, content) in &contents {
        for w in best_windows(content, &keywords, &weights) {
            candidates.push((
                w.score,
                *rank,
                Snippet {
                    file: (*f).clone(),
                    start_line: w.start_line,
                    text: w.text,
                },
            ));
        }
    }
    candidates.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));

    let mut snippets = Vec::new();
    let mut budget = max_evidence_chars();
    for (_, _, s) in candidates {
        if s.text.len() + 64 > budget {
            continue;
        }
        budget -= s.text.len() + 64;
        snippets.push(s);
        if snippets.len() >= max_snippets() {
            break;
        }
    }
    expand_definitions(&mut snippets, &contents, Some(intel));
    snippets
}

/// Vendored/generated trees poison evidence packs (measured: answers citing
/// qwen-code/ TS internals for a question about OUR sandbox). Project code only.
///
/// `experiments` is on this list for a sharper reason than noise: it is where
/// this scout's OWN eval harness lives, so leaving it in let the harness grade
/// itself. `run_eval.py` sets `TOKENADE_LOCAL_PARALLEL="1"` and
/// `TOKENADE_LOCAL_ADDR=f"127.0.0.1:{port}"`, and its results/journal JSON carry
/// model filenames like `qwen3-1.7b-q4_k_m.gguf` — so the retrieval fed the
/// eval's own config back to the model, which dutifully answered "1",
/// "127.0.0.1", "qwen3-1.7b" and "q4" to the very questions whose ground truth
/// is `unwrap_or(2)`, `127.0.0.1:8919`, `qwen3-4b` and `q8_0`. Every one of the
/// tier-1 failures was the harness reading itself, and it worsened as the
/// autoresearch wrote more journal entries INTO the indexed tree. An eval corpus
/// that contains the eval is not a measurement.
pub(crate) fn is_noise_path(p: &std::path::Path) -> bool {
    const NOISE: &[&str] = &[
        "node_modules",
        "target",
        "qwen-code",
        "competitors",
        ".serena",
        "benchmarks",
        "fixtures",
        "vendor",
        "dist",
        ".git",
        "experiments",
    ];
    p.components().any(|c| {
        let s = c.as_os_str().to_string_lossy();
        NOISE.contains(&s.as_ref())
    })
}

pub(crate) struct Window {
    start_line: usize,
    score: usize,
    text: String,
}

/// Best keyword-density windows of a file: score each line by rarity-weighted
/// keyword hits, take the top scoring centers, expand ±WINDOW_LINES/2, skip
/// overlaps. A window's score sums the line scores it covers (density, not
/// just the peak).
pub(crate) fn best_windows(content: &str, keywords: &[String], weights: &[usize]) -> Vec<Window> {
    best_windows_sized(content, keywords, weights, window_lines())
}

pub(crate) fn best_windows_sized(
    content: &str,
    keywords: &[String],
    weights: &[usize],
    window_lines: usize,
) -> Vec<Window> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return Vec::new();
    }
    let line_scores: Vec<usize> = lines
        .iter()
        .map(|l| {
            let low = l.to_lowercase();
            keywords
                .iter()
                .zip(weights)
                .filter(|(k, _)| low.contains(k.as_str()))
                .map(|(_, w)| *w)
                .sum()
        })
        .collect();
    let mut scored: Vec<(usize, usize)> = line_scores
        .iter()
        .enumerate()
        .filter(|(_, s)| **s > 0)
        .map(|(i, s)| (*s, i))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0));

    let half = window_lines / 2;
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for &(_, center) in scored.iter() {
        if ranges.len() >= windows_per_file() {
            break;
        }
        let s = center.saturating_sub(half);
        let e = (center + half).min(lines.len() - 1);
        if ranges.iter().any(|&(rs, re)| s <= re && rs <= e) {
            continue; // overlaps an already-taken window
        }
        ranges.push((s, e));
    }
    ranges.sort();
    ranges
        .into_iter()
        .map(|(s, e)| Window {
            start_line: s + 1,
            score: line_scores[s..=e].iter().sum(),
            text: lines[s..=e].join("\n"),
        })
        .collect()
}

pub(super) fn pack(snippets: &[Snippet]) -> String {
    let mut out = String::new();
    for (i, s) in snippets.iter().enumerate() {
        let end = s.start_line + s.text.lines().count().saturating_sub(1);
        out.push_str(&format!(
            "[{i}] {}:{}-{}\n",
            s.file.display(),
            s.start_line,
            end
        ));
        for (j, l) in s.text.lines().enumerate() {
            out.push_str(&format!("{}| {}\n", s.start_line + j, l));
        }
        out.push('\n');
    }
    out
}
