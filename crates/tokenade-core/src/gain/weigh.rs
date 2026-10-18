//! **What a row is WORTH.**
//!
//! Tokens are not a price, and an input token is not an output token. This
//! module carries the weighting, the per-model rate table, and the
//! computation cache — the only place in the ledger where one unit becomes
//! another.

use super::*;

/// Price-weighted savings — the number shown as plain "tokens" on every
/// surface (terminal, HTML, tokenade.net). Raw token counts are not
/// comparable across kinds: at Anthropic list prices an output token
/// costs 5x an input token, and a cache-read costs a tenth of one. So:
///   input-class measured savings ....... x1
///   output-class (brevity/nudges) ...... x5
///   compounding (avoided cache re-reads) x0.1
///   batching (avoided RE-sends) ........ x0.1 — same class as compounding
///   recovery ........................... x1 (input-class)
///
/// Batching sat at x1 until 1.1.5. Its own estimator sums `Σ sᵢ·(K−1−i)`,
/// which counts ONLY re-transmissions — the first send is never in it — and a
/// re-transmission of a block already in the prefix is billed as a cache read.
/// So the bucket was credited ten times its own price class, on the dashboard
/// and in what every installation reported upstream. Corrected forward only,
/// like the usage double-count before it; the server's `WEIGHTED_SQL` mirror
/// has to move in the same ship or the two disagree.
/// The ratios are stable across Anthropic models, so the unit stays
/// meaningful even when the exact model is unknown; per-model $ remains
/// the money truth where the model IS known.
pub fn weighted_tokens(input_class: u64, est: &EstimatedSummary) -> u64 {
    // `input_class` must already contain every x1 bucket (measured +
    // batching + recovery − overhead); `est` contributes only the
    // output-class (x5) and compounding (x0.1) parts.
    // PREFER `weighted_from_parts` — it also owns the input-class
    // assembly, so call sites can't drift on WHICH buckets go in.
    input_class
        .saturating_add(est.output.saturating_mul(5))
        .saturating_add(est.compounding / 10)
}

/// THE canonical weighted-tokens computation, owning the whole assembly:
///   (measured + batching + recovery − overhead) ×1
///   + output ×5 + compounding ×0.1
/// `measured` = the measured input-class savings (caller picks raw or
/// harness-capped realistic — that choice is display policy, the formula
/// is not). Every Rust surface MUST go through here (owner directive
/// 2026-07-04: one formula, one place). The HTML dashboard receives the
/// precomputed number; the server mirrors this in ONE shared SQL
/// fragment (drizzle/queries.ts WEIGHTED_SQL).
pub fn weighted_from_parts(measured: u64, cross_output: u64, est: &EstimatedSummary) -> u64 {
    let Some(w) = est.weighted else {
        return weighted_default_ratios(measured, cross_output, est);
    };
    // Each estimated row already counted at ITS model's ratios (`add_row`).
    // Cross-agent output has no model of its own: default ratio.
    let input_class = measured
        .saturating_add(est.recovery)
        .saturating_sub(est.overhead) as f64;
    let total =
        input_class + w.output + cross_output as f64 * DEFAULT_OUTPUT_WEIGHT + w.cache_credit
            - w.overhead_cache;
    total.max(0.0).round() as u64
}

/// [`weighted_from_parts`] for a summary without weighted twins (built by
/// hand): every bucket at the default ratios, integer arithmetic as before.
fn weighted_default_ratios(measured: u64, cross_output: u64, est: &EstimatedSummary) -> u64 {
    let w = weighted_tokens(
        measured
            .saturating_add(est.recovery)
            .saturating_sub(est.overhead),
        &EstimatedSummary {
            output: est.output.saturating_add(cross_output),
            // Batching rides the cache-read bucket: what it buys is re-sends
            // that never happen, which is what `compounding` already means.
            compounding: est.compounding.saturating_add(est.batching),
            ..Default::default()
        },
    );
    // Injected-context re-billing: same cache-read economics as the
    // compounding credit, opposite sign.
    w.saturating_sub(est.overhead_cache / 10)
}

/// The ratios of a row whose model is unknown — those of
/// [`crate::pricing::UNKNOWN_MODEL_STANDS_IN_AS`] at list price, and the server's
/// default for a row without `ioWeight` (tokenade.net `DEFAULT_IO_WEIGHTS`).
pub const DEFAULT_OUTPUT_WEIGHT: f64 = 5.0;
pub const DEFAULT_CACHE_WEIGHT: f64 = 0.1;

