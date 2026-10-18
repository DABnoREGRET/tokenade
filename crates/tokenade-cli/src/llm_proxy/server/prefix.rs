//! **Where does the already-cached prefix end?**
//!
//! Folding in the middle of a prefix the provider has cached costs more than
//! it saves: the cache is invalidated, and the whole call is paid again at
//! full price.
//!
//! The search is therefore done by hand rather than with a pattern library:
//! it works on bytes, without allocating, on a body that can weigh megabytes
//! and that we do not want to copy to answer a boundary question.

use super::*;

/// The proxy's own route, namespaced so it cannot shadow a provider path.
pub(crate) const HEALTH_ROUTE: &str = "/__tokenade/health";
/// Connecting is not thinking: a provider that has not accepted a TCP
/// connection in ten seconds is down, not slow.
pub(crate) const CONNECT_TIMEOUT_SECS: u64 = 10;
/// Substring search over bytes, so a multi-megabyte body is never turned into
/// a `String` just to ask one question of it.
///
/// First-byte scan rather than `windows().any()`: this runs on the hot path of
/// every turn that produced a fold candidate, and a request body is routinely
/// megabytes. `position` over a byte slice vectorises; a sliding window of
/// thirteen-byte comparisons does not.
/// Where the provider's cached prefix ENDS, in bytes of the raw request.
///
/// The gate's cost term is "the part of the cached prefix this rewrite
/// invalidates". It was `raw.len() - bytes_before_first_fold`: everything from
/// the fold point to the end of the REQUEST. That is only the same number when
/// the client caches all the way to the last byte.
///
/// It often does not. Anthropic's own guidance is to cache the tools and the
/// system prompt, and a client that does only that leaves the entire message
/// history uncached — so folding a tool result in the middle of it invalidates
/// NOTHING, while the shipped arithmetic charged a full cache write for the
/// whole tail. Measured 2026-08-26 on a 300 KB conversation with 8 KB of cached
/// system and a 20 KB tool result folding to 2 KB at the 100 KB mark: the gate
/// charged **50 000 tokens** of cache write and refused. The true cost is zero.
///
/// The reverse shape is real too and must keep refusing: a client that also
/// caches its last messages (Claude Code does) really does have a prefix
/// running to ~99 % of the request, and rewriting inside it really does
/// invalidate it. That case still scores 49 250 tokens and is still declined.
///
/// Located on the raw bytes for the same reason [`Server::prices_for`] is:
/// `cache_control` can sit on a system block, a tool definition or any message,
/// and a walk that missed one would price a cache that exists as one that does
/// not. From the LAST marker the prefix is extended forward to the start of the
/// next message (`"role"`), because the cached block runs to its own end and we
/// cannot see that boundary from here. Over-approximating by at most one
/// message is the safe direction: it can only make a rewrite look more
/// expensive than it is.
///
/// Returns `raw.len()` when no marker is present — with no cache the gate's
/// prices already carry `cache_write: 0.0`, so the cost term is zero whatever
/// this returns, and the conservative answer costs nothing.
pub(crate) fn cached_prefix_end(raw: &[u8]) -> usize {
    // The KEY, not the word: a tool result that mentions `cache_control`
    // (reading this very file) carries it escaped — `\"cache_control\"` — and
    // moved the boundary to wherever that text sat.
    let key = b"\"cache_control\"";
    let mut end = raw.len();
    let marker = loop {
        let Some(at) = rfind(&raw[..end], key) else {
            return raw.len();
        };
        if at == 0 || raw[at - 1] != b'\\' {
            break at;
        }
        end = at;
    };
    let after = marker + key.len();
    match find_from(raw, b"\"role\"", after) {
        Some(next_message) => next_message,
        None => raw.len(),
    }
}

/// Last occurrence of `needle`, byte-wise.
pub(crate) fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len())
        .rev()
        .find(|&i| &haystack[i..i + needle.len()] == needle)
}

/// First occurrence of `needle` at or after `from`, byte-wise.
pub(crate) fn find_from(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from >= haystack.len() || haystack.len() - from < needle.len() {
        return None;
    }
    (from..=haystack.len() - needle.len()).find(|&i| &haystack[i..i + needle.len()] == needle)
}

