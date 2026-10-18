//! Searching a stashed blob without re-reading it whole.
//!
//! BM25 over lines, so `search_stash` answers with the passages that match
//! rather than the file — the recovery path an agent actually takes.

use super::*;

/// How one `--grep` pattern is matched against a line: as a REGEX, falling
/// back to a substring test when it does not compile.
///
/// The flag is called `--grep` and every caller reads it as one.
/// MEASURED 2026-09-05 across this machine's transcripts, **38 of 128
/// `--grep` patterns (30%) were regex-shaped** — `OSCILLE|ligne\(s\)|suspect`,
/// `ok |FAIL|error|warning`, `\[8\]` — and every one matched nothing, on the
/// RECOVERY command, where "0 matching line(s)" reads as "the content is not
/// there" and the caller stops looking.
///
/// The substring test is kept as a UNION, not a fallback: a line matches when
/// the regex matches it **or** it contains the pattern verbatim. That makes
/// the change a strict superset of the old behaviour — nobody who was finding
/// lines stops finding them. It matters for the literal-minded caller:
/// `ligne(s)` is a perfectly valid regex meaning "lignes", so a pure-regex
/// reading would have taken a match away from someone who meant the
/// parentheses. Both paths into a stash — the in-memory blob and the stash
/// file — go through this one type, because they used to carry two copies of
/// the comparison and only one would have been fixed.
pub struct GrepMatcher {
    re: Option<regex::Regex>,
    pat: String,
    ignore_case: bool,
}

impl GrepMatcher {
    pub fn new(pat: &str, ignore_case: bool) -> Self {
        let spelled = if ignore_case {
            format!("(?i){pat}")
        } else {
            pat.to_string()
        };
        GrepMatcher {
            re: regex::Regex::new(&spelled).ok(),
            pat: pat.to_string(),
            ignore_case,
        }
    }

    pub fn is_match(&self, line: &str) -> bool {
        self.re.as_ref().is_some_and(|re| re.is_match(line)) || self.contains(line)
    }

    fn contains(&self, line: &str) -> bool {
        if self.ignore_case {
            line.to_lowercase().contains(&self.pat.to_lowercase())
        } else {
            line.contains(&self.pat)
        }
    }

    /// Where the match starts, in bytes, for clipping a long line around it.
    pub fn find(&self, line: &str) -> Option<usize> {
        let literal = if self.ignore_case {
            line.to_lowercase().find(&self.pat.to_lowercase())
        } else {
            line.find(&self.pat)
        };
        self.re
            .as_ref()
            .and_then(|re| re.find(line).map(|m| m.start()))
            .or(literal)
    }
}

/// Clip one matching line to [`GREP_LINE_MAX_CHARS`] around the first hit, so a
/// single-line file cannot dump itself. Marks both elided sides.
pub fn clip_match_line(line: &str, pat: &str) -> String {
    clip_match_line_at(line, &GrepMatcher::new(pat, false))
}

/// The same clip, centred on where THIS matcher hits — a regex match has no
/// literal position to look for, so the pattern cannot locate itself.
pub fn clip_match_line_at(line: &str, m: &GrepMatcher) -> String {
    if line.chars().count() <= GREP_LINE_MAX_CHARS {
        return line.to_string();
    }
    let chars: Vec<char> = line.chars().collect();
    // Centre the window on the match (char index, not byte).
    let at = m.find(line).map(|b| line[..b].chars().count()).unwrap_or(0);
    let half = GREP_LINE_MAX_CHARS / 2;
    let start = at.saturating_sub(half);
    let end = (start + GREP_LINE_MAX_CHARS).min(chars.len());
    let start = end.saturating_sub(GREP_LINE_MAX_CHARS);
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(&chars[start..end]);
    if end < chars.len() {
        out.push('…');
    }
    out
}

