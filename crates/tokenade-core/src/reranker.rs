//! Cross-encoder reranking.
//!
//! ## Status (L112)
//!
//! We deleted the BGE-reranker-v2-m3 dependency (568 MB cross-encoder)
//! when we swapped fastembed → model2vec-rs. The old pipeline ran a
//! transformer cross-encoder over the top-N dense results to fine-tune
//! ranking — useful for high-stakes retrieval but too costly when:
//! - the model itself was 568 MB on top of the 465 MB embedder,
//! - cross-encoder inference was ~200 ms per (query, doc) pair,
//! - it duplicated work that score-fusion + boosting already do well.
//!
//! Benchmarked at NDCG@10 0.854 with a 2 MB static model +
//! BM25/RRF hybrid + code-aware boosts, vs 0.862 with a 137M transformer
//! reranker), the quality delta isn't worth the operational cost.
//!
//! The replacement lives in [`crate::ranking`] (L115): RRF fusion of
//! dense + BM25 + definition-keyword boost + multi-chunk-file coherence
//! boost + test-path penalty. That stack is pure Rust, requires zero
//! extra weights, and runs in microseconds.
//!
//! [`rerank`] is preserved as a thin shim so callers don't break: it
//! returns the input documents in their original order with a flat
//! score, since the score-fusion path is now upstream of this call.

/// Errors are kept for API compatibility, but the implementation never
/// returns them.
#[derive(Debug, Clone)]
pub enum RerankError {
    ModelInit(String),
    Lock(String),
    Inference(String),
}

impl std::fmt::Display for RerankError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RerankError::ModelInit(m) => {
                write!(f, "reranker disabled in L112; this should not happen ({m})")
            }
            RerankError::Lock(m) => write!(f, "reranker model lock poisoned: {m}"),
            RerankError::Inference(m) => write!(f, "reranker inference failed: {m}"),
        }
    }
}

impl std::error::Error for RerankError {}

/// One reranked document with its score (always 1.0 in the L112 stub —
/// real ranking happens upstream via `ranking::apply_query_boost` &c.).
#[derive(Debug, Clone)]
pub struct Reranked {
    pub index: usize,
    pub score: f32,
}

/// Identity rerank: returns `docs` indices 0..min(len, top_n) with a
/// flat score. Real ranking has moved to [`crate::ranking`].
pub fn rerank(_query: &str, docs: &[String], top_n: usize) -> Result<Vec<Reranked>, RerankError> {
    let cap = top_n.min(docs.len());
    Ok((0..cap)
        .map(|i| Reranked {
            index: i,
            score: 1.0,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rerank_returns_input_order_capped_at_top_n() {
        let docs: Vec<String> = (0..10).map(|i| format!("doc {i}")).collect();
        let out = rerank("q", &docs, 5).unwrap();
        assert_eq!(out.len(), 5);
        for (i, r) in out.iter().enumerate() {
            assert_eq!(r.index, i);
            assert_eq!(r.score, 1.0);
        }
    }

    #[test]
    fn rerank_caps_at_input_len() {
        let docs = vec!["a".to_string(), "b".to_string()];
        let out = rerank("q", &docs, 10).unwrap();
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn rerank_empty_input() {
        let out = rerank("q", &[], 5).unwrap();
        assert!(out.is_empty());
    }
}
