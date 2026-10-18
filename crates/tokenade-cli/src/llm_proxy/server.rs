//! The transport: accept, decide, relay, count.
//!
//! Everything interesting about rank 4 lives in the other five modules, which
//! are pure. This one is the part that touches sockets, and its whole job is to
//! never be the reason a session breaks.
//!
//! **Fail-open is the first property, not the last.** The rule: "an LLM proxy
//! failure must never break the session. On an internal error, relay the
//! original request byte for byte." Every decision below has a defined answer for
//! "and if that went wrong?", and in each case the answer is the bytes the
//! agent sent us. There are exactly three places we do NOT relay, all of them
//! because there is nothing to relay: the client sent something that is not
//! HTTP, the client hung up, or the provider could not be reached at all.
//!
//! The order the work is done in matters:
//! relay first, understand second, rewrite third. A proxy built the other way
//! round works right up until the day it meets a body it does not know.

use super::{authority, cache_gate, config::Config, http, sse, wire};
use std::io::{BufReader, BufWriter, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;

/// What one turn through the proxy did. Nothing here is a request body, and
/// that is a requirement rather than an omission ("no persistence of
/// request bodies").
#[derive(Debug, Default, Clone)]
pub struct TurnRecord {
    pub status: u16,
    pub streamed: bool,
    /// Blocks actually rewritten on the way out.
    pub folded: usize,
    /// Tokens removed from the history by blocks folded for the FIRST time in
    /// this session — one credit per payload, per block.
    pub tokens_saved: u64,
    /// The same, as (before, after) over those first-time blocks.
    pub first_fold: (u64, u64),
    /// Tokens removed from blocks already folded on an earlier call: the
    /// agent re-sent them, so this call re-read them folded — the re-read the
    /// fold spared, counted on a real call.
    pub tokens_already_credited: u64,
    /// Tokens of the style preamble this call carried because the proxy put it
    /// there — injected context, re-read on every call.
    pub preamble_tokens: u64,
    /// Tokens put behind a recovery pointer by first-time folds.
    pub recovery_offered: u64,
    /// The agent this port serves, when the operator named it.
    pub agent: Option<&'static str>,
    /// The request's model, when the body names one.
    pub model: Option<String>,
    /// The provider's own numbers. `None` when the response carried none.
    pub usage: Option<sse::Usage>,
}

/// Where turn records go.
///
/// Injected so this module has no opinion about persistence. The shipping
/// implementation is [`LedgerAccounting`]: the gain ledger, plus the debug log
/// (numbers only, never a body). The proxy used to write ONLY the debug log —
/// every fold it made for an agent without hooks saved tokens no figure
/// showed. No double count with the hooks: a payload a higher rung folded is
/// marked and left alone (rules 1–2), and a block is credited once per
/// session (rule 3).
pub trait Accounting: Send + Sync {
    fn record(&self, turn: &TurnRecord);
}

/// Count nothing.
///
/// A test double, and marked as one: the shipping path uses
/// [`DebugLogAccounting`], which is already silent when the debug log is off.
/// Keeping a second no-op in the binary would be a switch nothing flips.
#[cfg(test)]
pub struct NoAccounting;
#[cfg(test)]
impl Accounting for NoAccounting {
    fn record(&self, _turn: &TurnRecord) {}
}

/// May a string of an editor's protobuf body be FOLDED? The schema is
/// unknown, so a long string may be the user's prompt, the system prompt or a
/// file sent as context — folding those made the model edit a summary. Only
/// text that reads as a command's OUTPUT is folded; everything else is only
/// scrubbed of credentials.
fn protobuf_output_kind(text: &str) -> bool {
    use tokenade_core::auto::Kind::*;
    matches!(
        tokenade_core::auto::detect(text),
        TerraformPlan
            | CurlVerbose
            | StackTrace
            | ArchiveListing
            | TabularCli
            | DashTable
            | Logs
            | SqlResult
            | PrometheusMetrics
            | AccessLog
            | Journal
            | Strace
            | JvmBuild
            | KubectlDescribe
            | Ansible
            | GhActions
            | PkgInstall
            | GitLog
            | Dns
            | JUnit
    )
}

/// Write turn records to the debug log — numbers only, never a body.
pub struct DebugLogAccounting;
impl Accounting for DebugLogAccounting {
    fn record(&self, t: &TurnRecord) {
        tokenade_core::debug_log::log(
            "llm_proxy",
            "turn",
            serde_json::json!({
                "status": t.status,
                "streamed": t.streamed,
                "folded": t.folded,
                "tokens_saved": t.tokens_saved,
                "tokens_already_credited": t.tokens_already_credited,
                "input": t.usage.map(|u| u.input),
                "output": t.usage.map(|u| u.output),
                "cache_read": t.usage.map(|u| u.cache_read),
                "cache_write": t.usage.map(|u| u.cache_write),
            }),
        );
    }
}

/// Serve every configured agent from ONE process.
///
/// The shape the operator asked for: three agents on a machine must not mean
/// three proxies, three autostart units and three things to notice when one
/// dies. One process, one service, one place the numbers land.
///
/// **One listener per agent, not one for all.** The port is what identifies the
/// agent, and the identity is not decoration: it decides the UPSTREAM (two
/// agents can talk to different providers) and it decides whether folding is
/// allowed at all (Claude Code's hooks already own its tool results, Codex
/// only sometimes). Both are answers this rung cannot infer from the bytes: the
/// same wire, the same provider and the same credentials can belong to two
/// agents with opposite fold policies. A port per agent is the cheapest thing
/// that carries the answer reliably, and it costs the user nothing — they never
/// type it, `install` writes it into the agent's own config.
///
/// **Concurrency, twice over.** A thread per accepted connection, so several
/// agents are served at once; and nothing here serialises an agent with itself,
/// so the several SIMULTANEOUS SESSIONS one agent can open are ordinary
/// concurrent connections. The per-turn work is per-connection state; the only
/// thing shared across sessions is the credit ledger, which is behind its own
/// lock and touched once per turn.
pub fn serve_all(proxies: Vec<Proxy>) -> Result<(), String> {
    if proxies.is_empty() {
        return Err(
            "nothing to serve: no agent is pointed at the proxy, and no \
                    --upstream was given. Run `tokenade llm-proxy install --agent <name>` \
                    first, or pass --upstream to serve one provider directly."
                .into(),
        );
    }
    // Bind EVERYTHING before serving anything. A partial bind would leave some
    // agents pointing at a dead port while the process looks healthy — and this
    // is the one rung whose failure breaks the agent rather than degrading it.
    let mut bound = Vec::with_capacity(proxies.len());
    for p in proxies {
        p.cfg.validate().map_err(|e| e.to_string())?;
        let l =
            TcpListener::bind(&p.cfg.listen).map_err(|e| format!("bind {}: {e}", p.cfg.listen))?;
        eprintln!("{}", p.cfg.banner());
        bound.push((l, Arc::new(p)));
    }
    let mut threads = Vec::with_capacity(bound.len());
    for (listener, me) in bound {
        threads.push(std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let me = Arc::clone(&me);
                std::thread::spawn(move || me.handle(stream));
            }
        }));
    }
    for t in threads {
        let _ = t.join();
    }
    Ok(())
}

