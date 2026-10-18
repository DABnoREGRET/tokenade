//! Token → USD cost translation for the dashboard.
//!
//! Pricing reflects the May 2026 list prices for Anthropic Claude models.
//! Update [`MODELS`] when prices change; the rest of the code consumes
//! `cost_for` so a single table edit propagates to the dashboard and any
//! reporting calls.
//!
//! Pricing is **input-only**. STS doesn't see the agent's output stream,
//! only the input it shrinks (compactor output → next-turn input). That's
//! also where the cache-prefix amplification happens, so input pricing is
//! the right number to convert against.

/// One model's per-million-token input price in USD.
pub struct Model {
    pub id: &'static str,
    pub label: &'static str,
    pub input_usd_per_mtok: f64,
}

/// Anthropic Claude models we surface in the dashboard, with their CURRENT
/// input list price. Kept in step with [`FALLBACK_EXACT`] below — these were
/// $15 for every Opus until 2026-09-10, three times the real rate, on the same
/// day the server had been carrying $5 for six weeks.
pub const MODELS: &[Model] = &[
    Model {
        id: "claude-opus-5",
        label: "Opus 5",
        input_usd_per_mtok: 5.0,
    },
    Model {
        id: "claude-opus-4-8",
        label: "Opus 4.8",
        input_usd_per_mtok: 5.0,
    },
    Model {
        id: "claude-opus-4-7",
        label: "Opus 4.7",
        input_usd_per_mtok: 5.0,
    },
    Model {
        id: "claude-fable-5",
        label: "Fable 5",
        input_usd_per_mtok: 10.0,
    },
    Model {
        id: "claude-sonnet-5",
        label: "Sonnet 5",
        input_usd_per_mtok: 3.0,
    },
    Model {
        id: "claude-sonnet-4-6",
        label: "Sonnet 4.6",
        input_usd_per_mtok: 3.0,
    },
    Model {
        id: "claude-haiku-4-5",
        label: "Haiku 4.5",
        input_usd_per_mtok: 1.0,
    },
];

/// Default model when the user doesn't specify one — the same stand-in
/// [`rates_for_model`] uses for an event that never named its model.
///
/// Lookup by id (not array index) so reordering [`MODELS`] for
/// `find()` priority can't silently change which model is the default.
pub fn default_model() -> &'static Model {
    MODELS
        .iter()
        .find(|m| m.id == UNKNOWN_MODEL_STANDS_IN_AS)
        .unwrap_or(&MODELS[0])
}

/// Lookup by `id` or `label` (case-insensitive prefix).
pub fn find(name: &str) -> Option<&'static Model> {
    let needle = name.to_lowercase();
    MODELS
        .iter()
        .find(|m| m.id.eq_ignore_ascii_case(name) || m.label.to_lowercase().contains(&needle))
}

/// Convert a token count into USD at `model`'s input price.
pub fn cost_for(model: &Model, tokens: u64) -> f64 {
    (tokens as f64) * model.input_usd_per_mtok / 1_000_000.0
}

/// Real per-model API rates ($/MTok) for input, cache-read and output.
#[derive(Debug, Clone, Copy)]
pub struct ModelRates {
    pub input: f64,
    pub cache: f64,
    pub output: f64,
}

impl ModelRates {
    /// USD for a per-direction token split at these rates.
    pub fn usd(&self, input: u64, cache: u64, output: u64) -> f64 {
        input as f64 / 1e6 * self.input
            + cache as f64 / 1e6 * self.cache
            + output as f64 / 1e6 * self.output
    }
}