/// **What an output token and a cache read of `model` are worth, in input
/// tokens of that same model** — `(output/input, cache/input)` at its list
/// price. The x5 / x0.1 above were Opus's; they are not everyone's
/// (`claude-fable-5-1` reads cache at x0.025, `gpt-5.5` writes at x6).
///
/// Same rule as the server's `ioWeightFor` (tokenade.net lib/tokenade/pricing.ts):
/// the rate [`crate::pricing::rates_for_model`] resolves, bounded to what a
/// real price list publishes (output in ]0, 50], cache in [0, 1]); an empty
/// model or an unusable rate falls back to the defaults. The server publishes
/// the table it applies at tokenade.net/model-weights.
pub fn io_weights_for_model(model: &str) -> (f64, f64) {
    let model = model.trim().to_ascii_lowercase();
    let model = model.as_str();
    if model.is_empty() {
        return (DEFAULT_OUTPUT_WEIGHT, DEFAULT_CACHE_WEIGHT);
    }
    let memo = io_weights_memo();
    let mut m = memo.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(w) = m.weights.get(model) {
        return *w;
    }
    let w = io_weights_from_rates(&crate::pricing::rates_for_model(model))
        .unwrap_or((DEFAULT_OUTPUT_WEIGHT, DEFAULT_CACHE_WEIGHT));
    m.weights.insert(model.to_string(), w);
    m.save();
    w
}

/// Resolved ratios, kept on disk (`~/.tokenade/io-weights.json`) and valid as
/// long as the LiteLLM cache they were read from has not changed. Resolving a
/// model parses that 3 MB table; the statusline is a fresh process on every
/// tick and meets new rows on most of them, so without this each tick would
/// pay the parse to learn the same two numbers.
struct IoWeightsMemo {
    src: u64,
    weights: std::collections::HashMap<String, (f64, f64)>,
}

impl IoWeightsMemo {
    fn path() -> Option<std::path::PathBuf> {
        Some(
            crate::platform::home_dir()?
                .join(".tokenade")
                .join("io-weights.json"),
        )
    }
    fn source_stamp() -> u64 {
        crate::platform::home_dir()
            .map(|h| h.join(".tokenade").join("litellm-costs.json"))
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
    fn load() -> Self {
        let src = Self::source_stamp();
        let weights = Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .filter(|v| v["src"].as_u64() == Some(src))
            .and_then(|v| {
                v["w"].as_object().map(|o| {
                    o.iter()
                        .filter_map(|(k, x)| Some((k.clone(), (x[0].as_f64()?, x[1].as_f64()?))))
                        .collect()
                })
            })
            .unwrap_or_default();
        Self { src, weights }
    }
    fn save(&self) {
        let Some(p) = Self::path() else { return };
        let w: serde_json::Map<String, serde_json::Value> = self
            .weights
            .iter()
            .map(|(k, (o, c))| (k.clone(), serde_json::json!([o, c])))
            .collect();
        let tmp = p.with_extension("json.tmp");
        if std::fs::write(
            &tmp,
            serde_json::json!({"src": self.src, "w": w}).to_string(),
        )
        .is_ok()
        {
            let _ = std::fs::rename(&tmp, &p);
        }
    }
}

fn io_weights_memo() -> &'static std::sync::Mutex<IoWeightsMemo> {
    static M: std::sync::OnceLock<std::sync::Mutex<IoWeightsMemo>> = std::sync::OnceLock::new();
    M.get_or_init(|| std::sync::Mutex::new(IoWeightsMemo::load()))
}

/// Ratios of one rate line, or `None` when it cannot carry them. Pure.
pub fn io_weights_from_rates(r: &crate::pricing::ModelRates) -> Option<(f64, f64)> {
    if !(r.input > 0.0) {
        return None;
    }
    let (out, cache) = (r.output / r.input, r.cache / r.input);
    (out > 0.0 && out <= 50.0 && (0.0..=1.0).contains(&cache)).then_some((out, cache))
}

/// Estimated buckets already converted to input tokens of each row's model.
#[derive(Default, Debug, Clone, Copy)]
pub struct WeightedBuckets {
    /// `output` rows x their model's output/input ratio.
    pub output: f64,
    /// `compounding` + `batching` rows x their model's cache/input ratio.
    pub cache_credit: f64,
    /// `overhead_cache` rows x their model's cache/input ratio.
    pub overhead_cache: f64,
}

