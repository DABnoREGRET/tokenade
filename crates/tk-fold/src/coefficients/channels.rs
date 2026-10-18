//! **Does the channel truncate, and from what point?**
//!
//! A channel that CUTS beyond a ceiling saves nothing beyond it: what
//! the agent would never have received cannot be credited. A channel that does not
//! truncate pays for everything.
//!
//! The distinction decides every credit, and it is not always
//! knowable — hence `credit_is_approximate`, which announces it instead of
//! choosing silently.

use super::*;

/// Overflow-credit ratio (see [`DEFAULT_OVERFLOW_CREDIT_RATIO`]). Calibrated
/// value wins when present and in `[0, 1]`, else the default.
pub fn overflow_credit_ratio() -> f64 {
    match read_f64("overflow_credit_ratio") {
        Some(r) if (0.0..=1.0).contains(&r) => r,
        _ => DEFAULT_OVERFLOW_CREDIT_RATIO,
    }
}

/// The harness delivery cap (tokens) for a compaction event, chosen by what
/// the agent would have done WITHOUT tokenade — keyed off both `source` and
/// `op` because the counterfactual isn't visible from `source` alone:
///   - READ channel: the baseline is a `Read` of a file — `read_content_dedup`,
///     `postread`, and the read-replacement CLIs (`skeleton`/`query`/`map`/
///     `semantic`/`impact`/`digest`). Capped at the Read line-limit.
///   - BASH channel: the baseline is a shell command's stdout (`wrap`,
///     `hookwrap`). Capped at the Bash byte-limit.
/// An event we can't tie to either returns `u64::MAX` — no cap, realistic ==
/// naive (we never invent a discount for a channel we can't reason about).
/// Per-agent overrides as documented above.
pub fn delivery_cap_tokens(agent: &str, source: &str, op: &str) -> u64 {
    // Self-bounded credit assiettes (web / binary-document extraction):
    // their `before` is already the per-agent credit from
    // `web_credit_savings` / `binary_credit_savings`, not raw bytes —
    // capping again would double-discount an honest figure.
    let o = op.to_ascii_lowercase();
    if o.starts_with("html-to-markdown") || o == "serp_compact" || o.starts_with("extract-") {
        return u64::MAX;
    }
    let (read_like, bash_like, mcp_like) = classify_channel(source, op);
    let (channel, default) = if read_like {
        ("read", DEFAULT_CAP_READ_TOKENS)
    } else if bash_like {
        ("bash", DEFAULT_CAP_BASH_TOKENS)
    } else if mcp_like {
        ("mcp", DEFAULT_CAP_MCP_TOKENS)
    } else {
        // Un-tied channel (web/binary/image/estimated levers): these record an
        // ALREADY-bounded `before` via their own per-model credit assiette
        // (`web_credit_savings` / `binary_credit_savings` / image downscale),
        // so realistic == naive is honest here — there is no raw-bytes
        // over-count to clamp on this path.
        return u64::MAX;
    };
    let pick = |k: &str| read_f64(k).filter(|v| *v > 0.0).map(|v| v as u64);
    // calibrated override > per-agent source default > global fallback.
    // The per-agent table only covers read/bash — do NOT fall back to it for
    // the mcp channel (it would return the agent's BASH cap, not an MCP one).
    pick(&format!("cap_{channel}_{agent}"))
        .or_else(|| pick(&format!("cap_{channel}")))
        .or_else(|| {
            if read_like || bash_like {
                agent_channel_default(agent, read_like)
            } else {
                None
            }
        })
        .unwrap_or(default)
}

