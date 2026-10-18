//! Reading the bill off a stream we are not allowed to hold.
//!
//! The requirement: "SSE end to end, without buffering the whole response (otherwise the
//! agent looks frozen). The count comes from the final `usage` event." Those two
//! sentences are in tension and this module is where the tension is resolved:
//! the count arrives in the LAST event, but we must have already forwarded
//! every earlier one. So the scanner is incremental and keeps a bounded window
//! — one partial line — never the stream.
//!
//! FOUR provider shapes put the same four numbers in four different places, and
//! two of them split them across two events. Note that the count is read from
//! more wires than the proxy can REWRITE: Google's `:generateContent` is relayed
//! untouched and still billed, and reading it is the only thing rank 4 offers
//! Gemini CLI. Getting this wrong is not a crash:
//! it is a dashboard that quietly under-reports, which is the one direction
//! this project has decided is never acceptable (an over-credited gain is a
//! lie; an under-credited one is merely a loss).

use serde_json::Value;

/// What a turn actually cost, as the PROVIDER billed it.
///
/// The reason rank 4 is worth building at all: every other rung estimates token
/// counts from bytes. This one is told.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    /// `input` already counts `cache_read` — OpenAI's `prompt_tokens` /
    /// `input_tokens` and Gemini's `promptTokenCount` do; Anthropic's
    /// `input_tokens` is the uncached part only.
    pub cache_in_input: bool,
}

impl Usage {
    /// The context this call sent, whatever the wire's convention.
    pub fn context(&self) -> u64 {
        if self.cache_in_input {
            self.input + self.cache_write
        } else {
            self.input + self.cache_read + self.cache_write
        }
    }

    fn is_empty(&self) -> bool {
        *self == Usage::default()
    }

    /// Take the larger of each field.
    ///
    /// Merge rather than overwrite, because Anthropic reports `output_tokens:
    /// 1` in `message_start` and the real figure in `message_delta`, while the
    /// input figures appear only in the first of the two. Last-writer-wins
    /// would report an output of 1 on every streamed Anthropic turn.
    fn merge(&mut self, o: Usage) {
        self.input = self.input.max(o.input);
        self.output = self.output.max(o.output);
        self.cache_read = self.cache_read.max(o.cache_read);
        self.cache_write = self.cache_write.max(o.cache_write);
        self.cache_in_input |= o.cache_in_input;
    }
}

