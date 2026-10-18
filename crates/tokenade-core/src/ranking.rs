//! Code-aware ranking — RRF score fusion, query-type detection,
//! definition-keyword + multi-chunk-file boosts, test-path penalty.
//!
//! RRF score fusion over two ranked lists. Stays
//! framework-agnostic: takes scored `(key, score)` pairs and returns
//! re-scored pairs. Wire it into `file_search::search` or any other
//! retriever to get hybrid + boosting on top.
//!
//! ## RRF (Reciprocal Rank Fusion)
//!
//! Combines two ranked lists (dense embedding + BM25, typically) by
//! converting each to RRF scores `1 / (k + rank)` (k=60,
//! the value from the original RRF paper). RRF is magnitude-agnostic:
//! cosine similarity in [0, 1] mixes correctly with raw BM25 scores
//! in [0, ∞).
//!
//! ## Auto-alpha
//!
//! Symbol-shape queries (`Connection`, `foo::Bar`, `_internal_helper`)
//! lean BM25 (alpha=0.3); natural-language queries balance (alpha=0.5).
//! Detector is a single regex over namespace / CamelCase / underscore
//! shapes — no parsing, no ML.
//!
//! ## Boosting
//!
//! - **Multi-chunk-file boost** — a file with N chunks in the result
//!   set gets its top chunk boosted by `0.2 * max_score * Σscore /
//!   max_Σ`. Reflects that "many hits in one file" is a strong signal.
//! - **Definition-keyword boost** — for symbol queries, chunks
//!   containing `fn X` / `class X` / `struct X` / `CREATE TABLE X` get
//!   ×3 the boost unit. File-stem match (`user.rs` for `User`) adds
//!   another ×1.5.
//! - **Test-path penalty** — for symbol queries with `alpha < 1.0`
//!   (BM25 in play), test-file paths lose 30% of their score so the
//!   impl ranks above the test that exercises it.

use std::collections::HashMap;
use std::hash::Hash;
use std::path::Path;

use regex::Regex;

/// Reciprocal Rank Fusion `k` constant. 60 is the value from the
/// original 2009 paper (Cormack/Clarke/Buettcher); lower `k` rewards top-rank items disproportionately.
pub const RRF_K: f32 = 60.0;

/// Default alpha when the query LOOKS like a bare symbol or
/// namespace-qualified identifier — lean toward BM25 (exact keyword
/// matching).
pub const ALPHA_SYMBOL: f32 = 0.3;

/// Default alpha for natural-language queries — balanced semantic+BM25.
pub const ALPHA_NL: f32 = 0.5;

/// Multiplier applied to the base boost when a chunk's content
/// contains a definition keyword (`fn`, `class`, `struct`, etc.) for
/// the queried symbol.
pub const DEFINITION_BOOST_MULTIPLIER: f32 = 3.0;

/// Multi-chunk-file coherence boost, expressed as a fraction of the
/// session-max score. The top chunk in a file with N hits gets boosted
/// by `FILE_COHERENCE_BOOST_FRAC * max_score * Σscore / max_Σ`.
pub const FILE_COHERENCE_BOOST_FRAC: f32 = 0.2;

/// Multiplier when the queried symbol matches the file stem (e.g.
/// `User` query → `user.rs` file).
pub const FILE_STEM_MATCH_MULTIPLIER: f32 = 1.5;

/// Test-file path penalty fraction (when alpha < 1.0 to indicate that
/// BM25 is contributing). A test file's score is multiplied by
/// `1.0 - TEST_PATH_PENALTY_FRAC` so the production code outranks its
/// test for symbol queries.
pub const TEST_PATH_PENALTY_FRAC: f32 = 0.30;

// ── Query shape detection ────────────────────────────────────────────

