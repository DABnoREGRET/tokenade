//! Static-embedding sentence encoder (potion-multilingual-128M, int8).
//!
//! ## What this is
//!
//! A model2vec static embedding model: **256-dimensional** vectors produced by
//! a token lookup table (no transformer layers, no ONNX runtime, no GPU), so
//! encoding is ~200× faster than a sentence-transformer — which fits "agents
//! calling search in a hot loop".
//!
//! Inference runs through [`crate::static_embed`], not `model2vec-rs`: same
//! vectors (verified, worst cosine 0.999994 over 100 133 real document
//! segments), but mmap'd instead of resident. See that module for why.
//!
//! ## Why potion-multilingual-128M replaced potion-code-16M
//!
//! The code model was picked on the theory that queries against a codebase are
//! English + identifiers. Measured on our RAG evaluation, that theory cost us:
//! its French gaps were 0.113/0.099 — so weak that `web_rag`'s intent-label
//! gate (cosine 0.3) sat above the maximum French cosine it could produce
//! (0.282) and never fired on French at all. Those labels have since been
//! removed: measured four ways, they were worth nothing even in English.
//!
//! multilingual-128M scores 0.571 mean / 0.414 worst against 0.353/0.083, and
//! it reads French code BETTER than the code model does (0.756 vs 0.416). The
//! one regression is English code retrieval (0.692 → 0.573).
//!
//! It is used int8-quantised: 0.570/0.413, i.e. free within noise, and the
//! quantisation is what makes the rows mmap-able.
//!
//! ## Vector-table migration
//!
//! Both models are 256-dim, so dimension is NOT a safe migration signal here —
//! see [`model_fingerprint`], which is.

use std::sync::{Mutex, OnceLock};

use crate::static_embed::StaticEmbedder;

/// Output dimensionality. Both potion-code-16M and potion-multilingual-128M
/// emit 256; see [`model_fingerprint`] for why that similarity is a hazard.
pub const DIM: usize = 256;

// The model bytes are no longer baked into the binary via `include_bytes!`
// (that made every artifact ~104 MB). They are fetched on first use from
// the signed R2 bucket and cached on disk — see `crate::model_fetch`.

/// What can go wrong loading or running the embedder.
#[derive(Debug, Clone)]
pub enum EmbedError {
    /// Couldn't initialise the model — should never happen with bundled
    /// bytes, but kept for API back-compat.
    ModelInit(String),
    /// Lock contention or panic in another thread.
    Lock(String),
    /// Inference itself failed (very rare — bad input, OOM).
    Inference(String),
}

impl std::fmt::Display for EmbedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EmbedError::ModelInit(m) => {
                write!(
                    f,
                    "couldn't load the embedding model ({m}). It is fetched once on first use and cached locally — this usually means no network was available for that first fetch, or the cache dir isn't writable. Re-run once online, or set TOKENADE_MODEL_DIR to a directory holding model.safetensors / vocab.fst / vocab.fst.scores / vocab.fst.norm / config.json."
                )
            }
            EmbedError::Lock(m) => write!(f, "embedding model lock poisoned: {m}"),
            EmbedError::Inference(m) => write!(f, "embedding inference failed: {m}"),
        }
    }
}

impl std::error::Error for EmbedError {}