impl EstimatedSummary {
    /// Book ONE estimated row (already bounded: `saved` comes from
    /// [`estimated_saved_of_row`]) into its bucket and its weighted twin. The
    /// single place a row becomes a bucket — the full scan, the per-day split
    /// and the statusline's incremental tally all go through it.
    ///
    /// `class` is the bucket. Rotation folds rows into a rollup that drops it,
    /// so it is recovered from `op` (1:1 with class in the estimated ledger) —
    /// otherwise aged-out totals would vanish after the first rotation.
    pub fn add_row(&mut self, v: &serde_json::Value, saved: u64) {
        let class = canonical_estimated_class(v["class"].as_str(), v["op"].as_str().unwrap_or(""));
        let (out_w, cache_w) = io_weights_for_model(v["model"].as_str().unwrap_or(""));
        let w = self.weighted.get_or_insert_with(WeightedBuckets::default);
        let f = saved as f64;
        match class {
            "output" => {
                self.output = self.output.saturating_add(saved);
                w.output += f * out_w;
            }
            "batching" => {
                self.batching = self.batching.saturating_add(saved);
                w.cache_credit += f * cache_w;
            }
            "compounding" => {
                self.compounding = self.compounding.saturating_add(saved);
                w.cache_credit += f * cache_w;
            }
            "overhead" => {
                if v["io"].as_str() == Some("cache") {
                    self.overhead_cache = self.overhead_cache.saturating_add(saved);
                    w.overhead_cache += f * cache_w;
                } else {
                    self.overhead = self.overhead.saturating_add(saved);
                }
            }
            // "digest_recovery" = legacy class literal already on field
            // disks (writer fixed to canonical "recovery").
            "recovery" | "digest_recovery" => self.recovery = self.recovery.saturating_add(saved),
            _ => {}
        }
    }

    /// The weighted twins, or the raw buckets at the default ratios when this
    /// summary was built by hand.
    pub fn weighted_or_default(&self) -> WeightedBuckets {
        self.weighted.unwrap_or(WeightedBuckets {
            output: self.output as f64 * DEFAULT_OUTPUT_WEIGHT,
            cache_credit: self.compounding.saturating_add(self.batching) as f64
                * DEFAULT_CACHE_WEIGHT,
            overhead_cache: self.overhead_cache as f64 * DEFAULT_CACHE_WEIGHT,
        })
    }
}

/// One-line legend explaining the weighting — keep every surface showing
/// the SAME text so the number is never mistaken for a raw count.
pub const WEIGHTED_LEGEND_EN: &str = "Weighted at each model's own price ratios (Claude Opus: 1 output token = 5 input = 50 cache-read; every model at tokenade.net/model-weights). Raw counts aren't comparable across kinds - this makes the total mean what it costs.";
pub const WEIGHTED_LEGEND_FR: &str = "Pondéré aux ratios de prix de chaque modèle (Claude Opus : 1 token output = 5 input = 50 cache-read ; tous les modèles sur tokenade.net/fr/model-weights). Les volumes bruts ne sont pas comparables entre catégories - ce total reflète ce que ça coûte.";

/// Which API price line a saved token would have been billed on.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Io {
    Input,
    Cache,
    Output,
}

/// Classify one ledger row into (io-direction, is-overhead-cost). Mirrors
/// the server's `COALESCE(io,'input')` + class handling so client and server
/// bucket a row the same way. Overhead is a COST tokenade adds, netted out
/// of its bucket rather than counted as a saving.
/// What a row counts before pricing. Credited rows count the REALISTIC
/// saving, not the naive `before − after`: a huge Read/web/blob never sat
/// uncompacted in front of the model (the harness caps delivery), so
/// `realistic_of_row` bounds them by the channel's delivery cap. Overhead rows
/// are a real DEBIT (the injected context genuinely re-bills) — their naive
/// figure, so the cost isn't understated.
pub(crate) fn row_saved(v: &serde_json::Value, is_overhead: bool) -> u64 {
    if is_overhead {
        v["saved"].as_u64().unwrap_or_else(|| {
            let b = v["before"].as_u64().unwrap_or(0);
            let a = v["after"].as_u64().unwrap_or(0);
            b.saturating_sub(a)
        })
    } else {
        realistic_of_row(v)
    }
}