/// Detect whether the query looks like a bare symbol or namespace-
/// qualified identifier (`Connection`, `Sinatra::Base`, `_helper`,
/// `auth->login`).
pub fn is_symbol_query(query: &str) -> bool {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        // Symbol-style query regex, in Rust regex syntax
        // (no look-around needed).
        Regex::new(
            r"(?x)^(?:
              # namespace-qualified: Foo::Bar / Foo\Bar / Foo->bar / Foo.bar
              [A-Za-z_][A-Za-z0-9_]*(?:(?:::|\\|->|\.)[A-Za-z_][A-Za-z0-9_]*)+
              # leading-underscore identifier
              |_[A-Za-z0-9_]*
              # contains uppercase or underscore mid-string
              |[A-Za-z][A-Za-z0-9]*[A-Z_][A-Za-z0-9_]*
              # starts with uppercase
              |[A-Z][A-Za-z0-9]*
            )$",
        )
        .expect("symbol regex compiles")
    });
    re.is_match(query.trim())
}

/// Resolve the dense-vs-BM25 mixing weight. `override_alpha = Some(x)`
/// forces `x`; `None` picks via [`is_symbol_query`].
pub fn resolve_alpha(query: &str, override_alpha: Option<f32>) -> f32 {
    if let Some(a) = override_alpha {
        return a.clamp(0.0, 1.0);
    }
    if is_symbol_query(query) {
        ALPHA_SYMBOL
    } else {
        ALPHA_NL
    }
}

// ── RRF fusion ───────────────────────────────────────────────────────

/// Convert a `(key, raw_score)` map into RRF scores `1 / (k + rank)`.
/// Higher raw score → rank 1.
///
/// Ties break on the KEY, ascending. The input is a `HashMap`, whose
/// iteration order is randomized per process, so "ties keep their input
/// order" would be a promise the caller cannot keep — two entries with
/// an equal score would swap ranks between runs, and when the tie
/// straddles a later top-k cut it is *membership* in the top-k that
/// changes, not just the order within it. A search result that varies
/// at constant content is also a prompt-cache buster: the same query
/// re-run in the next turn rewrites bytes the model already had cached.
pub fn rrf_scores<K: Eq + Hash + Clone + Ord>(scores: &HashMap<K, f32>) -> HashMap<K, f32> {
    if scores.is_empty() {
        return HashMap::new();
    }
    let mut sorted: Vec<_> = scores.iter().collect();
    sorted.sort_by(|a, b| {
        b.1.partial_cmp(a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(b.0))
    });
    let mut out = HashMap::with_capacity(sorted.len());
    for (rank, (k, _)) in sorted.iter().enumerate() {
        out.insert((*k).clone(), 1.0 / (RRF_K + (rank + 1) as f32));
    }
    out
}

/// Alpha-weighted RRF combination of dense + BM25 scores. Output map
/// is the union of both key sets; missing keys get 0 on their side.
pub fn combine_rrf<K: Eq + Hash + Clone + Ord>(
    dense: &HashMap<K, f32>,
    bm25: &HashMap<K, f32>,
    alpha: f32,
) -> HashMap<K, f32> {
    let dense_rrf = rrf_scores(dense);
    let bm25_rrf = rrf_scores(bm25);
    let mut all = HashMap::with_capacity(dense_rrf.len() + bm25_rrf.len());
    for k in dense_rrf.keys().chain(bm25_rrf.keys()) {
        if all.contains_key(k) {
            continue;
        }
        let d = dense_rrf.get(k).copied().unwrap_or(0.0);
        let b = bm25_rrf.get(k).copied().unwrap_or(0.0);
        all.insert(k.clone(), alpha * d + (1.0 - alpha) * b);
    }
    all
}

// ── Boosting ─────────────────────────────────────────────────────────

/// A scored chunk that the boosting helpers operate on. Just a thin
/// trait so we don't drag the full `Chunk` type from `file_search.rs`
/// into this module.
pub trait Scorable {
    fn file_path(&self) -> &str;
    fn content(&self) -> &str;
}