/// Lazily initialised global model. First call mmaps the artifacts (~1 ms);
/// there is no Mutex because `embed` takes `&self` and touches no interior
/// state — the old lock existed only because `model2vec_rs::StaticModel`
/// required `&mut`.
fn model() -> Result<&'static StaticEmbedder, EmbedError> {
    // Only a SUCCESSFUL load is remembered. This held a
    // `OnceLock<Result<_, String>>` until 2026-08-27, which memoised the
    // FAILURE too: one transient first touch — the 132 MB fetch interrupted,
    // a full disk, an EINTR — and every later call in the process got the
    // same cached string back. Harmless in a hook that lives for one command;
    // fatal in the two processes that do not, `mcp-proxy` and `llm-proxy`,
    // where it means semantic search and `read --prompt` stay dead until the
    // daemon is restarted, with nothing retrying and nothing saying so.
    static M: OnceLock<StaticEmbedder> = OnceLock::new();
    if let Some(m) = M.get() {
        return Ok(m);
    }
    // Double-checked, because the load is not the cheap part. `model_dir()`
    // DOWNLOADS when the cache is cold, so letting two first-touch callers
    // race would fetch 132 MB twice. The lock bounds it to one; the second
    // caller finds the model already there.
    static LOADING: Mutex<()> = Mutex::new(());
    let _g = LOADING.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(m) = M.get() {
        return Ok(m);
    }
    let dir = crate::model_fetch::model_dir().map_err(EmbedError::ModelInit)?;
    let loaded = StaticEmbedder::load(&dir).map_err(|e| EmbedError::ModelInit(e.to_string()))?;
    Ok(M.get_or_init(|| loaded))
}

/// Embed a single text. Process-wide LRU cache (capacity 256) keyed by
/// FNV-1a hash of the text — re-embedding the same query within a
/// session is a HashMap hit (microseconds) instead of a model run.
pub fn embed_text(text: &str) -> Result<Vec<f32>, EmbedError> {
    let key = crate::cache::hash(text.as_bytes());
    if let Some(v) = cache_get(key) {
        return Ok(v);
    }
    let mut batch = embed_batch(&[text.to_string()])?;
    let v = batch
        .pop()
        .ok_or_else(|| EmbedError::Inference("empty result for non-empty input".into()))?;
    cache_put(key, v.clone());
    Ok(v)
}

fn cache() -> &'static Mutex<crate::lru::LruCache<u64, Vec<f32>>> {
    static C: OnceLock<Mutex<crate::lru::LruCache<u64, Vec<f32>>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(crate::lru::LruCache::new(256)))
}

fn cache_get(k: u64) -> Option<Vec<f32>> {
    cache().lock().ok().and_then(|mut g| g.get(&k).cloned())
}

fn cache_put(k: u64, v: Vec<f32>) {
    if let Ok(mut g) = cache().lock() {
        g.put(k, v);
    }
}

/// Embed many texts at once. model2vec batches via per-token lookup so
/// this is just `texts.len()` independent lookups — no batching benefit
/// per se, but a single lock acquisition.
pub fn embed_batch(texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
    let m = model()?;
    Ok(texts.iter().map(|t| m.embed(t)).collect())
}

/// Vector dimensionality (256).
pub fn dim() -> usize {
    DIM
}

/// Identity of the model that produced a vector. Any cache holding embeddings
/// MUST store this and refuse rows that carry a different one.
///
/// Dimensionality is NOT an identity. The existing self-migration compares
/// `float[dim]` in the vec0 schema and drops the table on a mismatch — which
/// worked exactly once, for the 384→256 change, and is blind to the case that
/// matters now: `potion-code-16M` and `potion-multilingual-128M` are BOTH
/// 256-dim. Swapping them keeps every stored vector, and a cosine between a
/// vector from one model and a query from another is meaningless — no error, no
/// dimension clash, just silently wrong search results.
///
/// Keyed on [`crate::model_fetch::MODEL_VERSION`], which is already bumped
/// whenever the weights change (it names the cache dir and the R2 object), so
/// there is one thing to remember instead of two.
pub fn model_fingerprint() -> &'static str {
    crate::model_fetch::MODEL_VERSION
}

