//! Anthropic prompt-cache hint emission.
//!
//! Claude's prompt cache (https://docs.anthropic.com/en/docs/prompt-caching)
//! gives a ~90 % cost reduction when a turn's prefix is byte-identical to an
//! earlier cached prefix. STS compactors already produce shorter outputs;
//! this module wraps them with two guarantees that make the cache *hit* on
//! re-runs:
//!
//! 1. **Deterministic ordering.** Compactor outputs that derive from a hash
//!    map (JSON keys, YAML keys, log groups) get a stable sort so identical
//!    input produces byte-identical output — the precondition for any cache
//!    hit at all. Most STS compactors already do this; this module adds a
//!    fingerprint check helper so we can *verify* idempotence in tests.
//!
//! 2. **Cache-key marker prefix.** Each STS-compacted output is prefixed
//!    with `[tokenade-cache:k=<fnv16hex>]` — a fingerprint of (compactor name +
//!    canonical input). Agents that pass this prefix into Claude's prompt
//!    cache (via `cache_control`) get hit-amplification on repeated calls
//!    *within the same compactor*. Agents that ignore it lose nothing — it's
//!    just a header line.
//!
//! ## Written, tested, and not wired — same status as `query_parser`
//!
//! Verified 2026-08-28 by the compiler, not by grep: commenting out
//! `pub mod cachekey;` and running `cargo check --workspace --all-targets`
//! produces ZERO errors, so nothing in the product, the tests or the benches
//! calls any of this. It is kept for the same reason `query_parser` was kept
//! when five of its neighbours were deleted — the capability is real and the
//! code is written and tested, and deleting that is value git does not give
//! back cheaply.
//!
//! What it is NOT is adopted, and that sentence is the whole point of this
//! paragraph: a module whose header claims a use it does not have costs a
//! reader a full investigation. `cmd::locale` opened with "shared by every
//! per-command compactor" while having no caller at all, and that cost exactly
//! that; it was deleted the same day. Wiring this one in is a feature with its
//! own cycle — a caller that plumbs the prefix into `cache_control`, and a
//! decision about what happens when it does not.
//!
//! ## What this is NOT
//!
//! It is not itself the Anthropic prompt cache — that lives in the API.
//! This module produces hints the caller plumbs through. It also doesn't
//! mutate any existing compactor's content — only wraps the output. So a
//! v3 caller upgrading to v4 sees the same body with a header line added.

use crate::cache::hash;

/// Build a deterministic 16-hex-char fingerprint of `(op, input)`. Fits in
/// one short identifier so it doesn't bloat the prefix.
pub fn fingerprint(op: &str, input: &str) -> String {
    let mut h = String::with_capacity(op.len() + 1 + input.len());
    h.push_str(op);
    h.push('|');
    h.push_str(input);
    format!("{:016x}", hash(h.as_bytes()))
}

/// How long is the output expected to stay valid? Maps to how the agent
/// should place Anthropic-style `cache_control` breakpoints around it.
///
/// Inspired by the three-content-type heuristic surfaced in our analysis
/// of `flightlesstux/prompt-caching`:
/// system prompts, tool definitions, and large user messages each have
/// different stability profiles. We expose the equivalent classification
/// for STS-produced content so any caller that does its own request
/// construction can place breakpoints correctly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheClass {
    /// Output is byte-stable for the whole session (e.g. structure map of
    /// an unmodified repo, tool schemas). Aggressive caching pays off.
    Stable,
    /// Output is likely to be re-emitted within the next few turns (e.g.
    /// the same `git status` compacted twice). Cache with a short TTL.
    Warm,
    /// Output is single-use or query-specific (e.g. a semantic search
    /// result for a unique query). Caching is unlikely to amortise the
    /// 1.25× cache-write cost.
    Ephemeral,
}

impl CacheClass {
    pub fn as_str(self) -> &'static str {
        match self {
            CacheClass::Stable => "stable",
            CacheClass::Warm => "warm",
            CacheClass::Ephemeral => "ephemeral",
        }
    }
}

