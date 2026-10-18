//! **Does the ledger tell the truth about itself?**
//!
//! A savings ledger is an accounting base: it gets re-read, backed up,
//! re-imported. This module checks what those operations left behind — rows
//! counted twice because a backup was re-ingested, a total that no longer
//! matches its rows. Nothing here writes to the ledger; everything here passes
//! a verdict on what it contains.

use super::*;

/// Ledger self-consistency audit (the durable reconciliation: a measured
/// `before` — a command's stdout — can't be re-derived from disk once the
/// command has run, so we verify the INVARIANTS every honest row must
/// satisfy; a future double-count or decoupled-`saved` bug breaks one).
/// Returns (rows_checked, violations) with human-readable violation lines.
pub fn audit_gains() -> (u64, Vec<String>) {
    let mut checked = 0u64;
    let mut bad = Vec::new();
    let push = |bad: &mut Vec<String>, msg: String| {
        if bad.len() < 50 {
            bad.push(msg);
        }
    };
    // Schema note (validated against REAL field ledgers, not assumptions):
    // rows carry `before`/`after`/`saved` (saved is written as before-after
    // at record time — an inequality is either corruption or a foreign
    // writer), and the estimated flag IN FILE is `estimated: true` (`est`
    // is the UPLOAD-time field name). Rollup rows sum all three per bucket
    // so the same invariants hold. Class values are an OPEN set across
    // versions — unknown classes are informational NOTES, never hard
    // violations (an old user's ledger must not fail doctor).
    let est_flag = |v: &serde_json::Value| {
        v["estimated"].as_bool() == Some(true) || v["est"].as_bool() == Some(true)
    };
    for (n, line) in ledger_with_rollup(&measured_ledger_path())
        .lines()
        .enumerate()
    {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        checked += 1;
        let before = v["before"].as_u64().unwrap_or(0);
        let after = v["after"].as_u64().unwrap_or(0);
        if est_flag(&v) {
            push(&mut bad, format!("measured line {n}: estimated row in the MEASURED ledger (belongs in gain_estimated)"));
        }
        if after > before {
            push(&mut bad, format!("measured line {n}: after {after} > before {before} (negative saving in exact ledger)"));
        }
        if let Some(sv) = v["saved"].as_u64() {
            if sv > before.saturating_sub(after) {
                push(
                    &mut bad,
                    format!(
                        "measured line {n}: saved {sv} exceeds before-after {} (decoupled/forged)",
                        before.saturating_sub(after)
                    ),
                );
            }
        }
    }
    for (n, line) in ledger_with_rollup(&estimated_ledger_path())
        .lines()
        .enumerate()
    {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        checked += 1;
        if !est_flag(&v) {
            push(
                &mut bad,
                format!("estimated line {n}: missing estimated:true flag"),
            );
        }
        if v["op"].as_str() == Some("overhead_rebill") && v["io"].as_str() != Some("cache") {
            push(&mut bad, format!("estimated line {n}: overhead_rebill without io=cache (would weigh 10x at the server)"));
        }
        let before = v["before"].as_u64().unwrap_or(0);
        let after = v["after"].as_u64().unwrap_or(0);
        if let Some(sv) = v["saved"].as_u64() {
            if sv > before.saturating_sub(after) {
                push(
                    &mut bad,
                    format!(
                        "estimated line {n}: saved {sv} exceeds before-after {}",
                        before.saturating_sub(after)
                    ),
                );
            }
        }
    }
    for l in ingested_backup_overlap(&measured_ledger_path()) {
        push(&mut bad, l);
    }
    for l in ingested_backup_overlap(&estimated_ledger_path()) {
        push(&mut bad, l);
    }
    (checked, bad)
}

/// Rows that are in BOTH the live ledger and a backup already folded into the
/// rollup — i.e. counted twice by every reader of `ledger + rollup`.
///
/// The per-row invariants above cannot see this: each of the two copies is
/// individually well-formed. It went unnoticed on a dogfood install until the
/// day totals were compared and four separate days came out at EXACTLY 2x —
/// 29 216 rows, 199 194 952 tokens, +93 % on the lifetime figure.
///
/// `migrate_backups_for` no longer creates this (it seeds its dedup set from
/// the live ledger). Ledgers that already carry it need a human decision about
/// rewriting accounting history, so this reports rather than repairs.
///
/// Local display only: the reporter uploads ledger lines with its own byte
/// offset and never reads the rollup, so an inflated rollup never reached the
/// server.
pub(super) fn ingested_backup_overlap(ledger: &std::path::Path) -> Vec<String> {
    let (Some(dir), Some(base)) = (ledger.parent(), ledger.file_name().and_then(|s| s.to_str()))
    else {
        return Vec::new();
    };
    let Ok(live) = fs::read_to_string(ledger) else {
        return Vec::new();
    };
    let hash_line = |s: &str| -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        s.trim().hash(&mut h);
        h.finish()
    };
    let live_set: std::collections::HashSet<u64> = live
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(hash_line)
        .collect();
    if live_set.is_empty() {
        return Vec::new();
    }
    let prefix = format!("{base}.");
    let mut rows = 0u64;
    let mut tokens = 0u64;
    let mut seen: std::collections::HashSet<u64> = std::collections::HashSet::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    for ent in entries.flatten() {
        let fname = ent.file_name();
        let Some(name) = fname.to_str() else { continue };
        if !name.starts_with(&prefix) || !name.ends_with(".counted") {
            continue;
        }
        let Ok(body) = fs::read_to_string(ent.path()) else {
            continue;
        };
        for line in body.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let h = hash_line(line);
            if !live_set.contains(&h) || !seen.insert(h) {
                continue;
            }
            rows += 1;
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                tokens += v["saved"].as_u64().unwrap_or(0);
            }
        }
    }
    if rows == 0 {
        return Vec::new();
    }
    vec![format!(
        "{base}: {rows} rows are in BOTH the live ledger and an already-folded \
         backup — {tokens} tokens counted twice in ledger+rollup totals. Local \
         display only (the reporter never reads the rollup). New folds no longer \
         do this; repairing history is a manual decision."
    )]
}
