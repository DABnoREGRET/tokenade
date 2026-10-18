//! The A/B core.
//!
//! The component that makes every battery's verdict trustworthy, not just its
//! numbers. Four rules, each fixing a specific mistake that cost hours in this
//! project:
//!
//! 1. **Same session, both arms.** A battery must compute baseline AND
//!    candidate in the same process run and hand both to [`print_table`]
//!    together — never a number carried over from a previous run.
//! 2. **The baseline is the SHIPPED path through its real entry point** — the
//!    caller's job, not this module's: call `distill_page`, `Engine::parse`,
//!    `tokenizer::estimate`, never a hand-rolled stand-in.
//! 3. **Real captured bytes.** The caller feeds frozen fixtures, not
//!    synthesized text — again the caller's job; this module only grades.
//! 4. **A verdict is a printed TABLE, not an assertion.** `print_table` never
//!    panics. Hard-assert only on floors, with `assert!` calls the caller
//!    makes separately.
//!
//! Stage 1 has no candidate yet: every battery calls this with
//! `baseline == candidate`, so every delta is `0.0` and every verdict is
//! `ok`. The machinery — table shape, veto marker, floor helper — is what
//! Stage 1 proves; later stages wire a real candidate arm through the same
//! calls.

#![allow(dead_code)]

/// How a metric's value should be rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Pct,
    Tokens,
    Count,
    Ms,
    Ratio,
}

impl Unit {
    fn fmt(self, v: f64) -> String {
        match self {
            Unit::Pct => format!("{v:.1}%"),
            Unit::Tokens | Unit::Count => format!("{v:.0}"),
            Unit::Ms => format!("{v:.0}ms"),
            Unit::Ratio => format!("{v:.3}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    Better,
    Regression,
}

/// One row of an A/B table.
pub struct Metric {
    pub name: String,
    pub baseline: f64,
    pub candidate: f64,
    pub unit: Unit,
    /// `true` for fidelity/precision-shaped metrics (bigger is better);
    /// `false` for cost-shaped metrics (tokens, latency — smaller is better).
    pub higher_is_better: bool,
    /// A regression on this metric blocks a ship — Battery 6's token p95.
    pub veto: bool,
}

impl Metric {
    pub fn new(
        name: impl Into<String>,
        baseline: f64,
        candidate: f64,
        unit: Unit,
        higher_is_better: bool,
    ) -> Self {
        Self {
            name: name.into(),
            baseline,
            candidate,
            unit,
            higher_is_better,
            veto: false,
        }
    }

    /// Mark this metric as the veto row: a regression here prints
    /// `REGRESSION  <-- veto` instead of plain `REGRESSION`.
    pub fn vetoed(mut self) -> Self {
        self.veto = true;
        self
    }

    /// Signed percent change from baseline to candidate. `+inf`/`-inf` when
    /// baseline is ~0 and candidate is not (division by zero would lie).
    pub fn delta_pct(&self) -> f64 {
        if self.baseline.abs() < 1e-9 {
            if (self.candidate - self.baseline).abs() < 1e-9 {
                0.0
            } else if self.candidate > self.baseline {
                f64::INFINITY
            } else {
                f64::NEG_INFINITY
            }
        } else {
            (self.candidate - self.baseline) / self.baseline.abs() * 100.0
        }
    }

    /// Below this, a delta is noise, not a verdict — printed `ok` either way.
    const NOISE_PCT: f64 = 0.5;

    pub fn verdict(&self) -> Verdict {
        let d = self.delta_pct();
        if !d.is_finite() {
            return if d > 0.0 {
                if self.higher_is_better {
                    Verdict::Better
                } else {
                    Verdict::Regression
                }
            } else {
                if self.higher_is_better {
                    Verdict::Regression
                } else {
                    Verdict::Better
                }
            };
        }
        if d.abs() < Self::NOISE_PCT {
            return Verdict::Ok;
        }
        let improved = if self.higher_is_better {
            d > 0.0
        } else {
            d < 0.0
        };
        if improved {
            Verdict::Better
        } else {
            Verdict::Regression
        }
    }
}

/// Print the A/B table:
/// `metric | baseline | candidate | delta | verdict`, `verdict` one of
/// `ok` / `BETTER` / `REGRESSION`, with `<-- veto` appended when a
/// [`Metric::vetoed`] row regresses.
///
/// Never panics — rule 4: the table IS the verdict, an assertion is not.
pub fn print_table(metrics: &[Metric]) {
    println!(
        "{:<34}{:>12}{:>12}{:>10}  verdict",
        "metric", "baseline", "candidate", "delta"
    );
    for m in metrics {
        let verdict = m.verdict();
        let delta = m.delta_pct();
        let delta_s = if delta.is_finite() {
            format!("{delta:+.1}%")
        } else if delta > 0.0 {
            "+inf".to_string()
        } else {
            "-inf".to_string()
        };
        let verdict_s = match verdict {
            Verdict::Ok => "ok".to_string(),
            Verdict::Better => "BETTER".to_string(),
            Verdict::Regression if m.veto => "REGRESSION  <-- veto".to_string(),
            Verdict::Regression => "REGRESSION".to_string(),
        };
        println!(
            "{:<34}{:>12}{:>12}{:>10}  {}",
            m.name,
            m.unit.fmt(m.baseline),
            m.unit.fmt(m.candidate),
            delta_s,
            verdict_s,
        );
    }
}

/// A hard floor — an invariant that was a shipped bug once, so it is asserted
/// rather than merely tabled. Kept separate from `assert!` call sites only so
/// every floor in a battery reads the same way when grepped.
#[track_caller]
pub fn assert_floor(cond: bool, msg: impl AsRef<str>) {
    assert!(cond, "{}", msg.as_ref());
}
