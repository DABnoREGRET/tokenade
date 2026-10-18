//! One-shot recalculation of the LOCAL stats ledgers after the 0.8.x
//! accounting-rules fix (delivery caps incl. the new MCP cap, per-agent
//! web/binary credit assiettes, bounded compounding).
//!
//! Ledgers written under the OLD rules carry inflated figures in fields
//! that are read back verbatim:
//!   - rollup buckets store a precomputed `realistic` (folded per-event with
//!     the OLD caps — `realistic_of_row` returns it verbatim forever);
//!   - web/binary rows store a fictitious `before` (raw-HTML tokens or
//!     bytes/4) that the new credit models no longer claim;
//!   - legacy compounding estimates were unbounded (the session logs needed
//!     to re-derive them are gone);
//!   - `output_lean` used the pre-calibration ratio.
//!
//! SERVER-DEDUP INVARIANT (coordinated with the parallel server-side
//! correction, migrations 0036/0037): the server dedups ingested events on
//! `(licenseId, machineId, ts, op, beforeTokens, afterTokens)` with
//! `ON CONFLICT DO NOTHING`. This migration therefore NEVER modifies `ts`,
//! `op`, `before` or `after` on any KEPT row — live, rollup or
//! pending-upload — it rewrites ONLY the derived fields `saved` and
//! `realistic`. A re-POST of an already-uploaded historical row keeps its
//! exact dedup key, collides, and is dropped — the server keeps its own
//! corrected value (its correction applies to ts < the 2026-07-08 cutoff);
//! zero double-correction. Rows the reporter had NOT yet uploaded go out
//! with their corrected `saved` because `parse_gain_line` honours a
//! recalc-stamped `realistic` verbatim. Deleting a row (`cd`, compounding)
//! can never create a duplicate — it is simply never re-sent.
//!
//! Policy (never keep the old inflated figure; recompute from surviving
//! fields where possible, otherwise replace with the documented conservative
//! estimate, otherwise delete). Every corrected row gets BOTH `saved` and a
//! stamped `realistic` = the same corrected value, clamped to
//! `before − after` (the server's ingest clamp), so read-side aggregators
//! (`saved`-field readers) and upload/rollup paths (`realistic_of_row`)
//! all return it verbatim:
//!
//! MEASURED ledger (`gain.jsonl`, its rollup + pending-upload spill):
//!   - `webfetch_compact*`        → saving 0 (passthrough credit model: we
//!     cannot retroactively tell which fetches were legit PDF/big-needle
//!     credits, so credit nothing — conservative).
//!   - `webfetch_binary*`         → `binary_turn_tax()` (~800) × calls.
//!   - `read_image_dedup[_hit]`   → vision-token estimate (~1600) × calls
//!     (`_registered` rows are 0-saving bookkeeping, untouched).
//!   - `html-to-markdown*`        → Claude rows (and rollup buckets, whose
//!     agent is lost — conservative) re-credited at the Transparent model:
//!     `round(0.6·before) − after`, floored at 0 (never claim to beat the
//!     native clean text). Other agents keep their honest raw-HTML credit.
//!   - `cd` / `review-context`    → deleted (proven residue).
//!   - rollup buckets             → `realistic` recomputed with the NEW caps
//!     from the surviving (before, after, calls) via the per-event
//!     approximation `realistic_saved(before/calls, after/calls, cap)·calls`,
//!     and NEVER increased: `min(old realistic (or naive), recomputed, naive)`.
//!   - live rows on a NON-truncating channel (the ex-`u64::MAX` aider / cline /
//!     gemini-cli / grok / windsurf / antigravity read/shell) had their 50 M
//!     `strings`/`xxd` dumps credited IN FULL — re-cap `saved` + stamp
//!     `realistic` at the per-agent cap (overflow suppressed). This is the v2
//!     addition: recalc-v1 (u64::MAX caps) never bounded them.
//!   - other live measured rows (truncating channels) are untouched: they store
//!     no `realistic`, so readers/uploaders already derive it live with the NEW
//!     caps.
//!
//! Known residue (accepted): a rollup bucket's `before`/`after` sums keep the
//! legacy assiette (untouchable per the dedup invariant), so the NAIVE
//! `before − after` a future re-fold recomputes for a web bucket's `saved`
//! regresses to the legacy figure — the `realistic` chain (dashboard hero,
//! server quota) is the corrected metric and survives every re-fold via its
//! precomputed field.
//!
//! ESTIMATED ledger (`gain_estimated.jsonl`, rollup + pending-upload):
//!   - `class == "compounding"`   → DELETED. Not recomputable (per-session
//!     transcripts rotated away); a bounded re-derivation is impossible and
//!     any scaled guess would still be an old-rules number.
//!   - `class == "output"` (`output_lean`/`write_to_edit`) → saving × 0.5
//!     (typical prose share; exact recomputation would need the original
//!     per-session token splits which the ledger does not keep).
//!   - `overhead_rebill` before [`OVERHEAD_BOUND_TS`] → DELETED, for the same
//!     reason and by the same rule as `compounding` above: it is the charge
//!     side of the SAME horizon, it was written unbounded, and re-deriving it
//!     needs the per-session injected totals the ledger never kept. The two
//!     were fixed apart — the credit got its horizon and this migration in
//!     one move, the charge got its horizon on 2026-08-03 and nothing here.
//!   - `batching` / other `overhead` → kept (their formulas did not change).
//!
//! Safety: originals are copied to `~/.tokenade/backup-recalc-<ts>/` before
//! any rewrite; the whole migration is guarded by a marker file
//! (`recalc-v2.done`) + a cross-process flock, so it runs exactly once; every
//! error is swallowed (fail-open — stats must never break the CLI); rewritten
//! rows are tagged `"recalc": 1` for auditability.
//!
//! Reporter coherence: the reporter tracks a BYTE offset per ledger
//! (`reporter-offset[-estimated]`, format `off:size:last_ts:head`). A rewrite
//! shifts every byte, so the old offset would point mid-row (or a stale head
//! hash would trigger a full rotation-reset walk). We REMAP the offset to the
//! byte position, in the rewritten file, of the first not-yet-uploaded row —
//! already-uploaded rows are never re-sent (the server keeps whatever it
//! already summed; the parallel server-side correction owns that history),
//! and rows the reporter had NOT yet uploaded are sent with their corrected
//! values.

