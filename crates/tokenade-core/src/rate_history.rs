//! **A model's price is DATED, and ours forgot it.**
//!
//! A June saving was displayed at the September rate: the figure moved under
//! the user without a single token changing, and it could not match what
//! those tokens were actually worth at the moment they were saved.
//!
//! This module keeps the local history, and the reading rule is fixed, word
//! for word, because two different numbers for the same day are impossible to
//! justify:
//!
//!   the rate for day D = the LAST observation dated <= D,
//!   failing that the OLDEST known one, failing that the current rate.
//!
//! The granularity is the DAY, not the event: a rate changes two or three
//! times a year, so writing it on every ledger line would repeat it millions
//! of times for a value that does not move. The file holds one line per
//! (model, day) SEEN on this machine — a few dozen lines a year.

use crate::pricing::ModelRates;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;

fn history_path() -> Option<PathBuf> {
    Some(
        crate::platform::home_dir()?
            .join(".tokenade")
            .join("rate-history.jsonl"),
    )
}

/// `YYYY-MM-DD` UTC for an epoch instant in SECONDS. The ledger is UTC end to
/// end, and its daily rollup cuts its days the same way: a local boundary
/// would make the two diverge by one day.
pub fn utc_day(ts_secs: u64) -> String {
    let days = (ts_secs / 86_400) as i64;
    // civil-from-days algorithm (Howard Hinnant), era since 1970-01-01.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

#[derive(Debug, Clone)]
struct Observation {
    day: String,
    model: String,
    rates: ModelRates,
}

fn read_file(path: &std::path::Path) -> Vec<Observation> {
    let Ok(body) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut out: Vec<Observation> = body
        .lines()
        .filter_map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).ok()?;
            let day = v.get("day")?.as_str()?.to_string();
            let model = v.get("model")?.as_str()?.to_string();
            let f = |k: &str| v.get(k).and_then(|x| x.as_f64());
            Some(Observation {
                day,
                model,
                rates: ModelRates {
                    input: f("i")?,
                    cache: f("c")?,
                    output: f("o")?,
                },
            })
        })
        .collect();
    out.sort_by(|a, b| a.model.cmp(&b.model).then_with(|| a.day.cmp(&b.day)));
    out
}

/// **The history, loaded once, by the caller.**
///
/// It was first memoised in a module-level `OnceLock`, and that was wrong
/// twice: a long-lived process never saw an observation written after its
/// first read, and under `cargo test` — a single process for the whole suite
/// — the first test to touch the table froze it for the others. The release
/// caught it: the period-rate test read $3 instead of 15. Loading is a
/// decision of the caller, not a side effect of the module.
#[derive(Debug, Clone, Default)]
pub struct RateHistory(Vec<Observation>);

impl RateHistory {
    /// Reads `~/.tokenade/rate-history.jsonl`. Missing or unreadable = empty,
    /// which makes it answer with the current rate — never zero.
    pub fn load() -> Self {
        match history_path() {
            Some(p) => Self::load_from(&p),
            None => Self::default(),
        }
    }

    /// From a NAMED file. This is what the tests use: moving `HOME` to target
    /// a file is process-global, so two tests doing it in parallel — which
    /// `cargo test` does by default — steal each other's directory.
    pub fn load_from(path: &std::path::Path) -> Self {
        Self(read_file(path))
    }

    /// The rate of `model` on day `day` (`YYYY-MM-DD`).
    ///
    /// Last observation dated <= `day`, failing that the oldest known one,
    /// failing that the CURRENT rate.
    pub fn rate_on(&self, model: &str, day: &str) -> ModelRates {
        let mut first: Option<&Observation> = None;
        let mut found: Option<&Observation> = None;
        for o in &self.0 {
            if o.model != model {
                continue;
            }
            if first.is_none() {
                first = Some(o);
            }
            if o.day.as_str() <= day {
                found = Some(o);
            }
        }
        match found.or(first) {
            Some(o) => o.rates,
            None => crate::pricing::rates_for_model(model),
        }
    }
}

/// What this process has ALREADY written, the target file included in the key.
///
/// The path is part of the key, and not out of excess caution: without it, a
/// process that changes `HOME` — the whole test suite, which runs in ONE
/// process under `cargo test` — would believe it had already written to a
/// file it never touched. Same defect as the memoised table, in the same
/// place, caught by the same release.
static WRITTEN: Mutex<Option<HashSet<(String, String, String)>>> = Mutex::new(None);

