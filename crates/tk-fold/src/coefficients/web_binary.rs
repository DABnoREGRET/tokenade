//! **What the agent would REALLY have spent without us** — web and binary
//! documents.
//!
//! The credit is not the bytes we fold: it is the cost of the path the
//! agent would have taken. And that path depends on the AGENT — interception is
//! transparent on Claude Code alone (WebFetch hook, native PDF passthrough);
//! everywhere else it goes through MCP or the shell.
//!
//! Counting the folded bytes would amount to claiming a saving on a
//! path nobody would have taken.

use super::*;

// ---- Per-agent WEB / BINARY credit model ----------------------------------
//
// Mirror of `agent_channel_default`, but for the web + binary-document
// channels. The principle: credit "what an agent would REALLY have spent to
// get the useful info WITHOUT us", keyed on the agent — NOT the raw bytes we
// happen to fold. Web/binary interception is transparent ONLY on Claude Code
// (WebFetch hook + native-PDF passthrough); every other agent reaches these
// compactors via MCP (`web_html_to_markdown`/`serp`/`docs_extract`) or shell,
// so the counterfactual — and thus the honest credit — differs by agent.

/// What an agent's web tooling would have cost it WITHOUT tokenade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebCreditModel {
    /// Transparent native web-fetch that already returns model-clean text
    /// (Claude Code `WebFetch`). The counterfactual is that clean text — a
    /// FRACTION of the raw HTML — not the raw HTML itself.
    Transparent,
    /// The agent's own web tool already returns reader-clean markdown
    /// (Jina/Reader-style). tokenade adds ~no content value; the gain is
    /// reliability, credited as a small flat turn credit.
    ReaderClean,
    /// The native fetch hands the page to a SMALLER model, which answers the
    /// prompt; only that answer reaches the main model (Claude Code
    /// `WebFetch`, read in 2.1.299: turndown markdown, first 100 000
    /// characters, to the small fast model — Haiku by default). Pricing that
    /// call page by page needs the page as the small model saw it, its
    /// instructions, its answer and both models' prices — per agent. The
    /// baseline is instead the MEASURED task-level economy: the same web task
    /// costs ~3× more without tokenade (operator bench, ~150 runs, −67 %), so
    /// what tokenade delivers is credited at 3× — see
    /// [`web_economy_credit_savings`].
    NativeSummary,
    /// No clean reader — the agent would have ingested the RAW HTML via shell
    /// (`curl`/`cat`). Honest full-HTML saving (~−75%).
    RawHtmlShell,
    /// No web tool at all. Same crediting floor as `RawHtmlShell` (worst case
    /// it still pays raw HTML somehow, or fails).
    NoWebTool,
    /// We could NOT classify this agent's web tooling — it is absent from
    /// [`agent_web_binary_model`]'s table, or detection never named it at all.
    /// Credit the RELIABILITY FLOOR (delivered text + one turn), never the
    /// raw-HTML ceiling: `RawHtmlShell` asserts the agent would have ingested
    /// the whole page, and for an agent we cannot name that is a claim, not a
    /// measurement. Move an agent OUT of this state by measuring it and adding
    /// its row — the same "whitelist, extend only with evidence" rule
    /// `Agent::reads_pdf_natively` already applies to the binary side.
    Unmeasured,
}

/// What an agent's binary-document tooling would have cost it WITHOUT tokenade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryCreditModel {
    /// Renders the document NATIVELY (Claude Code PDFs — visual render +
    /// built-in pagination). tokenade adds nothing → credit 0.
    NativePdf,
    /// Extract-or-fail: without tokenade the agent burns fumbling turns
    /// (reasoning + a failed/garbled tool result) before giving up. Credit the
    /// turns avoided, NOT the fictional `bytes/4`.
    ExtractOrFail,
    /// No doc tool at all — same floor as `ExtractOrFail` (at least the turn
    /// tax; a future refinement could weight the delivered value).
    NoDocTool,
}

