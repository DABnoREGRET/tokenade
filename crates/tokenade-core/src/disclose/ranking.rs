//! **Ranking what was stashed, when the agent comes back looking.**
//!
//! BM25: it does not know which line its answer was on, only which words
//! it is looking for. A stream of RECORDS is handled separately — returning an
//! isolated line from it gives a fragment that means nothing.
//!
//! Distinct from `disclose/search.rs`, which answers the command; here we only
//! score relevance.

use super::*;

// ---------- BM25 sub-search over stashed content (headroom CCR pattern) ----------------

pub(crate) const BM25_K1: f64 = 1.5;
pub(crate) const BM25_B: f64 = 0.75;

/// Average document length in tokens — BM25's length-normalisation baseline.
///
/// `.max(1.0)` twice: an empty corpus must not divide by zero, and a corpus of
/// empty documents must not normalise against zero either.
pub(crate) fn bm25_avg_len(docs: &[Vec<String>]) -> f64 {
    let total: usize = docs.iter().map(|t| t.len()).sum();
    (total as f64 / (docs.len() as f64).max(1.0)).max(1.0)
}

/// REAL inverse document frequency per query term (df = #documents containing
/// it).
///
/// This is the fix for the "generic prompt word" failure: constant IDF let
/// ubiquitous words ("default"/"value"/"directive", one per section) outscore
/// the single discriminating term ("keepalive_timeout"), filling the excerpt
/// with the WRONG sections. With df-weighted IDF a term in every document
/// contributes ≈0 and the rare, answer-bearing term dominates.
///
/// One "document" is a LINE for the flat extractor and a SECTION for the RAG
/// one — the formula does not care, which is why both had written it out.
pub(crate) fn bm25_idf<'a>(
    terms: &'a [String],
    docs: &[Vec<String>],
) -> std::collections::HashMap<&'a str, f64> {
    let n = docs.len() as f64;
    terms
        .iter()
        .map(|term| {
            let df = docs
                .iter()
                .filter(|toks| toks.iter().any(|w| w == term))
                .count() as f64;
            let idf = if df > 0.0 {
                (1.0 + (n - df + 0.5) / (df + 0.5)).ln()
            } else {
                0.0
            };
            (term.as_str(), idf)
        })
        .collect()
}

/// BM25 score of ONE document against the query.
pub(crate) fn bm25_score_one(
    terms: &[String],
    toks: &[String],
    idf: &std::collections::HashMap<&str, f64>,
    avg_len: f64,
) -> f64 {
    let dl = toks.len() as f64;
    let mut score = 0.0;
    for term in terms {
        let tf = toks.iter().filter(|w| *w == term).count() as f64;
        if tf == 0.0 {
            continue;
        }
        let tf_norm =
            (tf * (BM25_K1 + 1.0)) / (tf + BM25_K1 * (1.0 - BM25_B + BM25_B * dl / avg_len));
        score += idf.get(term.as_str()).copied().unwrap_or(0.0) * tf_norm;
    }
    score
}

/// Tokenise text for the stash BM25. Two multi-language fixes over a naive
/// ASCII split:
///   * **Unicode** lowercase (not ASCII-only) so cased non-Latin scripts —
///     Cyrillic, Greek, accented Latin, Turkish — match regardless of case.
///   * Runs of word-less scripts (CJK / kana / Thai have no inter-word spaces
///     and `is_alphanumeric` keeps them glued into one giant token) are emitted
///     as overlapping **character bigrams**, à la Lucene's CJKBigramFilter, so a
///     stashed CJK blob is actually searchable instead of collapsing to a
///     single un-matchable token.
/// Latin/ASCII input is unaffected (no word-less chars → no bigrams), so
/// existing workloads tokenise exactly as before.
pub(crate) fn bm25_tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in text.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-') {
        if !raw.is_empty() {
            push_bm25_tokens(&raw.to_lowercase(), &mut out);
        }
    }
    out
}

