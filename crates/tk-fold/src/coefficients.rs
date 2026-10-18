//! Calibration coefficients for the APPROXIMATE savings levers (output-lean,
//! …). Defaults are bench-derived constants; `tokenade calibrate` overwrites
//! them from a fresh THOL run into `~/.tokenade/coefficients.json`. Readers
//! fall back to the default whenever the file is absent or a value is out of
//! range, so a missing/garbled calibration never produces a wild estimate.

use std::path::PathBuf;

mod calibration;
mod channels;
mod web_binary;
pub use calibration::*;
pub use channels::*;
pub use web_binary::*;

/// Default fraction of PRODUCED output tokens that the scaffold's "keep output
/// lean" rules save — i.e. `(control_out − tokenade_out) / tokenade_out`.
/// MEASURED on the latest board (Claude Code 2.1.206, 2026-07-10 → 07-18, 17
/// tasks, 170 runs per arm, paired by task): control 6 285 output tokens per
/// run, tokenade 4 827 — **0.302**. It was 0.35, a cut measured against the
/// PRIOR tokenade version rather than against control, which is what this
/// ratio means; every machine without a calibration was credited 16 % too
/// much. Recalibrate from a fresh `results.sqlite` with `tokenade calibrate`.
pub const DEFAULT_OUTPUT_LEAN_RATIO: f64 = 0.30;

/// Credit applied to the OVERFLOW tokens (raw above the harness delivery cap)
/// when estimating REALISTIC compaction savings. The agent never ingests a
/// 1.2 MB file raw — the harness truncates at the cap — so `raw − compacted`
/// overstates the gain (a measured "95%" on a huge file is against a world
/// that never happens). But the truncated tail isn't free either: without
/// tokenade the agent recovers it with grep/Read dances (more tokens, more
/// turns — real but unmeasurable, and growing with session length). We credit
/// a fraction of the overflow as saved. 0.27 is the 2026-06 middle-ground:
/// `cap + 0.27·overflow` deflates a "98% claimed" huge-file compaction to
/// ~30% effective — neither the fiction of 95% nor the under-count of a hard
/// cap. Calibratable via `~/.tokenade/coefficients.json:overflow_credit_ratio`.
pub const DEFAULT_OVERFLOW_CREDIT_RATIO: f64 = 0.27;

/// Fallback harness "delivery cap" in tokens — the most a single Read / Bash
/// result can place in front of the model before the harness truncates. Used
/// only for agents not in the per-agent table below (`hermes`, `unknown`).
/// Overridable in coefficients.json (`cap_read_<agent>` / `cap_bash_<agent>`,
/// then `cap_read` / `cap_bash`, then these defaults).
pub const DEFAULT_CAP_READ_TOKENS: u64 = 20_000;
pub const DEFAULT_CAP_BASH_TOKENS: u64 = 7_500;
/// MCP tool-result delivery cap. Without it the `wrap-mcp` / `mcp:*` channel
/// fell through to `u64::MAX` — a 500k JSON response credited 500k "saved"
/// that the agent could never have ingested. Override: `cap_mcp[_<agent>]`.
///
/// **20 000, and it is a DELIBERATE under-estimate (D7, 2026-08-10).** Two MCP
/// tools that do the same thing were capped differently because the rule reads
/// the tool's NAME, not what it does: `mcp:read_file` matches the read-like
/// pattern and inherited the 20 000 read cap, while `mcp:get_file_contents`
/// missed it and fell through to the MCP default. Same operation, two
/// counterfactuals, and the difference was a substring.
///
/// The measured Claude Code limit is ~25k (`MAX_MCP_OUTPUT_TOKENS`), so the
/// honest-for-that-agent number is higher than this. Levelling DOWN rather than
/// up is the conservative direction: it under-states savings on the MCP channel
/// instead of over-stating them, and the server sums this figure for the
/// monthly quota. Both halves of that trade-off are real, which is why the
/// standing decision is that caps must move to ONE table keyed by (agent,
/// channel) with each value traced to the harness constant it came from —
/// until then, no per-tool number here can be called correct, only consistent.
pub const DEFAULT_CAP_MCP_TOKENS: u64 = 20_000;

