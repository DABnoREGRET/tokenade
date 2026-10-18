//! Antigravity — usage read out of protobuf blobs in per-conversation SQLite.
//!
//! Antigravity is an IDE, and tokenade wires no hook into it: the Gemini-family
//! BeforeTool rewrite is installed for Gemini CLI only, and the AfterTool
//! replace path Antigravity would need is not one it exposes (decision record in
//! `hooks::gemini_aftertool`). Only its MCP traffic is folded. Its own token
//! accounting was therefore invisible to the dashboard: a supported agent the
//! user may run every day, contributing a blank row. This module closes that.
//!
//! Two things make it unlike every other entry in [`crate::agent_sqlite_usage`]:
//!
//!   * one database PER CONVERSATION under `conversations/`, not one per agent,
//!     and sub-conversations (subagents, the arms of a model comparison) get
//!     their own file — so path discovery has to recurse;
//!   * the numbers are not columns. `steps.metadata` and `gen_metadata.data`
//!     hold raw protobuf, and Antigravity ships no `.proto`.
//!
//! ## Where the field numbers come from
//!
//! They were recovered from the `FileDescriptorProto` blobs embedded in
//! Antigravity's own binary (credit: ccusage's `antigravity` adapter, which did
//! that work and published the numbers — commit 50614f1, 2026-07-29). Decoding
//! by number is safe for the same reason it is safe for Antigravity itself: it
//! has to read these blobs back after an upgrade, so it cannot renumber a field
//! without breaking its own persistence.
//!
//! Two consequences of the format are load-bearing and easy to get wrong:
//!
//!   * `output_tokens` ALREADY includes `thinking_output_tokens`. Adding the
//!     thinking count on top double-bills every reasoning response.
//!   * the model is stored as a numeric id, and the shipped descriptor only
//!     names those ids `MODEL_PLACEHOLDER_M<n>`. Only `gen_metadata` carries a
//!     real name, and only for invocations it recorded. Tokens from an
//!     invocation nothing named are still REAL, so they count toward the totals
//!     and are simply absent from the per-model panel — the same honest
//!     degradation `model_usage_by_day`'s default impl makes.

use crate::agent_transcripts::{Agent, ModelUsageByDay, TranscriptParser};
use crate::claudecode_transcript::ProjectUsage;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Nanoseconds in a second — the bound `google.protobuf.Timestamp` puts on its
/// nanos field.
const NANOS_PER_SECOND: u64 = 1_000_000_000;

/// How deep to recurse under `conversations/`. Sub-conversations nest, but not
/// unboundedly; a fixed ceiling keeps a symlink loop from walking forever.
const MAX_DEPTH: usize = 6;

// ── protobuf wire format ─────────────────────────────────────────────
// Only the three wire types Antigravity's schema uses are decoded. Everything
// else — unknown fields, deprecated groups — is skipped or ends the message,
// because a blob we half-understand must degrade to "fewer records", never to a
// misread length that turns the rest of the buffer into garbage.

enum Value<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
    /// Fixed-width payloads are skipped, not decoded: the schema has none we
    /// need, and the variant exists only so the cursor lands in the right place.
    Skipped,
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// Base-128 varint. `None` on truncation, and on anything longer than the
    /// 10 bytes a u64 can occupy — without that ceiling a malformed blob spins.
    fn varint(&mut self) -> Option<u64> {
        let mut value = 0u64;
        for i in 0..10 {
            let byte = *self.buf.get(self.pos)?;
            self.pos += 1;
            value |= u64::from(byte & 0x7f) << (i * 7);
            if byte & 0x80 == 0 {
                return Some(value);
            }
        }
        None
    }

    fn next_field(&mut self) -> Option<(u32, Value<'a>)> {
        if self.pos >= self.buf.len() {
            return None;
        }
        let tag = self.varint()?;
        let field = u32::try_from(tag >> 3).ok()?;
        if field == 0 {
            return None;
        }
        let value = match tag & 0x7 {
            0 => Value::Varint(self.varint()?),
            1 => {
                self.pos = self.pos.checked_add(8)?;
                Value::Skipped
            }
            2 => {
                let len = usize::try_from(self.varint()?).ok()?;
                let end = self.pos.checked_add(len)?;
                let slice = self.buf.get(self.pos..end)?;
                self.pos = end;
                Value::Bytes(slice)
            }
            5 => {
                self.pos = self.pos.checked_add(4)?;
                Value::Skipped
            }
            // 3/4 are deprecated groups, 6/7 invalid. Stopping is right:
            // guessing a length here would misread everything after it.
            _ => return None,
        };
        Some((field, value))
    }
}