/// Fraction of the RAW HTML tokens that a transparent native web-fetch
/// (Claude Code's `WebFetch`) delivers as model-clean text. The `Transparent`
/// credit is `ratio · raw` — NEVER the full raw HTML, because the agent never
/// ingests raw HTML.
///
/// CALIBRATION BENCH IN PROGRESS — conservative default. An A/B bench is running
/// to settle what Claude's native WebFetch really delivers (2k vs 17k). Until
/// it lands, DO NOT over-credit: 0.6 with the conservative guard in
/// [`web_credit_savings`] (if `0.6·raw ≤ after`, saving 0 — we never claim to
/// beat the native clean text). Recalibrate via
/// `~/.tokenade/coefficients.json:web_native_clean_ratio`.
pub const WEB_NATIVE_CLEAN_RATIO: f64 = 0.6;

/// Flat, low reliability credit (tokens) for a `ReaderClean` agent: the gain
/// is fewer failed fetches, not fewer tokens. Calibratable via
/// `web_reliability_turn_credit`.
pub const WEB_RELIABILITY_TURN_CREDIT: u64 = 400;

/// Turn-tax (tokens) credited for a binary document under `ExtractOrFail` /
/// `NoDocTool`: the fumbling turns (reasoning + a missed tool result) an agent
/// avoids by getting extracted text immediately. ≈ 2×(reasoning + failed
/// result). Calibratable via `binary_turn_tax`.
pub const BINARY_TURN_TAX: u64 = 800;

/// Calibrated `WEB_NATIVE_CLEAN_RATIO` (in `[0, 1]`) else the default.
pub fn web_native_clean_ratio() -> f64 {
    match read_f64("web_native_clean_ratio") {
        Some(r) if (0.0..=1.0).contains(&r) => r,
        _ => WEB_NATIVE_CLEAN_RATIO,
    }
}

/// Calibrated `WEB_RELIABILITY_TURN_CREDIT` (≥ 0) else the default.
pub fn web_reliability_turn_credit() -> u64 {
    read_f64("web_reliability_turn_credit")
        .filter(|v| *v >= 0.0)
        .map(|v| v as u64)
        .unwrap_or(WEB_RELIABILITY_TURN_CREDIT)
}

/// Calibrated `BINARY_TURN_TAX` (≥ 0) else the default.
pub fn binary_turn_tax() -> u64 {
    read_f64("binary_turn_tax")
        .filter(|v| *v >= 0.0)
        .map(|v| v as u64)
        .unwrap_or(BINARY_TURN_TAX)
}

/// Per-agent `(web, binary)` credit models — mirror of
/// [`agent_channel_default`]. Keyed on the canonical agent slug from
/// [`crate::detect::Agent::as_str`]. Unknown/new agents get the conservative
/// `RawHtmlShell`/`ExtractOrFail` default.
pub fn agent_web_binary_model(agent: &str) -> (WebCreditModel, BinaryCreditModel) {
    use BinaryCreditModel::*;
    use WebCreditModel::*;
    match agent {
        // Not `Transparent` (0.6 × raw HTML): Claude Code's `WebFetch` puts a
        // SUMMARY in the main context, written by a small model. MEASURED
        // 2026-10-08 on every native WebFetch result in the local history
        // (n = 10, the rest were served by tokenade): 230 tokens median, p75
        // 277, p90 2 461 — against a credit of 0.6 × raw that reached 435 k
        // for one page. Nor `ReaderClean` (1.2.3): the small model READ the
        // page, and that call is paid too — credited by the measured web
        // economy (operator, 2026-10-15).
        "claude-code" => (NativeSummary, NativePdf),
        "codex" => (RawHtmlShell, ExtractOrFail),
        "gemini-cli" | "qwen-code" => (ReaderClean, ExtractOrFail),
        "cursor" | "opencode" | "kilo-code" | "windsurf" | "devin-desktop" | "antigravity" => {
            (ReaderClean, ExtractOrFail)
        }
        // `copilot-vscode` is the same product surface as `copilot`; leaving it
        // to the catch-all made it the one Copilot accounted differently.
        "cline" | "copilot" | "copilot-vscode" => (NoWebTool, NoDocTool),
        "grok" | "droid" | "pi" | "aider" | "hermes" | "openclaw" => (RawHtmlShell, ExtractOrFail),
        // NOT classified: credit the floor, never the ceiling.
        //
        // It used to be `RawHtmlShell`, the table's LARGEST web credit: the
        // full raw HTML as `before`, uncapped downstream (`delivery_cap_tokens`
        // returns `u64::MAX` for this channel on purpose). Measured on this
        // machine's own 204 `webfetch_compact` rows, replaying them under that
        // default credits 2_560_186 tokens against the 1_439_804 the one agent
        // we have actually measured is credited: 1.78x MORE for knowing LESS.
        // The comment calling it "conservative" had it backwards; conservative
        // in an accounting figure means under-claiming.
        //
        // The BINARY side keeps `ExtractOrFail` for those same agents, and the
        // asymmetry is deliberate: its credit is a flat, bounded
        // [`BINARY_TURN_TAX`] (800) for a fumbling turn we can see we saved,
        // where the web ceiling grows without bound with the page.
        _ => (Unmeasured, ExtractOrFail),
    }
}