/// In-place boost: a file with multiple high-scoring chunks gets its
/// top chunk promoted. See `FILE_COHERENCE_BOOST_FRAC`. Operates on a
/// `Vec<(item, score)>` because we mutate scores; we can't index a
/// HashMap by reference for in-place writes.
pub fn boost_multi_chunk_files<C: Scorable>(scored: &mut [(C, f32)]) {
    if scored.is_empty() {
        return;
    }
    let max_score = scored.iter().map(|(_, s)| *s).fold(0.0_f32, f32::max);
    if max_score == 0.0 {
        return;
    }
    // For each file, find the sum of all scores AND the index of the
    // highest-scoring chunk.
    let mut by_file: HashMap<String, (f32, usize, f32)> = HashMap::new();
    //                       ^^^^^^   sum, best_idx, best_score
    for (i, (c, s)) in scored.iter().enumerate() {
        let entry = by_file
            .entry(c.file_path().to_string())
            .or_insert((0.0, i, f32::NEG_INFINITY));
        entry.0 += *s;
        if *s > entry.2 {
            entry.1 = i;
            entry.2 = *s;
        }
    }
    let max_sum = by_file
        .values()
        .map(|(sum, _, _)| *sum)
        .fold(0.0_f32, f32::max);
    if max_sum == 0.0 {
        return;
    }
    let boost_unit = max_score * FILE_COHERENCE_BOOST_FRAC;
    for (_path, (sum, best_idx, _)) in by_file {
        scored[best_idx].1 += boost_unit * sum / max_sum;
    }
}

/// Extract the final identifier from a possibly namespace-qualified
/// query. Examples: `Sinatra::Base` → `Base`, `Client` → `Client`,
/// `auth->login` → `login`.
fn extract_symbol_name(query: &str) -> String {
    for sep in &["::", "\\", "->", "."] {
        if let Some(idx) = query.rfind(sep) {
            return query[idx + sep.len()..].to_string();
        }
    }
    query.trim().to_string()
}

/// Does *content* contain a definition of *symbol_name*?
/// Recognises `fn X` / `class X` / `struct X` / `def X` / `interface X`
/// / `type X` / `enum X` / `trait X` / `func X` / `function X` /
/// `object X` / `module X` / `defmodule X` / `protocol X` / `record X`
/// / `typedef X`. SQL DDL (`CREATE TABLE X`, etc.) is matched case-
/// insensitively.
///
/// Allocates 2 regex compilations per call. For loops over many chunks
/// against the same symbol, use [`SymbolDefRegexes::compile`] +
/// [`SymbolDefRegexes::matches`] to compile once and reuse.
pub fn chunk_defines_symbol(content: &str, symbol_name: &str) -> bool {
    SymbolDefRegexes::compile(symbol_name).matches(content)
}

/// Pre-compiled regexes for symbol-definition detection. Build once
/// per (caller, symbol) pair instead of per-chunk. #602 — hoists the
/// 2-regex compilation out of `boost_symbol_definitions`'s loop; a
/// 50-chunk scoring pass used to compile the same regex 100 times.
pub struct SymbolDefRegexes {
    general: Option<Regex>,
    sql: Option<Regex>,
}

impl SymbolDefRegexes {
    pub fn compile(symbol_name: &str) -> Self {
        let esc = regex::escape(symbol_name);
        let kw = "(?:class|module|defmodule|def|interface|struct|enum|trait|type\
                  |func|function|object|fn|fun|package|namespace|protocol|record|typedef)";
        let ns_prefix = r"(?:[A-Za-z_][A-Za-z0-9_]*(?:\.|::))*";
        let suffix = r"(?:\s|[<({:\[;]|$)";
        let general_pat = format!(r"(?m)(?:^|\s){kw}\s+{ns_prefix}{esc}{suffix}");
        let sql_kw = r"(?i:CREATE\s+(?:TABLE|VIEW|PROCEDURE|FUNCTION))";
        let sql_pat = format!(r"(?m)(?:^|\s){sql_kw}\s+{ns_prefix}{esc}{suffix}");
        Self {
            general: Regex::new(&general_pat).ok(),
            sql: Regex::new(&sql_pat).ok(),
        }
    }

    pub fn matches(&self, content: &str) -> bool {
        self.general.as_ref().is_some_and(|re| re.is_match(content))
            || self.sql.as_ref().is_some_and(|re| re.is_match(content))
    }
}