/// `google.protobuf.Timestamp` → epoch millis. Field 1 seconds, field 2 nanos.
fn timestamp_ms(bytes: &[u8]) -> Option<i64> {
    let mut secs: Option<u64> = None;
    let mut nanos = 0u64;
    let mut r = Reader::new(bytes);
    while let Some((field, value)) = r.next_field() {
        match (field, value) {
            (1, Value::Varint(v)) => secs = Some(v),
            // Out-of-range nanos are DISCARDED, not normalised: carrying them
            // would report the call up to four seconds late, which near midnight
            // files it on the wrong day.
            (2, Value::Varint(v)) => nanos = if v < NANOS_PER_SECOND { v } else { 0 },
            _ => {}
        }
    }
    // Reject 0: an unset field must not become a 1970 date that then sorts as
    // this agent's "last activity".
    let secs = secs.filter(|s| *s > 0)?;
    i64::try_from(secs.checked_mul(1_000)?.checked_add(nanos / 1_000_000)?).ok()
}

fn text(bytes: &[u8]) -> Option<String> {
    let s = std::str::from_utf8(bytes).ok()?.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// `ModelUsageStats` — the token counts of one model invocation.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct ModelUsage {
    input: u64,
    output: u64,
    cache_write: u64,
    cache_read: u64,
    thinking_output: u64,
    response_output: u64,
    response_id: Option<String>,
    message_id: Option<String>,
    provider_message_id: Option<String>,
}

impl ModelUsage {
    /// `output` when present, else the thinking/response split it was computed
    /// from. Never the sum of all three — `output` already contains thinking.
    fn total_output(&self) -> u64 {
        if self.output > 0 {
            return self.output;
        }
        self.thinking_output.saturating_add(self.response_output)
    }

    fn has_tokens(&self) -> bool {
        self.input > 0 || self.cache_read > 0 || self.cache_write > 0 || self.total_output() > 0
    }

    /// Server-assigned identity, preferred over anything Antigravity minted
    /// locally. This is what keeps one call from being counted twice: a failed
    /// attempt's `retry_infos` repeats the successful one, and a
    /// sub-conversation's database can repeat a call its parent already
    /// recorded — two files, one bill.
    fn identity(&self) -> Option<&str> {
        self.response_id
            .as_deref()
            .or(self.provider_message_id.as_deref())
            .or(self.message_id.as_deref())
    }
}

fn model_usage(bytes: &[u8]) -> ModelUsage {
    let mut u = ModelUsage::default();
    let mut r = Reader::new(bytes);
    while let Some((field, value)) = r.next_field() {
        match (field, value) {
            // Field 1 is the numeric model id; we never price off it (see the
            // module docs) so it is not kept.
            (2, Value::Varint(v)) => u.input = v,
            (3, Value::Varint(v)) => u.output = v,
            (4, Value::Varint(v)) => u.cache_write = v,
            (5, Value::Varint(v)) => u.cache_read = v,
            (9, Value::Varint(v)) => u.thinking_output = v,
            (10, Value::Varint(v)) => u.response_output = v,
            (7, Value::Bytes(b)) => u.message_id = text(b),
            (11, Value::Bytes(b)) => u.response_id = text(b),
            (12, Value::Bytes(b)) => u.provider_message_id = text(b),
            _ => {}
        }
    }
    u
}

/// `RetryInfo.usage` (field 2).
fn retry_usage(bytes: &[u8]) -> Option<ModelUsage> {
    let mut r = Reader::new(bytes);
    while let Some((field, value)) = r.next_field() {
        if let (2, Value::Bytes(b)) = (field, value) {
            return Some(model_usage(b));
        }
    }
    None
}