/// The BINARY model for an agent AND a document format.
///
/// [`agent_web_binary_model`] answers on the agent alone, and one of its answers
/// is `NativePdf` — "this agent renders the document itself, so tokenade adds
/// nothing, credit 0". That is true of Claude Code **and PDFs**. It is not true
/// of a `.xlsx`, a `.docx`, an `.epub` or an `.odt`: nothing renders those
/// natively, tokenade's extraction is exactly what makes them readable, and the
/// saving is as real as it is for any other agent.
///
/// The read hook already says so in a comment — "Claude+PDF already passed
/// through above (NativePdf → credit 0 for anything that still reaches here)" —
/// and then asks the agent-only table anyway, so every spreadsheet and every
/// Word document Claude Code reads through tokenade was booked at ZERO. Same
/// shape as before: a model chosen by one half of a question that has two.
pub fn binary_model_for(agent: &str, is_pdf: bool) -> BinaryCreditModel {
    let (_, model) = agent_web_binary_model(agent);
    match model {
        // Native rendering is a claim about PDFs, made by the whitelist
        // `Agent::reads_pdf_natively`. Off the PDF path it does not apply.
        BinaryCreditModel::NativePdf if !is_pdf => BinaryCreditModel::ExtractOrFail,
        other => other,
    }
}

/// Has this agent's WEB tooling been classified in [`agent_web_binary_model`]?
///
/// `false` means the credit is the [`WebCreditModel::Unmeasured`] floor — not
/// that the agent is unsupported. Mirrors [`agent_truncation_is_known`], which
/// asks the same question of the truncation table.
pub fn agent_web_model_is_measured(agent: &str) -> bool {
    agent_web_binary_model(agent).0 != WebCreditModel::Unmeasured
}

/// Credit the WEB assiette for `model`. `raw_before` = the raw-HTML token
/// count (the current `sav.tokens_before`); `after` = the delivered markdown
/// tokens. Returns the credit `Savings` (before = what the agent would really
/// have paid). `saved = max(0, before − after)` is enforced downstream by the
/// anti-inflation backstop in `gain::record_inner`, so a `Transparent` credit
/// where `0.6·raw ≤ after` records a clean 0. This changes ONLY the credit
/// assiette — never what tokenade delivers to the agent.
pub fn web_credit_savings(
    model: WebCreditModel,
    raw_before: usize,
    after: usize,
) -> crate::Savings {
    let before = match model {
        // CALIBRATION BENCH IN PROGRESS — conservative default: 0.6·raw, with the
        // guard that if 0.6·raw ≤ after we credit 0 (never over-credit vs the
        // native clean text Claude's WebFetch would have delivered).
        WebCreditModel::Transparent => {
            (raw_before as f64 * web_native_clean_ratio()).round() as usize
        }
        // Honest full-HTML saving — the agent would have ingested raw HTML.
        WebCreditModel::RawHtmlShell | WebCreditModel::NoWebTool => raw_before,
        // Content gain ≈ 0; small reliability credit on top of delivered size.
        // No page-by-page truth for these: the measured web economy, taken on
        // Claude Code — the reference every agent's web tooling is held to
        // (operator, 2026-10-15). It replaced a flat reliability credit.
        WebCreditModel::NativeSummary
        | WebCreditModel::ReaderClean
        | WebCreditModel::Unmeasured => return web_economy_credit_savings(after),
    };
    let sav = crate::Savings::new(before, after);
    // Per BRANCH, not per function: two of the four read the real page,
    // two fabricate it from what was delivered. Stamping the whole
    // function would disown an honest measurement.
    match model {
        WebCreditModel::ReaderClean
        | WebCreditModel::NativeSummary
        | WebCreditModel::Unmeasured => sav.counterfactual(),
        WebCreditModel::Transparent | WebCreditModel::RawHtmlShell | WebCreditModel::NoWebTool => {
            sav
        }
    }
}