/// Apply definition-keyword + file-stem boosts in place. For each
/// chunk, if it defines the queried symbol, boost by
/// `DEFINITION_BOOST_MULTIPLIER * boost_unit`; if its filename stem
/// also matches the symbol, multiply by `FILE_STEM_MATCH_MULTIPLIER`.
pub fn boost_symbol_definitions<C: Scorable>(scored: &mut [(C, f32)], query: &str) {
    if scored.is_empty() {
        return;
    }
    let max_score = scored.iter().map(|(_, s)| *s).fold(0.0_f32, f32::max);
    if max_score == 0.0 {
        return;
    }
    let boost_unit = max_score * FILE_COHERENCE_BOOST_FRAC;
    let symbol = extract_symbol_name(query);
    if symbol.is_empty() {
        return;
    }
    let symbol_lc = symbol.to_lowercase();
    // #602 — compile the symbol-def regexes ONCE outside the loop.
    // The previous per-iteration `chunk_defines_symbol` call rebuilt
    // the same 2 regexes per chunk (N×2 compiles for N candidates).
    let symbol_regexes = SymbolDefRegexes::compile(&symbol);
    for (chunk, score) in scored.iter_mut() {
        if !symbol_regexes.matches(chunk.content()) {
            continue;
        }
        let stem = Path::new(chunk.file_path())
            .file_stem()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let stem_match = stem == symbol_lc
            || stem.replace('_', "") == symbol_lc
            || stem.trim_end_matches('s') == symbol_lc;
        let mult = if stem_match {
            FILE_STEM_MATCH_MULTIPLIER
        } else {
            1.0
        };
        *score += boost_unit * DEFINITION_BOOST_MULTIPLIER * mult;
    }
}

// ── Test-path penalty ────────────────────────────────────────────────

/// Detect test-file paths across the major languages we care about.
/// Test-file path regex across the major languages.
pub fn is_test_path(path: &str) -> bool {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(
            r"(?x)(?:^|/)(?:
              test_[^/]*\.py
              |[^/]*_test\.py
              |[^/]*_test\.go
              |[^/]*Tests?\.java
              |[^/]*Test\.php
              |[^/]*_spec\.rb
              |[^/]*_test\.rb
              |[^/]*\.test\.[jt]sx?
              |[^/]*\.spec\.[jt]sx?
              |[^/]*Tests?\.kt
              |[^/]*Spec\.kt
              |[^/]*Tests?\.swift
              |[^/]*Spec\.swift
              |[^/]*Tests?\.cs
              |test_[^/]*\.cpp
              |[^/]*_test\.cpp
              |test_[^/]*\.c
              |[^/]*_test\.c
              |[^/]*Spec\.scala
              |[^/]*Suite\.scala
              |[^/]*Test\.scala
              |[^/]*_test\.dart
              |test_[^/]*\.dart
              |[^/]*_spec\.lua
              |[^/]*_test\.lua
              |test_[^/]*\.lua
              # Rust: idiomatic per-module `tests` mod stays inline, but
              # `tests/*.rs` integration tests are demoted too.
              |[^/]*_test\.rs
              |tests?/[^/]*\.rs
            )$",
        )
        .expect("test path regex compiles")
    });
    re.is_match(path)
}

// ── File classification (single source of truth) ─────────────────────