/// One row of `steps.metadata` (`CortexStepMetadata`).
///
/// Retry attempts (field 28) are collected alongside the successful call
/// (field 9): a failed attempt still burned its input tokens, and dedup by
/// identity means gathering both cannot double-count the one that succeeded.
fn parse_step(bytes: &[u8]) -> (Option<i64>, Vec<ModelUsage>) {
    let mut created_at = None;
    let mut started_at = None;
    let mut usages = Vec::new();
    let mut r = Reader::new(bytes);
    while let Some((field, value)) = r.next_field() {
        match (field, value) {
            (1, Value::Bytes(b)) => created_at = timestamp_ms(b),
            (32, Value::Bytes(b)) => started_at = timestamp_ms(b),
            (9, Value::Bytes(b)) => usages.push(model_usage(b)),
            (28, Value::Bytes(b)) => usages.extend(retry_usage(b)),
            _ => {}
        }
    }
    (created_at.or(started_at), usages)
}

/// One named generation: `ChatModelMetadata`.
#[derive(Debug, Default)]
struct Generation {
    model: Option<String>,
    ts_ms: Option<i64>,
    usages: Vec<ModelUsage>,
}

impl Generation {
    fn names_a_model(&self) -> bool {
        self.model.is_some() && self.usages.iter().any(|u| u.identity().is_some())
    }
}

fn chat_model_metadata(bytes: &[u8]) -> Generation {
    let mut g = Generation::default();
    let mut r = Reader::new(bytes);
    while let Some((field, value)) = r.next_field() {
        match (field, value) {
            (4, Value::Bytes(b)) => g.usages.push(model_usage(b)),
            (17, Value::Bytes(b)) => g.usages.extend(retry_usage(b)),
            (19, Value::Bytes(b)) => g.model = text(b),
            // `chat_start_metadata.created_at` — the request time. Only matters
            // for usage that appears here and in no step.
            (9, Value::Bytes(b)) => {
                let mut inner = Reader::new(b);
                while let Some((f, v)) = inner.next_field() {
                    if let (4, Value::Bytes(stamp)) = (f, v) {
                        g.ts_ms = timestamp_ms(stamp);
                    }
                }
            }
            _ => {}
        }
    }
    g
}

/// One row of `gen_metadata.data`.
///
/// The `ChatModelMetadata` sits inside a wrapper whose field number we do NOT
/// hard-code: every length-delimited field is tried and only those that yield
/// both a model name and an identifiable usage row are kept. The sibling holding
/// the outbound request has neither, so it is rejected without us having to know
/// the wrapper's shape — one less number to be wrong about after an upgrade.
fn parse_generations(bytes: &[u8]) -> Vec<Generation> {
    let mut out = Vec::new();
    let mut r = Reader::new(bytes);
    while let Some((_, value)) = r.next_field() {
        if let Value::Bytes(b) = value {
            let g = chat_model_metadata(b);
            if g.names_a_model() {
                out.push(g);
            }
        }
    }
    out
}

// ── discovery ────────────────────────────────────────────────────────

/// Roots to search. `ANTIGRAVITY_DATA_DIR` accepts a comma-separated list so a
/// live profile and an archive report together.
fn antigravity_roots(home: &Path) -> Vec<PathBuf> {
    if let Some(raw) = crate::platform::ambient_override(home, "ANTIGRAVITY_DATA_DIR")
        .and_then(|p| p.to_str().map(str::to_string))
    {
        let roots: Vec<PathBuf> = raw
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .collect();
        if !roots.is_empty() {
            return roots;
        }
        // A blank override is a mistake, not a request for "nowhere": fall
        // through to the default rather than silently reporting zero.
    }
    // Antigravity is a Gemini-family tool, so its state lives under the shared
    // `.gemini` directory rather than one of its own.
    vec![home.join(".gemini/antigravity-cli")]
}

/// Every `*.db` under each root's `conversations/`, nested ones included, sorted.
///
/// Sorted because dedup keeps the FIRST record for a response id: unsorted
/// directory order would make which copy survives depend on the filesystem, and
/// `tests/determinism.rs` exists to stop exactly that.
fn antigravity_dbs(home: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for root in antigravity_roots(home) {
        collect_dbs(&root.join("conversations"), 0, &mut out);
    }
    out.sort();
    out
}