/// Cosine similarity for L2-normalised vectors (i.e. dot product).
/// The model outputs unit vectors (config sets `normalize: true`), so this is
/// the right primitive.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A failed model load must NOT be remembered. The lock held a
    /// `OnceLock<Result<StaticEmbedder, String>>`, so one transient first
    /// touch poisoned the process: in `mcp-proxy` and `llm-proxy`, which
    /// outlive a single command, semantic search and `read --prompt` stayed
    /// dead until restart with nothing retrying and nothing saying so.
    ///
    /// Asserted on the SOURCE because the cache is process-wide: any test that
    /// loads the model once makes every later assertion about "does it retry"
    /// unobservable, in whatever order the harness happens to run.
    #[test]
    fn only_a_successful_model_load_is_remembered() {
        const SRC: &str = include_str!("embedding.rs");
        let at = SRC
            .find("fn model() -> Result<&'static StaticEmbedder, EmbedError>")
            .expect("fn model is gone");
        let end = SRC[at..].find("\n}").map(|i| at + i).unwrap_or(SRC.len());
        // CODE only. The comment above the lock quotes the very type it warns
        // against, and a guard that reads its own prose proves nothing — the
        // same trap, one level removed.
        let body: String = SRC[at..end]
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        let body = body.as_str();
        assert!(
            !body.contains(concat!("OnceLock<", "Result<")),
            "the model cache memoises failures again:\n{body}"
        );
        assert!(
            body.contains("static M: OnceLock<StaticEmbedder>"),
            "the success cache is gone:\n{body}"
        );
        // Double-checked: `model_dir()` downloads when the cache is cold, so a
        // racing first touch must not fetch 132 MB twice.
        assert_eq!(
            body.matches("if let Some(m) = M.get()").count(),
            2,
            "the double check is gone — concurrent first touch fetches twice:\n{body}"
        );
    }
    #[test]
    fn cosine_self_is_near_one() {
        let v: Vec<f32> = (0..DIM).map(|i| (i as f32 + 1.0).sin()).collect();
        let n: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        let unit: Vec<f32> = v.iter().map(|x| x / n).collect();
        let c = cosine(&unit, &unit);
        assert!((c - 1.0).abs() < 1e-5, "self-cosine should be 1.0, got {c}");
    }

    #[test]
    fn cosine_mismatched_dims_yields_zero() {
        let c = cosine(&[1.0, 2.0, 3.0], &[1.0, 2.0]);
        assert_eq!(c, 0.0);
    }

    #[test]
    fn dim_is_256() {
        assert_eq!(dim(), 256);
    }

    #[test]
    fn embed_error_display_is_user_friendly() {
        let err = EmbedError::ModelInit("corrupt model bytes".into());
        let msg = format!("{err}");
        assert!(
            msg.contains("corrupt model bytes"),
            "must surface the cause: {msg}"
        );
        // The message must name the escape hatch and the files it expects,
        // because the only person who reads it is offline and stuck.
        assert!(
            msg.contains("TOKENADE_MODEL_DIR"),
            "must name the override: {msg}"
        );
        for f in crate::model_fetch::MODEL_FILES {
            assert!(msg.contains(f), "must list {f}: {msg}");
        }
    }

    /// Embed should always succeed for non-empty input *when the model is
    /// resolvable locally*. The model is no longer bundled (it's fetched
    /// once from R2 and cached — see `model_fetch`), so this test stays
    /// hermetic: it skips when no local model is present rather than firing
    /// a network download at `cargo test` time (which fails closed on a
    /// fresh/offline CI VM — exactly the false failure that broke the
    /// release build).
    #[test]
    fn bundled_model_embeds_arbitrary_text() {
        if crate::model_fetch::offline_model_dir().is_none() {
            eprintln!(
                "skipping bundled_model_embeds_arbitrary_text: no local potion-code-16M model"
            );
            return;
        }
        let v = embed_text("fn parse_args(argv: &[String])").unwrap();
        assert_eq!(v.len(), DIM);
        // Normalised vectors must have unit L2 norm.
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 0.01, "expected unit norm, got {norm}");
    }

    #[test]
    fn semantically_similar_code_clusters() {
        if crate::model_fetch::offline_model_dir().is_none() {
            eprintln!(
                "skipping semantically_similar_code_clusters: no local potion-code-16M model"
            );
            return;
        }
        let a = embed_text("fn parse_int(s: &str) -> Option<i64>").unwrap();
        let b = embed_text("fn parse_integer(input: String) -> Option<i64>").unwrap();
        let far = embed_text("HTTP request handler with TLS termination").unwrap();
        let near = cosine(&a, &b);
        let dist = cosine(&a, &far);
        assert!(
            near > dist,
            "synonym pair cosine {near} should beat unrelated {dist}"
        );
    }
}