/// Pull the four numbers out of whatever shape this provider chose.
///
/// Handles a `usage` object from any of the three wires. Unknown keys are
/// ignored rather than guessed at: a zero we did not measure is honest, an
/// invented number is not.
fn from_usage_object(u: &Value) -> Usage {
    let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
    let nested = |outer: &str| {
        u.get(outer)
            .and_then(|d| d.get("cached_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    Usage {
        // `input_tokens`: Anthropic and the Responses API. `prompt_tokens`: the
        // OpenAI chat wire.
        input: n("input_tokens").max(n("prompt_tokens")),
        output: n("output_tokens").max(n("completion_tokens")),
        // Anthropic bills cache reads as their own line; OpenAI reports them as
        // a DETAIL of the prompt tokens, which means on that wire `cache_read`
        // is a subset of `input` and must not be added to it.
        cache_read: n("cache_read_input_tokens")
            .max(nested("prompt_tokens_details"))
            .max(nested("input_tokens_details")),
        // Only Anthropic charges separately for writing the cache — which is
        // exactly what `cache_gate` is weighing.
        cache_write: n("cache_creation_input_tokens"),
        cache_in_input: u.get("prompt_tokens").is_some() || u.get("input_tokens_details").is_some(),
    }
}

/// Read Google's `usageMetadata`, which shares no field name with the others.
///
/// Why it is here at all: `agent_matrix` classes Gemini CLI as
/// "AccountOnlyForeignWire" — we cannot rewrite `:generateContent`, so the only
/// thing rank 4 offers that agent is the exact cost. Until this existed, that
/// promise delivered NOTHING: the wire was relayed untouched and the usage went
/// unread, so a Gemini user got a proxy that did nothing at all while the role
/// line said "run it for the cost figures".
///
/// Field names measured 2026-08-15 in the shipped `@google/gemini-cli` bundle:
/// `usageMetadata` ×193, `totalTokenCount` ×84, `promptTokenCount` ×62,
/// `cachedContentTokenCount` ×59, `candidatesTokenCount` ×35,
/// `thoughtsTokenCount` ×32.
fn from_gemini_metadata(u: &Value) -> Usage {
    let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
    Usage {
        input: n("promptTokenCount"),
        // Thinking tokens are billed as output and are NOT included in
        // `candidatesTokenCount` — `totalTokenCount` is the sum of all three.
        // Reporting candidates alone would under-count every reasoning turn,
        // which on this model family is most of them.
        output: n("candidatesTokenCount") + n("thoughtsTokenCount"),
        cache_read: n("cachedContentTokenCount"),
        // Google bills cache CREATION on a separate call, so nothing in this
        // object corresponds to it. A zero we did not measure beats a number
        // we inferred.
        cache_write: 0,
        cache_in_input: true,
    }
}

/// Find a usage object anywhere it is known to hide in one event or one
/// buffered body, and read it.
pub fn usage_from_json(v: &Value) -> Option<Usage> {
    // Anthropic `message_start` nests it under `message`; the Responses API
    // nests it under `response`; everything else puts it at the top level.
    let candidates = [
        v.get("usage"),
        v.get("message").and_then(|m| m.get("usage")),
        v.get("response").and_then(|r| r.get("usage")),
    ];
    let mut out = Usage::default();
    for c in candidates.into_iter().flatten() {
        out.merge(from_usage_object(c));
    }
    // Google's shape, whose field names overlap with none of the above and so
    // needs its own reader rather than another entry in `candidates`.
    for g in [
        v.get("usageMetadata"),
        v.get("response").and_then(|r| r.get("usageMetadata")),
    ]
    .into_iter()
    .flatten()
    {
        out.merge(from_gemini_metadata(g));
    }
    (!out.is_empty()).then_some(out)
}

/// The longest single SSE line we will assemble before giving up on ACCOUNTING
/// for it.
///
/// A `data:` line carrying a whole tool-call argument set can be large; four
/// megabytes is far past any real one. Past the cap we stop buffering and skip
/// to the next line — the bytes are still relayed by the caller, we simply
/// decline to hold them in order to read a number.
const MAX_LINE: usize = 4 * 1024 * 1024;

/// Incremental reader over a response body, extracting [`Usage`] as it flies past.
///
/// Deliberately owns NO copy of the stream. [`Self::feed`] is called with each
/// chunk after that chunk has already been written to the agent, so a slow or
/// hostile scanner can never delay a token reaching the screen.
#[derive(Default)]
pub struct UsageScanner {
    /// The tail of the last chunk, up to the first unfinished newline. This is
    /// the only thing retained, and [`MAX_LINE`] bounds it.
    partial: Vec<u8>,
    /// Set while discarding an over-long line.
    skipping: bool,
    seen: Usage,
    /// True when at least one line was dropped for length. Surfaced so a caller
    /// can decline to record a count it knows is short, rather than record a
    /// short one as if it were exact.
    pub truncated: bool,
}

impl UsageScanner {
    /// Feed the next chunk of body bytes.
    pub fn feed(&mut self, chunk: &[u8]) {
        for &b in chunk {
            if b == b'\n' {
                if !self.skipping {
                    let line = std::mem::take(&mut self.partial);
                    self.line(&line);
                }
                self.partial.clear();
                self.skipping = false;
                continue;
            }
            if self.skipping {
                continue;
            }
            if self.partial.len() >= MAX_LINE {
                self.partial.clear();
                self.skipping = true;
                self.truncated = true;
                continue;
            }
            self.partial.push(b);
        }
    }

    fn line(&mut self, line: &[u8]) {
        // SSE field syntax: `data: <payload>`. `event:` lines name the type,
        // which we do not need — the payload names itself on all three wires,
        // and keying off `event:` would mean carrying state between lines for
        // no gain.
        let Some(rest) = line.strip_prefix(b"data:") else {
            return;
        };
        let payload = rest.strip_prefix(b" ").unwrap_or(rest);
        let payload = payload.strip_suffix(b"\r").unwrap_or(payload);
        if payload == b"[DONE]" {
            return;
        }
        let Ok(v) = serde_json::from_slice::<Value>(payload) else {
            return;
        };
        if let Some(u) = usage_from_json(&v) {
            self.seen.merge(u);
        }
    }

    /// The count so far. `None` when the stream carried none — which happens
    /// for real (an OpenAI stream without `stream_options.include_usage`), and
    /// must be reported as "not measured" rather than as a zero-token turn.
    pub fn finish(&self) -> Option<Usage> {
        (!self.seen.is_empty()).then_some(self.seen)
    }

    /// Bytes currently retained. Exists for the test that proves this scanner
    /// does not become a buffer of the whole response.
    #[cfg(test)]
    fn retained(&self) -> usize {
        self.partial.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_needs_both_events_and_the_second_must_not_erase_the_first() {
        // The defect this pins: `message_start` says output=1, `message_delta`
        // says output=250 and carries no input at all. Overwrite loses the
        // input; last-wins on output would report 1. Both are silent.
        let mut s = UsageScanner::default();
        s.feed(b"event: message_start\n");
        s.feed(br#"data: {"type":"message_start","message":{"usage":{"input_tokens":1200,"cache_read_input_tokens":900,"cache_creation_input_tokens":300,"output_tokens":1}}}"#);
        s.feed(b"\n\n");
        s.feed(b"event: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":250}}\n\n");
        assert_eq!(
            s.finish(),
            Some(Usage {
                input: 1200,
                output: 250,
                cache_read: 900,
                cache_write: 300,
                cache_in_input: false
            })
        );
    }

    #[test]
    fn the_merge_does_not_depend_on_the_order_the_events_arrive_in() {
        // Why `merge` takes a max on every field rather than overwriting.
        //
        // Found by mutation: replacing the `output` max with a plain assignment
        // left every other test in this file green, because Anthropic happens
        // to send its small figure first. Relying on that is relying on a
        // provider's emission order, which is neither documented nor ours —
        // and a scanner that silently reports `output: 1` because two events
        // swapped is the under-reporting this project has decided never to
        // ship. So the property is pinned instead of the scenario.
        let start = br#"data: {"message":{"usage":{"input_tokens":1200,"output_tokens":1}}}"#;
        let delta = br#"data: {"usage":{"output_tokens":250}}"#;

        let mut forward = UsageScanner::default();
        forward.feed(start);
        forward.feed(b"\n");
        forward.feed(delta);
        forward.feed(b"\n");

        let mut backward = UsageScanner::default();
        backward.feed(delta);
        backward.feed(b"\n");
        backward.feed(start);
        backward.feed(b"\n");

        assert_eq!(forward.finish(), backward.finish());
        assert_eq!(
            forward.finish().map(|u| (u.input, u.output)),
            Some((1200, 250))
        );
    }

    #[test]
    fn the_openai_chat_wire_reports_cached_tokens_as_a_detail() {
        let mut s = UsageScanner::default();
        s.feed(br#"data: {"choices":[],"usage":{"prompt_tokens":800,"completion_tokens":40,"prompt_tokens_details":{"cached_tokens":640}}}"#);
        s.feed(b"\ndata: [DONE]\n\n");
        assert_eq!(
            s.finish(),
            Some(Usage {
                input: 800,
                output: 40,
                cache_read: 640,
                cache_write: 0,
                cache_in_input: true
            })
        );
    }

    #[test]
    fn the_responses_wire_nests_usage_under_response() {
        let mut s = UsageScanner::default();
        s.feed(br#"data: {"type":"response.completed","response":{"usage":{"input_tokens":10,"output_tokens":3,"input_tokens_details":{"cached_tokens":8}}}}"#);
        s.feed(b"\n\n");
        assert_eq!(
            s.finish(),
            Some(Usage {
                input: 10,
                output: 3,
                cache_read: 8,
                cache_write: 0,
                cache_in_input: true
            })
        );
    }

    #[test]
    fn an_event_split_across_chunks_is_still_read() {
        // The whole reason this is a scanner and not a `body.split('\n')`: TCP
        // decides where the chunks land, not the provider.
        let whole = br#"data: {"usage":{"input_tokens":42,"output_tokens":7}}"#;
        for cut in 1..whole.len() {
            let mut s = UsageScanner::default();
            s.feed(&whole[..cut]);
            s.feed(&whole[cut..]);
            s.feed(b"\n");
            assert_eq!(
                s.finish().map(|u| (u.input, u.output)),
                Some((42, 7)),
                "split at {cut}"
            );
        }
    }

    #[test]
    fn the_gemini_wire_is_counted_even_though_it_is_never_rewritten() {
        // The whole value rank 4 has for Gemini CLI. Its `:generateContent` is
        // relayed untouched — correct, we do not guess at a wire we cannot
        // read — so if the usage went unread too, the proxy would do literally
        // nothing for that agent while the matrix said "run it for the numbers".
        let mut s = UsageScanner::default();
        s.feed(br#"data: {"candidates":[],"usageMetadata":{"promptTokenCount":1500,"candidatesTokenCount":200,"thoughtsTokenCount":800,"cachedContentTokenCount":1200,"totalTokenCount":2500}}"#);
        s.feed(b"\n\n");
        assert_eq!(
            s.finish(),
            Some(Usage {
                input: 1500,
                // Thinking tokens are billed as output and sit OUTSIDE
                // `candidatesTokenCount`; counting candidates alone would
                // under-report every reasoning turn by 800 here.
                output: 1000,
                cache_read: 1200,
                cache_write: 0,
                cache_in_input: true
            })
        );
    }

    #[test]
    fn gemini_field_names_do_not_leak_into_the_other_wires() {
        // The three readers share a `Usage` and must not share fields. An
        // OpenAI body carrying no `usageMetadata` must be unaffected, and a
        // Gemini body carrying no `usage` must not fall through to zero.
        let openai = serde_json::json!({"usage":{"prompt_tokens":10,"completion_tokens":2}});
        assert_eq!(
            usage_from_json(&openai).map(|u| (u.input, u.output)),
            Some((10, 2))
        );
        let gemini = serde_json::json!({"usageMetadata":{"promptTokenCount":7}});
        assert_eq!(
            usage_from_json(&gemini).map(|u| (u.input, u.output)),
            Some((7, 0))
        );
    }

    #[test]
    fn a_stream_with_no_usage_reports_nothing_rather_than_zero() {
        let mut s = UsageScanner::default();
        s.feed(b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n");
        s.feed(b"data: [DONE]\n\n");
        assert_eq!(s.finish(), None);
    }

    #[test]
    fn a_long_stream_is_never_retained() {
        // The property that stops "read the usage event" from quietly becoming
        // "buffer the response". One megabyte of content deltas must leave the
        // scanner holding a bounded window, and the usage must still come out.
        let mut s = UsageScanner::default();
        for i in 0..20_000 {
            s.feed(
                format!("data: {{\"delta\":{{\"text\":\"chunk {i} of many\"}}}}\n\n").as_bytes(),
            );
            assert!(s.retained() < 4096, "retained {} bytes", s.retained());
        }
        s.feed(br#"data: {"usage":{"output_tokens":5}}"#);
        s.feed(b"\n\n");
        assert_eq!(s.finish().map(|u| u.output), Some(5));
    }

    #[test]
    fn an_over_long_line_is_dropped_and_says_so() {
        // Accounting degrades; the relay does not. `truncated` is what lets a
        // caller refuse to record a figure it knows is short.
        let mut s = UsageScanner::default();
        s.feed(b"data: ");
        s.feed(&vec![b'x'; MAX_LINE + 10]);
        s.feed(br#"{"usage":{"output_tokens":9}}"#);
        s.feed(b"\n");
        assert!(s.truncated);
        assert_eq!(s.finish(), None);
        // And the scanner recovers on the NEXT line rather than staying broken.
        s.feed(br#"data: {"usage":{"output_tokens":11}}"#);
        s.feed(b"\n");
        assert_eq!(s.finish().map(|u| u.output), Some(11));
    }

    #[test]
    fn crlf_line_endings_do_not_break_the_json() {
        // Some providers emit CRLF. A trailing `\r` inside the payload makes
        // `serde_json` fail and the count vanish without a trace.
        let mut s = UsageScanner::default();
        s.feed(b"data: {\"usage\":{\"output_tokens\":3}}\r\n\r\n");
        assert_eq!(s.finish().map(|u| u.output), Some(3));
    }

    #[test]
    fn a_buffered_body_is_read_by_the_same_rules() {
        // Non-streaming responses go through `usage_from_json` directly, and
        // must not need a second implementation of the three shapes.
        let v: Value = serde_json::from_str(
            r#"{"usage":{"input_tokens":5,"output_tokens":6,"cache_creation_input_tokens":7}}"#,
        )
        .unwrap();
        assert_eq!(
            usage_from_json(&v),
            Some(Usage {
                input: 5,
                output: 6,
                cache_read: 0,
                cache_write: 7,
                cache_in_input: false
            })
        );
        assert_eq!(usage_from_json(&serde_json::json!({"content": []})), None);
    }
}