fn collect_dbs(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        // `file_type` on the entry, so a symlinked directory is not recursed
        // into — that is the loop the depth cap would otherwise have to catch.
        match e.file_type() {
            Ok(t) if t.is_dir() => collect_dbs(&p, depth + 1, out),
            Ok(t) if t.is_file() && p.extension().is_some_and(|x| x == "db") => out.push(p),
            _ => {}
        }
    }
}

// ── aggregation ──────────────────────────────────────────────────────

/// One model invocation, with the model name attached when something named it.
struct Invocation {
    ts_ms: Option<i64>,
    model: Option<String>,
    usage: ModelUsage,
}

/// Read one conversation database.
///
/// `gen_metadata` is read first and only to learn model NAMES; the token counts
/// come from whichever record is seen first for a response id, so reading the
/// same invocation twice is harmless.
fn collect_invocations(db: &Path) -> Vec<Invocation> {
    let steps = crate::agent_sqlite_usage::query_db(
        db,
        "SELECT metadata FROM steps WHERE metadata IS NOT NULL ORDER BY idx",
        |r| r.get::<_, Vec<u8>>(0),
    );
    let gens = crate::agent_sqlite_usage::query_db(
        db,
        "SELECT data FROM gen_metadata WHERE data IS NOT NULL ORDER BY idx",
        |r| r.get::<_, Vec<u8>>(0),
    );

    let mut generations = Vec::new();
    for blob in &gens {
        generations.extend(parse_generations(blob));
    }
    let mut model_of: HashMap<String, String> = HashMap::new();
    for g in &generations {
        let Some(model) = g.model.as_deref() else {
            continue;
        };
        for u in &g.usages {
            if let Some(id) = u.identity() {
                model_of.insert(id.to_string(), model.to_string());
            }
        }
    }

    let mut out = Vec::new();
    for blob in &steps {
        let (ts, usages) = parse_step(blob);
        for usage in usages {
            let model = usage.identity().and_then(|id| model_of.get(id)).cloned();
            out.push(Invocation {
                ts_ms: ts,
                model,
                usage,
            });
        }
    }
    // Background calls Antigravity recorded in `gen_metadata` and in no step
    // would otherwise be lost. Their tokens are real; dedup drops the ones the
    // steps already covered.
    for g in generations {
        let model = g.model.clone();
        for usage in g.usages {
            out.push(Invocation {
                ts_ms: g.ts_ms,
                model: model.clone(),
                usage,
            });
        }
    }
    out
}

/// Every invocation across every conversation, each counted once.
fn deduped_invocations(home: &Path) -> Vec<Invocation> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for db in antigravity_dbs(home) {
        for inv in collect_invocations(&db) {
            if !inv.usage.has_tokens() {
                continue;
            }
            // No identity means nothing to match against, so it is KEPT.
            // Dropping it would delete real tokens to avoid a duplicate we
            // cannot prove exists.
            if let Some(id) = inv.usage.identity() {
                if !seen.insert(id.to_string()) {
                    continue;
                }
            }
            out.push(inv);
        }
    }
    out
}

fn iso_of(ts_ms: Option<i64>) -> String {
    match ts_ms {
        Some(ms) if ms > 0 => crate::platform::epoch_to_iso8601_z((ms / 1000) as u64),
        _ => String::new(),
    }
}

pub struct AntigravityParser;

impl TranscriptParser for AntigravityParser {
    fn agent(&self) -> Agent {
        Agent::Antigravity
    }

    fn aggregate_since(&self, home_dir: &Path, since_iso: Option<&str>) -> Vec<ProjectUsage> {
        // Antigravity records no working directory anywhere in the database —
        // its conversations are not project-scoped — so everything lands under
        // the unknown-cwd bucket rather than a path we would have invented.
        let mut by_cwd = HashMap::new();
        for inv in deduped_invocations(home_dir) {
            let iso = iso_of(inv.ts_ms);
            if !crate::agent_sqlite_usage::after_cutoff(&iso, since_iso) {
                continue;
            }
            crate::agent_sqlite_usage::add(
                &mut by_cwd,
                String::new(),
                inv.usage.input,
                inv.usage.total_output(),
                inv.usage.cache_write,
                inv.usage.cache_read,
                1,
                iso,
            );
        }
        crate::agent_sqlite_usage::finish(by_cwd)
    }

