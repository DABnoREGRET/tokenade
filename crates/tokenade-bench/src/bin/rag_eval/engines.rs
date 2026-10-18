//! **The engines compared.** Content windows, dense recall, RRF fusion,
//! query expansion, French stemming, and a home-made BM25 used only to
//! compare two LEVELS of chunking with the same implementation.

#[allow(unused_imports)]
use super::*;
use std::collections::HashMap;
use std::path::Path;
use tokenade_core::semantic;

/// The CONTENT cut into windows, as `claude-context` does (1,000
/// characters, 200 overlap) — the idea tested here is that dense retrieval
/// needs real material, not a 500-character excerpt.
pub(crate) fn chunks_of(repo: &Path, sem: &semantic::SemanticIndex) -> (Vec<String>, Vec<usize>) {
    const SIZE: usize = 1000;
    const OVERLAP: usize = 200;
    const MAX_PER_FILE: usize = 40;
    let (mut texts, mut owner) = (Vec::new(), Vec::new());
    for (i, d) in sem.docs.iter().enumerate() {
        let Ok(body) = std::fs::read_to_string(repo.join(&d.file)) else {
            continue;
        };
        let b: Vec<char> = body.chars().collect();
        let mut start = 0usize;
        let mut made = 0usize;
        while start < b.len() && made < MAX_PER_FILE {
            let end = (start + SIZE).min(b.len());
            // The path at the head of each window: it situates the fragment, and
            // it is also what today's excerpt does.
            texts.push(format!(
                "{} — {}",
                d.file,
                b[start..end].iter().collect::<String>()
            ));
            owner.push(i);
            made += 1;
            if end == b.len() {
                break;
            }
            start = end.saturating_sub(OVERLAP);
        }
    }
    (texts, owner)
}

/// **FRENCH stemming, which the product does not know.**
///
/// `semantic::text::stem_variants` carries nine suffixes, all English (`ing`,
/// `tion`, `ed`, `es`, `s`…). This repo's questions are written in
/// French — « quelles mesures », « les plis », « la récupération » — and a
/// French word in the plural or conjugated does not meet its form in the code.
/// This test adds common French variants to the QUERY; if it
/// pays off, the fix belongs in the tokenizer, not the bench.
pub(crate) fn fr_variants(q: &str) -> String {
    const SUF: &[(&str, &str)] = &[
        ("ations", "er"),
        ("ation", "er"),
        ("ements", "er"),
        ("ement", "er"),
        ("aient", "er"),
        ("ions", "er"),
        ("ées", "er"),
        ("ée", "er"),
        ("és", "er"),
        ("es", ""),
        ("s", ""),
    ];
    let mut out = q.to_string();
    for w in q.split(|c: char| !c.is_alphanumeric()) {
        let lw = w.to_lowercase();
        if lw.len() < 5 {
            continue;
        }
        for (suf, repl) in SUF {
            if lw.ends_with(suf) && lw.len() >= suf.len() + 3 {
                let stem = format!("{}{}", &lw[..lw.len() - suf.len()], repl);
                if stem.len() >= 3 {
                    out.push(' ');
                    out.push_str(&stem);
                }
                break;
            }
        }
    }
    out
}

/// **Home-made BM25, to compare one CHUNKING LEVEL with another.**
///
/// The hypothesis is that a big file dilutes everything and that one should
/// index by symbol or by fragment. Comparing it to the shipped BM25 would measure
/// two things at once (the implementation AND the chunking), so the bench carries
/// ITS OWN BM25 and runs it at both levels: whole file, then
/// 1,000-character windows folded per file (the best fragment
/// wins). Only the chunking changes.
pub(crate) struct Bm25 {
    pub(crate) inv: HashMap<String, Vec<(usize, f64)>>,
    pub(crate) len: Vec<f64>,
    pub(crate) avg: f64,
    pub(crate) owner: Vec<usize>,
}

pub(crate) fn toks(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in s.split(|c: char| !c.is_alphanumeric() && c != '_') {
        if raw.is_empty() {
            continue;
        }
        out.push(raw.to_lowercase());
        // camel/snake: the same unit the product indexes.
        let mut cur = String::new();
        for ch in raw.chars() {
            if (ch.is_uppercase() || ch == '_') && cur.len() >= 3 {
                out.push(std::mem::take(&mut cur).to_lowercase());
            }
            if ch != '_' {
                cur.push(ch);
            }
        }
        if cur.len() >= 3 {
            out.push(cur.to_lowercase());
        }
    }
    out
}