use std::fs;
use std::path::Path;

/// Marker file (in the `.tokenade` dir) proving the v1 recalc already ran.
// v2 (2026-07-09): re-run once more on already-migrated machines so the
// per-agent bash/read caps that replaced the ex-`u64::MAX` channels (aider/
// cline/gemini-cli/grok/windsurf/antigravity) AND the zeroed overflow credit
// for non-truncating channels re-cap the inflated `strings`/binary-dump rows
// that recalc-v1 left uncapped. `recalc-v1.done` alone would never re-fire.
// v3 (2026-08-28): the overhead re-bill was made TRIANGULAR-free on
// 2026-08-03 (commit 7ae2f8e6 bounded it at `H x injected`, mirroring the
// credit horizon) and no migration ever corrected the rows written before
// it. Measured on this machine's ledgers the day v3 was written: 2398 live
// + rollup rows totalling 47_418_848 tokens of old-rules charge, 79% of all
// `overhead_rebill` ever booked. Re-running the whole v2 policy is safe —
// every rule recomputes from the preserved `before`/`after`/`calls` or
// deletes, so a second pass is a fixpoint.
pub const MARKER_FILE: &str = "recalc-v3.done";
const LOCK_FILE: &str = "recalc-v3.lock";

/// Commit timestamp of the fix that bounded the overhead re-bill
/// (7ae2f8e6, 2026-08-03T18:34:24Z). Rows stamped before it were produced
/// by the unbounded formula; rows at or after it already obey the horizon
/// and are kept verbatim.
pub const OVERHEAD_BOUND_TS: u64 = 1_785_782_064;
const BACKUP_DIR_PREFIX: &str = "backup-recalc-";

/// Vision-token estimate credited per deduped image read (matches the
/// read-hook's fallback when dimensions are unknown).
const IMAGE_VISION_TOKENS: u64 = 1_600;

#[derive(Default, Debug, Clone, Copy)]
pub struct RecalcSummary {
    /// False when the marker already existed (no-op).
    pub ran: bool,
    pub rows_rewritten: u64,
    pub rows_deleted: u64,
    pub files_touched: u64,
}