    fn model_usage_by_day(&self, home_dir: &Path, since_iso: Option<&str>) -> ModelUsageByDay {
        let mut out = ModelUsageByDay::default();
        for inv in deduped_invocations(home_dir) {
            // Only `gen_metadata` names a model. An invocation nothing named
            // still counted in the totals above; it is absent here rather than
            // filed under a guess.
            let Some(model) = inv.model else {
                continue;
            };
            let iso = iso_of(inv.ts_ms);
            if !crate::agent_sqlite_usage::after_cutoff(&iso, since_iso) {
                continue;
            }
            let Some(day) = iso.get(..10).filter(|d| !d.is_empty()) else {
                continue;
            };
            let e = out
                .entry(day.to_string())
                .or_default()
                .entry(model)
                .or_insert((0, 0, 0));
            // Same convention as every other parser: fresh input counts cache
            // WRITES but not cache reads. `input` already excludes cache reads
            // here (the two fields are disjoint in this format), so there is
            // nothing to subtract.
            e.0 += inv.usage.input + inv.usage.cache_write;
            e.1 += inv.usage.total_output();
            e.2 += 1;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── blob builders ────────────────────────────────────────────────
    // A real conversation database embeds absolute paths, the user's prompts and
    // Antigravity's system prompt. None of that belongs in this repo, so the
    // tests synthesise the blobs they need.

    fn varint(mut v: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                return out;
            }
            out.push(b | 0x80);
        }
    }

    fn f_varint(n: u32, v: u64) -> Vec<u8> {
        let mut out = varint(u64::from(n) << 3);
        out.extend(varint(v));
        out
    }

    fn f_bytes(n: u32, v: &[u8]) -> Vec<u8> {
        let mut out = varint((u64::from(n) << 3) | 2);
        out.extend(varint(v.len() as u64));
        out.extend_from_slice(v);
        out
    }

    fn stamp(secs: u64, nanos: u64) -> Vec<u8> {
        let mut out = f_varint(1, secs);
        out.extend(f_varint(2, nanos));
        out
    }

    fn usage(input: u64, output: u64, cache_read: u64, response_id: &str) -> Vec<u8> {
        let mut out = f_varint(1, 1071); // numeric model id, deliberately unused
        out.extend(f_varint(2, input));
        out.extend(f_varint(3, output));
        out.extend(f_varint(5, cache_read));
        out.extend(f_bytes(11, response_id.as_bytes()));
        out
    }

    fn step_blob(secs: u64, usage_bytes: &[u8]) -> Vec<u8> {
        let mut out = f_bytes(1, &stamp(secs, 0));
        out.extend(f_bytes(9, usage_bytes));
        out
    }

    fn gen_blob(secs: u64, model: &str, usage_bytes: &[u8]) -> Vec<u8> {
        let mut chat = f_bytes(4, usage_bytes);
        chat.extend(f_bytes(9, &f_bytes(4, &stamp(secs, 0))));
        chat.extend(f_bytes(19, model.as_bytes()));
        // Wrapper: an unrelated request-shaped sibling plus the real payload.
        let mut wrapper = f_bytes(3, &f_bytes(28, b"gemini-3.6-flash-high"));
        wrapper.extend(f_bytes(1, &chat));
        wrapper
    }

    // ── wire format ──────────────────────────────────────────────────

    #[test]
    fn decodes_the_token_fields_of_one_invocation() {
        let u = model_usage(&usage(4050, 375, 16275, "resp-a"));
        assert_eq!(u.input, 4050);
        assert_eq!(u.output, 375);
        assert_eq!(u.cache_read, 16275);
        assert_eq!(u.response_id.as_deref(), Some("resp-a"));
    }

