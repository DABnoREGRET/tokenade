//! **What the repository already knows about itself.**
//!
//! A project carries definitions — API names, in-house acronyms, canonical
//! paths — that a query uses without spelling them out. Expanding them BEFORE
//! the search is what makes the difference between finding the right file
//! and finding the thousand that talk about the same thing.
//!
//! `looks_like_symbol` decides whether a word is an identifier or prose, and
//! that choice steers the whole collection: a word treated as a symbol looks
//! for a declaration, a word treated as prose looks for meaning.

use super::*;

/// The shared code-intel indices, loaded once per scout run (cheap when warm:
/// `ensure_fresh` is incremental, the semantic index rebuild is mtime-gated).
/// Same pipeline as `tokenade semantic`/`query`, called as library functions —
/// no self-exec, no stdout scraping.
pub(super) struct Intel {
    root: PathBuf,
    idx: tokenade_core::index::Index,
    sem: tokenade_core::semantic::SemanticIndex,
}

pub(super) fn load_intel() -> Result<Intel, String> {
    let root = crate::root();
    let mut idx = tokenade_core::index::Index::ensure_fresh(&root)
        .map_err(|e| format!("symbol index: {e}"))?;
    // `query_hits` ranks by call-graph centrality, and since the graph moved
    // to its own column `ensure_fresh` no longer
    // carries it. Without this the scores are all zero and the ranking
    // silently falls back to the legacy order — an absent graph and a graph
    // where nothing is central produce the same numbers.
    if let Err(e) = idx.load_calls(&root) {
        tokenade_core::debug_log::log(
            "scout",
            "call_graph_unavailable",
            serde_json::json!({ "error": e.to_string() }),
        );
    }
    let sem = match tokenade_core::semantic::SemanticIndex::load(&root) {
        Some(prev) if !prev.mtimes.is_empty() => {
            let s = tokenade_core::semantic::SemanticIndex::rebuild_incremental(&prev, &idx, &root);
            let _ = s.save(&root);
            s
        }
        _ => {
            let s = tokenade_core::semantic::SemanticIndex::build_from_index(&idx, &root);
            let _ = s.save(&root);
            s
        }
    };
    Ok(Intel { root, idx, sem })
}

impl Intel {
    /// Rank candidate files for the question. Hybrid (BM25 + bi-encoder +
    /// reranker) when the embedding models are available, BM25 otherwise —
    /// a degraded ranking beats a hard error inside a pipeline.
    fn semantic_files(&self, question: &str) -> Vec<PathBuf> {
        // UNION of hybrid (rerank) and plain BM25 rankings: the cross-encoder
        // sometimes tunnel-visions on one text-similar file and the whole
        // evidence pack inherits the mistake (measured: a flow question packed
        // only the Cline transcript parser while trace.rs held the answer
        // verbatim). BM25's independent view diversifies the pack.
        let n = top_files();
        // Same query-shape weighting as `tokenade semantic` — the scout asks
        // both prose questions and bare symbol lookups through this one call.
        // (`resolve_alpha` yields the dense weight, `search_hybrid` wants the
        // BM25 weight; hence the flip.)
        let alpha_bm25 = 1.0 - tokenade_core::ranking::resolve_alpha(question, None);
        let mut hits = match self.sem.search_hybrid(question, n, alpha_bm25) {
            Ok(h) => h,
            Err(_) => self.sem.search(question, n),
        };
        if bm25_union() {
            for h in self.sem.search(question, n) {
                if !hits.iter().any(|e| e.file == h.file) {
                    hits.push(h);
                }
            }
        }
        // Path-affinity boost: a file whose PATH matches question terms
        // ("light-agent run" → agent/…) is far more likely to hold the answer
        // than a text-similar file elsewhere (measured: a flow question packed
        // a transcript-parser file naming `ui_messages.json` and the model
        // attributed that file to OUR agent traces). Stable partition keeps
        // the semantic order within each group.
        let terms: Vec<String> = keywords(question)
            .into_iter()
            .filter(|k| k.len() >= 4)
            .collect();
        let path_match = |file: &str| {
            let low = file.to_lowercase();
            terms.iter().any(|t| low.contains(t.as_str()))
        };
        hits.sort_by_key(|h| !path_match(&h.file)); // matching paths first, stable
        let mut files = Vec::new();
        for h in hits {
            let p = self.root.join(&h.file);
            let rel = PathBuf::from(&h.file);
            let p = if p.exists() { p } else { rel };
            if p.exists() && !is_noise_path(&p) && !files.contains(&p) {
                files.push(p);
            }
            if files.len() >= n {
                break;
            }
        }
        files
    }

    pub(super) fn root(&self) -> &PathBuf {
        &self.root
    }

    /// Reverse-dependency lookup off the AST index's import graph: files whose
    /// recorded imports mention this file's stem. Approximate (module-name
    /// match), which is fine for an exploration hint.
    pub(super) fn dependents_of(&self, file: &std::path::Path) -> Vec<PathBuf> {
        let Some(stem) = file.file_stem().and_then(|s| s.to_str()) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (importer, imports) in &self.idx.imports {
            if imports.iter().any(|i| i.contains(stem)) {
                let p = self.root.join(importer);
                if p.exists() && !is_noise_path(&p) && p != file {
                    out.push(p);
                }
            }
            if out.len() >= 12 {
                break;
            }
        }
        out
    }