/// **The same four steps the server takes, in the same order.**
///
/// The server-side pricing prices a model by: the LIVE LiteLLM
/// table keyed on the EXACT id, then a known-lineup table, then a family
/// prefix, then the nearest priced sibling. This module used to answer from a
/// hand-written family table ALONE, and that table had rotted: `claude-opus`
/// carried $15/$75 where every Opus since 4.5 is $5/$25, and `claude-fable`
/// $5/$25 where it is $10/$50. So the statusline and `gain` overstated every
/// Opus user's savings **3×** and understated Fable **2×**, against a server
/// that had been corrected on 2026-08-01 — a number the client shows and the
/// number the server recomputes must not disagree, least of all by 3×.
///
/// The two tables below are the FALLBACK, mirroring the server's
/// `FALLBACK_EXACT` / `FALLBACK_FAMILY` row for row. They carry each family's
/// rate as of 2026-09-10 and will drift, which is exactly why the live table
/// is tried first and they only answer a cold start or an unreachable network.
const FALLBACK_EXACT: &[(&str, ModelRates)] = &[
    (
        "claude-opus-5",
        ModelRates {
            input: 5.0,
            cache: 0.5,
            output: 25.0,
        },
    ),
    (
        "claude-opus-4-8",
        ModelRates {
            input: 5.0,
            cache: 0.5,
            output: 25.0,
        },
    ),
    (
        "claude-opus-4-7",
        ModelRates {
            input: 5.0,
            cache: 0.5,
            output: 25.0,
        },
    ),
    (
        "claude-opus-4-6",
        ModelRates {
            input: 5.0,
            cache: 0.5,
            output: 25.0,
        },
    ),
    (
        "claude-opus-4-5",
        ModelRates {
            input: 5.0,
            cache: 0.5,
            output: 25.0,
        },
    ),
    (
        "claude-opus-4-1",
        ModelRates {
            input: 15.0,
            cache: 1.5,
            output: 75.0,
        },
    ),
    (
        "claude-fable-5",
        ModelRates {
            input: 10.0,
            cache: 1.0,
            output: 50.0,
        },
    ),
    (
        "claude-mythos-5",
        ModelRates {
            input: 10.0,
            cache: 1.0,
            output: 50.0,
        },
    ),
    (
        "claude-sonnet-5",
        ModelRates {
            input: 3.0,
            cache: 0.3,
            output: 15.0,
        },
    ),
    (
        "claude-sonnet-4-6",
        ModelRates {
            input: 3.0,
            cache: 0.3,
            output: 15.0,
        },
    ),
    (
        "claude-haiku-4-5",
        ModelRates {
            input: 1.0,
            cache: 0.1,
            output: 5.0,
        },
    ),
];

const FALLBACK_FAMILY: &[(&str, ModelRates)] = &[
    (
        "claude-fable",
        ModelRates {
            input: 10.0,
            cache: 1.0,
            output: 50.0,
        },
    ),
    (
        "claude-mythos",
        ModelRates {
            input: 10.0,
            cache: 1.0,
            output: 50.0,
        },
    ),
    (
        "claude-opus",
        ModelRates {
            input: 5.0,
            cache: 0.5,
            output: 25.0,
        },
    ),
    (
        "claude-sonnet",
        ModelRates {
            input: 3.0,
            cache: 0.3,
            output: 15.0,
        },
    ),
    (
        "claude-haiku",
        ModelRates {
            input: 1.0,
            cache: 0.1,
            output: 5.0,
        },
    ),
    (
        "gpt-5",
        ModelRates {
            input: 1.25,
            cache: 0.125,
            output: 10.0,
        },
    ),
    (
        "gemini-2",
        ModelRates {
            input: 1.25,
            cache: 0.315,
            output: 10.0,
        },
    ),
    (
        "gemini-3",
        ModelRates {
            input: 2.0,
            cache: 0.2,
            output: 12.0,
        },
    ),
];

/// The id whose CURRENT price stands in for an event that never named its
/// model. Sonnet, because it is the middle of the lineup rather than a
/// flattering end of it, and *current* because the value of an unattributed
/// token is what that token would cost today.
pub const UNKNOWN_MODEL_STANDS_IN_AS: &str = "claude-sonnet-5";

fn from_cost(c: crate::model_costs::ModelCost) -> ModelRates {
    ModelRates {
        input: c.input_per_1m,
        cache: c.cache_read_per_1m,
        output: c.output_per_1m,
    }
}