/// `saved` in the tokens `model` bills. The ledger counts in
/// `tokenizer::estimate` units (cl100k); the price is per token the MODEL
/// bills — converted here, at the one place a token becomes money.
/// `output_lean` is already in billed units (read from the transcript's
/// `output_tokens`), and `turns_context` / `recovery_turn` from the firing
/// message's `usage`. A row stamped `billed` at write time (an image's VISION
/// tokens, `Savings::billed`) too: ×1.6 inflated every image saving by 60 %.
/// Older rows lack the stamp and keep the value they were published with.
pub(crate) fn billed_tokens(v: &serde_json::Value, saved: u64, model: &str) -> u64 {
    if matches!(
        v["op"].as_str(),
        Some("output_lean" | "turns_context" | "recovery_turn")
    ) || v["billed"] == true
    {
        saved
    } else {
        (saved as f64 * crate::tokenizer::claude_tokens_per_estimated(model)).round() as u64
    }
}

/// One row's value in dollars at `model`'s rates, signed (overhead is a
/// cost): what the statusline adds up for a session, by the same rules as
/// [`real_savings_compute_with`] — less the read-time re-bill bound and the
/// credit de-duplication, which need the whole ledger.
pub fn row_usd(v: &serde_json::Value, model: &str) -> f64 {
    let (io, is_overhead) = row_io(v);
    let n = billed_tokens(v, row_saved(v, is_overhead), model);
    let r = crate::pricing::rates_for_model(model);
    let usd = match io {
        Io::Input => r.usd(n, 0, 0),
        Io::Cache => r.usd(0, n, 0),
        Io::Output => r.usd(0, 0, n),
    };
    if is_overhead {
        -usd
    } else {
        usd
    }
}

pub(super) fn row_io(v: &serde_json::Value) -> (Io, bool) {
    // Resolve the row's class from `class`, else the op→class map, so a row
    // written under either schema generation classifies the same.
    let class =
        super::canonical_estimated_class(v["class"].as_str(), v["op"].as_str().unwrap_or(""))
            .to_string();
    let is_overhead = class == "overhead";
    // Direction mirrors the server EXACTLY (license::tag_estimated_events +
    // the server's COALESCE(io,'input')): an explicit `io` on the row WINS;
    // only when it is absent do we derive it from the class. Anything else
    // (measured rows, unknown class) is input. This keeps a token priced the
    // same on the client and the server for every row shape.
    let io = match v["io"].as_str() {
        Some("cache") => Io::Cache,
        Some("output") => Io::Output,
        Some("input") => Io::Input,
        // Batching re-reads are cache reads (PO decision #5), as uploaded.
        _ => match class.as_str() {
            "output" | "output_lean" => Io::Output,
            "compounding" | "batching" => Io::Cache,
            _ => Io::Input,
        },
    };
    (io, is_overhead)
}

/// Real dollar value of savings + the net saved-token split by direction,
/// each event priced at the EXACT model that ran it. Attribution order:
/// the row's `sess` → that session's model; else the session whose time
/// span owns the row's `ts`; else the day-dominant model; else the default
/// rate. Priced at the shared per-model table ([`crate::pricing::rates_for_model`])
/// — the SAME figure the terminal + HTML dashboards and the server report.
#[derive(Default, Clone)]
pub struct RealSavings {
    pub usd: f64,
    pub input: u64,
    pub cache: u64,
    pub output: u64,
    /// Per-model breakdown: (model-id-or-"" for default, usd, input, cache,
    /// output), largest-$ first. Empty string = savings we couldn't attribute
    /// to a known model (no transcript for that session/day), priced at the
    /// default rate — the dashboards surface these as "default".
    pub by_model: Vec<(String, f64, u64, u64, u64)>,
    /// Epoch millis this was computed (drives cache freshness).
    pub computed_ms: u64,
}

pub(super) use crate::platform::now_unix_millis as now_ms;

/// Attribute every savings event to its exact model and price it. Walks the
/// measured + estimated ledgers and both rollups. HEAVY (parses transcripts):
/// callers on a hot path go through [`real_savings_cached`].
pub fn real_savings_compute() -> RealSavings {
    real_savings_compute_with(None)
}

/// [`real_savings_compute`], but reusing an attributor the caller already
/// built. Building one re-walks every transcript on the machine (measured
/// 461 ms here); a dashboard render has just done that walk, and handing the
/// result in is the difference between doing it once and doing it twice.
/// `None` keeps the old behaviour and builds one.
/// Turns a token of context is re-read at most, on either side of the ledger:
/// a folded token's compounding credit and an injected token's re-bill.
pub const HORIZON_TURNS: u64 = 12;

