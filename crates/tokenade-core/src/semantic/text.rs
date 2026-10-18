//! **From text to tokens** — the only half of BM25 that judges words.
//!
//! Segmentation cuts on non-words AND on snake_case / camelCase boundaries,
//! then lowercases: `parseConfig`, `parse_config` and `parse config` yield the
//! same tokens. Without it, an agent writing the query in one convention and
//! the code in another finds nothing, and nothing says so.
//!
//! Stem variants are deliberately poor — no Porter, no dependency. A wrong
//! stem costs more than a missing one: it brings together two words that
//! have nothing to do with each other, and the ranking lies.

use super::*;

pub(crate) fn path_components(file: &str) -> Vec<String> {
    file.split('/')
        .flat_map(|c| {
            // Strip extension on the last component.
            let stem = c.rsplit_once('.').map(|(a, _)| a).unwrap_or(c);
            split_camel_snake(stem)
        })
        .filter(|s| !s.is_empty() && !is_skip_dir_name(s))
        .collect()
}

/// Tokenize for BM25: split on non-word, then split snake_case + camelCase,
/// lowercase, drop stopwords and 1-char tokens (except a few useful ones).
pub(crate) fn tokenize(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    for chunk in word_re().split(s) {
        if chunk.is_empty() {
            continue;
        }
        // Han, kana and Thai are `XID_Continue`, so `word_re` leaves a mixed
        // fragment like `修复run函数的报错` whole and `split_camel_snake` — which
        // only knows ASCII case — cannot cut it either: the symbol name would
        // never reach the index or the query. Cut at the script boundary, and
        // emit the word-less halves as bigrams (they have no spaces to split
        // on, so left whole they are one token nothing matches).
        for (wordless, run) in crate::tokenizer::split_script_runs(chunk) {
            if wordless {
                crate::tokenizer::push_wordless_bigrams(&run, &mut out);
                continue;
            }
            let subs = split_camel_snake(&run);
            for sub in &subs {
                let lower = sub.to_lowercase();
                if lower.len() < 2 || STOPWORDS.contains(&lower.as_str()) {
                    continue;
                }
                // Also emit stem variants (e.g. `caching` → `cache`,
                // `eviction` → `evict`). The bare suffix-strip recall
                // wins ~6 % on real-world queries per codegraph's
                // benchmark; we get it for free here because the inverted
                // index dedupes on insertion. Costs ~3 cycles per token.
                for stem in stem_variants(&lower) {
                    out.push(stem);
                }
                out.push(lower);
            }
            // The identifier ITSELF, separators removed, so that
            // `drop_error_lines` and `dropErrorLines` both become
            // `droperrorlines`. Without it a compound name only ever
            // reaches the index as a bag of common words, and a file that
            // merely mentions "error" and "lines" competes on equal terms
            // with the one file that declares the symbol. This term's
            // document frequency is typically 1, so BM25's own IDF does
            // the ranking — no boost, no extra leg. Index and query both
            // tokenise here, so the two sides stay symmetric.
            //
            // No ASCII restriction: `split_script_runs` has already cut the
            // CJK away, so a run reaching here carries none and there is
            // nothing to glue back. An `is_ascii()` gate was tried and
            // removed — measured, the only thing it suppressed was
            // `café_naïve`, an accented identifier that should be findable
            // by the name its author wrote.
            if subs.len() >= 2 {
                let joined: String = subs.iter().map(|s| s.to_lowercase()).collect();
                if joined.len() >= 4 {
                    out.push(joined);
                }
            }
        }
    }
    out
}

/// Conservative suffix-strip stemmer. We only return stems that are
/// **morphologically meaningful** — i.e., the kind of variation a
/// programmer would write interchangeably in code vs prose
/// (`authenticate`/`auth`, `caching`/`cache`). We do NOT do a full
/// Porter stemmer because that would over-strip (`compute` → `comput`,
/// breaking matches against the literal `compute` in code).
///
/// Returns 0..=2 stems per input. Empty vec means "no useful variant"
/// (caller should still index the original).
///
/// Suffixes covered:
/// - `-ing`  → drop (`caching` → `cach`, dropped because <3 chars → no);
///   actually we keep ≥3-char result (`caching` → `cach`).
/// - `-tion`/`-sion` → drop (`eviction` → `evic`)
/// - `-ment` → drop (`management` → `manage`)
/// - `-ies`  → `y` (`entries` → `entry`)
/// - `-es`/`-s` → drop trailing plural marker (`handles` → `handle`)
/// - `-ed`   → drop (`handled` → `handle`)
/// - `-er`   → drop (`builder` → `build`)
///
/// Stems shorter than 3 chars are discarded (false-positive risk).
pub(crate) fn stem_variants(token: &str) -> Vec<String> {
    let mut out = Vec::new();
    let suffixes: &[(&str, &str)] = &[
        ("ing", ""),
        ("tion", ""),
        ("sion", ""),
        ("ment", ""),
        ("ies", "y"),
        ("ed", ""),
        ("er", ""),
        ("es", ""),
        ("s", ""),
    ];
    for (suf, repl) in suffixes {
        if token.len() >= suf.len() + 3 && token.ends_with(suf) {
            let mut stem = token[..token.len() - suf.len()].to_string();
            stem.push_str(repl);
            if stem.len() >= 3 && stem != token {
                out.push(stem);
                // First match wins — avoid emitting both `cache` and
                // `cach` for `caching`.
                break;
            }
        }
    }
    out
}