    #[test]
    fn thinking_tokens_are_never_added_on_top_of_the_output_total() {
        // `output` already includes thinking; only when it is absent may the
        // split be summed. Getting this backwards double-bills every reasoning
        // response.
        let mut both = f_varint(3, 375);
        both.extend(f_varint(9, 351));
        both.extend(f_varint(10, 24));
        assert_eq!(model_usage(&both).total_output(), 375);

        let mut split = f_varint(9, 351);
        split.extend(f_varint(10, 24));
        assert_eq!(model_usage(&split).total_output(), 375);
    }

    #[test]
    fn identity_falls_back_through_the_id_fields() {
        let mut both = f_bytes(7, b"message-a");
        both.extend(f_bytes(12, b"provider-a"));
        assert_eq!(model_usage(&both).identity(), Some("provider-a"));
        assert_eq!(
            model_usage(&f_bytes(7, b"message-a")).identity(),
            Some("message-a")
        );
        assert_eq!(model_usage(&[]).identity(), None);
    }

    #[test]
    fn a_step_carries_its_retry_attempts_too() {
        let mut blob = f_bytes(1, &stamp(1_785_328_986, 355_887_000));
        blob.extend(f_bytes(9, &usage(4050, 375, 16275, "resp-ok")));
        blob.extend(f_bytes(28, &f_bytes(2, &usage(4000, 0, 0, "resp-failed"))));
        let (ts, usages) = parse_step(&blob);
        assert_eq!(ts, Some(1_785_328_986_355));
        assert_eq!(usages.len(), 2);
        assert_eq!(usages[1].response_id.as_deref(), Some("resp-failed"));
    }

    #[test]
    fn an_unset_timestamp_is_none_not_1970() {
        assert_eq!(parse_step(&f_bytes(1, &[])).0, None);
        assert_eq!(timestamp_ms(&f_varint(1, 0)), None);
    }

    #[test]
    fn nanos_beyond_the_second_they_describe_are_discarded() {
        // Normalising them would move a call near midnight onto the next day.
        assert_eq!(
            timestamp_ms(&stamp(1_785_328_986, NANOS_PER_SECOND + 355_887_000)),
            Some(1_785_328_986_000)
        );
    }

    #[test]
    fn malformed_blobs_yield_nothing_and_do_not_panic() {
        // Truncated length prefix, unterminated varint, group wire type, noise.
        assert!(parse_step(&[0x0a, 0x7f]).1.is_empty());
        assert!(parse_step(&[0x08, 0xff, 0xff]).1.is_empty());
        assert!(parse_step(&[0x0b, 0x00]).1.is_empty());
        assert!(parse_generations(&[0xff, 0xff, 0xff]).is_empty());
    }