/// Entry point for the CLI: recalc the global `~/.tokenade` stats once.
/// Fail-open: any error or panic is absorbed — telemetry must never break
/// the caller.
pub fn run_if_needed() {
    let Some(dir) = crate::gain::current_ledger_path()
        .parent()
        .map(Path::to_path_buf)
    else {
        return;
    };
    let _ = std::panic::catch_unwind(move || {
        let _ = run_in_dir(&dir);
    });
}

/// Run the one-shot recalc against an explicit `.tokenade` directory.
/// Idempotent (marker + flock). Per-file failures are skipped, never fatal.
pub fn run_in_dir(dir: &Path) -> std::io::Result<RecalcSummary> {
    let marker = dir.join(MARKER_FILE);
    if marker.exists() {
        return Ok(RecalcSummary::default());
    }
    if !dir.exists() {
        // Fresh machine: nothing to recalc, but drop the marker so the new
        // rules are never "migrated" over rows they already produced.
        fs::create_dir_all(dir)?;
    }
    let lock = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(dir.join(LOCK_FILE))?;
    // Another process is already recalculating; its result is the one this
    // caller would have computed, so waiting for it buys nothing.
    if !crate::platform::lock_briefly(&lock) {
        return Ok(RecalcSummary::default());
    }
    // Re-check under the lock (another process may have just finished).
    if marker.exists() {
        return Ok(RecalcSummary::default());
    }

    let ts = crate::platform::now_unix_secs();
    let backup_dir = dir.join(format!("{BACKUP_DIR_PREFIX}{ts}"));

    let mut summary = RecalcSummary {
        ran: true,
        ..Default::default()
    };

    // (ledger file, estimated rules?, reporter offset file to remap)
    let targets: [(&str, bool, Option<&str>); 6] = [
        ("gain.jsonl", false, Some("reporter-offset")),
        ("gain.rollup.jsonl", false, None),
        ("gain.jsonl.pending-upload.jsonl", false, None),
        (
            "gain_estimated.jsonl",
            true,
            Some("reporter-offset-estimated"),
        ),
        ("gain_estimated.rollup.jsonl", true, None),
        ("gain_estimated.jsonl.pending-upload.jsonl", true, None),
    ];
    let horizon = legacy_horizon(dir);
    for (name, estimated, offset_file) in targets {
        // Per-file fail-open: an unreadable/corrupt file is left in place.
        if let Ok(Some((rw, del))) =
            recalc_file(dir, &backup_dir, name, estimated, offset_file, horizon)
        {
            summary.rows_rewritten += rw;
            summary.rows_deleted += del;
            summary.files_touched += 1;
        }
    }

    // The dashboard/statusline read a cached aggregate — drop it so the next
    // render recomputes from the rewritten ledgers immediately.
    let _ = fs::remove_file(dir.join("real_savings.json"));

    // Marker: JSON summary for `tokenade doctor`-style forensics.
    let marker_body = serde_json::json!({
        "v": 3,
        "ts": ts,
        "rows_rewritten": summary.rows_rewritten,
        "rows_deleted": summary.rows_deleted,
        "files_touched": summary.files_touched,
        "backup": backup_dir.file_name().and_then(|s| s.to_str()),
    });
    fs::write(&marker, marker_body.to_string())?;
    Ok(summary)
}

/// Timestamp after which a row was written under rules a PREVIOUS recalc
/// already installed, and must therefore be left alone by that recalc's
/// policy.
///
/// Why this exists (found the hard way, 2026-08-28): v1 and v2 were each
/// shipped ONCE, at a moment when every row on disk predated the rules they
/// correct, so "re-run the whole policy" was harmless. That property is not a
/// property of the policy — it is a property of running it before the new
/// rules had produced anything. A third pass on a machine migrated in July
/// inherits seven weeks of rows written under the CORRECT rules, and re-runs
/// the old-rules corrections over them: measured on this machine's real
/// ledger, a v3 pass with no horizon deleted 8_172_741 tokens of legitimate
/// bounded `compounding` credit (the v2 rule "legacy compounding is not
/// recomputable") and rewrote the measured ledger it had left untouched in
/// July. Restored from the backup this module takes first; the horizon is
/// the fix.
///
/// The previous marker already records WHEN it ran, so the boundary needs no
/// new state. No marker at all → nothing was ever migrated → every row is
/// old-rules (`u64::MAX`).
fn legacy_horizon(dir: &Path) -> u64 {
    for name in ["recalc-v2.done", "recalc-v1.done"] {
        if let Some(ts) = fs::read_to_string(dir.join(name))
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| v["ts"].as_u64())
            .filter(|ts| *ts > 0)
        {
            return ts;
        }
    }
    u64::MAX
}