/// Per-agent (read, bash) delivery caps in tokens, from a 2026-06 source audit
/// of each harness's truncation constants (lines×~10, chars÷4). `u64::MAX`
/// means the harness does NOT truncate that channel — it would put the whole
/// file/output in front of the model — so tokenade's raw saving there is
/// HONEST and gets no discount. Sources per agent:
///   claude-code  Read 2000 lines / Bash 30 000 chars (BASH_MAX_OUTPUT_LENGTH)
///   codex        no file tool (reads via shell); exec DEFAULT_MAX_OUTPUT_TOKENS=10k
///   gemini-cli   read_file 2000 lines / shell NO hard model-delivery trunc
///                (auto-truncation is opt-in preview, no default cap — gh
///                discussion #8297); conservative peer-default bash cap.
///   cursor       read_file 250 lines / shell ~20 000 chars (staff-confirmed)
///   copilot      view 20 KB / read_bash ~30 KB (CLI changelog + RE)
///   cline        read_file returns the WHOLE file, no built-in truncation
///                (cline#4576) / terminal 500 lines; read bounded to a
///                conservative peer read-default (a model ingests at most a big
///                Read in one delivery, never the raw buffer).
///   kilo-code    read 500 lines / terminal 500 lines (OpenCode lineage)
///   windsurf     read 20 000 chars, lines over 2 000 truncated / shell
///                announced above 30 000 chars, delivery unread (ceiling).
///   opencode     read 2000 lines or 50 KB / shell 2000 lines or 50 KB
///   pi,openclaw  read 2000 lines or 50 KB / shell 2000 lines or 50 KB (same lineage)
///   antigravity  view_file 800 lines / shell cap unverified (closed source) —
///                conservative peer default.
///   grok         read = pi's native read tool (truncates, pi lineage → 12 800)
///                / shell output only capped at 64 MB ON DISK after exit (x.ai
///                changelog) — that is the buffer, not model delivery, so a
///                conservative peer-default bash cap.
///   aider        run_cmd returns the FULL command output, no truncation
///                (Aider-AI run_cmd.py) and /add reads whole files — both
///                bounded to conservative peer defaults, never the raw buffer.
///   odysseus     read 20 000 chars / shell 10 000 chars
///
/// NOTE: the ex-`u64::MAX` "no trunc" channels above are NON-truncating — the
/// per-agent value is a conservative model-INGESTION bound, not a harness
/// truncation point. [`channel_is_nontruncating`] flags them so the overflow
/// credit (recover a truncated tail) is suppressed for them.
fn agent_channel_default(agent: &str, read_channel: bool) -> Option<u64> {
    // `crate::agent_caps` is the AUTHORITY wherever it has a row that yields a
    // scalar. It carries the measurement AND, where a decision overrode it, the
    // credited figure — so the two tables can no longer disagree by drifting
    // apart, which is what the divergence inventory below used to police by
    // hand. The hand-written table remains for agents the module does not
    // cover, and for the channels whose rule (per-line, head/tail) has no
    // scalar equivalent at all.
    let action = if read_channel {
        crate::agent_caps::Action::ReadFile
    } else {
        crate::agent_caps::Action::Command
    };
    if let Some(cap) = crate::agent_caps::cap(agent, action) {
        if let Some(tokens) = cap.credit_tokens() {
            return Some(tokens);
        }
    }
    let (read, bash): (u64, u64) = match agent {
        "claude-code" => (20_000, 7_500),
        "codex" => (10_000, 10_000),
        // Droid: the hook contract is Claude-Code-compatible, and the caps are
        // NOT. Measured 2026-08-10 from the installed Factory binary: shell
        // output is truncated at 40 000 characters = 10 000 tokens, against the
        // 7 500 inherited from claude-code. The old comment said "until
        // measured otherwise" and this is that. Read stays at the conservative
        // 20 000: the measured rule there is 2 400 LINES, which has no scalar
        // equivalent — see `crate::agent_caps`.
        "droid" => (20_000, 10_000),
        // shell: no hard model-delivery truncation (opt-in preview only) →
        // conservative peer default aligned with the 2000-line/50 KB class.
        "gemini-cli" => (20_000, 12_800),
        // qwen: 25 000 chars = 6 250 tokens, and the SAME rule bounds every
        // tool output — read and shell alike (`processSingleFileContent` reads
        // `getTruncateToolOutputThreshold()` as its char limit). Measured
        // 2026-08-10; before that qwen had no row at all and inherited the
        // 20 000/7 500 global default, i.e. over-stated its read channel 3x.
        "qwen" => (6_250, 6_250),
        "cursor" => (2_500, 5_000),
        // copilot: 51 200 bytes = 12 800 tokens on BOTH channels, measured
        // 2026-08-10 (`xBe = 50 * 1024` in app.js, the default
        // `largeOutputOptions.maxOutputSizeBytes`, passed to the file view and
        // to the shell session factory alike). Replaces (5 000, 7 500), which
        // came from a changelog reading of "view 20 KB / read_bash ~30 KB" —
        // two numbers, neither of them this one.
        "copilot" => (12_800, 12_800),
        // read_file = whole file (no trunc) → conservative peer read default.
        // bash 12 000 = the MEASURED 48 000 characters (2026-08-10, read out of
        // cline's own embedded prompt: "Output beyond ~48k characters is
        // middle-truncated"). It replaced a 5 000 that came from a "terminal
        // 500 lines" note no source ever supported. This RAISES the credited
        // figure on cline's shell channel, and it raises it toward the truth:
        // the old number under-stated what the agent really delivers.
        "cline" => (20_000, 12_000),
        "kilo-code" => (5_000, 5_000),
        // windsurf: read measured 2026-08-12 off the `devin` CLI bundled in
        // devin-desktop — 20 000 characters, so 5 000 tokens, replacing a
        // number taken from the IDE's `view_file`. The shell figure is a
        // CEILING, not a measurement: the agent announces truncation above
        // 30 000 characters and the delivered amount could not be read at rest,
        // so 7 500 tokens is the most it can be. The 12 800 it replaces was a
        // peer default on a channel this table called unpublished.
        // `devin-desktop` has been the agent's slug since 2026-09; `windsurf`
        // stays for the gain rows recorded before.
        "windsurf" | "devin-desktop" => (5_000, 7_500),
        "opencode" => (12_800, 12_800),
        // pi: measured 2026-08-11 — `truncate.js` caps at 2 000 lines OR
        // 51 200 bytes, whichever bites first. 12 800 tokens is the byte half;
        // the line half has no scalar equivalent (see `crate::agent_caps`).
        // The pre-existing 12 800 was a peer default that happens to be right.
        "pi" => (12_800, 12_800),
        // t3-code: no agent of its own, it drives the codex engine over
        // app-server. Same figures as codex, by delegation.
        "t3-code" => (10_000, 10_000),
        // openclaw: measured 2026-08-12 — it has no file or shell tool of its
        // own, it bundles `@mariozechner/pi-coding-agent` and inherits pi's
        // 2 000 lines / 51 200 bytes on both. 12 800 tokens is the byte half.
        // The pre-existing figure was a peer default that happens to be right,
        // and it is now right for a reason instead of by luck.
        "openclaw" => (12_800, 12_800),
        // shell cap unverified → conservative peer default.
        "antigravity" => (8_000, 12_800),
        // grok, measured 2026-08-11: read 1 000 lines (no scalar equivalent,
        // see `crate::agent_caps`), command delivers 20 000 characters =
        // 5 000 tokens above a 40 000-character threshold. The old 12 800 came
        // from "64 MB is a disk buffer, not model delivery" — correct, and it
        // stood in for a delivery cap nobody had looked for.
        "grok" => (12_800, 5_000),
        // run_cmd = full output, /add = whole file → conservative peer defaults.
        "aider" => (20_000, 12_800),
        "odysseus" => (5_000, 2_500),
        // NousResearch hermes-agent (Python): read_file hard-max 2000 lines/call
        // / terminal 50 000 chars. (The Agent enum's "generic MCP bucket" note is
        // stale — it's a concrete product: github.com/nousresearch/hermes-agent.)
        "hermes" => (20_000, 12_500),
        // unknown / anything new -> fall back to the global default.
        _ => return None,
    };
    Some(if read_channel { read } else { bash })
}

