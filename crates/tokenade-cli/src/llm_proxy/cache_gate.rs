//! Compacting an old block can COST more than it saves.
//!
//! Rewriting a block that sits inside the provider's cached prefix
//! invalidates that prefix. Everything from the first rewritten block onward
//! has to be written to the cache again. On a long history that write can be
//! worth far more than the tokens the fold removed — so the gate is not an
//! optimisation, it is what stops rank 4 from being a negative-gain machine.
//!
//! This project has already paid once for not modelling cache writes: 341.8 M
//! rewrite tokens existed in the ledger for weeks and were displayed nowhere.
//!
//! The gate is deliberately a pure function of four numbers. It does not know
//! about HTTP, providers or pricing tables — the caller supplies the prices it
//! is actually being charged, because they differ per model and per provider
//! and a table baked in here would rot exactly the way the delivery caps did.

/// Prices, in whatever unit the caller uses consistently (cost per token).
#[derive(Debug, Clone, Copy)]
pub struct Prices {
    /// What a plain, uncached input token costs. The unit everything else is
    /// expressed against; 1.0 is the natural choice.
    pub input: f64,
    /// What a cached-read input token costs.
    pub cache_read: f64,
    /// What WRITING a token into the cache costs (typically > `cache_read`,
    /// and on some providers > the plain input price).
    pub cache_write: f64,
}

impl Prices {
    /// The prices that apply when this request is NOT using a prompt cache.
    ///
    /// This distinction is not a refinement, it decides whether rank 4 does
    /// anything at all. With a cached prefix, folding an old block costs a full
    /// cache write and saves only cache-read-priced tokens — a 12.5:1 ratio on
    /// Anthropic, so a rewrite needs a couple of dozen remaining turns to pay
    /// for itself and the gate correctly refuses nearly everything. With no
    /// cache, the same fold costs nothing and saves FULL-price input tokens on
    /// every later turn.
    ///
    /// Measured on this proxy's own test corpus before the distinction existed:
    /// folding a 4 000-byte tool result that is essentially the whole request
    /// was refused, because the gate was pricing a cache write nobody had asked
    /// for. A gate that refuses everything is not conservative, it is off.
    pub fn without_prompt_cache(self) -> Self {
        Self {
            input: self.input,
            // The tokens the fold removes are billed at full input price every
            // turn, because nothing is caching them.
            cache_read: self.input,
            // And there is no cached prefix to invalidate.
            cache_write: 0.0,
        }
    }
}

/// One candidate rewrite, measured on the request we are about to relay.
#[derive(Debug, Clone, Copy)]
pub struct Candidate {
    /// Tokens the fold would remove from the history.
    pub tokens_saved: u64,
    /// Tokens that currently sit AT OR AFTER the first block we would rewrite,
    /// i.e. the part of the cached prefix the rewrite invalidates.
    pub tokens_after_first_rewrite: u64,
    /// How many more turns this conversation is expected to run. The saving
    /// recurs every turn; the cache write is paid once.
    pub remaining_turns: u64,
}

/// Should we rewrite?
///
/// The comparison is over the REMAINING life of the conversation, not over one
/// turn, and that is the whole subtlety. A rewrite costs one cache write now
/// and saves its tokens on every turn from here on:
///
/// ```text
///   gain = tokens_saved × cache_read × remaining_turns
///   cost = tokens_after_first_rewrite × cache_write        (paid once)
/// ```
///
/// On the LAST turn of a conversation `remaining_turns` is 1 and almost no
/// rewrite pays for itself. On turn 3 of 40 nearly every one does. A proxy that
/// ignored this would look like a win in a benchmark of long sessions and a
/// loss for the user who asks two questions and leaves.
pub fn worth_rewriting(c: Candidate, p: Prices) -> bool {
    if c.tokens_saved == 0 {
        return false;
    }
    // Nothing cached after the rewrite point means nothing to re-write: the
    // fold is free. This is the common case for the LAST tool result, which is
    // also the biggest one.
    if c.tokens_after_first_rewrite == 0 {
        return true;
    }
    let gain = c.tokens_saved as f64 * p.cache_read * c.remaining_turns.max(1) as f64;
    let cost = c.tokens_after_first_rewrite as f64 * p.cache_write;
    gain > cost
}

/// How many turns to assume when the caller cannot know.
///
/// Deliberately LOW. Guessing high makes every rewrite look profitable, which
/// turns the gate off while leaving it in the code — the most expensive kind of
/// guard, because it reports that it is protecting you. A conservative default
/// declines the marginal rewrites and keeps the obvious ones.
pub const ASSUMED_REMAINING_TURNS: u64 = 3;