/// Is this blob a stream of RECORDS rather than prose?
///
/// The distinction decides which recovery to recommend. Over prose, a question
/// beats a pattern: passages are topical, so neighbours of a hit are usually
/// also relevant. Over a record stream the opposite holds — adjacent lines of
/// `grep -r` output belong to unrelated files, so a retrieved "passage" is a
/// coincidence of ordering, and the exact-line path is both cheaper and right.
///
/// Signals, both required: no markdown headings, and most lines opening with a
/// key-like run followed by `:` or `=` (`path/to/file.rs:content`, `key=value`,
/// `2026-08-02 INFO msg` once the timestamp is skipped). Sampled, so this stays
/// cheap on a multi-megabyte stash.
pub fn is_record_stream(text: &str) -> bool {
    const SAMPLE: usize = 400;
    let mut lines = 0usize;
    let mut recordish = 0usize;
    for line in text.lines().filter(|l| !l.trim().is_empty()).take(SAMPLE) {
        if line.trim_start().starts_with('#') {
            return false; // a heading: treat the whole blob as prose
        }
        lines += 1;
        let head_len = line
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != ':' && *c != '=')
            .count();
        let sep = line.chars().nth(head_len);
        if head_len > 0 && matches!(sep, Some(':') | Some('=')) {
            recordish += 1;
        }
    }
    lines >= 8 && recordish * 5 >= lines * 3
}

/// Where "this stash entry is the ONLY copy" markers live.
///
/// Deliberately OUTSIDE the stash dir: `read_stash_raw` resolves a hash by
/// scanning that directory and taking the first file whose stem matches, so a
/// sibling marker could be served as content.
pub(super) fn keep_marker_dir() -> PathBuf {
    stash_dir().with_file_name("stash-keep")
}

pub(super) fn keep_marker_path(hash: &str) -> PathBuf {
    keep_marker_dir().join(hash)
}

/// Stash bytes that exist NOWHERE ELSE, and mark them exempt from the TTL sweep.
///
/// The ordinary fold is TTL-evictable because its source survives: the command
/// can be re-run, the file is still on disk. A block masked out of a transcript
/// has neither — the bytes were REPLACED by the pointer, so the stash entry is
/// the last copy. Letting the 7-day sweep take it leaves the transcript holding
/// a pointer to nothing, forever, and `--resume` on a two-week-old session is
/// exactly when that is discovered.
///
/// Still bounded: the size cap applies as before, so this cannot grow without
/// limit — it only means "last to go", not "never".
pub fn stash_sole_copy(text: &str, hint: &SourceHint) -> Option<(String, PathBuf)> {
    let (hash, path) = stash_for_reference(text, hint)?;
    let dir = keep_marker_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = crate::platform::write_private(&keep_marker_path(&hash), b"");
    Some((hash, path))
}

/// Is this stash entry the only copy of its bytes?
pub(super) fn is_sole_copy(stem: &str) -> bool {
    keep_marker_path(stem).exists()
}

/// Stash `text` unconditionally and return `(hash, path)` so a caller
/// can build a one-line recovery pointer without inlining a full folded
/// summary. Used by the WebFetch scoped-extract path: it serves a small
/// prompt-scoped passage inline and only needs to point at the full
/// distilled page (`expand-ref <hash>` / `search-stash` / `Read
/// <path>`). Redacts before writing, mirroring [`maybe_stash`]. Returns
/// `None` on any I/O error (caller then omits the recovery line — the
/// served extract is still self-contained).
pub fn stash_for_reference(text: &str, hint: &SourceHint) -> Option<(String, PathBuf)> {
    let redacted = crate::redact::redact(text);
    let ext = compose_extension(&redacted, hint);
    let hash = crate::dedup_ref::hash_content(redacted.as_bytes());
    // Same rule as `maybe_stash_with_threshold`: a stash that cannot be
    // written costs the caller its recovery pointer, so the reason has to
    // land somewhere a human can read it later.
    let path = match ensure_stashed(&redacted, &hash, &ext) {
        Ok(p) => p,
        Err(e) => {
            crate::debug_log::log(
                "disclose",
                "stash_write_failed",
                serde_json::json!({
                    "hash": hash, "bytes": redacted.len(), "ext": ext,
                    "error": e.to_string(), "site": "stash_for_reference",
                }),
            );
            return None;
        }
    };
    Some((hash, path))
}
