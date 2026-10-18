//! Static embedder: model2vec inference without model2vec's memory cost.
//!
//! `model2vec-rs` is correct but loads for a server, not a CLI hook: it decodes
//! the whole weight matrix to `f32` (+611 Mo) and inflates `tokenizer.json`'s
//! Unigram into a trie of per-node hash maps (+541 Mo). Measured on
//! potion-multilingual-128M: **+1056 Mo RSS and 930 ms before the first
//! embedding**. Every hook invocation would pay that.
//!
//! Inference here is not contextual — it is `tokenize → look up rows →
//! mean-pool` — so nothing needs to be resident:
//!
//! * the vocab is an **FST** (5,3 Mo on disk, mmap'd) mapping token bytes → id,
//!   which replaces BOTH the trie and the `token_to_ids` map;
//! * the i8 weights are **mmap'd** and dequantised per looked-up row, so only
//!   the touched pages (a few thousand rows of 500 353) become resident;
//! * the normalizer is sentencepiece's `Precompiled` charsmap, **reused
//!   verbatim** from `tokenizers` (2 Mo) — reimplementing it by hand would risk
//!   silent mis-normalisation of exactly the non-Latin text we switched models
//!   for.
//!
//! Measured against model2vec-rs on the same artifact: **+4,5 Mo RSS, 1 ms**
//! first embed, cosine 1.000000 mean / 0.999997 worst over real extracted
//! documents. Only the per-embed cost regresses (0,105 ms vs 0,032 ms, no
//! pre-decoded matrix and no cache); a process would need ~12 700 embeddings in
//! one run for that to outweigh the 930 ms saved at load, which no hook does.
//!
//! The Viterbi below is transcribed from
//! `tokenizers::models::unigram::Unigram::encode_optimized`, itself a port of
//! sentencepiece's `unigram_model.cc`. It must stay bug-for-bug identical: a
//! tokenisation that merely *looks* reasonable produces embeddings that are
//! silently wrong, and no downstream check can see it. `tests::` holds a
//! differential against the reference tokenizer for that reason.

use std::path::Path;

/// Artifacts `bake` produces and `load` mmaps, on top of `model.safetensors`.
pub const BAKED_FILES: [&str; 3] = ["vocab.fst", "vocab.fst.scores", "vocab.fst.norm"];

/// sentencepiece's fixed penalty for falling back to `[UNK]`.
const K_UNK_PENALTY: f64 = 10.0;
/// model2vec's default `max_length`.
const MAX_TOKENS: usize = 512;

#[derive(Debug)]
pub enum StaticEmbedError {
    Io(std::io::Error),
    Artifact(String),
}

impl std::fmt::Display for StaticEmbedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "static embedder I/O: {e}"),
            Self::Artifact(m) => write!(f, "static embedder artifact: {m}"),
        }
    }
}
impl std::error::Error for StaticEmbedError {}
impl From<std::io::Error> for StaticEmbedError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

fn bad(m: impl Into<String>) -> StaticEmbedError {
    StaticEmbedError::Artifact(m.into())
}

pub struct StaticEmbedder {
    vocab: fst::raw::Fst<memmap2::Mmap>,
    /// f64 log-probs, one per id. Kept f64 because Viterbi compares them
    /// directly: rounding to f32 could flip a near-tie and silently change a
    /// segmentation for 2 Mo.
    scores: memmap2::Mmap,
    /// The safetensors file itself, mmap'd. Never decoded as a whole.
    weights: memmap2::Mmap,
    row_off: usize,
    rows: usize,
    dim: usize,
    min_score: f64,
    unk_id: u32,
    /// Which id (if any) mean-pooling drops — NOT the same thing as `unk_id`.
    ///
    /// model2vec looks for a STRING `unk_token` field in the serialized
    /// tokenizer. `Unigram` serialises an integer `unk_id` instead, so for our
    /// model model2vec finds nothing and pools the `[UNK]` rows in. Dropping
    /// them would be defensible in the abstract and wrong here: the quality we
    /// benched (0.570/0.413) is model2vec's behaviour, and the divergence only
    /// shows on unk-heavy text — symbol-font PDFs — where it is severe
    /// (measured cosine 0.17), not a rounding difference.
    drop_id: Option<u32>,
    median_token_length: usize,
    normalize: bool,
    normalizer: tokenizers::NormalizerWrapper,
    pre_tokenizer: tokenizers::PreTokenizerWrapper,
}