#[cfg(test)]
mod tests {
    use super::*;

    const ANTHROPIC_ISH: Prices = Prices {
        input: 1.0,
        cache_read: 0.1,
        cache_write: 1.25,
    };

    #[test]
    fn a_fold_at_the_end_of_the_history_is_always_worth_it() {
        // Nothing is cached after it, so there is no prefix to invalidate.
        assert!(worth_rewriting(
            Candidate {
                tokens_saved: 10,
                tokens_after_first_rewrite: 0,
                remaining_turns: 1
            },
            ANTHROPIC_ISH
        ));
    }

    #[test]
    fn rewriting_early_in_a_short_conversation_is_refused() {
        // 500 tokens saved, but 200 000 tokens of cached prefix behind it and
        // one turn left to amortise the write over. This is the case that makes
        // a naive proxy lose money.
        assert!(!worth_rewriting(
            Candidate {
                tokens_saved: 500,
                tokens_after_first_rewrite: 200_000,
                remaining_turns: 1
            },
            ANTHROPIC_ISH
        ));
    }

    #[test]
    fn the_same_rewrite_flips_answer_with_the_horizon() {
        // ONE candidate, two horizons. The saving recurs every turn; the cache
        // write is paid once — so the same rewrite is a loss on the last turn
        // and a win early on. That is why the horizon is an input and not a
        // constant folded into the formula.
        //
        // The first draft of this test asserted the flip with numbers that
        // never flipped (5 000 saved against a 200 000-token prefix pays back
        // 20 000 over 40 turns, against a 250 000 write). The test was wrong
        // and the gate was right — worth recording, because a gate that
        // refuses more than you expected is exactly what this one is for.
        let c = |turns| Candidate {
            tokens_saved: 40_000,
            tokens_after_first_rewrite: 100_000,
            remaining_turns: turns,
        };
        assert!(
            !worth_rewriting(c(1), ANTHROPIC_ISH),
            "4 000 of saving does not justify a 125 000 cache write"
        );
        assert!(
            worth_rewriting(c(40), ANTHROPIC_ISH),
            "160 000 of saving does justify the same 125 000 write"
        );
    }

    #[test]
    fn without_a_prompt_cache_the_same_fold_is_taken() {
        // The candidate that exposed the missing distinction: a 1 000-token
        // tool result that IS essentially the whole request, halved. Against a
        // cached prefix it costs 1 250 to save 300, and the gate is right to
        // refuse. With no cache in play there is nothing to invalidate and the
        // saving is at FULL input price on every later turn.
        let c = Candidate {
            tokens_saved: 500,
            tokens_after_first_rewrite: 1_000,
            remaining_turns: ASSUMED_REMAINING_TURNS,
        };
        assert!(!worth_rewriting(c, ANTHROPIC_ISH));
        assert!(worth_rewriting(c, ANTHROPIC_ISH.without_prompt_cache()));
    }

    #[test]
    fn a_provider_that_does_not_bill_cache_writes_never_blocks_a_fold() {
        // The OpenAI wires cache automatically and charge nothing to write.
        // Modelling them with Anthropic's numbers would leave most of this
        // rung switched off on the provider where it costs nothing to run.
        let openai = Prices {
            input: 1.0,
            cache_read: 0.5,
            cache_write: 0.0,
        };
        assert!(worth_rewriting(
            Candidate {
                tokens_saved: 1,
                tokens_after_first_rewrite: 1_000_000,
                remaining_turns: 1
            },
            openai
        ));
    }

    #[test]
    fn saving_nothing_is_never_worth_a_cache_write() {
        assert!(!worth_rewriting(
            Candidate {
                tokens_saved: 0,
                tokens_after_first_rewrite: 0,
                remaining_turns: 100
            },
            ANTHROPIC_ISH
        ));
    }

    #[test]
    fn the_default_horizon_is_low_enough_to_actually_refuse_things() {
        // Anti-vacuity for the constant itself: if `ASSUMED_REMAINING_TURNS`
        // ever grows, the gate stops declining anything and becomes decoration.
        let marginal = Candidate {
            tokens_saved: 1_000,
            tokens_after_first_rewrite: 100_000,
            remaining_turns: ASSUMED_REMAINING_TURNS,
        };
        assert!(
            !worth_rewriting(marginal, ANTHROPIC_ISH),
            "the default horizon now approves a rewrite that costs 125 000 to save 300"
        );
    }
}
