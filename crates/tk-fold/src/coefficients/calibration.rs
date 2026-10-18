//! **The default values, and what replaces them.**
//!
//! The defaults come from the bench; `tokenade calibrate` rewrites them from a
//! fresh measurement. A reader falls back to the default as soon as the file is missing
//! or a value leaves its range, so that a missing or
//! damaged calibration never produces a wild estimate.
//!
//! The write is a READ-MERGE-WRITE: the file also carries the user's
//! settings, and the old fresh-object write destroyed them
//! on every `calibrate`.

use super::*;

pub(crate) fn coefficients_path() -> PathBuf {
    crate::platform::home_dir()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join(".tokenade")
        .join("coefficients.json")
}

pub(crate) fn read_f64(key: &str) -> Option<f64> {
    let txt = std::fs::read_to_string(coefficients_path()).ok()?;
    let v: serde_json::Value = serde_json::from_str(&txt).ok()?;
    v.get(key).and_then(|x| x.as_f64())
}

/// $ per 1M INPUT tokens used to price savings (default: blended Claude-class
/// input rate). Override with TOKENADE_PRICE_INPUT_PER_MTOK. Same defaults as
/// the server's admin costing so dashboard and server agree.
pub fn price_input_per_mtok() -> f64 {
    std::env::var("TOKENADE_PRICE_INPUT_PER_MTOK")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|v: &f64| *v >= 0.0)
        .unwrap_or(3.0)
}

/// $ per 1M CACHE-READ tokens (default: Claude-class cache-read rate = 10%
/// of input). The rounds/cache "compounding" saving is priced at THIS rate —
/// the re-read context it avoids would have been billed as cache reads, not
/// fresh input. Override with TOKENADE_PRICE_CACHE_PER_MTOK.
pub fn price_cache_per_mtok() -> f64 {
    std::env::var("TOKENADE_PRICE_CACHE_PER_MTOK")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|v: &f64| *v >= 0.0)
        .unwrap_or(0.30)
}

/// $ per 1M OUTPUT tokens (default: blended Claude-class output rate — ~5x
/// input, which is WHY savings must be split by direction before pricing).
/// Override with TOKENADE_PRICE_OUTPUT_PER_MTOK.
pub fn price_output_per_mtok() -> f64 {
    std::env::var("TOKENADE_PRICE_OUTPUT_PER_MTOK")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|v: &f64| *v >= 0.0)
        .unwrap_or(15.0)
}

/// Saved output tokens per produced output token. Uses the calibrated value
/// from `~/.tokenade/coefficients.json` when present and in `(0, 1)`, else
/// the bench default.
pub fn output_lean_ratio() -> f64 {
    match read_f64("output_lean_ratio") {
        Some(r) if r > 0.0 && r < 1.0 => r,
        _ => DEFAULT_OUTPUT_LEAN_RATIO,
    }
}

/// Compute the output-lean ratio from a THOL `results.sqlite`: the relative
/// output-token reduction of `tokenade` vs `control` at the most recent
/// `claude_version`, i.e. `(control_mean_out − tokenade_mean_out) /
/// tokenade_mean_out`. Scoped to one claude_version so a version bump never
/// mixes boards. Returns `Err` on db/shape problems or a ratio outside
/// `(0, 1)` (we refuse to persist a nonsensical coefficient).
pub fn calibrate_output_lean_from_db(db_path: &std::path::Path) -> Result<f64, String> {
    let conn =
        rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| format!("open {}: {e}", db_path.display()))?;
    let cv: String = conn
        .query_row(
            "SELECT claude_version FROM runs ORDER BY started_at DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .map_err(|e| format!("read latest claude_version: {e}"))?;
    // The board names the arm `competitor` since it ranks other tools too;
    // older boards named it `tool`. Read whichever this one has: against the
    // current board the old name failed outright ("no such column: tool").
    let arm: String = conn
        .query_row(
            "SELECT name FROM pragma_table_info('runs') WHERE name IN ('competitor', 'tool') ORDER BY name = 'competitor' DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .map_err(|e| format!("runs has neither `competitor` nor `tool`: {e}"))?;
    let mean_out = |tool: &str| -> Result<f64, String> {
        conn.query_row(
            &format!(
                "SELECT AVG(output_tokens) FROM runs WHERE {arm}=?1 AND status='ok' \
                 AND claude_version=?2 AND output_tokens IS NOT NULL"
            ),
            rusqlite::params![tool, cv],
            |r| r.get::<_, Option<f64>>(0),
        )
        .map_err(|e| format!("mean output for {tool}: {e}"))?
        .ok_or_else(|| format!("no ok runs for '{tool}' @ {cv}"))
    };
    let control = mean_out("control")?;
    let tokenade = mean_out("tokenade")?;
    if tokenade <= 0.0 {
        return Err("tokenade mean output is zero".into());
    }
    let ratio = (control - tokenade) / tokenade;
    if !(0.0..1.0).contains(&ratio) {
        return Err(format!(
            "computed ratio {ratio:.4} outside (0,1) — control={control:.0} \
             tokenade={tokenade:.0}; refusing to persist"
        ));
    }
    Ok(ratio)
}

/// Persist calibrated coefficients (called by `tokenade calibrate`). Plain
/// `0644` — coefficients carry no secrets, only aggregate ratios.
pub fn write_output_lean_ratio(ratio: f64) -> std::io::Result<()> {
    let path = coefficients_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Read-merge-write: the file also carries user-calibrated overrides
    // (overflow_credit_ratio, cap_read_*/cap_bash_*) — the old fresh-object
    // write destroyed them on every `calibrate`. Unparsable → start fresh
    // (this file is tokenade-owned, unlike agent configs).
    let mut body = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    let obj = body.as_object_mut().expect("object ensured above");
    obj.insert("output_lean_ratio".into(), serde_json::json!(ratio));
    obj.insert(
        "calibrated_at".into(),
        serde_json::json!(crate::platform::now_unix_secs()),
    );
    // Atomic same-dir rename so concurrent readers never see a torn file.
    crate::paths::write_atomic(&path, &body.to_string())
}

#[cfg(test)]
mod calibrate_db_tests {
    use super::*;

    /// The current board names the arm `competitor`, an older one `tool`:
    /// both calibrate, from the latest Claude Code version only.
    #[test]
    fn either_arm_column_calibrates() {
        for col in ["competitor", "tool"] {
            let p =
                std::env::temp_dir().join(format!("tk-cal-{col}-{}.sqlite", std::process::id()));
            let _ = std::fs::remove_file(&p);
            let c = rusqlite::Connection::open(&p).unwrap();
            c.execute_batch(&format!("CREATE TABLE runs ({col} TEXT, status TEXT, claude_version TEXT, started_at TEXT, output_tokens INTEGER);")).unwrap();
            for (arm, v, out) in [
                ("control", "2.1.206", 1300),
                ("tokenade", "2.1.206", 1000),
                ("control", "2.1.100", 9000),
                ("tokenade", "2.1.100", 1000),
            ] {
                let at = if v == "2.1.206" {
                    "2026-07-18"
                } else {
                    "2026-06-01"
                };
                c.execute(&format!("INSERT INTO runs ({col}, status, claude_version, started_at, output_tokens) VALUES (?1, 'ok', ?2, ?3, ?4)"), rusqlite::params![arm, v, at, out]).unwrap();
            }
            drop(c);
            let r = calibrate_output_lean_from_db(&p).unwrap();
            assert!((r - 0.3).abs() < 1e-9, "{col}: {r}");
            let _ = std::fs::remove_file(&p);
        }
    }
}