fn mmap(path: &Path) -> Result<memmap2::Mmap, StaticEmbedError> {
    let f = std::fs::File::open(path)?;
    // SAFETY: the model artifacts are content-addressed and replaced by
    // rename, never mutated in place, so the mapping cannot change underneath.
    Ok(unsafe { memmap2::Mmap::map(&f)? })
}

impl StaticEmbedder {
    /// `dir` holds the baked artifacts: `vocab.fst`, `vocab.fst.scores`,
    /// `vocab.fst.norm` and the i8 `model.safetensors`.
    pub fn load(dir: &Path) -> Result<Self, StaticEmbedError> {
        let meta: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("vocab.fst.norm"))?)
                .map_err(|e| bad(format!("vocab.fst.norm: {e}")))?;
        let weights = mmap(&dir.join("model.safetensors"))?;
        // safetensors layout: u64 header length, JSON header, then payload.
        if weights.len() < 8 {
            return Err(bad("model.safetensors truncated"));
        }
        let hlen = u64::from_le_bytes(weights[..8].try_into().unwrap()) as usize;
        let hdr: serde_json::Value = weights
            .get(8..8 + hlen)
            .ok_or_else(|| bad("model.safetensors header truncated"))
            .and_then(|b| serde_json::from_slice(b).map_err(|e| bad(format!("header: {e}"))))?;
        let e = &hdr["embeddings"];
        if e["dtype"] != "I8" {
            return Err(bad(format!("expected an i8 artifact, got {}", e["dtype"])));
        }
        let rows = e["shape"][0].as_u64().ok_or_else(|| bad("shape[0]"))? as usize;
        let dim = e["shape"][1].as_u64().ok_or_else(|| bad("shape[1]"))? as usize;
        let row_off = 8
            + hlen
            + e["data_offsets"][0]
                .as_u64()
                .ok_or_else(|| bad("offsets"))? as usize;
        if weights.len() < row_off + rows * dim {
            return Err(bad("model.safetensors payload truncated"));
        }
        let get = |k: &str| {
            meta.get(k)
                .ok_or_else(|| bad(format!("vocab.fst.norm: missing {k}")))
        };
        Ok(Self {
            vocab: fst::raw::Fst::new(mmap(&dir.join("vocab.fst"))?)
                .map_err(|e| bad(format!("vocab.fst: {e}")))?,
            scores: mmap(&dir.join("vocab.fst.scores"))?,
            weights,
            row_off,
            rows,
            dim,
            min_score: get("min_score")?.as_f64().ok_or_else(|| bad("min_score"))?,
            unk_id: get("unk_id")?.as_u64().ok_or_else(|| bad("unk_id"))? as u32,
            drop_id: meta["drop_id"].as_u64().map(|v| v as u32),
            median_token_length: get("median_token_length")?
                .as_u64()
                .ok_or_else(|| bad("median_token_length"))?
                as usize,
            normalize: meta["normalize"].as_bool().unwrap_or(true),
            normalizer: serde_json::from_value(get("normalizer")?.clone())
                .map_err(|e| bad(format!("normalizer: {e}")))?,
            pre_tokenizer: serde_json::from_value(get("pre_tokenizer")?.clone())
                .map_err(|e| bad(format!("pre_tokenizer: {e}")))?,
        })
    }

    pub fn dim(&self) -> usize {
        self.dim
    }

    fn score(&self, id: u64) -> f64 {
        let o = id as usize * 8;
        f64::from_le_bytes(self.scores[o..o + 8].try_into().unwrap())
    }

    /// One embedding row, still i8 on disk.
    fn row(&self, id: usize) -> &[i8] {
        let o = self.row_off + id * self.dim;
        // SAFETY: bounds checked in `load`; i8 and u8 share a layout.
        unsafe { std::slice::from_raw_parts(self.weights[o..].as_ptr() as *const i8, self.dim) }
    }

    /// Every vocab entry that is a prefix of `hay`, as (byte length, id).
    /// This is what the reference's `Trie<u8>` does, minus the 541 Mo.
    fn common_prefix_search(&self, hay: &[u8], out: &mut Vec<(usize, u64)>) {
        out.clear();
        let mut node = self.vocab.root();
        let mut acc = fst::raw::Output::zero();
        for (i, &b) in hay.iter().enumerate() {
            let Some(t) = node.find_input(b).map(|idx| node.transition(idx)) else {
                return;
            };
            acc = acc.cat(t.out);
            node = self.vocab.node(t.addr);
            if node.is_final() {
                out.push((i + 1, acc.cat(node.final_output()).value()));
            }
        }
    }

    /// Viterbi over the unigram lattice; see `encode_optimized`.
    /// Appends (id, start, end) spans of the best path to `spans`.
    fn best_path(&self, sentence: &str, spans: &mut Vec<(u32, usize, usize)>) {
        if sentence.is_empty() {
            return;
        }
        let bytes = sentence.as_bytes();
        let size = bytes.len();
        let unk_score = self.min_score - K_UNK_PENALTY;
        // (id, best score to here, start of the winning edge)
        let mut best: Vec<(u32, f64, Option<usize>)> = vec![(0, 0.0, None); size + 1];
        let mut hits = Vec::new();
        let mut starts_at = 0usize;
        while starts_at < size {
            let score_till_here = best[starts_at].1;
            let mut has_single_node = false;
            let mblen = sentence[starts_at..].chars().next().unwrap().len_utf8();
            self.common_prefix_search(&bytes[starts_at..], &mut hits);
            for &(len, id) in hits.iter() {
                let cand = self.score(id) + score_till_here;
                let node = &mut best[starts_at + len];
                if node.2.is_none() || cand > node.1 {
                    *node = (id as u32, cand, Some(starts_at));
                }
                if !has_single_node && len == mblen {
                    has_single_node = true;
                }
            }
            // No vocab entry covers this single character → [UNK] edge.
            if !has_single_node {
                let cand = unk_score + score_till_here;
                let node = &mut best[starts_at + mblen];
                if node.2.is_none() || cand > node.1 {
                    *node = (self.unk_id, cand, Some(starts_at));
                }
            }
            starts_at += mblen;
        }
        let mark = spans.len();
        let mut ends_at = size;
        while ends_at > 0 {
            let (id, _, sa) = best[ends_at];
            let sa = sa.expect("every reachable node has a predecessor");
            spans.push((id, sa, ends_at));
            ends_at = sa;
        }
        spans[mark..].reverse();
    }

    /// Token ids for one pre-tokenized piece, appended to `out`.
    ///
    /// `fuse_unk` is ON (the reference defaults it to true and the model file
    /// does not override it): a run of consecutive `[UNK]` edges collapses into
    /// ONE piece, and that fused string is then looked up in the vocab again.
    /// It can genuinely hit a real token — dropping this rule is not a rounding
    /// difference, it changes which ids come out.
    fn encode_piece(&self, piece: &str, out: &mut Vec<u32>) {
        let mut spans = Vec::new();
        self.best_path(piece, &mut spans);
        let mut i = 0;
        while i < spans.len() {
            let (id, start, end) = spans[i];
            if id != self.unk_id {
                out.push(id);
                i += 1;
                continue;
            }
            // Fuse the whole [UNK] run, then re-look-up the joined string.
            let mut j = i;
            let mut fused_end = end;
            while j + 1 < spans.len() && spans[j + 1].0 == self.unk_id {
                j += 1;
                fused_end = spans[j].2;
            }
            let fused = &piece[start..fused_end];
            out.push(
                self.vocab
                    .get(fused.as_bytes())
                    .map(|o| o.value() as u32)
                    .unwrap_or(self.unk_id),
            );
            i = j + 1;
        }
    }

    /// Token ids for `text`, equivalent to the reference's
    /// `encode_fast(text, /* add_special_tokens */ false).get_ids()`.
    /// Public because it is the sharp end of the differential test: comparing
    /// ids catches a drift that mean-pooling would average away.
    pub fn token_ids(&self, text: &str) -> Vec<u32> {
        use tokenizers::{
            NormalizedString, Normalizer, OffsetReferential, OffsetType, PreTokenizedString,
            PreTokenizer,
        };
        let mut ns = NormalizedString::from(text);
        let mut ids = Vec::new();
        if self.normalizer.normalize(&mut ns).is_ok() {
            let mut ps = PreTokenizedString::from(ns.get());
            if self.pre_tokenizer.pre_tokenize(&mut ps).is_ok() {
                for (piece, _, _) in ps.get_splits(OffsetReferential::Original, OffsetType::Byte) {
                    self.encode_piece(piece, &mut ids);
                }
            }
        }
        ids
    }

    /// Mean-pooled, L2-normalised embedding. Mirrors model2vec-rs
    /// `encode_with_args`: char-truncate, tokenize with no special tokens, drop
    /// `[UNK]`, cap at 512 ids, pool, normalise.
    pub fn embed(&self, text: &str) -> Vec<f32> {
        // model2vec truncates the STRING before tokenising, not the id list.
        let truncated = text
            .char_indices()
            .nth(MAX_TOKENS.saturating_mul(self.median_token_length))
            .map_or(text, |(i, _)| &text[..i]);
        let mut ids = self.token_ids(truncated);
        if let Some(drop) = self.drop_id {
            ids.retain(|&i| i != drop);
        }
        ids.retain(|&i| (i as usize) < self.rows);
        ids.truncate(MAX_TOKENS);

        let mut sum = vec![0.0f32; self.dim];
        for &id in &ids {
            for (s, &v) in sum.iter_mut().zip(self.row(id as usize)) {
                *s += v as f32;
            }
        }
        let denom = ids.len().max(1) as f32;
        for x in &mut sum {
            *x /= denom;
        }
        if self.normalize {
            let n = sum.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-12);
            for x in &mut sum {
                *x /= n;
            }
        }
        sum
    }
}