pub fn real_savings_compute_with(
    attributor: Option<crate::agent_transcripts::ModelAttributor>,
) -> RealSavings {
    use std::collections::HashMap;
    let home = crate::platform::home_dir();
    let attributor = attributor.or_else(|| {
        home.as_deref()
            .map(crate::agent_transcripts::ModelAttributor::build)
    });
    // **The row first, the inference second** — the server's exact order
    // (`stamp_event_models` does not touch an event that already carries its
    // model). Since 1.1.12 the hook writes the EXACT model at the moment it
    // writes the row; the attributor, which at best answers a session's
    // dominant model, only serves older rows. Without this order, the local
    // figure and the server-side figure attribute the same tokens to two
    // different models, and the gap is that between one rate and the other.
    let attribute = |row: &serde_json::Value, sess: Option<&str>, ts: u64| -> String {
        if let Some(m) = row
            .get("model")
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|m| !m.is_empty())
        {
            return m.to_ascii_lowercase().chars().take(64).collect();
        }
        attributor
            .as_ref()
            .and_then(|a| a.model_for(sess, ts))
            .unwrap_or_default()
            .to_ascii_lowercase()
    };

    // Net signed tokens per model per direction; overhead subtracts.
    let mut per_model: HashMap<(String, String), [i64; 3]> = HashMap::new();
    let ledgers = [
        current_ledger_path(),
        rollup_path_for(&current_ledger_path()),
        estimated_ledger_path(),
        rollup_path_for(&estimated_ledger_path()),
    ];
    // PO #37 (2026-08-04). A machine loop repeating one command books its full
    // counterfactual on every pass, and the counterfactual assumes a MODEL TURN
    // that never happened. MEASURED: a `cat` loop at ~37 identical events per
    // second put 89 245 193 tokens into one day — 28 % of this install's entire
    // lifetime figure — for roughly twenty seconds of work.
    //
    // Credited once per event — see `credit_key`, and `server_dedup_key` for
    // the coarser key the server's ingest uses. This comment claimed the two
    // were the same key; they were, until the local one gained the label and
    // the session, and the claim then stood false for a day. The local key is
    // FINER by construction, so this figure can sit above the metered one:
    // measured, by 62 events and 1 473 059 tokens, 0.24 %. Nothing is deleted
    // — every row stays in the ledger, only the credit is collapsed.
    let mut credited: std::collections::HashSet<(u64, String, u64, u64, u64)> =
        std::collections::HashSet::new();
    let mut rebill = super::scan::RebillBound::over(&ledger_with_rollup(&estimated_ledger_path()));
    for path in ledgers {
        let Ok(body) = fs::read_to_string(&path) else {
            continue;
        };
        for line in body.lines() {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if !credited.insert(credit_key(&v)) {
                continue;
            }
            // Ledger `ts` is epoch SECONDS; the attributor works in millis
            // (transcript spans are millis). Convert before matching.
            let ts = v["ts"].as_u64().unwrap_or(0).saturating_mul(1000);
            let (io, is_overhead) = row_io(&v);
            let saved = row_saved(&v, is_overhead);
            // A session's re-bill is bounded by what it injected — see
            // `scan::RebillBound`.
            let saved = rebill.take(&v, saved);
            if saved == 0 {
                continue;
            }
            let model = attribute(&v, v["sess"].as_str(), ts);
            let saved = billed_tokens(&v, saved, &model);
            // **The row's DAY is part of its key.** A model's rate changes;
            // valuing June at September's price moves a past figure without
            // any token having changed, and makes it diverge from the
            // server-side figure. Same UTC split as the server's daily rollup.
            let day = crate::rate_history::utc_day(ts / 1000);
            let slot = per_model.entry((model, day)).or_insert([0i64; 3]);
            let idx = match io {
                Io::Input => 0,
                Io::Cache => 1,
                Io::Output => 2,
            };
            // Saturating: a corrupt/huge ledger can't overflow the tally into
            // a negative (which max(0) would then read as zero savings).
            let delta = saved.min(i64::MAX as u64) as i64;
            slot[idx] = if is_overhead {
                slot[idx].saturating_sub(delta)
            } else {
                slot[idx].saturating_add(delta)
            };
        }
    }

    let mut out = RealSavings::default();
    // Sorted, for two reasons that both end up in the money figure: floating
    // addition is not associative, so summing the per-model dollars in hash
    // order made the TOTAL differ in its last bits between runs; and the
    // per-model list below is sorted by cost with a STABLE sort, so two models
    // costing the same kept whatever order the hash happened to produce.
    let mut per_model: Vec<_> = per_model.into_iter().collect();
    per_model.sort_by(|a, b| a.0.cmp(&b.0));
    // Folded per model AFTER valuation: each day is valued at its own rate,
    // and the displayed list stays per model.
    let mut folded: std::collections::HashMap<String, (f64, u64, u64, u64)> =
        std::collections::HashMap::new();
    // Loaded ONCE, here, and not per row: it is the caller who decides when
    // to read the file (see `RateHistory`).
    let history = crate::rate_history::RateHistory::load();
    for ((model, day), [i, c, o]) in per_model {
        let (i, c, o) = (i.max(0) as u64, c.max(0) as u64, o.max(0) as u64);
        if i == 0 && c == 0 && o == 0 {
            continue;
        }
        let usd = history.rate_on(&model, &day).usd(i, c, o);
        out.input += i;
        out.cache += c;
        out.output += o;
        out.usd += usd;
        let e = folded.entry(model).or_insert((0.0, 0, 0, 0));
        e.0 += usd;
        e.1 += i;
        e.2 += c;
        e.3 += o;
    }
    let mut folded: Vec<_> = folded.into_iter().collect();
    folded.sort_by(|a, b| a.0.cmp(&b.0));
    for (model, (usd, i, c, o)) in folded {
        out.by_model.push((model, usd, i, c, o));
    }
    out.by_model.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    out.computed_ms = now_ms();
    out
}