/// The compactor, injected.
///
/// A `dyn Fn` rather than a direct call so the decision logic can be tested
/// against a fold that is predictable — and, in one test, against a fold that
/// PANICS, which is the only honest way to assert fail-open.
/// `(text, command hint)` — the hint comes from the payload itself (the
/// `tool_use` that produced this result) and picks the compactor, like the
/// `--cmd` the bridges already pass.
pub type Folder = Arc<dyn Fn(&str, Option<&str>) -> Option<String> + Send + Sync>;

/// The product's real compactor, wired the way every other channel wires it.
///
/// Mirrors `hookwrap::copilot_fold` deliberately, including the two things that
/// module had to be corrected for: the `already_folded` guard and
/// the recovery pointer. A channel that drops bytes without leaving a way back
/// is not a saving, it is a loss the user cannot even see.
pub fn default_folder() -> Folder {
    Arc::new(|text: &str, hint: Option<&str>| {
        if !authority::may_fold(text) {
            return None;
        }
        // Redaction is NOT gated on the compaction floor, and the difference is
        // the point.
        //
        // Everything below this line is token economics: a short block costs
        // more in banner and recovery pointer than the fold saves, so 800 is
        // where compacting stops paying. Keeping a secret out of the provider's
        // logs is a SAFETY property and has no such trade — a 40-character
        // `AWS_SECRET_ACCESS_KEY=…` in a tool result is the cheapest possible
        // block and the most expensive possible leak.
        //
        // Until this was split out, the proxy's redaction fired only on blocks
        // it also compacted, which made "secrets never reach the model" an
        // accident of length. On the two agents this rung may rewrite for
        // (codex, aider — everywhere else a higher rung owns the payload and
        // already redacts) that is the difference between the feature existing
        // and not.
        if text.len() < 800 {
            let clean = tokenade_core::redact::redact_for_model(text);
            return (clean != text).then_some(clean);
        }
        let before = tokenade_core::tokenizer::estimate(text);
        // The compactor chosen by the COMMAND when the payload carries it,
        // exactly as `read - --cmd` does for the bridges. The generic one
        // stays the floor: we keep the shorter of the two, so a misleading
        // hint cannot degrade the result.
        let generic = tokenade_core::auto::compact(text).1;
        let by_hint = hint.filter(|c| !c.trim().is_empty()).and_then(|c| {
            crate::compactors::hint_programs(c)
                .into_iter()
                .filter_map(|prog| tokenade_core::cmd::dispatch_named(prog, text))
                .min_by_key(String::len)
        });
        let folded = match by_hint {
            Some(h) if generic.is_empty() || h.len() < generic.len() => h,
            _ => generic,
        };
        if folded.is_empty() || folded.len() >= text.len() {
            // No compactor recognises this block — PROSE, typically a
            // document the agent read. `read` has always known what to do
            // with it: it sets it aside and returns a bounded summary with its
            // recovery pointer. This path did not, and the proxy relayed
            // 134 298 bytes of prose intact where the same product returns
            // 4 617 through the other door. Two paths, one product, and the
            // "documents & media" column was reading the gap.
            let stashed = tokenade_core::disclose::maybe_stash(
                text.to_string(),
                tokenade_core::disclose::SourceHint::Shell,
            );
            if stashed.len() < text.len() {
                return Some(tokenade_core::redact::redact_for_model(&stashed));
            }
            // Nothing to compact is still something to redact.
            let clean = tokenade_core::redact::redact_for_model(text);
            return (clean != text).then_some(clean);
        }
        let folded = tokenade_core::redact::redact_for_model(&folded);
        let dropped = before.saturating_sub(tokenade_core::tokenizer::estimate(&folded));
        let folded = crate::wrap::attach_drop_recovery_noting(
            folded,
            text,
            dropped,
            before,
            &tokenade_core::disclose::SourceHint::Shell,
            "llm-proxy",
            false,
        );
        (folded.len() < text.len()).then_some(folded)
    })
}