/// Credit a page — whole or an excerpt — by the measured web economy:
/// Claude Code ([`WebCreditModel::NativeSummary`]), and every agent whose
/// counterfactual is not read page by page (`ReaderClean`, `Unmeasured`).
/// `after` = what tokenade delivers.
///
/// The baseline is the measured web economy, not a per-page reconstruction:
/// on the operator's bench (~150 runs across web tasks, Claude Code) the same
/// task cost 67 % less with tokenade, so the native path is `after × 3`
/// ([`web_fetch_scoped_native_multiplier`]). It includes the small model's
/// reading of the page, which a per-page count would have to price for every
/// agent; no cap at the page's size, since the small model read the page
/// whole and the main model then worked from its answer.
pub fn web_economy_credit_savings(after: usize) -> crate::Savings {
    let before = (after as f64 * web_fetch_scoped_native_multiplier()).round() as usize;
    crate::Savings::new(before.max(after), after).counterfactual()
}

/// Credit a SITE-READER intercept (`site_readers::Plan`). `delivered` = the
/// tokens of the rendered markdown the agent receives.
///
/// Neither [`web_credit_savings`] nor [`web_fetch_scoped_credit_savings`] can
/// serve this path: both need the ORIGINAL page's size, and a reader
/// deliberately never fetches it — it fetches an ALTERNATE representation (a
/// REST API, `raw.githubusercontent.com`). Measuring the fetched body against
/// the delivered markdown, as this path used to, is not a saving at all:
///
///   * the GitHub-issue reader booked ~11.5k tokens "saved" for compressing an
///     API JSON blob that would NEVER have entered the agent's context;
///   * the blob reader booked ~0 for handing over a source file the native
///     code-viewer page could not have produced at all.
///
/// So the reader's gain is not compression, it is RELIABILITY: content the
/// native path could not reach (lazy-loaded comments, a JS shell, a code
/// viewer). That is exactly what [`web_reliability_turn_credit`] already
/// prices, so a reader books that flat credit and nothing more.
///
/// Deliberately conservative and NOT model-aware: an agent with no native web
/// tool would genuinely have ingested the raw page, but we cannot know its
/// size without fetching what we just avoided fetching. Under-crediting is the
/// safe direction — and it keeps this path from double-counting the stash
/// layer, which separately credits holding a large body out of context.
pub fn reader_credit_savings(delivered: usize) -> crate::Savings {
    crate::Savings::new(
        delivered.saturating_add(web_reliability_turn_credit() as usize),
        delivered,
    )
    .counterfactual()
}

/// Credit the BINARY assiette for `model`. `after_tokens` = extracted-text
/// tokens (what the agent actually ingests). Never `bytes/4`.
pub fn binary_credit_savings(model: BinaryCreditModel, after_tokens: usize) -> crate::Savings {
    let before = match model {
        // Native render (Claude PDFs): tokenade adds nothing → saving 0.
        BinaryCreditModel::NativePdf => after_tokens,
        // Extract-or-fail / no doc tool: credit the fumbling turns avoided.
        BinaryCreditModel::ExtractOrFail | BinaryCreditModel::NoDocTool => {
            after_tokens.saturating_add(binary_turn_tax() as usize)
        }
    };
    crate::Savings::new(before, after_tokens).counterfactual()
}

/// Multiplier between a NATIVE web-search result set and tokenade's compacted
/// union list. The native `WebSearch` returns a rendered SERP (title + URL +
/// long snippet + sitelinks per hit) that reaches the model whole; tokenade
/// serves a compacted union of several engines. Field measurement (2026-07-10
/// research-run transcripts): native rendered result set is 1500-2200 tokens
/// for ~10 hits; our compacted list runs ~500-900 — a measured 2.5-4×. The
/// default sits at the LOW (conservative) end so we never over-claim.
/// Calibratable via `~/.tokenade/coefficients.json:web_search_native_multiplier`.
pub const WEB_SEARCH_NATIVE_MULTIPLIER: f64 = 3.0;

/// Calibrated [`WEB_SEARCH_NATIVE_MULTIPLIER`] (≥ 1.0) else the default.
pub fn web_search_native_multiplier() -> f64 {
    match read_f64("web_search_native_multiplier") {
        Some(r) if r >= 1.0 => r,
        _ => WEB_SEARCH_NATIVE_MULTIPLIER,
    }
}