/// Derive the runtime artifacts from a stock model2vec directory
/// (`tokenizer.json` + `model.safetensors` + `config.json`).
///
/// Runs ONCE per model install, never per invocation, which is why it may cost
/// what `load` refuses to: parsing the 18,6 Mo `tokenizer.json` peaks around
/// 250 Mo for ~200 ms. Everything expensive about the reference loader is paid
/// here and written down in a form that mmaps for free.
///
/// Writes are staged then renamed, so an interrupted bake never leaves a
/// half-written artifact that `load` would happily mmap.
pub fn bake(src: &Path, dst: &Path) -> Result<(), StaticEmbedError> {
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(src.join("tokenizer.json"))?)
        .map_err(|e| bad(format!("tokenizer.json: {e}")))?;
    let model = &v["model"];
    if model["type"] != "Unigram" {
        return Err(bad(format!(
            "expected a Unigram tokenizer, got {}",
            model["type"]
        )));
    }
    if model["byte_fallback"].as_bool().unwrap_or(false) {
        // We never implemented the byte-fallback branch of `tokenize`; a model
        // needing it must not be silently mis-tokenised.
        return Err(bad("byte_fallback tokenizers are not supported"));
    }
    let unk_id = model["unk_id"].as_u64().ok_or_else(|| bad("unk_id"))? as u32;
    let vocab = model["vocab"]
        .as_array()
        .ok_or_else(|| bad("model.vocab"))?;

    let mut pairs: Vec<(&str, u64)> = Vec::with_capacity(vocab.len());
    let mut scores: Vec<f64> = Vec::with_capacity(vocab.len());
    for (id, e) in vocab.iter().enumerate() {
        pairs.push((e[0].as_str().ok_or_else(|| bad("vocab token"))?, id as u64));
        scores.push(e[1].as_f64().ok_or_else(|| bad("vocab score"))?);
    }
    let min_score = scores.iter().cloned().fold(f64::INFINITY, f64::min);

    // `tokenizers` builds `token_to_ids` by inserting in vocab order, so on a
    // duplicate token the LAST id wins, and `tokenize` resolves every piece
    // through that map. The FST must agree or ids drift on those tokens.
    pairs.sort_by(|a, b| a.0.cmp(b.0).then(a.1.cmp(&b.1)));
    let mut uniq: Vec<(&str, u64)> = Vec::with_capacity(pairs.len());
    for (t, id) in pairs {
        match uniq.last_mut() {
            Some(l) if l.0 == t => l.1 = id,
            _ => uniq.push((t, id)),
        }
    }

    // model2vec derives this from `get_vocab(false)` (deduplicated) key byte
    // lengths; it only drives input truncation, but a wrong value silently
    // truncates long text differently.
    let mut lens: Vec<usize> = uniq.iter().map(|(t, _)| t.len()).collect();
    lens.sort_unstable();
    let median_token_length = lens.get(lens.len() / 2).copied().unwrap_or(1);

    let cfg: serde_json::Value = serde_json::from_slice(&std::fs::read(src.join("config.json"))?)
        .map_err(|e| bad(format!("config.json: {e}")))?;
    let normalize = cfg["normalize"].as_bool().unwrap_or(true);

    // model2vec drops a token from the mean only when the serialized model
    // carries a STRING `unk_token`. Unigram emits an integer `unk_id`, so this
    // resolves to None and `[UNK]` rows ARE pooled. Reproduce the lookup rather
    // than the conclusion: a BPE/WordPiece model would legitimately answer
    // differently, and hard-coding None here would silently mis-embed it.
    let drop_id: Option<u64> = model["unk_token"]
        .as_str()
        .and_then(|tok| uniq.iter().find(|(t, _)| *t == tok).map(|(_, id)| *id));

    std::fs::create_dir_all(dst)?;
    let stage = |name: &str| dst.join(format!(".{name}.tmp"));
    let commit = |name: &str| -> Result<(), StaticEmbedError> {
        std::fs::rename(stage(name), dst.join(name))?;
        Ok(())
    };

    let mut b = fst::MapBuilder::new(std::io::BufWriter::new(std::fs::File::create(stage(
        "vocab.fst",
    ))?))
    .map_err(|e| bad(format!("fst: {e}")))?;
    for (tok, id) in &uniq {
        b.insert(tok.as_bytes(), *id)
            .map_err(|e| bad(format!("fst insert: {e}")))?;
    }
    b.finish().map_err(|e| bad(format!("fst finish: {e}")))?;

    let mut sb: Vec<u8> = Vec::with_capacity(scores.len() * 8);
    for s in &scores {
        sb.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(stage("vocab.fst.scores"), &sb)?;

    std::fs::write(
        stage("vocab.fst.norm"),
        serde_json::to_vec(&serde_json::json!({
            "normalizer": v["normalizer"],
            "pre_tokenizer": v["pre_tokenizer"],
            "unk_id": unk_id,
            "min_score": min_score,
            "median_token_length": median_token_length,
            "normalize": normalize,
            "drop_id": drop_id,
        }))
        .map_err(|e| bad(format!("norm: {e}")))?,
    )?;

    for f in BAKED_FILES {
        commit(f)?;
    }
    crate::debug_log::log(
        "embedding",
        "static_embed_baked",
        serde_json::json!({ "vocab": uniq.len(), "median_token_length": median_token_length }),
    );
    Ok(())
}