/// **Records today's rate for this model**, unless already done.
///
/// Called on the ledger write path, where the model is known: a line written
/// today can still be valued at today's rate ten months from now. Does
/// nothing for an empty model, and at most one write per model and per day —
/// the in-memory cache avoids re-reading on the hot path, the file decides
/// when the cache does not know.
pub fn observe(model: &str, now_secs: u64) {
    if let Some(p) = history_path() {
        observe_into(&p, model, now_secs);
    }
}

/// [`observe`], into a NAMED file — see [`RateHistory::load_from`] for why
/// the tests go through here rather than through `HOME`.
pub fn observe_into(path: &std::path::Path, model: &str, now_secs: u64) {
    if model.is_empty() {
        return;
    }
    let day = utc_day(now_secs);
    let key = (
        path.to_string_lossy().into_owned(),
        model.to_string(),
        day.clone(),
    );
    {
        let Ok(mut guard) = WRITTEN.lock() else {
            return;
        };
        let seen = guard.get_or_insert_with(HashSet::new);
        if seen.contains(&key) {
            return;
        }
        // The cache does not know: the FILE decides. A fresh process thus
        // inherits what a previous one already wrote.
        if read_file(path)
            .iter()
            .any(|o| o.model == model && o.day == day)
        {
            seen.insert(key);
            return;
        }
        seen.insert(key);
    }
    let r = crate::pricing::rates_for_model(model);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let line = serde_json::json!({
        "day": day, "model": model,
        "i": r.input, "c": r.cache, "o": r.output,
    });
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_day_is_the_utc_calendar_day() {
        assert_eq!(utc_day(0), "1970-01-01");
        assert_eq!(utc_day(86_399), "1970-01-01");
        assert_eq!(utc_day(86_400), "1970-01-02");
        // 2026-09-10T00:00:00Z
        assert_eq!(utc_day(1_788_998_400), "2026-09-10");
        // One second earlier: the day before. This is the boundary that must
        // hold, because a local hour would make the client diverge from the
        // rollup.
        assert_eq!(utc_day(1_788_998_399), "2026-09-09");
        // A leap year, where a one-day shift shows.
        assert_eq!(utc_day(1_709_164_800), "2024-02-29");
    }

    #[test]
    fn the_rate_of_a_day_is_the_last_observation_on_or_before_it() {
        let dir = std::env::temp_dir().join(format!(
            "tokenade-rate-hist-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("rate-history.jsonl");
        std::fs::write(
            &file,
            [
                r#"{"day":"2026-08-01","model":"m","i":5,"c":0.5,"o":25}"#,
                r#"{"day":"2026-06-01","model":"m","i":15,"c":1.5,"o":75}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        let h = RateHistory::load_from(&file);
        assert_eq!(h.rate_on("m", "2026-06-15").input, 15.0, "period rate");
        assert_eq!(
            h.rate_on("m", "2026-08-01").input,
            5.0,
            "the same day counts"
        );
        assert_eq!(h.rate_on("m", "2026-09-30").input, 5.0);
        // Before the first observation: the oldest one, NOT today's.
        assert_eq!(h.rate_on("m", "2026-01-01").input, 15.0);
        // A model unknown to the history takes the current rate.
        assert_eq!(
            h.rate_on("claude-haiku-4-5", "2026-06-15").input,
            crate::pricing::rates_for_model("claude-haiku-4-5").input
        );
        // An EMPTY history never answers zero.
        assert!(RateHistory::default().rate_on("m", "2026-06-15").input > 0.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn one_line_per_model_and_day_at_most() {
        let dir = std::env::temp_dir().join(format!(
            "tokenade-rate-obs-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("rate-history.jsonl");
        for _ in 0..5 {
            observe_into(&file, "claude-opus-5", 1_788_998_400);
        }
        observe_into(&file, "claude-opus-5", 1_788_998_400 + 86_400);
        observe_into(&file, "", 1_788_998_400);
        let body = std::fs::read_to_string(&file).unwrap();
        assert_eq!(body.lines().count(), 2, "{body}");
        assert!(body.contains("2026-09-10") && body.contains("2026-09-11"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