/// Classify a path into a *file class* for exploration ranking.
/// **Lower = more likely the real source** a human is looking for:
///
/// | class | meaning                                            |
/// |-------|----------------------------------------------------|
/// | `0`   | source code (our supported langs + common ones)    |
/// | `3`   | docs / prose (`.md`, `/docs/`, LICENSE, README…)   |
/// | `4`   | test code (reuses [`is_test_path`] + dir markers)  |
/// | `5`   | unknown / other                                    |
/// | `6`   | data / generated (`.json`, lockfiles, `.min.js`…)  |
/// | `9`   | junk / vendored / build output (`node_modules`…)   |
///
/// Deterministic — extension + path heuristics only, no I/O. This is
/// the ONE classifier: both the ranking penalty below and semantic's
/// `test_path_weight` route through it, so the notion of "which files
/// outrank which" lives in exactly one place. Adapted from
/// FastContext's `classify_file`, extended to our language set.
pub fn classify_file(path: &str) -> u8 {
    let p = path.replace('\\', "/").to_ascii_lowercase();
    let last = p.rsplit('/').next().unwrap_or(p.as_str());
    // Leading-slash form so a root-relative dir (`docs/x`, `fixtures/y`)
    // matches the same `/marker/` contains-check as a nested one.
    let pslash = format!("/{}", p.trim_start_matches('/'));

    // Junk / vendored / build output — never the answer.
    const JUNK_DIRS: &[&str] = &[
        "/node_modules/",
        "/vendor/",
        "/.git/",
        "/dist/",
        "/build/",
        "/target/",
        "/.next/",
        "/out/",
        "/coverage/",
        "/__pycache__/",
        "/.venv/",
        "/venv/",
        "/site-packages/",
    ];
    if JUNK_DIRS.iter().any(|d| pslash.contains(d)) {
        return 9;
    }

    // Minified / sourcemaps / lockfiles → generated data.
    if last.ends_with(".min.js") || last.ends_with(".min.css") || last.ends_with(".map") {
        return 6;
    }
    const LOCKFILES: &[&str] = &[
        "cargo.lock",
        "package-lock.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "poetry.lock",
        "composer.lock",
        "gemfile.lock",
        "go.sum",
    ];
    if LOCKFILES.contains(&last) {
        return 6;
    }

    // Test code — union of the language-specific regex ([`is_test_path`],
    // pass the ORIGINAL case so `FooTests.cs` etc. still match) and the
    // directory/filename markers semantic search relied on, so routing
    // both callers here never loosens either's prior coverage.
    if is_test_path(path) || is_test_pathish(&pslash, last) {
        return 4;
    }

    // Docs / prose.
    if pslash.contains("/docs/")
        || pslash.contains("/doc/")
        || last.ends_with(".md")
        || last.ends_with(".mdx")
        || last.ends_with(".rst")
        || last.ends_with(".txt")
        || last.starts_with("license")
        || last.starts_with("readme")
        || last.starts_with("changelog")
    {
        return 3;
    }

    // Data / config / generated.
    const DATA_EXT: &[&str] = &[
        ".json", ".yaml", ".yml", ".toml", ".xml", ".csv", ".sql", ".proto", ".svg", ".lock",
    ];
    if pslash.contains("/generated/") || DATA_EXT.iter().any(|e| last.ends_with(e)) {
        return 6;
    }

    // Real source code.
    const SRC_EXT: &[&str] = &[
        ".rs", ".py", ".pyi", ".js", ".mjs", ".cjs", ".jsx", ".ts", ".mts", ".cts", ".tsx", ".go",
        ".java", ".kt", ".kts", ".c", ".h", ".cpp", ".cc", ".cxx", ".hpp", ".hh", ".hxx", ".cs",
        ".php", ".rb", ".rake", ".swift", ".scala", ".sc", ".ex", ".exs", ".lua", ".dart", ".sh",
        ".bash",
    ];
    if SRC_EXT.iter().any(|e| last.ends_with(e)) {
        return 0;
    }

    5
}

/// Directory / filename test markers not covered by the language-specific
/// [`is_test_path`] regex (fixtures, testdata, cypress, conftest…).
/// `pslash` is the lowercased, forward-slashed path with a leading slash
/// (so root-relative dirs match too); `last` its final component.
fn is_test_pathish(pslash: &str, last: &str) -> bool {
    const DIR_MARKERS: &[&str] = &[
        "/tests/",
        "/test/",
        "/__tests__/",
        "/__test__/",
        "/spec/",
        "/specs/",
        "/e2e/",
        "/integration_tests/",
        "/_test/",
        "/fixtures/",
        "/testdata/",
        "/cypress/",
    ];
    if DIR_MARKERS.iter().any(|m| pslash.contains(m)) {
        return true;
    }
    last == "conftest.py"
        || last == "tests.rs"
        || last.contains("_test.")
        || last.contains("_tests.")
        || last.contains(".test.")
        || last.contains(".spec.")
        || (last.starts_with("test_") && last.ends_with(".py"))
}

