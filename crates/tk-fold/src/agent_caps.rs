//! What each agent will actually DELIVER to its model, per action.
//!
//! Not to be confused with [`crate::caps`], which is how much *tokenade's own*
//! compactors print. This module answers a different question: **if the user
//! had not installed tokenade, how much of this payload would their agent have
//! put in front of the model anyway?** That number is the denominator of every
//! saving we claim, so it decides whether a reported gain is honest.
//!
//! ## A cap is a RULE, not a number
//!
//! The scalar table this module replaces (`coefficients::agent_channel_default`)
//! flattened every agent to one `u64` per channel, and that flattening is
//! wrong for the most common shape of all. gemini-cli's `read_file` caps
//! **2 000 lines** *and* **2 000 characters per line**: the same 2 MB payload
//! is delivered almost whole if it is 500 long lines, and cut by 95% if it is
//! 200 000 short ones. One scalar cannot express that, so any scalar chosen for
//! gemini-cli is wrong for most inputs — in one direction or the other.
//!
//! So a [`Limit`] is evaluated AGAINST THE PAYLOAD:
//! [`Limit::delivered_chars`] takes the real text and returns what survives.
//! Callers that have the bytes should pass them. Callers that genuinely do not
//! (the ledger recomputation walks rows long after the payload is gone) get
//! [`Limit::worst_case_chars`], which is explicitly labelled as an upper bound
//! rather than silently pretending to be a measurement.
//!
//! ## Every value carries its provenance
//!
//! [`Provenance`] separates *read out of the installed package* from *borrowed
//! from a peer because the agent is closed source*. The distinction is not
//! decoration: a `PeerDefault` is a guess, and a product that cannot tell its
//! guesses from its measurements will eventually publish the guess as a fact.
//! `every_cap_states_where_it_came_from` enforces that no entry is nameless.

/// A thing an agent does that ends with bytes in the model's context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    /// Reading a file with the agent's native file tool.
    ReadFile,
    /// Shell / terminal command output.
    Command,
    /// A generic tool result (neither file nor shell).
    ToolResult,
    /// A tool result on the ERROR path — usually capped far tighter.
    ToolError,
    /// A result coming back from an MCP server.
    McpToolResult,
    /// A page fetched by the agent's own web tool.
    WebFetch,
    /// A file the USER attaches / pastes in, as opposed to one a tool read.
    InputFile,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Action::ReadFile => "read_file",
            Action::Command => "command",
            Action::ToolResult => "tool_result",
            Action::ToolError => "tool_error",
            Action::McpToolResult => "mcp_tool_result",
            Action::WebFetch => "web_fetch",
            Action::InputFile => "input_file",
        }
    }
}

mod limit;
mod table;
pub use limit::*;
pub use table::*;

/// Where a number in this table came from. Guesses must not look like facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provenance {
    /// Read out of the INSTALLED package: the file it lives in and the symbol
    /// it is bound to. Re-checkable by anyone with the same package.
    Package {
        file: &'static str,
        symbol: &'static str,
    },
    /// Vendor-published: changelog, docs, or a tracked issue. Named.
    Published(&'static str),
    /// NOT measured. The agent ships no readable source, so this is a
    /// conservative bound borrowed from a peer of the same lineage. Carries the
    /// reason so it can be retired the day the real value is measured.
    PeerDefault { because: &'static str },
}

impl Provenance {
    /// Whether the value is a measurement or a stand-in.
    pub fn is_measured(&self) -> bool {
        !matches!(self, Provenance::PeerDefault { .. })
    }

    pub fn describe(&self) -> String {
        match self {
            Provenance::Package { file, symbol } => format!("{symbol} in {file}"),
            Provenance::Published(what) => format!("published: {what}"),
            Provenance::PeerDefault { because } => format!("peer default ({because})"),
        }
    }
}

/// Whether the credit derived from a cap is a MEASUREMENT or an approximation.
///
/// The two are not interchangeable and the difference is not pedantry. Under a
/// truncating cap the arithmetic is exact: the agent would have delivered
/// exactly `cap` and we folded it to `after`, so `cap - after` is measured.
/// Under a REFUSING cap there is no such number. The agent delivers nothing,
/// the user reads the file in slices instead, and what that really costs
/// depends on how many slices they need and what they search for — a quantity
/// no ledger row can reconstruct.
///
/// Decision (Paul, 2026-08-10): credit the cap anyway. Zero would be worse than
/// the approximation, because the agent demonstrably does spend tokens getting
/// at the content — it greps and re-reads in parts. The figure is therefore a
/// deliberate stand-in, and this type exists so that every surface reporting it
/// can SAY so rather than presenting it as a measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreditBasis {
    /// `delivered - after` is exact arithmetic on a known delivery.
    Measured,
    /// The delivery is not what the user would really have paid; the credit is
    /// a deliberate stand-in, with the reason attached.
    Approximated { because: &'static str },
}

impl CreditBasis {
    pub fn is_approximate(&self) -> bool {
        matches!(self, CreditBasis::Approximated { .. })
    }
}

/// One row of the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cap {
    pub limit: Limit,
    pub provenance: Provenance,
    /// What tokenade CREDITS on this channel, when a decision says that is not
    /// the measured limit. `None` means the measurement is credited as-is.
    ///
    /// This field exists so that a decision stops living as a DISAGREEMENT
    /// between two tables. Both surviving gaps were of that shape — measured
    /// 25 000, credited 20 000 — and each was defensible on its own while the
    /// pair was indefensible: two numbers, no stated relation, and a test whose
    /// whole job was to remember that somebody meant it. Written here, the
    /// decision travels with the measurement it overrides and the two tables
    /// agree by construction rather than by vigilance.
    pub credited_tokens: Option<u64>,
}