/// Classify a Tokenade operation by stability. Heuristic, not exhaustive:
/// any unrecognised op defaults to `Warm` (cheap to cache, safe default).
///
/// Stable ops we know about today:
///   - `structure_map` / `skeleton`           — change only when code changes
///   - `filter:tool_defs` (future)            — once-per-session
///
/// Ephemeral ops:
///   - `semantic_search`, `symbol_find`       — query-keyed
///   - `exec_compact` / `exec_script`         — output depends on live state
///
/// Warm (everything else): per-format compactors, json/yaml/diff/etc. Often
/// re-run on the same artifact in close succession.
pub fn classify(op: &str) -> CacheClass {
    if op.starts_with("structure_map") || op.starts_with("skeleton") {
        CacheClass::Stable
    } else if op.starts_with("semantic_search")
        || op.starts_with("symbol_find")
        || op.starts_with("exec")
    {
        CacheClass::Ephemeral
    } else {
        CacheClass::Warm
    }
}

/// Wrap a compactor's output with a cache-hint marker IF the user has
/// opted into prompt-cache amplification via `TOKENADE_CACHE_HINTS=1`. Default
/// off so existing test fixtures and downstream parsers don't see the new
/// header line unless the user asks for it.
pub fn wrap_if_enabled(op: &str, input: &str, output: &str) -> String {
    if crate::env_compat::is_truthy("TOKENADE_CACHE_HINTS", "STS_CACHE_HINTS") {
        let wrapped = wrap(op, input, output);
        // Anti-inflation guard (mirrors filter.rs SAFETY NET 3). The cache
        // marker is pure overhead UNLESS this prefix actually hits the prompt
        // cache on a later run. If prepending it pushes the result past the
        // RAW input size, the marker isn't worth it on this call — emit the
        // unwrapped (already-compacted) body, which is still a net win.
        // Tokenade must never ship output larger than the raw bytes.
        if crate::tokenizer::estimate(&wrapped) >= crate::tokenizer::estimate(input) {
            output.to_string()
        } else {
            wrapped
        }
    } else {
        output.to_string()
    }
}

/// Wrap a compactor's output with a cache-hint marker.
///
/// The marker is a single line, distinguishable, and only carries the
/// fingerprint — no payload. Idempotent: wrapping an already-wrapped output
/// is a no-op.
pub fn wrap(op: &str, input: &str, output: &str) -> String {
    if output.starts_with("[tokenade-cache:k=") {
        return output.to_string();
    }
    format!("[tokenade-cache:k={}]\n{}", fingerprint(op, input), output)
}

/// Returns `(op, fingerprint)` if `output` starts with the marker.
pub fn parse_marker(output: &str) -> Option<(String, String)> {
    let first = output.lines().next()?;
    let stripped = first
        .strip_prefix("[tokenade-cache:k=")?
        .strip_suffix(']')?;
    Some((String::new(), stripped.to_string()))
}