    /// Exact symbol hits from the AST index → (file, line).
    pub(super) fn query_hits(&self, symbol: &str) -> Vec<(PathBuf, usize)> {
        let centrality = self.idx.centrality();
        self.idx
            .find_ranked(symbol, 8, &centrality)
            .into_iter()
            .filter_map(|s| {
                let p = self.root.join(&s.file);
                let p = if p.exists() {
                    p
                } else {
                    PathBuf::from(&s.file)
                };
                (p.exists() && !is_noise_path(&p)).then_some((p, s.line))
            })
            .collect()
    }

    /// Definition line of an identifier via the AST index (any language the
    /// tree-sitter extractors cover) — replaces string-matching on `const X`.
    fn definition_of(&self, ident: &str) -> Option<(PathBuf, usize)> {
        self.idx
            .find(ident, 16)
            .into_iter()
            .filter(|s| s.name == ident)
            .map(|s| {
                let p = self.root.join(&s.file);
                let p = if p.exists() {
                    p
                } else {
                    PathBuf::from(&s.file)
                };
                (p, s.line)
            })
            .find(|(p, _)| p.exists() && !is_noise_path(p))
    }
}

/// A window that references a constant defined elsewhere is a hallucination
/// trap: the model sees `DEFAULT_MAX_EDGE` mentioned (next to some other
/// number) but never its value, and confidently invents one (measured: every
/// model answered "64" off the `>= 64` threshold in the env-var doc while
/// `DEFAULT_MAX_EDGE: u32 = 1024` sat 30 lines above the window). For each
/// SCREAMING_CASE identifier referenced in the pack whose defining line isn't
/// packed, append a 1-line definition snippet found in the selected files.
pub(crate) fn expand_definitions(
    snippets: &mut Vec<Snippet>,
    contents: &[(usize, &PathBuf, String)],
    intel: Option<&Intel>,
) {
    const MAX_EXTRA: usize = 6;
    let packed: String = snippets.iter().map(|s| s.text.as_str()).collect();
    let mut idents: Vec<&str> = packed
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|t| {
            t.len() >= 4
                && t.chars().next().is_some_and(|c| c.is_ascii_uppercase())
                && t.contains('_')
                && t.chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        })
        .collect();
    idents.sort_unstable();
    idents.dedup();

    let covered = |snips: &[Snippet], f: &PathBuf, line: usize| {
        snips.iter().any(|s| {
            s.file == *f && line >= s.start_line && line < s.start_line + s.text.lines().count()
        })
    };
    let defines = |ident: &str, l: &str| {
        (l.contains(&format!("const {ident}")) || l.contains(&format!("static {ident}")))
            && l.contains('=')
    };

    let mut extra = 0;
    for ident in idents {
        if extra >= MAX_EXTRA {
            break;
        }
        if snippets
            .iter()
            .any(|s| s.text.lines().any(|l| defines(ident, l)))
        {
            continue; // definition already visible
        }
        // AST index first (any language tree-sitter covers, cross-file);
        // lexical scan of the already-read files as fallback (identifiers the
        // index doesn't record, e.g. some env-var-shaped locals).
        let found: Option<(PathBuf, usize, String)> = intel
            .and_then(|i| i.definition_of(ident))
            .and_then(|(p, line)| {
                let text = std::fs::read_to_string(&p).ok()?;
                let l = text.lines().nth(line.saturating_sub(1))?.to_string();
                Some((p, line, l))
            })
            .or_else(|| {
                contents.iter().find_map(|(_, f, content)| {
                    content.lines().enumerate().find_map(|(i, l)| {
                        defines(ident, l).then(|| ((*f).clone(), i + 1, l.to_string()))
                    })
                })
            });
        if let Some((f, line, text)) = found {
            if !covered(snippets, &f, line) {
                snippets.push(Snippet {
                    file: f,
                    start_line: line,
                    text,
                });
                extra += 1;
            }
        }
    }
}

pub(crate) fn keywords(question: &str) -> Vec<String> {
    const STOP: &[&str] = &[
        "the", "and", "for", "are", "was", "what", "which", "where", "when", "how", "does", "this",
        "that", "with", "from", "into", "its", "any", "can", "will", "default", "value", "used",
        "use", "should",
    ];
    let words: Vec<String> = question
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .map(|w| w.to_lowercase())
        .filter(|w| !w.is_empty())
        .collect();
    let mut ks: Vec<String> = words
        .iter()
        .filter(|w| w.len() >= 3 && !STOP.contains(&w.as_str()))
        .cloned()
        .collect();
    // Compound variants of adjacent word pairs: "K cache" in a question is
    // "k-cache" / "k_cache" / "kcache" in code and comments — the joined forms
    // are rare, thus high-signal for window ranking.
    for pair in words.windows(2) {
        if STOP.contains(&pair[0].as_str()) || STOP.contains(&pair[1].as_str()) {
            continue;
        }
        for sep in ["-", "_", ""] {
            ks.push(format!("{}{sep}{}", pair[0], pair[1]));
        }
    }
    ks.dedup();
    ks
}

pub(crate) fn looks_like_symbol(tok: &str) -> bool {
    tok.len() >= 4
        && (tok.contains('_')
            || tok.contains("::")
            || (tok.chars().next().is_some_and(|c| c.is_lowercase())
                && tok.chars().any(|c| c.is_uppercase())))
}