fn row_ts(v: &serde_json::Value) -> u64 {
    v["ts"].as_u64().unwrap_or(0)
}

/// Rewrite one ledger file under the recalc policy. Returns
/// `Ok(None)` when the file does not exist, `Ok(Some((rewritten, deleted)))`
/// on success. The original is copied into `backup_dir` FIRST — if that copy
/// fails the file is left untouched.
fn recalc_file(
    dir: &Path,
    backup_dir: &Path,
    name: &str,
    estimated: bool,
    offset_file: Option<&str>,
    horizon: u64,
) -> std::io::Result<Option<(u64, u64)>> {
    let path = dir.join(name);
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };

    // Backup before ANY rewrite (safety: rewrite only proceeds past here).
    fs::create_dir_all(backup_dir)?;
    fs::copy(&path, backup_dir.join(name))?;

    let mut out = String::with_capacity(content.len());
    let mut rewritten: u64 = 0;
    let mut deleted: u64 = 0;
    // (old byte start, new byte start) of every KEPT line — offset remap.
    let mut kept_map: Vec<(u64, u64)> = Vec::new();
    let mut old_pos: u64 = 0;
    for line in content.lines() {
        let old_start = old_pos;
        old_pos += line.len() as u64 + 1;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(mut v) = serde_json::from_str::<serde_json::Value>(line) else {
            // Unparseable line: preserve verbatim (fail-open, never lose data).
            kept_map.push((old_start, out.len() as u64));
            out.push_str(line);
            out.push('\n');
            continue;
        };
        let action = if estimated {
            recalc_estimated_row(&mut v, horizon)
        } else {
            recalc_measured_row(&mut v, horizon)
        };
        match action {
            RowAction::Delete => deleted += 1,
            RowAction::Keep(changed) => {
                kept_map.push((old_start, out.len() as u64));
                if changed {
                    v["recalc"] = serde_json::Value::from(1);
                    rewritten += 1;
                    out.push_str(&v.to_string());
                } else {
                    // Untouched rows keep their exact original bytes.
                    out.push_str(line);
                }
                out.push('\n');
            }
        }
    }

    // Atomic replace, through the one declarer of that discipline: a temp
    // name chosen here would be shared by every process, and this rewrites
    // the ledger.
    crate::platform::write_atomic_600(&path, out.as_bytes())?;

    // Remap the reporter byte offset onto the rewritten file: point it at the
    // new position of the first row the server had NOT yet received. Rows
    // before it are never re-sent; rows after it upload with corrected values.
    if let Some(off_name) = offset_file {
        remap_reporter_offset(&dir.join(off_name), &kept_map, out.len() as u64);
    }
    Ok(Some((rewritten, deleted)))
}

enum RowAction {
    Delete,
    /// Keep the row; `true` when a field was rewritten.
    Keep(bool),
}