/// Verify that running the SAME compactor on the SAME input twice yields
/// byte-identical output. Used in adversarial tests as a precondition for
/// prompt-cache hits. `compactor` takes the raw input and returns the
/// compacted body.
pub fn assert_idempotent<F: Fn(&str) -> String>(compactor: F, input: &str) -> bool {
    let a = compactor(input);
    let b = compactor(input);
    a == b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_stable_and_distinct() {
        let a = fingerprint("json", "{\"a\":1}");
        let b = fingerprint("json", "{\"a\":1}");
        let c = fingerprint("json", "{\"a\":2}");
        let d = fingerprint("yaml", "{\"a\":1}");
        assert_eq!(a, b, "same op+input → same fingerprint");
        assert_ne!(a, c, "different input → different fingerprint");
        assert_ne!(a, d, "different op → different fingerprint");
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn wrap_adds_marker_once() {
        let w1 = wrap("test", "hello", "world");
        let w2 = wrap("test", "hello", &w1);
        assert!(w1.starts_with("[tokenade-cache:k="));
        assert!(w1.contains("\nworld"));
        // Idempotent wrap — no double marker.
        assert_eq!(w1, w2);
    }

    #[test]
    fn parse_recovers_fingerprint() {
        let w = wrap("op", "in", "out");
        let (_, fp) = parse_marker(&w).expect("marker present");
        assert_eq!(fp.len(), 16);
        assert!(fp.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn parse_returns_none_on_plain_output() {
        assert!(parse_marker("just some text").is_none());
        assert!(parse_marker("").is_none());
    }

    /// `TOKENADE_CACHE_HINTS` is PROCESS-global, so the two tests that toggle it
    /// were racing: one removed the var between the other's `remove_var` and its
    /// assertion, and the "disabled" case came back carrying the marker.
    /// MEASURED 2026-08-04: 1 failure in 6 full runs of `tokenade-core --lib`.
    ///
    /// The old note here said to fall back to `--test-threads=1` if this ever
    /// went flaky. It did; serialising the whole 6 691-test suite to protect two
    /// of them is the wrong trade. Same module-local guard `net.rs` and
    /// `debug_log.rs` already use for their own env vars.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static L: std::sync::Mutex<()> = std::sync::Mutex::new(());
        L.lock().unwrap_or_else(|p| p.into_inner())
    }

    #[test]
    fn wrap_if_enabled_respects_env_var() {
        // SAFETY: test sets process-wide env under `env_lock`; restored after.
        // Use a realistic raw input large enough that the ~30-char marker does
        // NOT push the wrapped result past the raw bytes (otherwise the
        // anti-inflation guard correctly suppresses the marker — see
        // `wrap_if_enabled_suppresses_marker_when_it_would_inflate`).
        // NOTE: must be VARIED text, not a single repeated char — the token
        // estimator scores long runs of one byte very low, so `"x".repeat(N)`
        // estimates near-zero tokens and trips the anti-inflation guard even
        // for large N. Real compactor inputs are varied prose/code.
        let raw = "fn process(input: &str) -> Result<Vec<Token>, Error> { let mut out = Vec::new(); for line in input.lines() { out.push(Token::parse(line)?); } Ok(out) }\n".repeat(4);
        let compacted = "fn process -> Result<Vec<Token>, Error>".to_string();
        let _g = env_lock();
        let prev = std::env::var("TOKENADE_CACHE_HINTS").ok();
        std::env::set_var("TOKENADE_CACHE_HINTS", "1");
        let on = wrap_if_enabled("op", &raw, &compacted);
        std::env::remove_var("TOKENADE_CACHE_HINTS");
        let off = wrap_if_enabled("op", &raw, &compacted);
        assert!(on.starts_with("[tokenade-cache:k="));
        assert_eq!(off, compacted);
        if let Some(v) = prev {
            std::env::set_var("TOKENADE_CACHE_HINTS", v);
        }
    }

    #[test]
    fn wrap_if_enabled_suppresses_marker_when_it_would_inflate() {
        // Anti-inflation guard: when the raw input is so small that prepending
        // the cache marker would make the result LARGER than the raw bytes,
        // emit the unwrapped (already-compacted) body — Tokenade never ships
        // output bigger than the raw input. Mirrors filter.rs SAFETY NET 3.
        let _g = env_lock();
        let prev = std::env::var("TOKENADE_CACHE_HINTS").ok();
        std::env::set_var("TOKENADE_CACHE_HINTS", "1");
        let out = wrap_if_enabled("op", "in", "out");
        std::env::remove_var("TOKENADE_CACHE_HINTS");
        assert_eq!(out, "out", "marker must be suppressed on tiny input");
        if let Some(v) = prev {
            std::env::set_var("TOKENADE_CACHE_HINTS", v);
        }
    }

    #[test]
    fn idempotent_helper_catches_non_determinism() {
        // A deterministic compactor passes.
        assert!(assert_idempotent(|s| s.to_uppercase(), "hello"));
        // A non-deterministic one (depends on outside state) fails — emulate
        // by inserting wall-clock nanos.
        use std::sync::atomic::{AtomicU64, Ordering};
        let counter = AtomicU64::new(0);
        assert!(!assert_idempotent(
            |_| counter.fetch_add(1, Ordering::SeqCst).to_string(),
            "x"
        ));
    }
}
