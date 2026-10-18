//! **Which LINES answer the question?**
//!
//! BM25 over lines, and everything needed for « line » to mean
//! something: a list entry is not a sentence, a heading is not
//! content, and a function word is an anchor for nobody.
//!
//! Stopwords are declared rather than guessed by frequency: on a
//! single-page blob, frequency would make the document's subject pass for
//! noise — it is the word that comes back most.

use super::*;

/// Default token budget for a WebFetch prompt-scoped extract. Kept
/// generous (multiple passages fit) so the answer's neighbourhood
/// survives, yet an order of magnitude below a full 10-18k distilled
/// page. Overridable via `TOKENADE_WEBFETCH_SCOPED_TOKENS`.
pub const WEBFETCH_SCOPED_BUDGET_TOKENS: usize = 1500;

/// Lines of context kept on each side of a scored hit line. Wide enough
/// to pull a directive's `Default:` line, an adjacent table row, or the
/// number that answers the prompt when it sits one line away from the
/// keyword.
pub const SCOPED_CONTEXT_LINES: usize = 4;

/// Minimum real-IDF a prompt term must reach for the prompt to count as
/// having a discriminating lexical anchor. `LN_2` corresponds to a term
/// present on roughly half the lines — anything rarer is a genuine
/// anchor; a prompt whose only surviving terms sit on ~every line
/// (vague) yields no anchor and the caller falls back to serving the
/// page whole. Keeps the "vague prompt" adversarial case honest.
pub const SCOPED_MIN_ANCHOR_IDF: f64 = std::f64::consts::LN_2;

/// Closed-class function words — the ones every English sentence carries.
///
/// [`SCOPE_STOPWORDS`] was built from META-REQUEST phrasing ("summarise this
/// page") and so never listed these. They are NOT stripped from scoring: a
/// query word that happens to sit in the answering line still helps rank it.
/// They are excluded from one decision only — whether the question has a
/// lexical anchor at all.
///
/// Rarity cannot make that call on its own. A document's blank lines,
/// headings and code blocks contain no `of`, so `of` clears an IDF threshold
/// comfortably at line granularity, and a question the source cannot answer
/// ("airspeed velocity of an unladen swallow", not one content word of which
/// appears) came back as a confident excerpt ranked on `of` and `an` alone —
/// shaped exactly like a real answer.
pub const FUNCTION_WORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "been", "being", "but", "by", "can", "could", "did",
    "do", "for", "from", "had", "has", "have", "here", "if", "in", "into", "is", "it", "its",
    "may", "might", "must", "not", "of", "off", "on", "onto", "or", "our", "out", "over", "shall",
    "should", "since", "so", "some", "such", "than", "then", "there", "these", "they", "those",
    "to", "too", "under", "until", "up", "was", "were", "when", "where", "while", "who", "whom",
    "why", "will", "with", "would", "you", "your",
];

/// True for a closed-class word that must never be a question's only anchor.
pub fn is_function_word(term: &str) -> bool {
    FUNCTION_WORDS.contains(&term)
}

/// Minimum non-empty lines before the enumeration-density gate may
/// trigger. Below this a handful of bullets flips the fraction on what
/// is really a tiny page the window path already serves whole.
pub const SCOPED_ENUM_MIN_LINES: usize = 8;

/// Fraction of non-empty lines that must classify as enumeration items
/// ([`scoped_line_is_item`]) before packing switches to one-liner item
/// mode. 0.40 — middle of the plausible 0.35-0.45 band: curated link
/// lists, directory READMEs, docs TOCs, API indexes and changelogs
/// measure far above it (typically 0.6-0.9 of non-empty lines are
/// entries), while prose documentation with occasional bullet asides
/// stays under ~0.25. Sitting in the gap keeps mostly-prose pages on
/// the unchanged window path while still catching enumeration pages
/// padded with intro prose.
pub const SCOPED_ENUM_DENSE_FRACTION: f64 = 0.40;

/// Character cap for one packed enumeration item. List/link/table
/// entries are self-contained one-liners; 200 chars keeps a link target
/// plus its description while bounding a pathological single-line blob
/// to roughly half the cost of a prose context window.
pub const SCOPED_ITEM_MAX_CHARS: usize = 200;

/// Meta-request words stripped from a WebFetch prompt before scoping —
/// they carry no page-specific signal ("summarise this page", "what is
/// the main content"), so scoring on them would let a vague prompt
/// anchor on an incidental "page"/"content" hit. Stripping them makes a
/// truly vague prompt tokenise to nothing → `None` → whole-page fallback.
pub const SCOPE_STOPWORDS: &[&str] = &[
    "summarise",
    "summarize",
    "summary",
    "explain",
    "describe",
    "tell",
    "about",
    "page",
    "this",
    "that",
    "the",
    "content",
    "contents",
    "main",
    "tldr",
    "overview",
    "give",
    "provide",
    "what",
    "which",
    "does",
    "how",
    "value",
    "default",
    "please",
    "find",
    "show",
    "list",
    "detail",
    "details",
];