/// The `op`s for which EVERY ledger row carries a fabricated baseline.
///
/// [`web_search_credit_savings`] is their only writer, so the `op` is enough to
/// decide — unlike `html-to-markdown`, which two paths write
/// (a measured distillation and a flat reader credit), and where only
/// the row can tell.
///
/// The list exists for HISTORY: the `counterfactual` stamp only marks
/// the rows written since, and a one-hundred-and-four-day ledger would stay
/// silent. MEASURED 2026-09-08 on that ledger: `web-search` **40 rows out of 40**
/// and `web-search-answer` **1 out of 1** satisfy `before == round(after × 3)`,
/// i.e. exactly the multiplier. No other `op` comes close to that count —
/// `generic` has 1 out of 2,664, an arithmetic coincidence.
pub const ALWAYS_COUNTERFACTUAL_OPS: &[&str] = &["web-search", "web-search-answer"];

/// Credit the web-SEARCH assiette. `served` = tokens tokenade actually
/// delivered (the compacted result list). The native counterfactual is the
/// full SERP the model would otherwise ingest, ≈ `multiplier · served`
/// (measured ~3×). `saved = before − after` is clamped ≥ 0 downstream by
/// [`crate::gain`]'s anti-inflation backstop. This changes ONLY the credit
/// assiette — never what tokenade delivers to the agent.
pub fn web_search_credit_savings(served: usize) -> crate::Savings {
    let before = (served as f64 * web_search_native_multiplier()).round() as usize;
    crate::Savings::new(before.max(served), served).counterfactual()
}

/// Native-baseline multiplier for a SCOPED page intercept: the base system's
/// consumption for the same answer — the native `WebFetch` scoped result, or
/// the agent's partial-page recovery when native truncates/fails on the huge &
/// PDF pages this gate targets — is ~this many times the scoped excerpt we
/// deliver. Same measured ~3× economy as web search; kept as its own constant
/// so it can be calibrated independently. Always bounded by the real distilled
/// page so we never book bytes the page didn't contain. Calibratable via
/// `~/.tokenade/coefficients.json:web_fetch_scoped_native_multiplier`.
pub const WEB_FETCH_SCOPED_NATIVE_MULTIPLIER: f64 = 3.0;

/// Calibrated [`WEB_FETCH_SCOPED_NATIVE_MULTIPLIER`] (≥ 1.0) else the default.
pub fn web_fetch_scoped_native_multiplier() -> f64 {
    match read_f64("web_fetch_scoped_native_multiplier") {
        Some(r) if r >= 1.0 => r,
        _ => WEB_FETCH_SCOPED_NATIVE_MULTIPLIER,
    }
}

/// Credit a scoped web-page intercept. `full_distilled` = tokens of the whole
/// distilled page (what the agent would otherwise carry); `extract` = the
/// scoped excerpt we actually deliver; `floor` = minimum credit (one avoided
/// turn). `before` = `extract · mult` (~3×), **capped** at the real distilled
/// page so a huge page can't book 60×-runaway credit. The floor (one avoided
/// turn) is applied FIRST and the cap LAST, so the cap always wins.
/// `saved = before − after` is clamped ≥ 0 downstream.
///
/// Round 38 — the two bounds used to run in the other order
/// (`target.min(full_distilled).max(extract + floor)`), which let `.max()`
/// re-inflate the value above the cap the doc comment promises. Whenever
/// `full_distilled < extract + floor` — i.e. any page small enough that the
/// floor bites — the result exceeded what the page actually contained, by up
/// to `floor` (800) tokens. You cannot save bytes a page never had, so the
/// cap is the invariant and the floor is only a preference.
pub fn web_fetch_scoped_credit_savings(
    full_distilled: usize,
    extract: usize,
    floor: usize,
) -> crate::Savings {
    let target = (extract as f64 * web_fetch_scoped_native_multiplier()).round() as usize;
    let wanted = target.max(extract + floor);
    let ceiling = full_distilled.max(extract);
    let before = wanted.min(ceiling);
    let sav = crate::Savings::new(before, extract);
    // The baseline is measured exactly when the CEILING bit: `before` is
    // then the real page. Otherwise the multiplier or the floor
    // set it, and none of that was read.
    if wanted <= ceiling {
        sav.counterfactual()
    } else {
        sav
    }
}
