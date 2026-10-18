//! **What ASKS for a change, and what informs.**
//!
//! The dashboard shows an "adherence" column for everything that goes through
//! `record_nudge`. Measured over 30 days of real usage: twelve kinds out of
//! sixteen wrote ONLY `fire` — 21 999 fires out of 49 387 without any outcome
//! at all — and the column showed "—" for all of them, indiscriminately.
//!
//! Yet those twelve are not of the same nature. "You ran three commands one
//! per turn, group them" asks for an action: not knowing whether it was
//! followed is a GAP. "This file is 4 000 lines long" asks for nothing: there
//! is no outcome to observe, and showing "—" is the right answer there.
//!
//! This module carries the table, the type of the measured quantity and the
//! two recorders — moved out of `record.rs`, which was past 800 lines.
//!
//! This table separates the two. What it makes possible: a guard requiring a
//! `Steer` to be able to write `ok` and `ignored`, and a dashboard that stops
//! counting as missing what does not exist.

/// The nature of a nudge kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NudgeNature {
    /// Asks the agent to do otherwise. MUST be able to say `ok`/`ignored`:
    /// without that it can neither be improved nor justify its cost.
    Steer,
    /// Informs of a fact. No outcome to observe; "—" is the truth.
    Notice,
}

/// Every kind this product emits, and its nature.
///
/// Kept by hand, hence guarded: `every_nudge_kind_is_classified` rejects a
/// `record_nudge` whose kind is not here.
pub const NUDGE_KINDS: &[(&str, NudgeNature)] = &[
    // The four that can already measure themselves.
    ("batch-read", NudgeNature::Steer),
    ("batch-bash", NudgeNature::Steer),
    ("batch-edit", NudgeNature::Steer),
    ("loop-guard", NudgeNature::Steer),
    // Grouping advice, same family, observable outcome — not wired yet.
    ("batch-questions", NudgeNature::Steer),
    ("batch-web-calls", NudgeNature::Steer),
    // Explicit request to switch tools.
    ("cat-edit", NudgeNature::Steer),
    ("large-file", NudgeNature::Steer),
    ("write-rewrite", NudgeNature::Steer),
    ("media-tooling", NudgeNature::Steer),
    ("bypass", NudgeNature::Steer),
    // Notices: they observe, they ask for nothing.
    ("quota-gate", NudgeNature::Notice),
    ("compactor-gap", NudgeNature::Notice),
    ("grep-capped", NudgeNature::Notice),
    ("redundant-bypass", NudgeNature::Notice),
    ("safe-class-bypass", NudgeNature::Notice),
    ("stale-mcp", NudgeNature::Notice),
    ("websearch_banner", NudgeNature::Notice),
];

/// The nature of a kind, or `None` if it is not declared.
///
/// `webfetch-spa-shell/<domain>` carries its subject in the name: the table
/// cannot enumerate domains, so the prefix decides.
pub fn nature_of(kind: &str) -> Option<NudgeNature> {
    if kind.starts_with("webfetch-spa-shell/") {
        return Some(NudgeNature::Notice);
    }
    NUDGE_KINDS
        .iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, n)| *n)
}

/// The kinds that MUST be able to write `ok` and `ignored`.
pub fn steers() -> impl Iterator<Item = &'static str> {
    NUDGE_KINDS
        .iter()
        .filter(|(_, n)| *n == NudgeNature::Steer)
        .map(|(k, _)| *k)
}

/// **An OBSERVATION, not advice.**
///
/// Some counters record a fact — a bypass happened, a page has no reader —
/// without telling the agent anything. Filed among the nudges, they inflated
/// the volume and suggested zero adherence: over 30 days, `bypass` weighed
/// 19 037 of the 49 387 counted fires, and its message reached nobody (stderr
/// of a PreToolUse hook answering "allow").
///
/// `op` becomes `observed:<kind>`: a reader aggregating `nudge:*` no longer
/// sees them, and the adherence rate finally covers what was actually said.
pub fn record_observation(kind: &str, count: usize) {
    super::record::record_inner(
        "gain_estimated.jsonl",
        "hook",
        &format!("observed:{kind}"),
        kind,
        crate::Savings::new(0, 0),
        None,
        None,
        Some("observation"),
        Some(serde_json::json!({ "count": count })),
    );
}