/// Penalty fraction for a file class — the fraction of score a hit of
/// that class *loses*. Kept in sync with [`classify_file`]; test stays at
/// [`TEST_PATH_PENALTY_FRAC`] so the pre-existing behaviour is preserved.
fn class_penalty_frac(class: u8) -> f32 {
    match class {
        0 => 0.0,                    // source — never penalised
        3 => 0.15,                   // docs
        4 => TEST_PATH_PENALTY_FRAC, // test → keep 0.70 (retro-compat)
        6 => 0.45,                   // data / generated
        9 => 0.60,                   // junk
        _ => 0.0,                    // unknown — neutral
    }
}

/// Multiplicative search weight for a file class, used by cross-file
/// rankers (semantic search) that blend into an absolute score rather
/// than applying a subtractive penalty. Source is neutral (`1.0`); test
/// stays at `0.5` to preserve semantic's prior behaviour.
pub fn class_search_weight(class: u8) -> f32 {
    match class {
        0 => 1.0, // source
        3 => 0.6, // docs
        4 => 0.5, // test (retro-compat)
        6 => 0.5, // data / generated
        9 => 0.4, // junk
        _ => 1.0, // unknown — neutral
    }
}

/// Down-weight scores by their file class in place: at ~equal relevance,
/// source outranks docs outranks test outranks data/junk. Reconciles the
/// old test-only penalty with [`classify_file`] — one logic, no
/// double-penalisation. Only meaningful when `alpha < 1.0` (BM25 is
/// contributing); at `alpha == 1.0` we trust the dense ranker fully.
///
/// (Name kept for API stability; it now applies the full class penalty,
/// of which the test demotion is one case.)
pub fn apply_test_path_penalty<C: Scorable>(scored: &mut [(C, f32)], alpha: f32) {
    if alpha >= 1.0 {
        return;
    }
    for (chunk, score) in scored.iter_mut() {
        let frac = class_penalty_frac(classify_file(chunk.file_path()));
        if frac > 0.0 {
            *score *= 1.0 - frac;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── is_symbol_query ──
    #[test]
    fn is_symbol_recognises_bare_identifier_with_uppercase() {
        assert!(is_symbol_query("Client"));
        assert!(is_symbol_query("UserSession"));
    }

    #[test]
    fn is_symbol_recognises_namespace() {
        assert!(is_symbol_query("auth::Login"));
        assert!(is_symbol_query("auth.login"));
        assert!(is_symbol_query("Foo\\Bar"));
        assert!(is_symbol_query("ctx->user"));
    }

    #[test]
    fn is_symbol_recognises_leading_underscore() {
        assert!(is_symbol_query("_internal"));
        assert!(is_symbol_query("__init__"));
    }

    #[test]
    fn is_symbol_rejects_natural_language() {
        assert!(!is_symbol_query("how does authentication work"));
        assert!(!is_symbol_query("find the login flow"));
    }

    // ── resolve_alpha ──
    #[test]
    fn resolve_alpha_picks_symbol_for_pascal_case() {
        assert_eq!(resolve_alpha("UserSession", None), ALPHA_SYMBOL);
    }

    #[test]
    fn resolve_alpha_picks_nl_for_prose() {
        assert_eq!(resolve_alpha("how do we cache results", None), ALPHA_NL);
    }

    #[test]
    fn resolve_alpha_override_wins() {
        assert_eq!(resolve_alpha("anything", Some(0.7)), 0.7);
        assert_eq!(resolve_alpha("UserSession", Some(0.9)), 0.9);
    }

    // ── RRF fusion ──
    #[test]
    fn rrf_top_score_outranks_second() {
        let mut m = HashMap::new();
        m.insert("a", 10.0);
        m.insert("b", 1.0);
        let rrf = rrf_scores(&m);
        assert!(rrf["a"] > rrf["b"]);
        // Top rank-1 RRF: 1/(60+1) = 0.0163...
        assert!((rrf["a"] - 1.0 / 61.0).abs() < 1e-6);
    }

    #[test]
    fn rrf_breaks_ties_on_key_not_on_map_order() {
        // Every score is equal: rank is decided ENTIRELY by the
        // tie-break. Without one, `HashMap` iteration order (randomized
        // per process) would decide it, so the same corpus would rank
        // differently between two runs of the same query.
        let keys = ["delta", "alpha", "charlie", "bravo", "echo"];
        let mut m = HashMap::new();
        for k in keys {
            m.insert(k, 1.0);
        }
        let rrf = rrf_scores(&m);
        // Lexicographic order ⇒ rank 1..=5 ⇒ strictly decreasing RRF.
        let mut expected = keys;
        expected.sort_unstable();
        for pair in expected.windows(2) {
            assert!(
                rrf[pair[0]] > rrf[pair[1]],
                "{} must outrank {} on the key tie-break",
                pair[0],
                pair[1]
            );
        }
        assert!((rrf["alpha"] - 1.0 / (RRF_K + 1.0)).abs() < 1e-6);
    }

    #[test]
    fn rrf_is_stable_across_rebuilds_of_the_same_map() {
        // Two maps with identical content but built in opposite
        // insertion order must produce byte-identical rankings.
        let pairs = [("a", 3.0f32), ("b", 3.0), ("c", 3.0), ("d", 1.0)];
        let forward: HashMap<_, _> = pairs.iter().copied().collect();
        let backward: HashMap<_, _> = pairs.iter().rev().copied().collect();
        assert_eq!(rrf_scores(&forward), rrf_scores(&backward));
    }

    #[test]
    fn combine_rrf_blends_with_alpha() {
        let mut dense = HashMap::new();
        dense.insert("a", 0.9);
        dense.insert("b", 0.5);
        let mut bm25 = HashMap::new();
        bm25.insert("b", 10.0);
        bm25.insert("c", 5.0);
        // alpha=0.5 balanced.
        let combined = combine_rrf(&dense, &bm25, 0.5);
        // 'b' appears in both, top in BM25, second in dense.
        assert!(combined.contains_key("b"));
        assert!(combined.contains_key("a"));
        assert!(combined.contains_key("c"));
    }

    // ── boosting ──
    struct FakeChunk {
        path: String,
        body: String,
    }
    impl Scorable for FakeChunk {
        fn file_path(&self) -> &str {
            &self.path
        }
        fn content(&self) -> &str {
            &self.body
        }
    }

    #[test]
    fn multi_chunk_file_boost_promotes_top_chunk() {
        let mut scored: Vec<(FakeChunk, f32)> = vec![
            (
                FakeChunk {
                    path: "a.rs".into(),
                    body: "x".into(),
                },
                1.0,
            ),
            (
                FakeChunk {
                    path: "a.rs".into(),
                    body: "y".into(),
                },
                0.9,
            ),
            (
                FakeChunk {
                    path: "a.rs".into(),
                    body: "z".into(),
                },
                0.8,
            ),
            (
                FakeChunk {
                    path: "b.rs".into(),
                    body: "w".into(),
                },
                0.7,
            ),
        ];
        let original_top = scored[0].1;
        boost_multi_chunk_files(&mut scored);
        // 'a.rs' is the multi-chunk-file → top chunk should be boosted.
        assert!(scored[0].1 > original_top);
    }

    #[test]
    fn definition_boost_promotes_chunk_defining_symbol() {
        let mut scored: Vec<(FakeChunk, f32)> = vec![
            (
                FakeChunk {
                    path: "src/parser.rs".into(),
                    body: "fn parse() { ... }".into(),
                },
                0.5,
            ),
            (
                FakeChunk {
                    path: "src/parser.rs".into(),
                    body: "fn parse_int() { ... }".into(),
                },
                0.4,
            ),
            // The chunk that actually defines `Parser` should win.
            (
                FakeChunk {
                    path: "src/parser.rs".into(),
                    body: "struct Parser { input: String }".into(),
                },
                0.3,
            ),
        ];
        boost_symbol_definitions(&mut scored, "Parser");
        // The 3rd chunk had the lowest base score but should now be
        // ranked top because it defines the symbol.
        let max = scored
            .iter()
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            .unwrap();
        assert!(max.0.body.contains("struct Parser"));
    }

    // ── test-path penalty ──
    #[test]
    fn is_test_path_recognises_python_test() {
        assert!(is_test_path("tests/test_foo.py"));
        assert!(is_test_path("src/foo_test.py"));
    }

    #[test]
    fn is_test_path_recognises_rust_integration() {
        assert!(is_test_path("tests/integration.rs"));
        assert!(is_test_path("src/parser_test.rs"));
    }

    #[test]
    fn is_test_path_rejects_production_code() {
        assert!(!is_test_path("src/parser.rs"));
        assert!(!is_test_path("crates/core/src/lib.rs"));
    }

    #[test]
    fn apply_test_path_penalty_demotes_when_alpha_below_one() {
        let mut scored: Vec<(FakeChunk, f32)> = vec![
            (
                FakeChunk {
                    path: "src/foo.rs".into(),
                    body: "".into(),
                },
                1.0,
            ),
            (
                FakeChunk {
                    path: "tests/foo_test.rs".into(),
                    body: "".into(),
                },
                1.0,
            ),
        ];
        apply_test_path_penalty(&mut scored, 0.5);
        assert!(scored[0].1 > scored[1].1, "test file should be demoted");
    }

    #[test]
    fn apply_test_path_penalty_noop_at_alpha_one() {
        let mut scored: Vec<(FakeChunk, f32)> = vec![(
            FakeChunk {
                path: "tests/foo_test.rs".into(),
                body: "".into(),
            },
            1.0,
        )];
        apply_test_path_penalty(&mut scored, 1.0);
        assert_eq!(scored[0].1, 1.0);
    }

    // ── classify_file ──
    #[test]
    fn classify_file_orders_source_below_docs_test_data() {
        // Lower = higher priority. Source < docs < test < data/junk.
        assert_eq!(classify_file("crates/core/src/parser.rs"), 0);
        assert_eq!(classify_file("src/app.py"), 0);
        assert_eq!(classify_file("README.md"), 3);
        assert_eq!(classify_file("docs/guide.rs"), 3);
        assert_eq!(classify_file("tests/parser_test.rs"), 4);
        assert_eq!(classify_file("src/foo_test.py"), 4);
        assert_eq!(classify_file("fixtures/sample.rs"), 4);
        assert_eq!(classify_file("config/data.json"), 6);
        assert_eq!(classify_file("Cargo.lock"), 6);
        assert_eq!(classify_file("web/app.min.js"), 6);
        assert_eq!(classify_file("node_modules/left-pad/index.js"), 9);
        // strict ordering source < docs < test < data < junk
        assert!(classify_file("src/a.rs") < classify_file("docs/a.md"));
        assert!(classify_file("docs/a.md") < classify_file("tests/a_test.rs"));
        assert!(classify_file("tests/a_test.rs") < classify_file("a.json"));
        assert!(classify_file("a.json") < classify_file("node_modules/a.js"));
    }

    #[test]
    fn class_penalty_keeps_test_at_legacy_factor() {
        // Test class must still lose exactly TEST_PATH_PENALTY_FRAC so the
        // pre-existing 0.70 behaviour is preserved.
        assert_eq!(class_penalty_frac(classify_file("tests/x_test.rs")), 0.30);
        assert_eq!(class_penalty_frac(classify_file("src/x.rs")), 0.0);
    }

    #[test]
    fn class_penalty_ranks_source_above_test_above_data() {
        let mut scored: Vec<(FakeChunk, f32)> = vec![
            (
                FakeChunk {
                    path: "src/main.rs".into(),
                    body: "".into(),
                },
                1.0,
            ),
            (
                FakeChunk {
                    path: "src/main_test.rs".into(),
                    body: "".into(),
                },
                1.0,
            ),
            (
                FakeChunk {
                    path: "data/seed.json".into(),
                    body: "".into(),
                },
                1.0,
            ),
            (
                FakeChunk {
                    path: "docs/readme.md".into(),
                    body: "".into(),
                },
                1.0,
            ),
        ];
        apply_test_path_penalty(&mut scored, 0.5);
        let src = scored[0].1;
        let test = scored[1].1;
        let data = scored[2].1;
        let docs = scored[3].1;
        assert!(src > docs, "source must outrank docs");
        assert!(docs > test, "docs must outrank test");
        assert!(test > data, "test must outrank data/generated");
    }
}