pub(crate) fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    let last_start = haystack.len() - needle.len();
    let mut i = 0;
    while i <= last_start {
        let Some(off) = haystack[i..=last_start]
            .iter()
            .position(|b| *b == needle[0])
        else {
            return false;
        };
        let s = i + off;
        if &haystack[s..s + needle.len()] == needle {
            return true;
        }
        i = s + 1;
    }
    false
}

/// Bytes per token, for the gate's arithmetic only.
///
/// The gate compares a saving against a cache write whose prices differ by an
/// order of magnitude, so a four-versus-three-and-a-half estimate never decides
/// anything. The real count comes from the provider (`sse::Usage`) and is what
/// gets recorded; this constant never reaches a ledger.
pub(crate) const BYTES_PER_TOKEN: usize = 4;

/// Relays the response as it arrives, and reads the bill off it in passing.
pub(crate) struct RelaySink<'a, W: Write> {
    pub(crate) out: &'a mut W,
    pub(crate) scanner: sse::UsageScanner,
    /// Set only for NON-streaming responses, where the count lives in the
    /// complete JSON body. A streamed response is never accumulated — that is
    /// the whole point of the scanner.
    pub(crate) buffered: Option<Vec<u8>>,
    pub(crate) status: u16,
    pub(crate) head_written: bool,
    /// The agent hung up. Stop relaying; do not treat it as a provider fault.
    pub(crate) broken: bool,
}

/// A non-streaming body we are willing to hold in order to read `usage` from
/// it. Past this we relay and decline to count, rather than grow without bound.
pub(crate) const MAX_BUFFERED_FOR_USAGE: usize = 8 * 1024 * 1024;

impl<W: Write> RelaySink<'_, W> {
    pub(crate) fn finish_usage(&self) -> Option<sse::Usage> {
        if let Some(b) = &self.buffered {
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(b) {
                if let Some(u) = sse::usage_from_json(&v) {
                    return Some(u);
                }
            }
        }
        // A count read off a stream that lost a line is short, and a short
        // count recorded as exact is the under-reporting this project keeps
        // finding. Decline instead.
        if self.scanner.truncated {
            return None;
        }
        self.scanner.finish()
    }
}

impl<W: Write> tokenade_core::net::StreamSink for RelaySink<'_, W> {
    fn head(&mut self, head: tokenade_core::net::StreamHead) -> bool {
        self.status = head.status;
        let headers: Vec<(String, String)> = head
            .headers
            .into_iter()
            .filter(|(k, _)| !http::drop_from_response(k))
            .collect();
        if http::write_head(self.out, self.status, &headers).is_err() {
            self.broken = true;
            return false;
        }
        self.head_written = true;
        true
    }

    fn chunk(&mut self, bytes: &[u8]) -> bool {
        // Relay FIRST, count second. Accounting must never sit between a token
        // and the screen.
        if http::write_chunk(self.out, bytes).is_err() {
            self.broken = true;
            return false;
        }
        match &mut self.buffered {
            Some(b) if b.len() + bytes.len() <= MAX_BUFFERED_FOR_USAGE => {
                b.extend_from_slice(bytes)
            }
            // Past the cap we drop the buffer entirely rather than keep a
            // truncated one: half a JSON body parses as nothing, and a
            // half-buffer that kept growing would be the memory bound we just
            // declined to have.
            Some(_) => self.buffered = None,
            None => self.scanner.feed(bytes),
        }
        true
    }
}

/// The gate's cost term must stop at the end of the CACHED prefix, not at the
/// end of the request. See [`cached_prefix_end`].
#[cfg(test)]
mod cached_prefix_boundary {
    use super::*;

    /// Anthropic's own guidance: cache the tools and the system prompt. The
    /// message history that follows is not cached.
    pub(crate) fn system_only_cache(history_blocks: usize) -> Vec<u8> {
        let mut b = String::from(
            r#"{"system":[{"type":"text","text":"you are helpful","cache_control":{"type":"ephemeral"}}],"messages":["#,
        );
        for i in 0..history_blocks {
            if i > 0 {
                b.push(',');
            }
            b.push_str(&format!(
                r#"{{"role":"user","content":"turn {i} carrying a long tool result {}"}}"#,
                "x".repeat(200)
            ));
        }
        b.push_str("]}");
        b.into_bytes()
    }

