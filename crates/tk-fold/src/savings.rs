//! **The before / after measurement, in one place.**
//!
//! `Savings` used to live at the root of `tokenade-core` and 214 files name it.
//! Yet its place is neither the base nor the root: `with_agent_delivery`
//! asks `agent_caps` what the agent would REALLY have delivered for this
//! payload, and `agent_caps` is a table of the fold layer. So the type moves
//! down with it; `tokenade_core::Savings` stays the same name at the same path,
//! via re-export.

/// A before/after token measurement, used everywhere we report savings.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct Savings {
    pub tokens_before: usize,
    pub tokens_after: usize,
    /// What the AGENT would really have delivered for this exact payload, in
    /// tokens — computed at fold time by `crate::agent_caps`, which needs the
    /// bytes and the line structure and therefore cannot be applied later.
    ///
    /// `None` means "not computed here", and every consumer must fall back to
    /// the per-agent scalar rather than assume zero. That fallback is what
    /// keeps already-written ledger rows meaning exactly what they meant: this
    /// field makes NEW rows exact without retroactively moving a published
    /// figure, which is the only shape this change could safely take.
    pub deliverable_tokens: Option<u64>,
    /// Was this `tokens_before` COMPUTED from `tokens_after`?
    ///
    /// Three functions in `coefficients` build their baseline this way — a
    /// published multiplier for a web search, a flat per-turn credit for a
    /// reader or a binary — and `code_intel_heuristic` does the
    /// same. Nothing was read, so the rate that comes out is the constant
    /// itself. The flag travels WITH the value so that `gain::record`
    /// can stamp the row without any of the thirteen callers having to think
    /// about it: a stamp you have to remember to apply is only applied by
    /// those who remembered.
    ///
    /// `serde(skip)`: this is a ROUTING hint at write time, not
    /// a ledger field — the row carries `class` instead.
    #[serde(skip)]
    pub baseline_is_counterfactual: bool,
    /// Both numbers are ALREADY tokens billed by the model (vision
    /// tokens of an image), not `tokenizer::estimate` units: the
    /// valuation must not convert them. Written into the row (`billed`)
    /// rather than inferred from the op at read time, so that no row already
    /// written changes value (no retroactive recount).
    #[serde(skip)]
    pub in_billed_tokens: bool,
}

impl Savings {
    pub fn new(before: usize, after: usize) -> Self {
        Self {
            tokens_before: before,
            tokens_after: after,
            deliverable_tokens: None,
            baseline_is_counterfactual: false,
            in_billed_tokens: false,
        }
    }

    /// Voir [`Self::in_billed_tokens`].
    pub fn billed(mut self) -> Self {
        self.in_billed_tokens = true;
        self
    }

    /// Declares that `tokens_before` comes from a computation on `tokens_after`
    /// and not from a reading. Chained constructor, like `deliverable_tokens`:
    /// ordinary measurements keep staying silent by omission.
    pub fn counterfactual(mut self) -> Self {
        self.baseline_is_counterfactual = true;
        self
    }

    /// Attach what the agent would really have delivered for `payload`.
    ///
    /// Deliberately a builder and not a parameter of `new`: the 140-odd call
    /// sites that cannot know the payload keep saying so by omission, and the
    /// handful that hold the bytes opt in. An extra argument everywhere would
    /// have forced every one of them to pass something, and what most of them
    /// would have passed is a guess.
    pub fn with_agent_delivery(
        mut self,
        agent: &str,
        action: crate::agent_caps::Action,
        payload: &str,
    ) -> Self {
        if let Some(chars) = crate::agent_caps::delivered_chars(
            agent,
            action,
            payload,
            crate::agent_caps::Ctx::UNKNOWN,
        ) {
            self.deliverable_tokens = Some(chars / 4);
        }
        self
    }

    /// Fraction of tokens removed in `[0.0, 1.0]`. Returns 0.0 when there was
    /// nothing to save (avoids a divide-by-zero on empty input).
    pub fn ratio(&self) -> f64 {
        if self.tokens_before == 0 {
            return 0.0;
        }
        let saved = self.tokens_before.saturating_sub(self.tokens_after);
        saved as f64 / self.tokens_before as f64
    }

    /// Savings as a percentage, e.g. `73.4`.
    pub fn percent(&self) -> f64 {
        self.ratio() * 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn savings_ratio_basic() {
        let s = Savings::new(1000, 250);
        assert!((s.percent() - 75.0).abs() < 1e-9);
    }

    #[test]
    fn savings_ratio_empty_is_zero() {
        assert_eq!(Savings::new(0, 0).ratio(), 0.0);
    }

    #[test]
    fn savings_never_negative() {
        // Filter that somehow grew the output should clamp to 0, not panic.
        let s = Savings::new(100, 200);
        assert_eq!(s.ratio(), 0.0);
    }
}