/// What a nudge MEASURES at the moment it speaks.
///
/// **A single numeric field carried FIVE quantities.** `record_nudge` took a
/// bare `usize` that landed in `before`/`after`, and the dashboard turned it
/// into an "average context at fire" column. Measured over 30 days of real
/// usage, what the call sites really put there:
///
/// * `batch-bash`/`read`/`edit` → context tokens (158.4k / 251.0k /
///   284.9k) — the only family for which the column told the truth;
/// * `batch-questions`, `batch-web-calls`, `compactor-gap`, `bypass` → an
///   occurrence COUNTER, shown as "3 tok", "8 tok", "402 tok";
/// * `grep-capped` → a number of matches ("682 tok");
/// * `write-rewrite` → tokens rewritten identically ("2.3k tok");
/// * `websearch_banner` → the size of the banner;
/// * `large-file`, `cat-edit`, `quota-gate`, `media-tooling`, `stale-mcp` → a
///   literal `0`, indistinguishable from a genuinely empty context.
///
/// A column averaging five magnitudes is not a measurement. The type forces
/// every site to SAY what it counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NudgeMetric {
    /// The model's context tokens at fire time. The only quantity the
    /// "context" column is allowed to aggregate.
    ContextTokens(usize),
    /// Nth time the pattern is seen in this session.
    Occurrence(usize),
    /// Number of items concerned (matches, lines, calls).
    Items(usize),
    /// Tokens at stake in what the nudge describes (banner served, bytes
    /// rewritten) — NOT the agent's context.
    Tokens(usize),
    /// Nothing measurable here. Written as such rather than as zero.
    None,
}

impl NudgeMetric {
    /// The name under which the value is published, and the value.
    fn named(self) -> (&'static str, Option<usize>) {
        match self {
            Self::ContextTokens(n) => ("context_tokens", Some(n)),
            Self::Occurrence(n) => ("occurrence", Some(n)),
            Self::Items(n) => ("items", Some(n)),
            Self::Tokens(n) => ("tokens", Some(n)),
            Self::None => ("none", None),
        }
    }

    /// What lands in `before`/`after`. ONLY the context goes there: that is
    /// what the dashboard aggregates under that name, and pouring anything
    /// else in is what made the column wrong.
    fn ledger_value(self) -> usize {
        match self {
            Self::ContextTokens(n) => n,
            _ => 0,
        }
    }
}

/// Nudge telemetry counter — accounting-neutral (before == after, saved 0)
/// so no savings figure moves anywhere, but the event rides the estimated
/// ledger + reporter to the server like any other row. `op` becomes
/// `nudge:<kind>:<event>` (event ∈ fire|ok|ignored), `label` carries the
/// kind for local dashboards.
///
/// The quantity is NAMED: `before`/`after` only carry context tokens, and
/// `nudge_metric` / `nudge_metric_value` publish what the site really
/// counted. A reader that only knows `before` therefore reads "0" where it
/// used to read a counter disguised as tokens — that is the fix, not a
/// loss.
pub fn record_nudge(kind: &str, event: &str, metric: NudgeMetric) {
    let (name, value) = metric.named();
    let metric = metric.ledger_value();
    super::record::record_inner(
        "gain_estimated.jsonl",
        "hook",
        // **The quantity is named in the `op`, for lack of a column for it.**
        //
        // The server only stores `op`, `beforeTokens`, `afterTokens` and a
        // class: the named fields below do not leave the machine. A FOURTH
        // segment lets them travel without a migration — `split_part(op,
        // ':', 2)` and `':', 3)` still return the kind and the event, so
        // existing reads do not move, and a row from before this change
        // simply returns an empty string ("we do not know what this number
        // counted"), which is exactly true.
        &format!("nudge:{kind}:{event}:{name}"),
        kind,
        crate::Savings::new(metric, metric),
        None,
        None,
        Some("nudge"),
        Some(serde_json::json!({
            "nudge_metric": name,
            "nudge_metric_value": value,
        })),
    );
}