/// Classify the delivery channel of a compaction event from its `source`/`op`.
/// Returns `(read_like, bash_like, mcp_like)`; the first true one wins.
///   - READ: a `Read` of a file — `read_content_dedup`, `postread`, and the
///     read-replacement CLIs (`skeleton`/`query`/`map`/…) plus raw-output CLI
///     compactors that stand in for a Read (`csv`/`docs`/`pack`/…).
///   - BASH: a shell command's stdout (`wrap`, `hookwrap`).
///   - MCP: `mcp_tool_report` — source `wrap-mcp`, ops `mcp:*` / `mcp-proxy:*`.
pub(crate) fn classify_channel(source: &str, op: &str) -> (bool, bool, bool) {
    let s = source.to_ascii_lowercase();
    let o = op.to_ascii_lowercase();
    let read_like = s.contains("read")
        || o.contains("read")
        || o.contains("skeleton")
        || o.contains("query")
        || o == "map"
        || o.contains("semantic")
        || o.contains("impact")
        || o.contains("digest")
        || o.contains("turn-summary")
        || o.contains("docs")
        || o.contains("csv")
        || o.contains("openapi")
        || o.contains("graphql")
        || o.contains("sourcemap")
        || o.contains("pack")
        || o.contains("code-strip")
        // The code-intel commands book the IMPLEMENTATION's name, not the
        // command's: `tokenade query` writes `symbol_find`, `tokenade map`
        // writes `structure_map` (see `log_code_intel` call sites). The
        // clauses above name `query` and `map`, so neither ever matched —
        // the same mismatch as the `auto:*` one below, found by the same
        // method: read the ops the ledger CARRIES, not the ones the rule
        // hopes for. `impact` and `skeleton` matched only because
        // `impact_of` and `skeleton` happen to contain them.
        || o == "symbol_find"
        || o == "structure_map"
        // `source="cli"` file-content folds (`tokenade read` and friends):
        // `auto:*` is keyed off content shape, i.e. a Read stand-in.
        //
        // BOTH spellings, because the first one alone matched NOTHING. The
        // `auto:<kind>` string is the ledger row's LABEL; its `op` on that
        // path is always the literal `auto-compact`. Counted 2026-08-08 on a
        // real 33 817-row ledger: `s == "cli" && op.starts_with("auto:")`
        // matched 0 rows, while 2 162 rows are `("cli", "auto-compact")` with
        // an `auto:*` label. So every `tokenade read` fold — the channel all
        // six plugin bridges call — was discounted with the BASH cap (7 500)
        // instead of the READ one (20 000). Recomputed over those rows: 23.3M
        // realistic tokens against 44.8M, i.e. **1.92x under-reported**, and
        // the server sums exactly this figure for the quota.
        || (s == "cli" && (o.starts_with("auto:") || o == "auto-compact"));
    let mcp_like = s.contains("mcp") || o.starts_with("mcp:") || o.starts_with("mcp-proxy");
    // `source="cli"` is the PATH-shim / direct-CLI channel: raw shell-output
    // folds (`proxy`, `strings`, `grep`, `sysinfo`, `diff-compact`, …).
    // 0042-class over-count: classified to NO channel → u64::MAX cap →
    // unbounded claims (field: `auto:diff` 9.77M over 41 events, single
    // rows > 1M tokens "saved"). Same read/bash split as the server's
    // 0042 backfill so client and server agree. Kept DISJOINT from the
    // read/mcp flags: `channel_is_nontruncating` receives both booleans, so
    // a cli row must claim exactly one channel.
    let bash_like = s == "wrap"
        // EVERY hook spelling, not just `hookwrap`. The Claude/Cursor/
        // Antigravity pretools record `source="hook"`, which `contains
        // ("hookwrap")` does not match — so those rows fell through to the
        // un-tied arm and were credited with NO cap at all. Measured on a
        // live ledger 2026-09-02: 33 rows, 30 712 tokens (`session_resume_
        // saved`, `cd`, `passthrough`, `auto:*`), all of them a shell
        // command's stdout, i.e. the bash channel by definition.
        // Disjoint, like the `cli` clause below: an op that is read-like
        // keeps the READ channel even when the hook delivered it.
        || (s.starts_with("hook") && !read_like && !mcp_like)
        || s.contains("bash")
        || (s == "cli" && !read_like && !mcp_like);
    (read_like, bash_like, mcp_like)
}

/// Whether the `(agent, channel)` is NON-truncating — the harness would put the
/// WHOLE tool output in front of the model, so the per-agent cap is a
/// conservative model-INGESTION bound, NOT a harness truncation point. There is
/// then no truncated tail for a grep/Read dance to recover, so the overflow
/// credit must be 0 (else 27% of a 50 M `strings`/`xxd` binary dump is credited
/// — the over-count this exists to kill). Truncating channels (claude-code,
/// cursor, hermes, opencode, …) return false and keep the normal credit.
pub(crate) fn channel_is_nontruncating(agent: &str, read_like: bool, bash_like: bool) -> bool {
    // `crate::agent_caps` is the AUTHORITY wherever it has a measured row: it
    // holds the truncation rule read out of the installed package, and
    // "does this channel truncate" is literally `Limit::is_unbounded`. The
    // hand-written list below is the fallback for agents the table does not
    // cover yet. Keeping two answers to one question is what let gemini-cli sit
    // in the list for weeks after its package started truncating by default.
    let action = if read_like {
        Some(crate::agent_caps::Action::ReadFile)
    } else if bash_like {
        Some(crate::agent_caps::Action::Command)
    } else {
        None
    };
    if let Some(action) = action {
        if let Some(cap) = crate::agent_caps::cap(agent, action) {
            return cap.limit.is_unbounded();
        }
    }
    match agent {
        // read_file returns the whole file, no built-in truncation (cline#4576).
        "cline" => read_like,
        // shell output is not truncated at model-delivery time (disk-only
        // buffer / unpublished — see the table above).
        //
        // gemini-cli LEFT this list on 2026-08-10. It was here on the strength
        // of gh discussion #8297 ("auto-truncation is an opt-in preview"), and
        // the installed package contradicts that twice over:
        // `enableToolOutputTruncation ?? true` (config.js) and
        // `saveTruncatedContent` applied to every SHELL_TOOL_NAME result
        // (scheduler/tool-executor.js). See `crate::agent_caps`, which now
        // holds the measured rule, and `gemini_shell_truncates_by_default…`.
        // antigravity LEFT this list on 2026-08-10, measured in a live session:
        // a 9 000-line command output came back as `<truncated 8801 lines>`
        // plus the final 199. It truncates, so a recoverable tail exists and
        // the credit is earned. Second agent to leave for the same reason.
        // grok LEFT on 2026-08-11, measured in a live session with a
        // self-indexing payload: above 40 000 characters it keeps 10 000 at
        // each end. The note it left behind said its 64 MB figure was a disk
        // buffer and not model delivery — true, and it hid a real delivery cap
        // behind that truth. THIRD agent to leave this list (gemini-cli,
        // antigravity, grok), all three in the same direction: tokenade was
        // under-declaring its gain on each.
        "windsurf" | "devin-desktop" => bash_like,
        // run_cmd = full output AND /add = whole file: both channels untruncated.
        "aider" => read_like || bash_like,
        _ => false,
    }
}

