//! Rank 4 of the ladder: the tokenade LLM proxy.
//!
//! **NOT WIRED INTO THE PRODUCT, ON PURPOSE.** This module lives on the
//! `llm-proxy` branch and is not declared by `main.rs` on `main`. `tokenade
//! install` must never look for a proxy, start one, or write a base URL
//! anywhere. It ships when it is ready, not when it compiles.
//!
//! Why it exists at all: an agent that talks to an API rather than to a
//! subscription can be pointed at a local base URL, and that is the only
//! channel left for the users who will never approve a hook. Codex is the
//! concrete case — the hook approval step is a click most people never find,
//! and a ladder that requires it covers them on paper only.
//!
//! What it is NOT allowed to become:
//!
//! * a place that persists request bodies. It sees API keys and whole
//!   conversations. Nothing is written to disk beyond counters;
//! * something that can break a session. Any internal error relays the
//!   ORIGINAL bytes and gets out of the way — fail-open is not a nicety here,
//!   a proxy that 500s takes the user's agent down with it;
//! * a guesser. A body whose wire is not positively recognised is passed
//!   through untouched ([`wire::detect`] returns [`wire::Wire::Unknown`]).
//!
//! The pieces, smallest first: [`wire`] is a pure JSON transformation and where
//! the product value is; [`cache_gate`] decides whether a rewrite pays for
//! itself once the provider's cached prefix is invalidated. Both are testable
//! with no network, no HOME and no binary, which is why they are separate from
//! the transport.

pub(crate) mod agent_matrix;
pub(crate) mod authority;
pub(crate) mod autostart;
pub(crate) mod cache_gate;
pub(crate) mod cli;
pub(crate) mod config;
pub(crate) mod features;
pub mod hook_state;
pub(crate) mod http;
pub(crate) mod install;
pub(crate) mod protowire;
pub(crate) mod server;
pub(crate) mod sse;
pub(crate) mod wire;

/// One pass over an outgoing request body.
///
/// Returns `None` when the body must be relayed VERBATIM — unparseable,
/// unrecognised wire, or nothing worth folding. `None` is not a failure; on
/// this path it is the safe answer, and the caller must never re-serialise a
/// body just because it parsed.
pub(crate) fn compact_outgoing<F>(
    raw: &[u8],
    fold: F,
    preamble: Option<&str>,
) -> Option<(Vec<u8>, wire::Rewrite, wire::Wire)>
where
    F: FnMut(&str, Option<&str>) -> Option<String>,
{
    compact_outgoing_at(raw, "", fold, preamble)
}

/// [`compact_outgoing`] for a request sent to `target` (its path decides the
/// wire first — [`wire::detect_at`]).
pub(crate) fn compact_outgoing_at<F>(
    raw: &[u8],
    target: &str,
    fold: F,
    preamble: Option<&str>,
) -> Option<(Vec<u8>, wire::Rewrite, wire::Wire)>
where
    F: FnMut(&str, Option<&str>) -> Option<String>,
{
    compact_outgoing_settled(raw, target, fold, preamble, &|_| true)
}

/// [`compact_outgoing_at`] that leaves alone what the provider cached raw —
/// see [`wire::rewrite_tool_results_settled`].
pub(crate) fn compact_outgoing_settled<F>(
    raw: &[u8],
    target: &str,
    fold: F,
    preamble: Option<&str>,
    settled: &dyn Fn(u64) -> bool,
) -> Option<(Vec<u8>, wire::Rewrite, wire::Wire)>
where
    F: FnMut(&str, Option<&str>) -> Option<String>,
{
    let mut body: serde_json::Value = serde_json::from_slice(raw).ok()?;
    let w = wire::detect_at(&body, target);
    if w == wire::Wire::Unknown {
        return None;
    }
    let mut r = wire::rewrite_tool_results_settled(&mut body, w, fold, settled);
    r.model = body
        .get("model")
        .and_then(|m| m.as_str())
        .map(str::to_string);
    // The preamble is the one rewrite that is not a fold, and the one an agent
    // with no prompt-time hook cannot get any other way. Eleven agents on the
    // roster are in that state — their `sobriety` cell reads `Partial bin`
    // because `tokenade style` writes a preamble nothing reads at the moment
    // it would matter.
    //
    // It runs AFTER the folds so the length rule below sees the true final
    // size, and it sets `rewrote` so a body that gained only a preamble is
    // still sent instead of being discarded as a no-op.
    if let Some(text) = preamble {
        if wire::prepend_system_preamble(&mut body, w, text) {
            r.preambled = true;
        }
    }
    if r.is_noop() {
        return None;
    }
    let out = serde_json::to_vec(&body).ok()?;
    // Re-serialising can, in principle, produce something LARGER than what we
    // were handed (whitespace the sender omitted, escaping choices). Sending
    // more bytes than we received while claiming to compact would be the
    // ledger lie this project keeps finding, so refuse the swap.
    //
    // UNLESS something was redacted. `AWS_SECRET_ACCESS_KEY=wJalr…` becomes
    // `AWS_SECRET_ACCESS_KEY=<redacted>` and can be LONGER than what it
    // replaced, so a length-only rule throws the safe body away and hands the
    // provider the raw key. The openclaw bridge learned this and wrote it down;
    // this path had the same hole and no note.
    // A preamble makes the body LONGER on purpose. It is not a compaction and
    // it must not be judged by one: the same exemption the redaction path
    // needed, for the same reason — a length-only rule throws away the body we
    // meant to send.
    if out.len() >= raw.len() && !r.redacted && preamble.is_none() {
        return None;
    }
    Some((out, r, w))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn halve(s: &str, _hint: Option<&str>) -> Option<String> {
        (s.len() > 40).then(|| s[..s.len() / 2].to_string())
    }

    #[test]
    fn garbage_in_is_relayed_untouched() {
        assert!(compact_outgoing(b"not json at all", halve, None).is_none());
        assert!(compact_outgoing(b"", halve, None).is_none());
        // Valid JSON, unrecognised shape.
        assert!(compact_outgoing(br#"{"prompt":"hi"}"#, halve, None).is_none());
    }

    #[test]
    fn a_body_with_nothing_to_fold_keeps_its_original_bytes() {
        // The caller must be able to relay the bytes it received. Returning a
        // re-serialised "identical" body would reorder keys and change the
        // provider's cache key for no gain at all.
        let raw = br#"{"messages":[{"role":"tool","content":"short"}]}"#;
        assert!(compact_outgoing(raw, halve, None).is_none());
    }

    #[test]
    fn a_real_fold_comes_back_smaller() {
        let long = "x".repeat(400);
        let raw = format!(r#"{{"messages":[{{"role":"tool","content":"{long}"}}]}}"#);
        let (out, r, w) = compact_outgoing(raw.as_bytes(), halve, None).expect("folded");
        assert_eq!(r.folded, 1);
        assert_eq!(w, wire::Wire::OpenAiChat);
        assert!(out.len() < raw.len(), "{} vs {}", out.len(), raw.len());
    }
}

#[cfg(test)]
#[path = "live_agent_tests.rs"]
mod live_agent_tests;