#[cfg(test)]
include!("coefficients_tests.rs");

// ─────────────────────────────────────────────────────────────────────
// "Does this row have a BORROWED baseline?"
// ─────────────────────────────────────────────────────────────────────
//
// The rule saying what counts as measured lived in `gain`, with the
// two classes it names. Three consumers read it — the dashboard,
// the value ranking, and the code-intel heuristic, which is the
// CODE layer and so climbed all the way up to the product layer to ask
// a coefficient question.
//
// Its place is here, with `ALWAYS_COUNTERFACTUAL_OPS` which it already consults.

/// Like [`record`] but tags the exact row with a `class` (e.g. `"dedup"`).
/// Still lands in `gain.jsonl` — the saving is real/measured, not estimated.
/// Use for exact non-compaction savings that were previously unlogged
/// (content-addressed command-output dedup, where we hold the folded bytes).
/// The class of a row whose baseline was COMPUTED from what was
/// delivered: a published multiplier, a flat per-turn credit.
pub const CLASS_COUNTERFACTUAL: &str = "counterfactual";

/// The class of a code-intelligence row, whose baseline is the published ratio
/// of `code_intel_heuristic`.
pub const CLASS_CI_HEURISTIC: &str = "ci_heuristic";

/// **Was this row's "before" READ, or computed?**
///
/// One rule, three provenances, because the product fabricates a baseline in three
/// places and a consumer cannot know them one by one:
///
/// * `ci_heuristic` — CodeGraph's published ratio applied to the answer;
/// * `counterfactual` — a multiplier or a per-turn credit from
///   `coefficients`, set by `record_inner` from the flag the
///   `Savings` itself carries;
/// * `before == after` — the row arrived without a delta and a reader
///   multiplies it;
/// * and the `op` appears in [`crate::coefficients::ALWAYS_COUNTERFACTUAL_OPS`],
///   which answers for rows WRITTEN BEFORE the stamp — without which one hundred
///   and four days of ledger would stay silent.
///
/// Everything else had a real input: `skeleton` tokenises the file it is
/// handed, and only the ALTERNATIVE ("you would have read it whole") is assumed.
///
/// It lives here, with the ledger it talks about, because an earlier version
/// lived next to ONE of the three ratios and so could not see the other
/// two: `web-search` came out at −67 %, exactly 1 − 1/3, amid
/// measured rates.
pub fn baseline_is_borrowed(class: Option<&str>, op: &str, before: u64, after: u64) -> bool {
    matches!(class, Some(CLASS_CI_HEURISTIC) | Some(CLASS_COUNTERFACTUAL))
        || ALWAYS_COUNTERFACTUAL_OPS.contains(&op)
        || before == after
}