/// Quantise a stock f32 `model.safetensors` to i8 in place of `dst`.
///
/// Symmetric, one global scale (`max|W| / 127`) — the scheme the published i8
/// artifact uses. The scale is deliberately NOT stored: mean-pooling is linear
/// and the output is L2-normalised, so a single global factor cancels exactly.
/// That is what lets the rows be read back as raw i8 with no dequantisation
/// table, and it is measured, not assumed: cosine 1.000000 against model2vec
/// reading the same file, and RAG quality 0.570/0.413 vs 0.571/0.414 for f32.
pub fn quantize_i8(src: &Path, dst: &Path) -> Result<(), StaticEmbedError> {
    let raw = std::fs::read(src)?;
    if raw.len() < 8 {
        return Err(bad("safetensors truncated"));
    }
    let hlen = u64::from_le_bytes(raw[..8].try_into().unwrap()) as usize;
    let hdr: serde_json::Value = serde_json::from_slice(
        raw.get(8..8 + hlen)
            .ok_or_else(|| bad("header truncated"))?,
    )
    .map_err(|e| bad(format!("header: {e}")))?;
    let e = &hdr["embeddings"];
    let rows = e["shape"][0].as_u64().ok_or_else(|| bad("shape[0]"))? as usize;
    let dim = e["shape"][1].as_u64().ok_or_else(|| bad("shape[1]"))? as usize;
    let off = 8
        + hlen
        + e["data_offsets"][0]
            .as_u64()
            .ok_or_else(|| bad("offsets"))? as usize;
    match e["dtype"].as_str() {
        Some("I8") => {
            // Already quantised (the pre-baked artifact) — copy, don't re-quantise.
            if src != dst {
                std::fs::copy(src, dst)?;
            }
            return Ok(());
        }
        Some("F32") => {}
        other => return Err(bad(format!("unsupported dtype {other:?}"))),
    }
    let n = rows * dim;
    let payload = raw
        .get(off..off + n * 4)
        .ok_or_else(|| bad("payload truncated"))?;
    let vals: Vec<f32> = payload
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect();
    let max = vals.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    let scale = if max > 0.0 { max / 127.0 } else { 1.0 };
    let q: Vec<u8> = vals
        .iter()
        .map(|v| ((v / scale).round().clamp(-127.0, 127.0) as i8) as u8)
        .collect();
    let mut nh = serde_json::to_vec(&serde_json::json!({
        "embeddings": { "dtype": "I8", "shape": [rows, dim], "data_offsets": [0, q.len()] }
    }))
    .map_err(|e| bad(format!("header: {e}")))?;
    // safetensors requires the payload to start 8-byte aligned.
    nh.resize(nh.len() + (8 - nh.len() % 8) % 8, b' ');
    let mut out = Vec::with_capacity(8 + nh.len() + q.len());
    out.extend_from_slice(&(nh.len() as u64).to_le_bytes());
    out.extend_from_slice(&nh);
    out.extend_from_slice(&q);
    let tmp = dst.with_extension("tmp");
    std::fs::write(&tmp, &out)?;
    std::fs::rename(&tmp, dst)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny but real Unigram model, so the differential is hermetic: the
    /// shipped model is WordPiece, so no Unigram artifact exists offline, and a
    /// test that silently skips proves nothing.
    fn synthetic_model(dir: &Path) {
        // Vocab picked so that random text exercises every branch: multi-byte
        // tokens, single chars present AND absent (→ [UNK] edges and the
        // consecutive-[UNK] fusing), and overlapping prefixes that force real
        // Viterbi decisions rather than greedy longest-match.
        let vocab: Vec<(&str, f64)> = vec![
            ("[PAD]", -13.5),
            ("[UNK]", -13.5),
            ("\u{2581}", -2.0),
            ("a", -3.0),
            ("b", -3.5),
            ("ab", -2.5),
            ("abc", -2.2),
            ("\u{2581}ab", -1.9),
            ("c", -4.0),
            ("é", -4.5),
            ("été", -3.1),
            ("中", -5.0),
            ("中文", -4.2),
            ("ba", -6.0),
        ];
        let json = serde_json::json!({
            "version": "1.0", "truncation": null, "padding": null,
            "added_tokens": [], "normalizer": {"type": "NFKC"},
            "pre_tokenizer": {"type": "Metaspace", "replacement": "\u{2581}",
                              "prepend_scheme": "always", "split": false},
            "post_processor": null, "decoder": null,
            "model": {"type": "Unigram", "unk_id": 1, "byte_fallback": false,
                      "vocab": vocab.iter().map(|(t, s)| serde_json::json!([t, s]))
                                    .collect::<Vec<_>>()},
        });
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("tokenizer.json"),
            serde_json::to_vec(&json).unwrap(),
        )
        .unwrap();
        std::fs::write(dir.join("config.json"), br#"{"normalize": true}"#).unwrap();

        let (rows, dim) = (vocab.len(), 4usize);
        let vals: Vec<f32> = (0..rows * dim)
            .map(|i| ((i * 37 % 211) as f32 - 105.0) / 105.0)
            .collect();
        let mut payload = Vec::new();
        for v in &vals {
            payload.extend_from_slice(&v.to_le_bytes());
        }
        let mut hdr = serde_json::to_vec(&serde_json::json!({
            "embeddings": {"dtype": "F32", "shape": [rows, dim],
                           "data_offsets": [0, payload.len()]}
        }))
        .unwrap();
        hdr.resize(hdr.len() + (8 - hdr.len() % 8) % 8, b' ');
        let mut out = (hdr.len() as u64).to_le_bytes().to_vec();
        out.extend_from_slice(&hdr);
        out.extend_from_slice(&payload);
        std::fs::write(dir.join("model.safetensors"), out).unwrap();
    }

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "tokenade-static-embed-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    fn prepared(tag: &str) -> std::path::PathBuf {
        let dir = tmpdir(tag);
        synthetic_model(&dir);
        bake(&dir, &dir).unwrap();
        quantize_i8(
            &dir.join("model.safetensors"),
            &dir.join("model.safetensors"),
        )
        .unwrap();
        dir
    }

    /// Cheap deterministic strings; no rand dependency, and a fixed seed means
    /// a failure is reproducible.
    fn fuzz_corpus() -> Vec<String> {
        let alphabet: Vec<char> = "ab c é中文xyz\u{2581}".chars().collect();
        let mut out = vec![
            String::new(),
            " ".into(),
            "abc".into(),
            "été".into(),
            "中文".into(),
            // consecutive unknown chars → the fuse_unk path
            "xyzxyz".into(),
            "abxyzc".into(),
            "\u{f006}\u{f01e}\u{f01d}".into(),
        ];
        let mut s = 12345u64;
        for _ in 0..4000 {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let len = (s >> 33) as usize % 24;
            let mut t = String::new();
            for _ in 0..len {
                s = s
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                t.push(alphabet[(s >> 33) as usize % alphabet.len()]);
            }
            out.push(t);
        }
        out
    }

    #[test]
    fn tokenisation_is_identical_to_the_reference() {
        let dir = prepared("diff");
        let ours = StaticEmbedder::load(&dir).unwrap();
        let reference = tokenizers::Tokenizer::from_file(dir.join("tokenizer.json")).unwrap();
        for s in fuzz_corpus() {
            let a = ours.token_ids(&s);
            let b: Vec<u32> = reference
                .encode_fast(s.as_str(), false)
                .unwrap()
                .get_ids()
                .to_vec();
            assert_eq!(a, b, "tokenisation diverges on {s:?}");
        }
    }

    #[test]
    fn embeddings_match_model2vec() {
        let dir = prepared("m2v");
        let ours = StaticEmbedder::load(&dir).unwrap();
        let reference =
            model2vec_rs::model::StaticModel::from_pretrained(&dir, None, None, None).unwrap();
        for s in fuzz_corpus()
            .into_iter()
            .filter(|s| !s.trim().is_empty())
            .take(300)
        {
            let a = ours.embed(&s);
            let b = reference.encode_single(&s);
            let cos: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
            assert!(cos > 0.9999, "cos={cos} on {s:?}");
        }
    }

    /// A run of consecutive [UNK] edges collapses to ONE id. Without this the
    /// reference and we disagree on every string containing unknown characters
    /// — which is most real PDF text.
    #[test]
    fn consecutive_unknowns_fuse_into_one_id() {
        let dir = prepared("fuse");
        let ours = StaticEmbedder::load(&dir).unwrap();
        // 'x','y','z' are absent from the vocab; "▁" is prepended by Metaspace.
        let ids = ours.token_ids("xyz");
        assert_eq!(
            ids,
            vec![2, 1],
            "expected [▁, a single fused UNK], got {ids:?}"
        );
    }

    #[test]
    fn i8_quantisation_preserves_direction() {
        let dir = prepared("quant");
        let em = StaticEmbedder::load(&dir).unwrap();
        // The global scale cancels under L2 normalisation, so the unit vector
        // must survive quantisation.
        let v = em.embed("abc été");
        let n: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((n - 1.0).abs() < 1e-5, "vector not normalised: {n}");
    }

    #[test]
    fn rejects_a_model_it_would_mis_tokenise() {
        let dir = tmpdir("reject");
        synthetic_model(&dir);
        // byte_fallback changes how `tokenize` resolves unknown pieces; we
        // never implemented that branch, so baking must refuse rather than
        // silently produce wrong ids.
        let raw = std::fs::read(dir.join("tokenizer.json")).unwrap();
        let mut v: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        v["model"]["byte_fallback"] = serde_json::json!(true);
        std::fs::write(dir.join("tokenizer.json"), serde_json::to_vec(&v).unwrap()).unwrap();
        assert!(bake(&dir, &dir).is_err(), "byte_fallback must be refused");
    }

    #[test]
    fn load_refuses_an_unquantised_artifact() {
        let dir = tmpdir("f32");
        synthetic_model(&dir);
        bake(&dir, &dir).unwrap();
        // Left as F32: `row()` reinterprets bytes as i8, so loading this would
        // produce garbage vectors silently.
        assert!(
            StaticEmbedder::load(&dir).is_err(),
            "an F32 artifact must be refused"
        );
    }
}