/// Rates for a model id, by the chain described above.
///
/// An EMPTY id is not a model we failed to price, it is an event that never
/// said — those get the current Sonnet rate ([`UNKNOWN_MODEL_STANDS_IN_AS`]),
/// unless the operator has set the `TOKENADE_PRICE_*_PER_MTOK` overrides, which
/// still win on both sides.
pub fn rates_for_model(model: &str) -> ModelRates {
    let model = model.trim();
    if model.is_empty() {
        return unknown_model_rates();
    }
    if let Some(c) = crate::model_costs::cached_cost_for_model(model) {
        return from_cost(c);
    }
    if let Some(r) = fallback_rates(model) {
        return r;
    }
    if let Some(c) = crate::model_costs::cached_nearest_cost_for_model(model) {
        return from_cost(c);
    }
    unknown_model_rates()
}

/// The two built-in tables, exact rows first — the step between "upstream
/// knows this id" and "borrow a sibling's price". Pure: no disk, no network,
/// no env, so a test can pin it without pinning the live table.
pub fn fallback_rates(model: &str) -> Option<ModelRates> {
    let m = model.trim().to_ascii_lowercase();
    if m.is_empty() {
        return None;
    }
    if let Some((_, r)) = FALLBACK_EXACT.iter().find(|(id, _)| *id == m) {
        return Some(*r);
    }
    FALLBACK_FAMILY
        .iter()
        .find(|(p, _)| m.starts_with(p))
        .map(|(_, r)| *r)
}

/// What an event with no model is worth: the operator's explicit override if
/// there is one, else the CURRENT price of [`UNKNOWN_MODEL_STANDS_IN_AS`].
fn unknown_model_rates() -> ModelRates {
    let env = |k: &str| {
        std::env::var(k)
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|v| *v >= 0.0)
    };
    let (i, c, o) = (
        env("TOKENADE_PRICE_INPUT_PER_MTOK"),
        env("TOKENADE_PRICE_CACHE_PER_MTOK"),
        env("TOKENADE_PRICE_OUTPUT_PER_MTOK"),
    );
    if i.is_some() || c.is_some() || o.is_some() {
        return ModelRates {
            input: i.unwrap_or_else(crate::coefficients::price_input_per_mtok),
            cache: c.unwrap_or_else(crate::coefficients::price_cache_per_mtok),
            output: o.unwrap_or_else(crate::coefficients::price_output_per_mtok),
        };
    }
    sonnet_now()
}

/// Today's Sonnet rate, from the live table when it has one, else its
/// `FALLBACK_EXACT` row. Never recurses into the unknown branch.
fn sonnet_now() -> ModelRates {
    if let Some(c) = crate::model_costs::cached_cost_for_model(UNKNOWN_MODEL_STANDS_IN_AS) {
        return from_cost(c);
    }
    FALLBACK_EXACT
        .iter()
        .find(|(id, _)| *id == UNKNOWN_MODEL_STANDS_IN_AS)
        .map(|(_, r)| *r)
        .unwrap_or(ModelRates {
            input: 3.0,
            cache: 0.3,
            output: 15.0,
        })
}

#[cfg(test)]
mod rate_tests {
    use super::*;