/// Grep a stash. Returns `(hits, total_matches, total_lines, blob_tokens)` —
/// `hits.len() < total_matches` means the cap bit, and the caller MUST say so:
/// a truncated result that reads as exhaustive is how an agent concludes
/// "only 3 occurrences" from a file with 900. Hits are clipped per line and
/// capped in count — see [`GREP_LINE_MAX_CHARS`] / [`GREP_MAX_HITS`].
/// `ignore_case` folds both sides before comparing. A literal grep that finds
/// nothing looks exactly like content that does not hold the term, and on a
/// recovery path that is read as an answer — see `expand-ref`'s miss note.
pub fn grep_stash_by_hash(
    hash: &str,
    pat: &str,
    ignore_case: bool,
) -> Option<(Vec<(usize, String)>, usize, usize, usize)> {
    let (raw, _ext) = read_stash_raw(hash)?;
    let m = GrepMatcher::new(pat, ignore_case);
    let mut hits = Vec::new();
    let mut total = 0usize;
    let mut matched = 0usize;
    for (i, line) in raw.lines().enumerate() {
        total += 1;
        if m.is_match(line) {
            matched += 1;
            if hits.len() < GREP_MAX_HITS {
                hits.push((i + 1, clip_match_line_at(&crate::redact::redact(line), &m)));
            }
        }
    }
    Some((hits, matched, total, crate::tokenizer::estimate(&raw)))
}

// Emit BM25 tokens from one already-lowercased fragment: contiguous word-less
// (CJK-like) characters become overlapping bigrams (a lone such char stays a
// unigram); other stretches (Latin, Cyrillic, …) stay whole. Tokens under

/// Emit BM25 tokens from one already-lowercased fragment: contiguous word-less
/// (CJK-like) characters become overlapping bigrams (a lone such char stays a
/// unigram); other stretches (Latin, Cyrillic, …) stay whole. Tokens under
/// 2 bytes are dropped, matching the historical behaviour.
pub(super) fn push_bm25_tokens(frag: &str, out: &mut Vec<String>) {
    for (wordless, run) in crate::tokenizer::split_script_runs(frag) {
        if wordless {
            crate::tokenizer::push_wordless_bigrams(&run, out);
        } else if run.len() >= 2 {
            out.push(run);
        }
    }
}

// REMOVED 2026-08-08: `search_stash` / `search_stash_paged`.
//
// The `tokenade search-stash` COMMAND was retired long ago — `expand-ref
// --prompt` subsumes it, and `tests/misc.rs` even guards that the name no
// longer resolves. These two functions outlived it with no production caller,
// and the paging banner they printed told the agent to
//
//     re-run `tokenade search-stash <hash> "<query>" --offset N`
//
// which the binary answers with `unknown command`. A dead code path is cheap;
// a dead code path that hands the agent a command that does not exist costs a
// turn every time it is reached. The live way into a stash is
// `tokenade expand-ref <hash> [--grep <pat> | --prompt "q" | --raw]`.
//
// Kept: `clip_match_line`, `grep_stash_by_hash`, `push_bm25_tokens`,
// `bm25_score_line`, `truncate_to_tokens` — all have live callers.

/// Cut `text` so its token estimate fits `budget`, on a char boundary, marking
/// the cut. Returns the input untouched when it already fits.
///
/// Exists for ONE case, and it was a real overshoot: the packing loop below
/// admits its FIRST pick regardless of size, so that a page always gets an
/// answer instead of an empty extract. Nothing bounded that pick. On a page whose
/// units are few and very long — a `.txt`, an `llms.txt`, a doc with
/// paragraph-long lines — one unit was 11x the budget (MEASURED 2026-08-01:
/// `tokenade.net/llms.txt`, budget 500 tokens, extract 5 667). The intent was
/// "never return nothing"; the implementation put no ceiling on the something.
pub(super) fn truncate_to_tokens(text: &str, budget: usize) -> String {
    if budget == 0 || crate::tokenizer::estimate(text) <= budget {
        return text.to_string();
    }
    // Start from the prose ratio and shrink until the estimate fits. Bounded
    // iterations: each step drops a tenth, so it converges well before 64.
    let mut keep_chars = (budget * 4).max(1);
    for _ in 0..64 {
        let cut: String = text.chars().take(keep_chars).collect();
        // Measure the string that will actually be RETURNED, ellipsis included —
        // measuring the cut and appending the marker afterwards overshoots by a
        // token, which is exactly the kind of off-by-one a budget cannot afford
        // to have at every call site.
        let marked = format!("{}…", cut.trim_end());
        if crate::tokenizer::estimate(&marked) <= budget || keep_chars <= 16 {
            return marked;
        }
        keep_chars = keep_chars * 9 / 10;
    }
    text.chars().take(16).collect()
}