    /// Claude Code's shape: the tail is cached too.
    pub(crate) fn tail_cached(history_blocks: usize) -> Vec<u8> {
        let mut b = system_only_cache(history_blocks);
        // Drop the closing `]}` and append one last, cache_control'd message.
        b.truncate(b.len() - 2);
        b.extend_from_slice(
            br#",{"role":"user","content":[{"type":"text","text":"last","cache_control":{"type":"ephemeral"}}]}]}"#,
        );
        b
    }

    /// A tool result that QUOTES the key does not move the boundary.
    #[test]
    fn an_escaped_cache_control_in_a_tool_result_is_not_the_marker() {
        let raw = br#"{"system":[{"type":"text","text":"s","cache_control":{"type":"ephemeral"}}],"messages":[{"role":"user","content":"read: {\"cache_control\": 1}"}]}"#;
        let end = cached_prefix_end(raw);
        assert!(
            end < raw.len() / 2 + 40,
            "boundary moved to the quoted key: {end} of {}",
            raw.len()
        );
    }

    #[test]
    pub(crate) fn a_system_only_cache_ends_before_the_history() {
        let raw = system_only_cache(40);
        let end = cached_prefix_end(&raw);
        assert!(
            end < raw.len() / 4,
            "the cached prefix must stop near the system block, got {end} of {}",
            raw.len()
        );
    }

    #[test]
    pub(crate) fn a_tail_cached_request_is_cached_to_the_end() {
        let raw = tail_cached(40);
        assert_eq!(
            cached_prefix_end(&raw),
            raw.len(),
            "a marker in the last message leaves nothing uncached after it"
        );
    }

    /// No marker means no cache, and the conservative answer is free: the
    /// prices carry `cache_write: 0.0` on that path.
    #[test]
    pub(crate) fn no_marker_returns_the_whole_request() {
        let raw = br#"{"messages":[{"role":"user","content":"hi"}]}"#;
        assert_eq!(cached_prefix_end(raw), raw.len());
    }

    /// The wiring, end to end, on a Proxy that actually plans a relay.
    ///
    /// The three tests above pin the boundary function and the arithmetic
    /// separately, and BOTH stay green if the two are never connected — a
    /// mutation reverting the call site to `raw.len()` passed all three. Only
    /// this one fails, so it is the guard that matters.
    #[test]
    pub(crate) fn a_fold_past_the_cached_prefix_is_relayed_folded() {
        let mut raw = String::from(
            r#"{"model":"claude-opus-5","system":[{"type":"text","text":"you are helpful","cache_control":{"type":"ephemeral"}}],"messages":["#,
        );
        for i in 0..60 {
            if i > 0 {
                raw.push(',');
            }
            raw.push_str(&format!(
                r#"{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"t{i}","content":"{}"}}]}}"#,
                "result line that folds ".repeat(60)
            ));
        }
        raw.push_str("]}");

        let proxy = Proxy::new(
            Config {
                compaction: true,
                prices: cache_gate::Prices {
                    input: 1.0,
                    cache_read: 0.1,
                    cache_write: 1.25,
                },
                ..Config::default()
            },
            std::sync::Arc::new(|s: &str, _h: Option<&str>| {
                (s.len() > 40).then(|| s[..s.len() / 8].to_string())
            }),
            Box::new(NoAccounting),
        );
        let req = http::Request {
            method: "POST".into(),
            target: "/v1/messages".into(),
            headers: vec![("content-type".into(), "application/json".into())],
            body: raw.into_bytes(),
        };
        let out = proxy.plan_outgoing(&req);
        assert!(
            out.rewrite.is_some(),
            "the history after a system-only cache is not cached, so the fold \
             costs nothing and must be relayed. Refused because: {:?}",
            out.relayed_because
        );
    }
}