impl Bm25 {
    pub(crate) fn build(texts: &[String], owner: Vec<usize>) -> Self {
        let mut inv: HashMap<String, Vec<(usize, f64)>> = HashMap::new();
        let mut len = Vec::with_capacity(texts.len());
        for (i, t) in texts.iter().enumerate() {
            let tk = toks(t);
            len.push(tk.len() as f64);
            let mut tf: HashMap<String, f64> = HashMap::new();
            for w in tk {
                *tf.entry(w).or_insert(0.0) += 1.0;
            }
            for (w, f) in tf {
                inv.entry(w).or_default().push((i, f));
            }
        }
        let avg = if len.is_empty() {
            1.0
        } else {
            len.iter().sum::<f64>() / len.len() as f64
        };
        Self {
            inv,
            len,
            avg,
            owner,
        }
    }

    /// Files, from best to worst; a file is worth its BEST
    /// fragment.
    pub(crate) fn search(&self, q: &str, ndocs: usize, k: usize) -> Vec<usize> {
        const K1: f64 = 1.2;
        const B: f64 = 0.75;
        let n = self.len.len() as f64;
        let mut score: HashMap<usize, f64> = HashMap::new();
        for w in toks(q) {
            let Some(post) = self.inv.get(&w) else {
                continue;
            };
            let idf = ((n - post.len() as f64 + 0.5) / (post.len() as f64 + 0.5) + 1.0).ln();
            for (i, f) in post {
                let norm = f * (K1 + 1.0) / (f + K1 * (1.0 - B + B * self.len[*i] / self.avg));
                let e = score.entry(self.owner[*i]).or_insert(0.0);
                *e = e.max(idf * norm);
            }
        }
        let _ = ndocs;
        let mut v: Vec<(usize, f64)> = score.into_iter().collect();
        v.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });
        v.into_iter().take(k).map(|(i, _)| i).collect()
    }
}

/// **Query expansion through the index's VOCABULARY.**
///
/// Three competitors out of eight do it (`Context-Engine`, `archex`,
/// `headroom`). Here it costs neither a model nor a network: for each word of the
/// question, we add the inverted-index terms it PREFIXES —
/// "limit" catches `limiter`, `rate_limit`, `limits` — keeping the
/// RAREST (high IDF), because a frequent term only adds noise.
pub(crate) fn expand_query(sem: &semantic::SemanticIndex, q: &str, per_term: usize) -> String {
    let mut out = q.to_string();
    let n = sem.docs.len().max(1) as f64;
    for w in q.split(|c: char| !c.is_alphanumeric()) {
        let w = w.to_lowercase();
        if w.len() < 4 {
            continue;
        }
        let mut cand: Vec<(f64, &String)> = sem
            .inv
            .iter()
            .filter(|(t, _)| t.len() > w.len() && t.starts_with(&w))
            .map(|(t, post)| ((n / post.len().max(1) as f64).ln(), t))
            .collect();
        cand.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.1.cmp(b.1))
        });
        for (_, t) in cand.into_iter().take(per_term) {
            out.push(' ');
            out.push_str(t);
        }
    }
    out
}

/// INDEPENDENT dense recall: cosine against the whole corpus, not only
/// against what BM25 deigned to bring up.
pub(crate) fn dense_top(
    q: &[f32],
    vecs: &[Vec<f32>],
    sem: &semantic::SemanticIndex,
    k: usize,
) -> Vec<String> {
    let mut scored: Vec<(f32, usize)> = vecs
        .iter()
        .enumerate()
        .map(|(i, v)| (tokenade_core::embedding::cosine(q, v), i))
        .collect();
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
    });
    scored
        .iter()
        .take(k)
        .map(|(_, i)| sem.docs[*i].file.clone())
        .collect()
}

/// Rank fusion (RRF, k=60): insensitive to the score scale, whereas
/// linear mixing depends on a single extreme BM25 score to normalise itself.
pub(crate) fn rrf(lists: &[Vec<String>], k: usize) -> Vec<String> {
    const RRF_K: f64 = 60.0;
    let mut score: HashMap<&str, f64> = HashMap::new();
    for l in lists {
        for (i, f) in l.iter().enumerate() {
            *score.entry(f.as_str()).or_insert(0.0) += 1.0 / (RRF_K + (i + 1) as f64);
        }
    }
    let mut v: Vec<(&str, f64)> = score.into_iter().collect();
    v.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(b.0))
    });
    v.iter().take(k).map(|(f, _)| (*f).to_string()).collect()
}

pub(crate) fn dcg(rels: &[f64]) -> f64 {
    rels.iter()
        .enumerate()
        .map(|(i, r)| r / ((i + 2) as f64).log2())
        .sum()
}