fn row_op(v: &serde_json::Value) -> String {
    v["op"]
        .as_str()
        .or_else(|| v["filter"].as_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn is_rollup_row(v: &serde_json::Value) -> bool {
    v["rollup"].as_bool() == Some(true) || v["source"].as_str() == Some("rollup")
}

/// New-rules rewrite of one MEASURED row (live ledger or rollup bucket).
fn recalc_measured_row(v: &mut serde_json::Value, horizon: u64) -> RowAction {
    // Written after a previous recalc installed these rules: already correct.
    if row_ts(v) >= horizon {
        return RowAction::Keep(false);
    }
    let op = row_op(v);
    if op == "cd" || op == "review-context" {
        return RowAction::Delete;
    }
    let before = v["before"].as_u64().unwrap_or(0);
    let after = v["after"].as_u64().unwrap_or(0);
    let calls = v["rollup_calls"].as_u64().unwrap_or(1).max(1);
    let rollup = is_rollup_row(v);
    let naive = before.saturating_sub(after);

    // 1) Web/binary credit assiette: the stored SAVING is fictitious under
    //    the new models — replace `saved` + stamp `realistic` with the new
    //    credit, clamped to `before − after` (the server's ingest clamp).
    //    `before`/`after`/`ts`/`op` are NEVER touched: they are the server's
    //    dedup key (see module doc).
    let new_saved: Option<u64> = if op.starts_with("webfetch_compact") {
        // Passthrough model, unknowable legit credits retroactively → 0.
        Some(0)
    } else if op.starts_with("webfetch_binary") {
        Some(crate::coefficients::binary_turn_tax().saturating_mul(calls))
    } else if op.starts_with("read_image_dedup") && !op.ends_with("_registered") {
        Some(IMAGE_VISION_TOKENS.saturating_mul(calls))
    } else if op.starts_with("html-to-markdown") {
        let agent = v["agent"].as_str().unwrap_or("");
        // Rollup buckets lost the agent — apply the (smaller) Transparent
        // credit to stay conservative.
        if rollup || agent == "claude-code" {
            let credit_before =
                (before as f64 * crate::coefficients::web_native_clean_ratio()).round() as u64;
            Some(credit_before.saturating_sub(after))
        } else {
            None // honest raw-HTML credit for shell-web agents: keep as-is.
        }
    } else {
        None
    };

    let mut changed = false;
    if let Some(ns) = new_saved {
        let ns = ns.min(naive); // server clamps saved ≤ before − after.
        if v["saved"].as_u64() != Some(ns) || v["realistic"].as_u64() != Some(ns) {
            v["saved"] = serde_json::Value::from(ns);
            // Stamp `realistic` so `realistic_of_row` (upload, rollup folds,
            // dashboard) returns the corrected value verbatim instead of
            // re-deriving an inflated one from the fictitious `before`.
            v["realistic"] = serde_json::Value::from(ns);
            changed = true;
        }
        return RowAction::Keep(changed);
    }

    // 2) Rollup buckets: the stored per-event `realistic` was folded with the
    //    OLD caps and is read back verbatim — recompute with the new caps.
    if rollup {
        let old_real = v["realistic"].as_u64().unwrap_or(naive);
        // Per-event approximation from the surviving aggregate: the cap is
        // per event, so apply it to the bucket's mean event. Source/agent
        // were lost at fold time, so only op-identifiable channels (read-like
        // ops, `mcp:*`) get a cap; the rest fall back to naive — and the
        // `min` below guarantees we never INCREASE a legacy figure.
        let cap = crate::coefficients::delivery_cap_tokens("", "", &op);
        // Agent was lost at fold time, so we can't tell a non-truncating channel
        // apart here — keep the standard (truncating) overflow credit; the
        // `.min(naive)`/`.min(old)` below still guarantees no INCREASE.
        let ratio = crate::coefficients::overflow_credit_ratio();
        let per_event = crate::gain::realistic_saved(before / calls, after / calls, cap, ratio);
        let recomputed = per_event.saturating_mul(calls);
        let new_real = old_real.min(recomputed).min(naive);
        if v["realistic"].as_u64() != Some(new_real) {
            v["realistic"] = serde_json::Value::from(new_real);
            changed = true;
        }
    } else if !is_credit_model_op(&op) {
        // 3) Live row on a NON-truncating channel (the ex-`u64::MAX` aider/
        //    cline/gemini-cli/grok/windsurf/antigravity read/shell): recalc-v1
        //    credited a 50 M `strings`/`xxd` binary dump IN FULL (u64::MAX cap
        //    ⇒ realistic == naive). Re-cap `saved` + stamp `realistic` with the
        //    per-agent cap (overflow suppressed — nothing was truncated). Rows
        //    carry their real agent/source, so the cap is exact. TRUNCATING
        //    channels (claude-code, cursor, …) fall through untouched — their
        //    naive `saved` stays and `realistic` is derived live, exactly as
        //    before. `before`/`after`/`ts`/`op` are never touched (dedup key).
        let agent = v["agent"].as_str().unwrap_or("");
        let source = v["source"].as_str().unwrap_or("");
        if crate::coefficients::channel_nontruncating(agent, source, &op) {
            let cap = crate::coefficients::delivery_cap_tokens(agent, source, &op);
            let ratio = crate::coefficients::overflow_ratio_for_channel(agent, source, &op);
            let real = crate::gain::realistic_saved(before, after, cap, ratio).min(naive);
            if real < v["saved"].as_u64().unwrap_or(naive) {
                v["saved"] = serde_json::Value::from(real);
                v["realistic"] = serde_json::Value::from(real);
                changed = true;
            }
        }
    }
    RowAction::Keep(changed)
}

/// Ops carrying their own per-model credit assiette (web / binary / image):
/// their `before` is already a bounded credit figure, NOT a raw shell/read
/// dump, so the non-truncating re-cap must skip them (they are handled — or
/// deliberately kept — by the web/binary/image branches above).
fn is_credit_model_op(op: &str) -> bool {
    op.starts_with("webfetch") || op.starts_with("html-to-markdown") || op.starts_with("read_image")
}

/// New-rules rewrite of one ESTIMATED row (live ledger or rollup bucket).
fn recalc_estimated_row(v: &mut serde_json::Value, horizon: u64) -> RowAction {
    let op = row_op(v);
    // Charge side of the same horizon: unbounded before the 2026-08-03 fix,
    // and not recomputable (the per-session injected totals it was derived
    // from are not in the row). Same verdict as `compounding` below. Rows
    // written under the bound are already correct and stay untouched. This
    // rule carries its OWN cutoff, so it runs ahead of the v1/v2 horizon.
    if op == "overhead_rebill" && row_ts(v) < OVERHEAD_BOUND_TS {
        return RowAction::Delete;
    }
    if row_ts(v) >= horizon {
        return RowAction::Keep(false);
    }
    if op == "cd" || op == "review-context" {
        return RowAction::Delete;
    }
    let class = v["class"].as_str().unwrap_or("").to_ascii_lowercase();
    // Legacy compounding: unbounded old-rules estimate, not recomputable
    // (session transcripts are gone) → delete rather than keep any old-rules
    // number.
    if class == "compounding" || op == "compounding" {
        return RowAction::Delete;
    }
    let is_output = class == "output" || op == "output_lean" || op == "write_to_edit";
    if !is_output {
        return RowAction::Keep(false); // batching / overhead: formulas unchanged.
    }
    let before = v["before"].as_u64().unwrap_or(0);
    let after = v["after"].as_u64().unwrap_or(0);
    let halved = before.saturating_sub(after) / 2;
    // `before`/`after`/`ts` preserved (server dedup key) — only the derived
    // `saved` + a stamped `realistic` carry the halved estimate.
    let mut changed = false;
    if v["saved"].as_u64() != Some(halved) || v["realistic"].as_u64() != Some(halved) {
        v["saved"] = serde_json::Value::from(halved);
        v["realistic"] = serde_json::Value::from(halved);
        changed = true;
    }
    RowAction::Keep(changed)
}

/// Rewrite `off:size:last_ts:head` so the reporter neither re-uploads
/// already-sent rows (offset lands on the first unsent row's NEW position;
/// size/head match the rewritten file so no rotation reset fires) nor skips
/// unsent ones. `last_ts` is preserved as the final safety net. Absent file →
/// nothing was ever reported → nothing to remap.
fn remap_reporter_offset(offset_path: &Path, kept_map: &[(u64, u64)], new_len: u64) {
    let Ok(raw) = fs::read_to_string(offset_path) else {
        return;
    };
    let mut parts = raw.trim().split(':');
    let old_off: u64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let last_ts: u64 = parts.nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let new_off = kept_map
        .iter()
        .find(|(old, _)| *old >= old_off)
        .map(|(_, new)| *new)
        .unwrap_or(new_len);
    // head=0 disables the fingerprint check for exactly one reporter pass —
    // it re-saves a v2 hash on its next successful save. size == the real
    // rewritten size so the shrink check cannot fire either.
    let tmp = offset_path.with_extension("rotating");
    if fs::write(&tmp, format!("{new_off}:{new_len}:{last_ts}:0")).is_ok() {
        let _ = fs::rename(&tmp, offset_path);
    }
}
#[cfg(test)]
include!("recalc_tests.rs");