    #[test]
    fn a_generation_is_found_inside_its_wrapper_without_knowing_its_field() {
        let found = parse_generations(&gen_blob(
            1_785_328_980,
            "gemini-3.6-flash",
            &usage(20110, 33, 0, "resp-a"),
        ));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].model.as_deref(), Some("gemini-3.6-flash"));
        assert_eq!(found[0].ts_ms, Some(1_785_328_980_000));
    }

    #[test]
    fn a_model_name_with_no_identifiable_usage_is_not_a_generation() {
        let blob = f_bytes(1, &f_bytes(19, b"gemini-3.6-flash"));
        assert!(parse_generations(&blob).is_empty());
    }

    // ── end to end over real SQLite ──────────────────────────────────

    fn tmp_home(tag: &str) -> PathBuf {
        let p =
            std::env::temp_dir().join(format!("tokenade-antigravity-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// Write one conversation database with the two tables we read.
    fn make_db(path: &Path, steps: &[Vec<u8>], gens: &[Vec<u8>]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE steps (idx INTEGER PRIMARY KEY, metadata BLOB);
             CREATE TABLE gen_metadata (idx INTEGER PRIMARY KEY, data BLOB);",
        )
        .unwrap();
        for (i, b) in steps.iter().enumerate() {
            conn.execute(
                "INSERT INTO steps (idx, metadata) VALUES (?1, ?2)",
                rusqlite::params![i as i64, b],
            )
            .unwrap();
        }
        for (i, b) in gens.iter().enumerate() {
            conn.execute(
                "INSERT INTO gen_metadata (idx, data) VALUES (?1, ?2)",
                rusqlite::params![i as i64, b],
            )
            .unwrap();
        }
    }

    #[test]
    fn a_missing_data_dir_is_silence_not_an_error() {
        let home = tmp_home("missing");
        assert!(AntigravityParser.aggregate_since(&home, None).is_empty());
        assert!(AntigravityParser.model_usage_by_day(&home, None).is_empty());
    }

    #[test]
    fn reads_nested_conversations_and_names_the_model_from_gen_metadata() {
        let home = tmp_home("nested");
        let convos = home.join(".gemini/antigravity-cli/conversations");
        make_db(
            &convos.join("aaa.db"),
            &[step_blob(1_785_328_986, &usage(4050, 375, 16275, "resp-a"))],
            &[gen_blob(
                1_785_328_986,
                "gemini-3.6-flash",
                &usage(4050, 375, 16275, "resp-a"),
            )],
        );
        // A sub-conversation, one directory down.
        make_db(
            &convos.join("aaa/sub.db"),
            &[step_blob(1_785_328_990, &usage(10, 20, 0, "resp-b"))],
            &[],
        );
        // Not a database, and must not be opened.
        std::fs::write(convos.join("notes.txt"), b"x").unwrap();

        let rows = AntigravityParser.aggregate_since(&home, None);
        assert_eq!(rows.len(), 1, "one unknown-cwd bucket: {rows:?}");
        assert_eq!(rows[0].input_tokens, 4060);
        assert_eq!(rows[0].output_tokens, 395);
        assert_eq!(rows[0].cache_read_tokens, 16275);
        assert_eq!(rows[0].assistant_messages, 2);

        let models = AntigravityParser.model_usage_by_day(&home, None);
        let day = models.values().next().unwrap();
        // Only `resp-a` was named; `resp-b`'s tokens counted above but cannot
        // be attributed to a model here.
        assert_eq!(day.len(), 1);
        assert_eq!(day.get("gemini-3.6-flash").unwrap().1, 375);

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn one_call_recorded_in_two_databases_is_billed_once() {
        // A sub-conversation repeats a call its parent already recorded — two
        // files, one bill. This is what `identity()` is for.
        let home = tmp_home("dedup");
        let convos = home.join(".gemini/antigravity-cli/conversations");
        let shared = step_blob(1_785_328_986, &usage(1000, 100, 0, "resp-same"));
        let one = std::slice::from_ref(&shared);
        make_db(&convos.join("parent.db"), one, &[]);
        make_db(&convos.join("child.db"), one, &[]);

        let rows = AntigravityParser.aggregate_since(&home, None);
        assert_eq!(rows[0].input_tokens, 1000);
        assert_eq!(rows[0].assistant_messages, 1);

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn an_invocation_with_no_identity_is_kept_rather_than_guessed_away() {
        let home = tmp_home("noid");
        let convos = home.join(".gemini/antigravity-cli/conversations");
        // Two distinct calls, neither carrying a server id.
        let mut anon = f_varint(2, 500);
        anon.extend(f_varint(3, 50));
        make_db(
            &convos.join("a.db"),
            &[
                step_blob(1_785_328_986, &anon),
                step_blob(1_785_328_987, &anon),
            ],
            &[],
        );

        let rows = AntigravityParser.aggregate_since(&home, None);
        assert_eq!(rows[0].input_tokens, 1000, "both must count");

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn the_since_cutoff_filters_by_timestamp() {
        let home = tmp_home("since");
        let convos = home.join(".gemini/antigravity-cli/conversations");
        make_db(
            &convos.join("a.db"),
            &[
                // 2026-07-30 and 2026-07-31 respectively.
                step_blob(1_785_369_600, &usage(100, 10, 0, "old")),
                step_blob(1_785_456_000, &usage(200, 20, 0, "new")),
            ],
            &[],
        );
        let rows = AntigravityParser.aggregate_since(&home, Some("2026-07-31T00:00:00Z"));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].input_tokens, 200);

        let _ = std::fs::remove_dir_all(&home);
    }
}