/// One running proxy.
pub struct Proxy {
    pub cfg: Config,
    fold: Folder,
    acct: Box<dyn Accounting>,
    ledger: authority::CreditLedger,
}

/// What we decided to send upstream, and why.
///
/// `bytes` borrows the original when nothing was rewritten. That is not an
/// optimisation: re-serialising an unchanged body reorders keys and changes the
/// provider's cache key, so an identical-looking request would miss the cache
/// and cost a full write. Relaying the ORIGINAL bytes is the only correct
/// answer when we changed nothing.
pub struct Outgoing<'a> {
    pub bytes: std::borrow::Cow<'a, [u8]>,
    pub rewrite: Option<wire::Rewrite>,
    /// Why we left the body alone, for the log line. `None` when we rewrote it.
    pub relayed_because: Option<&'static str>,
}

impl Proxy {
    pub fn new(cfg: Config, fold: Folder, acct: Box<dyn Accounting>) -> Self {
        Self {
            cfg,
            fold,
            acct,
            ledger: authority::CreditLedger::default(),
        }
    }

    /// Decide what body to send upstream. Pure: no sockets, no clock.
    ///
    /// Every `return` in here that yields the original bytes is a fail-open
    /// path, and each one names itself so the reason ends up in the log instead
    /// of in somebody's head.
    pub fn plan_outgoing<'a>(&self, req: &'a http::Request) -> Outgoing<'a> {
        let original = |why: &'static str| Outgoing {
            bytes: std::borrow::Cow::Borrowed(req.body.as_slice()),
            rewrite: None,
            relayed_because: Some(why),
        };
        if !self.cfg.compaction {
            return original("compaction is off");
        }
        if req.body.len() > http::PARSE_CAP {
            return original("body too large to parse");
        }
        // One authority per payload: the highest rung that touched this owns it. A hook or a
        // plugin bridge upstream of us has already folded these results, and a
        // second pass re-summarises a summary.
        if authority::a_higher_rung_is_active(req.header(authority::CHANNEL_HEADER)) {
            return original("a higher rung owns this session");
        }
        // Same rule again, from the other side. The header above is the runtime
        // evidence and only exists when an agent-side channel sets it; this is
        // the STATIC answer, from the measured matrix, for the agent the
        // operator named — today that means a wire we cannot parse, or a row
        // nobody measured. It used to mean "this agent has hooks", which
        // switched the proxy off on eleven agents whose histories carry
        // payloads no hook ever sees.
        // Two gates and not one, because the two refusals do not say the
        // same thing. "Out of scope" and "owned by another agent's line" are
        // questions of RIGHT and hold whatever the format. "Vendor wire" was
        // a question of CAPABILITY, and the capability has changed: the
        // protobuf body can now be walked. Refusing here would repeat an old
        // mistake — a gate per agent deciding in place of a test per payload.
        let role = self
            .cfg
            .agent
            .map(|a| super::agent_matrix::entry(a).role(a.coverage()));
        if role.is_some_and(|r| !r.may_fold_a_wire_we_can_walk()) {
            return original("the measured matrix says this agent is not ours to fold for");
        }

        // The body is not necessarily JSON. Four agents of the matrix —
        // Cursor, Windsurf, Devin Desktop, Antigravity — speak their editor's
        // protocol, and rank 4 relayed them intact saying "an
        // unrecognised body is relayed untouched rather than guessed at".
        // That was true and it was not inevitable: the protobuf wire format
        // carries the field number and its type, so it can be walked without
        // a schema. This branch comes BEFORE the JSON attempt because the
        // `content-type` is a safer answer than a `serde_json` that fails.
        if let Some(framing) = req
            .header("content-type")
            .and_then(super::protowire::framing_of)
        {
            return self.plan_protobuf(req, framing);
        }
        // Past this point we are on the JSON path, and there the "vendor
        // wire" row regains its full meaning: their JSON, if they send any,
        // is not a wire we have measured.
        if role.is_some_and(|r| !r.may_fold()) {
            return original("the measured matrix says this agent is not ours to fold for");
        }

        // The rewrite itself. `catch_unwind` is meaningful here and was not
        // always: the workspace sets `panic = "unwind"` precisely so that a
        // panic in a compactor becomes a relayed request instead of a dead
        // process. Under `panic = "abort"` this guard would be inert, which is
        // a defect this project has already shipped once.
        let fold = Arc::clone(&self.fold);
        // A block of the history folds only if it already went out folded.
        let session = authority::session_key(req.header(authority::SESSION_HEADER));
        let settled = |id: u64| self.ledger.seen_block(&session, id);
        let raw = req.body.as_slice();
        // Re-read dedup, where nothing outside the proxy carries it (Codex
        // with unapproved hooks…): decided per request, like the hook state.
        let dedup = self.cfg.agent.is_some_and(|a| {
            !super::features::carried_without_proxy(a, super::features::Feature::Dedup)
        });
        let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut fold = |s: &str, h: Option<&str>| fold(s, h);
            let deduped = super::wire::dedup_identical(dedup, &mut fold);
            super::compact_outgoing_settled(
                raw,
                &req.target,
                deduped,
                self.cfg.system_preamble.as_deref(),
                &settled,
            )
        }));
        let (bytes, rewrite, w) = match attempt {
            Err(_) => return original("the compactor panicked"),
            Ok(None) => return original("nothing worth rewriting"),
            Ok(Some(p)) => p,
        };

        // The cache gate, last, on a rewrite we can now measure. Rewriting a
        // block inside the provider's cached prefix invalidates everything
        // after it, and that write can cost more than the fold saves.
        let saved = rewrite.bytes_before.saturating_sub(rewrite.bytes_after);
        let candidate = cache_gate::Candidate {
            tokens_saved: (saved / BYTES_PER_TOKEN) as u64,
            tokens_after_first_rewrite: (cached_prefix_end(raw)
                .saturating_sub(rewrite.bytes_before_first_fold)
                / BYTES_PER_TOKEN) as u64,
            remaining_turns: cache_gate::ASSUMED_REMAINING_TURNS,
        };
        // The gate weighs a FOLD: bytes saved against the cached prefix that
        // rewriting invalidates. A preamble saves nothing by design, so a
        // preamble-only rewrite scores zero-saved against a non-zero cost and
        // is always vetoed — measured 2026-08-20, that is why the style
        // preamble reached the provider on a request carrying tool results and
        // never on a plain one.
        //
        // It is exempt for the same reason the length rule exempts redaction:
        // both make the body longer on purpose, and judging them by a
        // compaction's arithmetic throws away the body we meant to send. The
        // cost is real but bounded and one-off — the preamble is idempotent by
        // content, so from the second turn the prefix is byte-identical again
        // and the provider caches it like any other.
        // Sticky: once a block of this conversation went out folded, the
        // prefix the provider caches IS the folded one, and relaying the
        // original would be the invalidation the gate exists to avoid — the
        // whole history written to cache again, the turn the suffix grew past
        // the gate's line.
        let continuing = rewrite
            .blocks
            .iter()
            .any(|b| self.ledger.seen_block(&session, b.0));
        let fold_only = rewrite.folded > 0;
        if fold_only
            && !continuing
            && !cache_gate::worth_rewriting(candidate, self.prices_for(raw, w))
        {
            return self.redacted_only(
                req,
                true,
                "the cache write would cost more than the fold saves",
            );
        }

        Outgoing {
            bytes: std::borrow::Cow::Owned(bytes),
            rewrite: Some(rewrite),
            relayed_because: None,
        }
    }

    /// The same plan, for a protobuf body.
    ///
    /// Kept apart from [`Self::plan_outgoing`] rather than merged into it: the
    /// two paths share NEITHER the parser NOR the notion of "wire", and mixing
    /// them would produce a function where every line asks which format we
    /// are talking about. What they share — the compactor, the cache gate,
    /// the accounting — goes through the same types.
    fn plan_protobuf<'a>(
        &self,
        req: &'a http::Request,
        framing: super::protowire::Framing,
    ) -> Outgoing<'a> {
        let original = |why: &'static str| Outgoing {
            bytes: std::borrow::Cow::Borrowed(req.body.as_slice()),
            rewrite: None,
            relayed_because: Some(why),
        };
        let fold = Arc::clone(&self.fold);
        let raw = req.body.as_slice();
        // Same safety net as on the JSON path, and for the same reason: a
        // panic in a compactor must become a relayed request, never a dead
        // process.
        let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // No command hint here: the editor's protocol does not say which
            // command produced this text. The generic compactor recognises the
            // shape of the output, which is precisely what it can do without a
            // hint.
            super::protowire::fold_body(raw, framing, |s| {
                if protobuf_output_kind(s) {
                    fold(s, None)
                } else {
                    let clean = tokenade_core::redact::redact_for_model(s);
                    (clean != s).then_some(clean)
                }
            })
        }));
        let (bytes, pr) = match attempt {
            Err(_) => return original("the protobuf compactor panicked"),
            Ok(None) => return original("nothing worth rewriting in this protobuf body"),
            Ok(Some(p)) => p,
        };
        if bytes.len() >= raw.len() && !pr.redacted {
            return original("the rewritten protobuf body was not smaller");
        }
        let rewrite = wire::Rewrite {
            preambled: false,
            seen: pr.folded,
            folded: pr.folded,
            bytes_before: pr.bytes_before,
            bytes_after: pr.bytes_after,
            bytes_before_first_fold: pr.first_fold_at.min(raw.len()),
            redacted: pr.redacted,
            blocks: pr.blocks,
            model: None,
            in_tail: false,
        };
        let saved = rewrite.bytes_before.saturating_sub(rewrite.bytes_after);
        let candidate = cache_gate::Candidate {
            tokens_saved: (saved / BYTES_PER_TOKEN) as u64,
            tokens_after_first_rewrite: (cached_prefix_end(raw)
                .saturating_sub(rewrite.bytes_before_first_fold)
                / BYTES_PER_TOKEN) as u64,
            remaining_turns: cache_gate::ASSUMED_REMAINING_TURNS,
        };
        // The price: these editors bill through their own provider and no
        // price list describes their prefix cache to us. So we treat them as
        // an unknown wire — with no cache to invalidate — which is the only
        // answer we can defend, and the same one already given to any JSON
        // body whose provider we do not know.
        let session = authority::session_key(req.header(authority::SESSION_HEADER));
        let continuing = rewrite
            .blocks
            .iter()
            .any(|b| self.ledger.seen_block(&session, b.0));
        if !continuing
            && !rewrite.redacted
            && !cache_gate::worth_rewriting(candidate, self.prices_for(raw, wire::Wire::Unknown))
        {
            return original("the cache write would cost more than the fold saves");
        }
        Outgoing {
            bytes: std::borrow::Cow::Owned(bytes),
            rewrite: Some(rewrite),
            relayed_because: None,
        }
    }

    /// The request with its credentials scrubbed and nothing else changed —
    /// what goes out when the cache gate declines a fold. Relaying the
    /// original there forwarded a secret a redaction had already caught: a
    /// key in a short tool result rode a declined fold to the provider.
    fn redacted_only<'a>(
        &self,
        req: &'a http::Request,
        with_preamble: bool,
        why: &'static str,
    ) -> Outgoing<'a> {
        let original = Outgoing {
            bytes: std::borrow::Cow::Borrowed(req.body.as_slice()),
            rewrite: None,
            relayed_because: Some(why),
        };
        let scrub = |s: &str, _: Option<&str>| {
            let clean = tokenade_core::redact::redact_for_model(s);
            (clean != s).then_some(clean)
        };
        let preamble = self
            .cfg
            .system_preamble
            .as_deref()
            .filter(|_| with_preamble);
        match super::compact_outgoing_at(&req.body, &req.target, scrub, preamble) {
            Some((bytes, rewrite, _)) if rewrite.redacted || rewrite.preambled => Outgoing {
                bytes: std::borrow::Cow::Owned(bytes),
                rewrite: Some(wire::Rewrite {
                    blocks: Vec::new(),
                    folded: 0,
                    ..rewrite
                }),
                relayed_because: Some(why),
            },
            _ => original,
        }
    }

    /// What this particular request is actually charged, which is not always
    /// what the provider's price list says.
    ///
    /// Two facts that decide whether rank 4 does anything at all:
    ///
    /// * **the OpenAI wires do not bill cache writes.** Their prefix cache is
    ///   automatic and free to populate, so there is no cost to weigh and every
    ///   fold that shortens the history is a win;
    /// * **Anthropic caches only what the request asks it to cache.** Prompt
    ///   caching is opt-in per block via `cache_control`, and a request that
    ///   carries none has no cached prefix to invalidate — the fold is free and
    ///   the tokens it removes were being billed at FULL input price on every
    ///   turn, not at the cache-read price.
    ///
    /// Detected on the raw bytes rather than on a parsed field because
    /// `cache_control` can sit on a system block, a tool definition or any
    /// message, and a walk that missed one of those places would price a cache
    /// that exists as one that does not — the expensive direction.
    fn prices_for(&self, raw: &[u8], w: wire::Wire) -> cache_gate::Prices {
        let cached = match w {
            wire::Wire::Anthropic => contains(raw, b"cache_control"),
            // Automatic, and free to write.
            wire::Wire::OpenAiChat | wire::Wire::OpenAiResponses | wire::Wire::Unknown => false,
        };
        if cached {
            self.cfg.prices
        } else {
            self.cfg.prices.without_prompt_cache()
        }
    }

    /// Credit a rewrite exactly once per (session, content) — one credit per payload.
    ///
    /// The proxy sees the whole conversation on every turn. Crediting what it
    /// sees would recount turn 1's tool result on turns 2 through 40, which is
    /// precisely how a gain figure grows by 93 % without anything improving.
    ///
    /// Per BLOCK, not per body: the body grows by a message every call, so a
    /// whole-body key saw every call as new and re-credited the whole history
    /// each time. Returns ((before, after) of first-time blocks, tokens
    /// removed from repeated blocks).
    fn credit(&self, session: &str, out: &Outgoing<'_>) -> ((u64, u64), u64, u64) {
        let Some(r) = &out.rewrite else {
            return ((0, 0), 0, 0);
        };
        let tok = |b: usize| (b / BYTES_PER_TOKEN) as u64;
        let (mut first, mut again, mut offered) = ((0u64, 0u64), 0u64, 0u64);
        for &(id, before, after, fresh, pointer) in &r.blocks {
            if self.ledger.first_time_block(session, id) && fresh {
                first.0 += tok(before);
                first.1 += tok(after);
                if pointer {
                    offered += tok(before).saturating_sub(tok(after));
                }
            } else {
                again += tok(before).saturating_sub(tok(after));
            }
        }
        (first, again, offered)
    }

    /// Bind and serve until the listener dies. Blocks.
    pub fn serve(self) -> Result<(), String> {
        serve_all(vec![self])
    }

    /// Serve one connection from a test's own listener.
    ///
    /// [`Self::serve`] owns its listener and blocks forever, which no test can
    /// call. This is the same connection handling reached from the outside, so
    /// a test exercises the real loop rather than a parallel one.
    #[cfg(test)]
    pub(crate) fn handle_for_test(&self, stream: TcpStream) {
        self.handle(stream);
    }

    /// Serve one connection, honouring keep-alive.
    fn handle(&self, stream: TcpStream) {
        // Nagle batches small writes, which on an SSE stream means holding
        // events back until the buffer fills — the "agent looks frozen"
        // failure, produced by the transport rather than by us.
        let _ = stream.set_nodelay(true);
        let Ok(read_half) = stream.try_clone() else {
            return;
        };
        let mut reader = BufReader::new(read_half);
        let mut writer = BufWriter::new(stream);
        loop {
            let req = match http::read_request(&mut reader) {
                Ok(r) => r,
                // A closed idle connection is the normal end of a keep-alive
                // session, not an incident.
                Err(http::ReadError::Eof) | Err(http::ReadError::Io) => return,
                Err(http::ReadError::Malformed(why)) => {
                    tokenade_core::debug_log::log(
                        "llm_proxy",
                        "malformed_request",
                        serde_json::json!({ "reason": why }),
                    );
                    let _ = http::write_error(&mut writer, 400, "malformed request");
                    return;
                }
            };
            let keep_going = req.keep_alive();
            if req.target.starts_with(HEALTH_ROUTE) {
                let _ = self.write_health(&mut writer);
                if keep_going {
                    continue;
                }
                return;
            }
            if self.turn(&req, &mut writer).is_err() || !keep_going {
                return;
            }
        }
    }

    /// One request/response cycle.
    fn turn<W: Write>(&self, req: &http::Request, out: &mut W) -> Result<(), ()> {
        let out_plan = self.plan_outgoing(req);
        let session = authority::session_key(req.header(authority::SESSION_HEADER));
        let (first_fold, tokens_already_credited, recovery_offered) =
            self.credit(&session, &out_plan);
        let tokens_saved = first_fold.0.saturating_sub(first_fold.1);

        let url = format!("{}{}", self.cfg.upstream.trim_end_matches('/'), req.target);
        let headers: Vec<(String, String)> = req
            .headers
            .iter()
            .filter(|(k, _)| !http::drop_from_request(k))
            .cloned()
            .collect();
        let header_refs: Vec<(&str, &str)> = headers
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        let mut sink = RelaySink {
            out,
            scanner: sse::UsageScanner::default(),
            buffered: (!req.wants_stream()).then(Vec::new),
            status: 0,
            head_written: false,
            broken: false,
        };
        let res = tokenade_core::net::http_stream(
            &req.method,
            &url,
            &header_refs,
            out_plan.bytes.to_vec(),
            CONNECT_TIMEOUT_SECS,
            self.cfg.idle_timeout_secs,
            &mut sink,
        );

        let status = sink.status;
        let head_written = sink.head_written;
        let broken = sink.broken;
        let usage = sink.finish_usage();
        // Record BEFORE the terminating chunk. The agent treats that chunk as
        // "the turn is over" and may ask its next question immediately, so a
        // record written after it races the next turn's read of the same
        // books — a race a test would see as a missing row and a user would
        // see as a turn that vanished.
        self.acct.record(&TurnRecord {
            status,
            streamed: req.wants_stream(),
            folded: out_plan.rewrite.as_ref().map(|r| r.folded).unwrap_or(0),
            tokens_saved,
            first_fold,
            tokens_already_credited,
            recovery_offered,
            preamble_tokens: match (&out_plan.rewrite, self.cfg.system_preamble.as_deref()) {
                (Some(r), Some(p)) if r.preambled => tokenade_core::tokenizer::estimate(p) as u64,
                _ => 0,
            },
            agent: self.cfg.agent.map(|a| a.as_str()),
            model: out_plan.rewrite.as_ref().and_then(|r| r.model.clone()),
            usage,
        });
        if let Some(why) = out_plan.relayed_because {
            tokenade_core::debug_log::log(
                "llm_proxy",
                "relayed_unchanged",
                serde_json::json!({ "reason": why }),
            );
        }
        if head_written && !broken {
            let _ = http::end_chunks(&mut *sink.out);
        }
        drop(sink);

        match res {
            Err(e) if !head_written => {
                // Nothing has been said to the agent yet, so we still owe it an
                // answer it can parse. `e` is already scrubbed at the transport
                // boundary; it names a host, never a key.
                tokenade_core::debug_log::log(
                    "llm_proxy",
                    "upstream_failed",
                    serde_json::json!({ "error": e }),
                );
                let _ = http::write_error(out, 502, "tokenade proxy could not reach the provider");
                Err(())
            }
            // The head is out and the body was cut short. There is no way to
            // retract a 200, so the connection is closed and the agent's own
            // stream handling reports the truncation — which is what it would
            // have seen talking to the provider directly.
            Err(_) => Err(()),
            Ok(()) if broken => Err(()),
            Ok(()) => Ok(()),
        }
    }

    fn write_health<W: Write>(&self, out: &mut W) -> std::io::Result<()> {
        let body = serde_json::json!({
            "ok": true,
            // The supervisor needs it to decide "does this port serve the
            // installed binary?". Without this field, a proxy left in memory
            // since the previous update answers exactly like a fresh proxy,
            // and the auto-install concludes "already there".
            "version": env!("CARGO_PKG_VERSION"),
            "deployment": if self.cfg.deployment == super::config::Deployment::Local { "local" } else { "remote" },
            "compaction": self.cfg.compaction,
            "web_capabilities": self.cfg.web_capabilities(),
        })
        .to_string();
        write!(
            out,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )?;
        out.flush()
    }
}

mod accounting;
pub use accounting::*;
mod prefix;
pub(crate) use prefix::*;

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