impl Cap {
    /// Tokens to credit for this channel: the decision if one was taken, else
    /// the measurement. `None` when the limit has no scalar equivalent (a
    /// per-line or head/tail rule) or is unbounded.
    pub fn credit_tokens(&self) -> Option<u64> {
        if let Some(decided) = self.credited_tokens {
            return Some(decided);
        }
        // ONLY a flat limit converts. `worst_case_chars` on a per-line rule is
        // an upper bound on a different quantity — gemini-cli's 2 000 x 2 000
        // yields 1 000 500 tokens, which is not a credit, it is the largest
        // file that rule could theoretically pass. Returning it here would have
        // multiplied that channel's credited figure by fifty, and the first
        // wiring of this function did exactly that until a test caught it.
        if !self.limit.is_flat() {
            return None;
        }
        self.limit.worst_case_chars().map(|c| c / 4)
    }

    /// True when a decision overrides the measurement here.
    pub fn credit_is_a_decision(&self) -> bool {
        self.credited_tokens.is_some()
    }
}

/// On what footing a credit derived from this (agent, action) stands.
///
/// Only the REFUSING channels are approximations today, and only above the cap:
/// a payload under it is delivered whole and measured exactly. Callers that
/// present a total should carry the label when any approximated row fed it.
pub fn credit_basis(agent: &str, action: Action) -> CreditBasis {
    match cap(agent, action).map(|c| c.limit) {
        Some(Limit::RefusedAbove { .. }) => CreditBasis::Approximated {
            because: "the agent REFUSES above the cap; what the user really \
                      pays is the slice-by-slice re-reading that follows, which \
                      no ledger row can reconstruct. Crediting the cap is a \
                      deliberate stand-in — closer than zero, not exact.",
        },
        _ => CreditBasis::Measured,
    }
}

/// The cap for one (agent, action), if the table knows it.
///
/// Unknown is returned as `None` rather than a default, so a caller must decide
/// what an unknown agent means instead of inheriting somebody else's number.
pub fn cap(agent: &str, action: Action) -> Option<Cap> {
    // Same product under its current agent name: the table rows were
    // measured under `windsurf`, on the CLI shipped in devin-desktop.
    let agent = if agent == "devin-desktop" {
        "windsurf"
    } else {
        agent
    };
    TABLE
        .iter()
        .find(|(a, act, _)| *a == agent && *act == action)
        .map(|(_, _, c)| *c)
}

/// What `agent` would really have delivered for `payload` on `action`.
///
/// `None` when the table has no entry — the caller keeps its own fallback.
/// Name a truncation the HOST performs and does not mark.
///
/// Claude Code caps a Bash result at `BASH_MAX_OUTPUT_LENGTH` and cuts the
/// tail with no marker of any kind. Measured on 41 833 real tool results from
/// one machine's transcripts: `stdout` maxes at exactly 30 000 bytes, fifteen
/// results sit on that line, and every one of them ends mid-token. Nothing in
/// the payload tells the reader that a cut happened, so a truncated result is
/// indistinguishable from a command that simply finished there.
///
/// This table has carried the number the whole time and spent it only on
/// billing the fold correctly. Returns `None` whenever nothing is lost — an
/// unbounded channel, an agent with no measured cap, or a payload that fits —
/// so silence means the reader has everything.
pub fn host_cut_advisory(agent: &str, action: Action, payload: &str) -> Option<String> {
    if payload.is_empty() {
        return None;
    }
    let cap = cap(agent, action)?;
    let total = payload.chars().count() as u64;
    let delivered = cap.limit.delivered_chars(payload, Ctx::UNKNOWN);
    if delivered >= total {
        return None;
    }
    Some(format!(
        "[tokenade] {agent} cuts a {} result at {delivered} characters and this one is {total}: \
         about {} characters of the tail are missing, and the host marks the cut nowhere. \
         Treat what you read as incomplete, and re-run narrowed to see the rest.",
        action.as_str(),
        total - delivered,
    ))
}

pub fn delivered_chars(agent: &str, action: Action, payload: &str, ctx: Ctx) -> Option<u64> {
    cap(agent, action).map(|c| c.limit.delivered_chars(payload, ctx))
}

/// Does the table hold ANY measured row for this agent?
///
/// The question is not "what is its cap" but "has anyone looked". An agent
/// nobody has measured must not be credited for a truncated tail that may not
/// exist — see [`crate::coefficients::overflow_ratio_for_channel`].
pub fn has_any_row(agent: &str) -> bool {
    // No `is_empty` guard: an empty name matches no row, and a check that
    // cannot change an answer is a rule that looks like it is doing something.
    // The test asserts the empty case directly instead.
    TABLE.iter().any(|(a, _, _)| *a == agent)
}

/// Every agent named anywhere in the table.
pub fn agents() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = TABLE.iter().map(|(a, _, _)| *a).collect();
    v.sort_unstable();
    v.dedup();
    v
}

#[cfg(test)]
include!("agent_caps_tests.rs");