/// Does a credit on this channel rest on an APPROXIMATION rather than exact
/// arithmetic? True where the agent REFUSES above its cap instead of
/// truncating: nothing is delivered, the user re-reads in slices, and no ledger
/// row can reconstruct what that really cost. Crediting the cap is a decision
/// (see [`crate::agent_caps::CreditBasis`]); this predicate is what lets a
/// surface LABEL the resulting figure instead of passing it off as measured.
/// How much of one row's saving rests on an APPROXIMATION rather than exact
/// arithmetic — 0 when the row is exact.
///
/// The rule is: the channel credits on a stand-in ([`credit_is_approximate`])
/// AND the delivery cap actually bit ([`delivery_cap_tokens`]). A payload
/// under the cap is delivered whole and its saving is exact, so it must not be
/// labelled. Both halves matter and the second is easy to forget.
///
/// This exists as one function because two ledger scanners already walk the
/// same rows — `gain::LedgerScan` and `dashboard_cli::scan_gain_ledger` — and
/// only the first had the test, so `tokenade gain` printed a total with no
/// mention of the slice while the signed receipt disclosed it. A third scanner
/// must call this rather than restate it.
pub fn approximated_slice(agent: &str, source: &str, op: &str, before: u64, after: u64) -> u64 {
    if !credit_is_approximate(agent, source, op) {
        return 0;
    }
    if before <= delivery_cap_tokens(agent, source, op) {
        return 0;
    }
    before.saturating_sub(after)
}

pub fn credit_is_approximate(agent: &str, source: &str, op: &str) -> bool {
    let (read_like, bash_like, mcp_like) = classify_channel(source, op);
    let action = if read_like {
        crate::agent_caps::Action::ReadFile
    } else if mcp_like {
        crate::agent_caps::Action::McpToolResult
    } else if bash_like {
        crate::agent_caps::Action::Command
    } else {
        return false;
    };
    crate::agent_caps::credit_basis(agent, action).is_approximate()
}

/// Public predicate: is the `(agent, source, op)` channel NON-truncating (see
/// [`channel_is_nontruncating`])? The recalc migration uses it to target ONLY
/// the ex-`u64::MAX` binary-dump rows for re-capping — truncating channels keep
/// their existing figures untouched.
pub fn channel_nontruncating(agent: &str, source: &str, op: &str) -> bool {
    let (read_like, bash_like, _mcp) = classify_channel(source, op);
    channel_is_nontruncating(agent, read_like, bash_like)
}

/// Overflow-credit ratio for the `(agent, source, op)` channel:
/// [`overflow_credit_ratio`] for a real (truncating) channel whose truncated
/// tail an agent recovers with grep/Read dances, but 0 for a NON-truncating
/// channel (see [`channel_is_nontruncating`]) where nothing was truncated —
/// crediting a fraction of a raw binary dump's "tail" is the over-count.
pub fn overflow_ratio_for_channel(agent: &str, source: &str, op: &str) -> f64 {
    // An agent NOBODY has measured is not an agent that truncates.
    //
    // `channel_is_nontruncating` falls back to `_ => false`, and `false` means
    // "it truncates", which is the direction that CLAIMS — a recoverable tail
    // credited at `overflow_credit_ratio` on no evidence at all. A third state
    // is the honest one: nobody looked, so nothing is claimed.
    if !agent_truncation_is_known(agent) {
        return 0.0;
    }
    // A recovery row IS the grep/Read dance. The overflow credit exists to
    // pay for the dance a truncated tail would have cost; on this op that
    // dance is the row itself, so adding it again credits the same recovery
    // twice. Bound is then exactly `min(before, cap) - after`: one re-run,
    // truncated like any other, minus what this round delivered.
    if op.eq_ignore_ascii_case("digest_recovery") {
        return 0.0;
    }
    let (read_like, bash_like, _mcp) = classify_channel(source, op);
    if channel_is_nontruncating(agent, read_like, bash_like) {
        0.0
    } else {
        overflow_credit_ratio()
    }
}

/// Has anyone measured this agent's truncation behaviour at all?
///
/// The table is the record of that measurement; an agent absent from it has
/// never been looked at, and [`overflow_ratio_for_channel`] credits it nothing.
pub fn agent_truncation_is_known(agent: &str) -> bool {
    crate::agent_caps::has_any_row(agent)
}