pub(super) fn real_savings_cache_path() -> Option<std::path::PathBuf> {
    crate::platform::home_dir().map(|h| h.join(".tokenade").join("real_savings.json"))
}

/// Read the real-savings cache WITHOUT ever computing. Returns
/// `(value, fresh)` — `fresh` is false when the file is older than
/// `max_age_secs`. `None` only when there is no readable cache at all.
/// The hot path (statusline) uses this so it NEVER blocks on the ~2s
/// transcript walk: a stale value is shown immediately while a detached
/// process refreshes it (see the CLI's `real-savings-refresh`).
pub fn real_savings_read_cache(max_age_secs: u64) -> Option<(RealSavings, bool)> {
    let p = real_savings_cache_path()?;
    let body = fs::read_to_string(&p).ok()?;
    let v: serde_json::Value = serde_json::from_str(&body).ok()?;
    let computed = v["computed_ms"].as_u64().unwrap_or(0);
    let fresh =
        computed > 0 && now_ms().saturating_sub(computed) < max_age_secs.saturating_mul(1000);
    let usd = v["usd"].as_f64().filter(|x| x.is_finite()).unwrap_or(0.0);
    Some((
        RealSavings {
            usd,
            input: v["input"].as_u64().unwrap_or(0),
            cache: v["cache"].as_u64().unwrap_or(0),
            output: v["output"].as_u64().unwrap_or(0),
            by_model: Vec::new(),
            computed_ms: computed,
        },
        fresh,
    ))
}

pub(super) fn real_savings_write_cache(rs: &RealSavings) {
    let Some(p) = real_savings_cache_path() else {
        return;
    };
    let json = serde_json::json!({
        "usd": rs.usd,
        "input": rs.input,
        "cache": rs.cache,
        "output": rs.output,
        "by_model": rs.by_model.iter().map(|(m, u, i, c, o)|
            serde_json::json!({"model": m, "usd": u, "input": i, "cache": c, "output": o})
        ).collect::<Vec<_>>(),
        "computed_ms": rs.computed_ms,
    });
    if let Some(dir) = p.parent() {
        let _ = fs::create_dir_all(dir);
    }
    // Atomic: concurrent refreshers race; a half-written file would fail to
    // parse and force a recompute every tick. Per-pid temp then rename.
    let tmp = p.with_extension(format!("json.tmp.{}", std::process::id()));
    if fs::write(&tmp, json.to_string()).is_ok() && fs::rename(&tmp, &p).is_err() {
        let _ = fs::remove_file(&tmp);
    }
}

/// Recompute (heavy) AND write the cache. The detached `real-savings-refresh`
/// process and the dashboards call this; the statusline never does.
pub fn real_savings_refresh() -> RealSavings {
    real_savings_refresh_with(None)
}

/// [`real_savings_refresh`] with a caller-supplied attributor — see
/// [`real_savings_compute_with`].
pub fn real_savings_refresh_with(
    attributor: Option<crate::agent_transcripts::ModelAttributor>,
) -> RealSavings {
    let rs = real_savings_compute_with(attributor);
    real_savings_write_cache(&rs);
    rs
}