/// Structural test: does this line look like a list / table / link
/// ENTRY — a self-contained one-liner — rather than prose? PURELY
/// structural (bullet, ordered marker, table row, link-dominated); no
/// domain keywords and no URL-host weighting, so it generalises to API
/// indexes, changelogs, docs TOCs, directories and package lists alike.
pub fn scoped_line_is_item(line: &str) -> bool {
    let t = line.trim_start();
    if t.is_empty() {
        return false;
    }
    // Bullet marker + space — the space keeps `---` rules, `*emphasis*`
    // prose and unified-diff `-`/`+` lines out.
    if ["- ", "* ", "+ ", "• "].iter().any(|m| t.starts_with(m)) {
        return true;
    }
    // Ordered entry: `12.` / `12)` + space. ≤4 digits: list markers are
    // short; a long number starting a prose sentence isn't a marker.
    let digits = t.bytes().take_while(|b| b.is_ascii_digit()).count();
    if (1..=4).contains(&digits) && (t[digits..].starts_with(". ") || t[digits..].starts_with(") "))
    {
        return true;
    }
    // Markdown table row: `| cell | cell |`.
    if t.starts_with('|') && t[1..].contains('|') {
        return true;
    }
    // Link-dominated: `[text](url)` spans cover at least half the
    // content — catches link lists whose distiller dropped the bullets.
    let total = t.chars().count();
    let mut link_chars = 0usize;
    let mut rest = t;
    while let Some(open) = rest.find('[') {
        let after = &rest[open..];
        let (mid, close) = match after.find("](").map(|m| (m, after[m + 2..].find(')'))) {
            Some((m, Some(c))) => (m, c),
            _ => break,
        };
        let span = mid + 2 + close + 1; // byte length; ends after ASCII ')'
        link_chars += after[..span].chars().count();
        rest = &after[span..];
    }
    link_chars > 0 && link_chars * 2 >= total
}

/// `#`-heading test — used to prefix a packed item group with its
/// nearest section title for orientation.
pub fn scoped_line_is_heading(line: &str) -> bool {
    let t = line.trim_start();
    let hashes = t.bytes().take_while(|&b| b == b'#').count();
    (1..=6).contains(&hashes) && t.as_bytes().get(hashes) == Some(&b' ')
}

/// Body of [`prompt_scoped_extract`]. `enum_aware=false` forces the
/// legacy pure-window packing on every page — kept only so tests can
/// assert the enumeration path actually beats it (the delta, not just
/// the new absolute).
/// Query terms as the scorer sees them: deduplicated, and with the meta-request
/// stopwords (`show`, `what`, `tell`…) dropped so a question's grammar cannot
/// out-vote its subject.
/// The terms of a query, deduplicated and stripped of stopwords.
///
/// `pub(crate)` because `rag.rs` carried a byte-for-byte copy of it under
/// the name `query_terms`. It lives HERE and not elsewhere because this module
/// owns `SCOPE_STOPWORDS`: the function and its vocabulary stay in the
/// same place, so enriching the list can no longer serve only one caller in two.
pub fn scoped_query_terms(prompt: &str) -> Vec<String> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    bm25_tokenize(prompt)
        .into_iter()
        .filter(|t| !SCOPE_STOPWORDS.contains(&t.as_str()))
        .filter(|t| seen.insert(t.clone()))
        .collect()
}

/// BM25 score per line, keeping only the lines that matched something.
/// Length normalisation is against the document's own average line, so a page
/// of long prose lines and a page of one-word list entries are scored on the
/// same scale.
pub fn scoped_line_scores(
    query_terms: &[String],
    line_toks: &[Vec<String>],
    idf: &std::collections::HashMap<&str, f64>,
) -> Vec<(usize, f64)> {
    let avg_line_len = bm25_avg_len(line_toks);
    line_toks
        .iter()
        .enumerate()
        .map(|(i, toks)| (i, bm25_score_one(query_terms, toks, idf, avg_line_len)))
        .filter(|(_, s)| *s > 0.0)
        .collect()
}

/// For each line, the line that OWNS it: itself if it is an item head, the head
/// above if it is that item's wrapped continuation (indented, non-heading,
/// non-empty), `None` otherwise. One unit for scoring and packing, so a wrapped
/// entry competes exactly once.
pub fn scoped_item_heads(lines: &[&str]) -> Vec<Option<usize>> {
    let mut item_head: Vec<Option<usize>> = vec![None; lines.len()];
    for i in 0..lines.len() {
        if scoped_line_is_item(lines[i]) {
            item_head[i] = Some(i);
        } else if i > 0
            && item_head[i - 1].is_some()
            && !lines[i].trim().is_empty()
            && !scoped_line_is_heading(lines[i])
            && lines[i].starts_with(char::is_whitespace)
        {
            item_head[i] = item_head[i - 1];
        }
    }
    item_head
}

/// Nearest preceding heading per line — one orientation line per packed group.
pub fn scoped_last_headings(lines: &[&str]) -> Vec<Option<usize>> {
    let mut v = Vec::with_capacity(lines.len());
    let mut cur: Option<usize> = None;
    for line in lines {
        if scoped_line_is_heading(line) {
            cur = Some(v.len());
        }
        v.push(cur);
    }
    v
}