    /// **What the server would answer, row for row.** The values are copied
    /// from the server-side `FALLBACK_EXACT` / `FALLBACK_FAMILY` tables. They
    /// diverged for six weeks: Opus was worth 15 here and 5 there, i.e. a
    /// displayed saving at THREE TIMES what the server recomputes.
    #[test]
    fn the_fallback_tables_mirror_the_server_row_for_row() {
        for (id, want) in [
            ("claude-opus-5", (5.0, 0.5, 25.0)),
            ("claude-opus-4-8", (5.0, 0.5, 25.0)),
            ("claude-opus-4-1", (15.0, 1.5, 75.0)),
            ("claude-fable-5", (10.0, 1.0, 50.0)),
            ("claude-mythos-5", (10.0, 1.0, 50.0)),
            ("claude-sonnet-5", (3.0, 0.3, 15.0)),
            ("claude-haiku-4-5", (1.0, 0.1, 5.0)),
            // families: an unknown version falls onto the CURRENT rate
            ("claude-opus-9-9", (5.0, 0.5, 25.0)),
            ("claude-fable-7", (10.0, 1.0, 50.0)),
            ("gpt-5.5", (1.25, 0.125, 10.0)),
            ("gemini-2.0-flash", (1.25, 0.315, 10.0)),
            ("gemini-3-pro", (2.0, 0.2, 12.0)),
            ("CLAUDE-OPUS-5", (5.0, 0.5, 25.0)),
        ] {
            let r = fallback_rates(id).unwrap_or_else(|| panic!("{id} absent"));
            assert_eq!((r.input, r.cache, r.output), want, "{id}");
        }
        // An OLDER version keeps its rate of the time: that is the only
        // reason to have exact rows ahead of the families.
        assert_eq!(fallback_rates("claude-opus-4-1").unwrap().input, 15.0);
        assert_eq!(fallback_rates("claude-opus-4-5").unwrap().input, 5.0);
        // Nothing known, nothing invented.
        assert!(fallback_rates("some-unknown-llm").is_none());
        assert!(fallback_rates("").is_none());
        assert!(fallback_rates("   ").is_none());
    }

    /// The event that never named its model is worth the CURRENT Sonnet
    /// price, not a $3 blend that belongs to no model. The operator's
    /// instruction: "if we do not know the model, we go by the current
    /// Sonnet price".
    #[test]
    fn an_event_with_no_model_is_priced_as_sonnet() {
        let sonnet = rates_for_model(UNKNOWN_MODEL_STANDS_IN_AS);
        let unknown = rates_for_model("");
        assert_eq!(
            (unknown.input, unknown.cache, unknown.output),
            (sonnet.input, sonnet.cache, sonnet.output)
        );
        assert!(unknown.input > 0.0, "a zero price is not a fallback");
    }

    #[test]
    fn usd_sums_three_directions() {
        let r = ModelRates {
            input: 15.0,
            cache: 1.5,
            output: 75.0,
        };
        assert!((r.usd(1_000_000, 1_000_000, 1_000_000) - 91.5).abs() < 1e-9);
        assert_eq!(r.usd(0, 0, 0), 0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_model_is_known() {
        assert!(default_model().input_usd_per_mtok > 0.0);
    }

    #[test]
    fn find_by_id_works() {
        assert!(find("claude-opus-5").is_some());
    }

    /// The display rate and the valuation rate are the SAME number. `MODELS`
    /// carried $15 for Opus while `FALLBACK_EXACT` carried 5: two tables in
    /// one file, two answers for one model.
    #[test]
    fn the_display_table_agrees_with_the_rate_table() {
        for m in MODELS {
            let r = fallback_rates(m.id).unwrap_or_else(|| panic!("{} absent", m.id));
            assert_eq!(
                r.input, m.input_usd_per_mtok,
                "{}: {} displayed against {} valued",
                m.id, m.input_usd_per_mtok, r.input
            );
        }
    }

    #[test]
    fn find_by_label_prefix_case_insensitive() {
        assert!(find("sonnet").is_some());
        assert!(find("OPUS").is_some());
    }

    #[test]
    fn cost_scales_with_tokens() {
        let m = default_model();
        assert!((cost_for(m, 1_000_000) - m.input_usd_per_mtok).abs() < 1e-9);
        assert_eq!(cost_for(m, 0), 0.0);
    }

    #[test]
    fn default_model_stays_sonnet_after_reorder() {
        // Regression for the bug introduced when Opus 4.8 was added at
        // index 0 — `MODELS[1]` flipped from Sonnet to Opus 4.7, so
        // the default user would silently start being valued at Opus
        // pricing. `default_model()` now resolves by id.
        assert_eq!(default_model().id, UNKNOWN_MODEL_STANDS_IN_AS);
    }
}