pub(crate) fn split_camel_snake(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    for piece in s.split(['_', '-', '.']) {
        if piece.is_empty() {
            continue;
        }
        // Split camelCase: insert boundary before each uppercase that's
        // preceded by a lowercase, or before the last upper of a run that's
        // followed by a lower (so HTTPRequest → HTTP, Request).
        let chars: Vec<char> = piece.chars().collect();
        let mut start = 0;
        for i in 1..chars.len() {
            let prev = chars[i - 1];
            let cur = chars[i];
            let boundary = (prev.is_ascii_lowercase() && cur.is_ascii_uppercase())
                || (i + 1 < chars.len()
                    && prev.is_ascii_uppercase()
                    && cur.is_ascii_uppercase()
                    && chars[i + 1].is_ascii_lowercase());
            if boundary {
                parts.push(chars[start..i].iter().collect());
                start = i;
            }
        }
        parts.push(chars[start..].iter().collect());
    }
    parts
        .into_iter()
        .filter(|p: &String| !p.is_empty())
        .collect()
}

crate::regexes::lazy_regex!(fn word_re() = r"[^\p{XID_Continue}]+");

/// Stopwords stripped at both index AND query time. Strictly **natural-
/// language** words — words that occur frequently in comments / docstrings
/// but never carry signal.
///
/// We deliberately keep this list **tiny**. Anything that doubles as a
/// code identifier in a popular language goes back in the index. Concrete
/// examples we WON'T strip:
///   `type` `class` `def` `fn` `pub` `mut` `const` `var` `let` `return`
///   `import` `null` `none` `nil` `false` `true` `self` `use` `uses` `used`
///   `into` `from` `for` `with` `not` `this` `that`            (Rust traits, JS keywords)
///
/// Bench regression noted 2026-05-28: prior stopword list included `into`,
/// `from`, `for`, etc. — fragments of `into_iter`, `from_str`, etc. — which
/// made `tokenade ask` silently return zero hits whenever the user asked about
/// these very common identifiers. The bench's `nav:semantic-search` task
/// caught 18 such cases by picking top-frequency name fragments as queries.
pub(crate) const STOPWORDS: &[&str] = &[
    "the", "and", "but", "was", "are", "you", "your", "any", "all", "can",
];

/// Read up to 256 KiB of `path` and extract just the comments + docstrings.
/// We strip code so symbol identifiers don't double-count (they're already
/// emitted from the symbol list).
/// Comments harvested from a file, REDACTED.
///
/// The comments are what make meaning-search work — a symbol name says what a
/// thing is called and the comment above it says what it is for — so they are
/// seeded into every document. They are also where a credential gets written
/// down, and unlike the source file the document is copied into
/// `~/.tokenade/semantic.db`, a GLOBAL store outside the repository.
///
/// Measured 2026-08-27: a marker in a doc comment reaches that store and no
/// other cache. Scrubbing it afterwards is a treadmill — dropping the affected
/// index and re-indexing puts the secret straight back — so the redaction
/// belongs here, on the way IN. The scrubbers stay for blobs written before
/// this, and `scrub-secret` for the emergency.
pub(crate) fn read_comments(path: &PathBuf) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    if content.len() > 256 * 1024 {
        return None;
    }
    let mut out = String::new();
    let mut in_block = false;
    let mut in_triple = false;
    for line in content.lines() {
        let trim = line.trim_start();
        if in_block {
            out.push_str(line);
            out.push('\n');
            if line.contains("*/") {
                in_block = false;
            }
            continue;
        }
        if in_triple {
            out.push_str(line);
            out.push('\n');
            if line.contains("\"\"\"") || line.contains("'''") {
                in_triple = false;
            }
            continue;
        }
        if let Some(stripped) = trim.strip_prefix("///") {
            out.push_str(stripped);
            out.push('\n');
        } else if let Some(stripped) = trim.strip_prefix("//!") {
            out.push_str(stripped);
            out.push('\n');
        } else if let Some(stripped) = trim.strip_prefix("//") {
            out.push_str(stripped);
            out.push('\n');
        } else if let Some(stripped) = trim.strip_prefix("# ") {
            out.push_str(stripped);
            out.push('\n');
        } else if let Some(rest) = trim.strip_prefix("/*") {
            out.push_str(rest);
            out.push('\n');
            if !rest.contains("*/") {
                in_block = true;
            }
        } else if trim.starts_with("\"\"\"") || trim.starts_with("'''") {
            let rest = &trim[3..];
            out.push_str(rest);
            out.push('\n');
            if !rest.contains("\"\"\"") && !rest.contains("'''") {
                in_triple = true;
            }
        }
    }
    Some(crate::redact::redact(&out))
}
