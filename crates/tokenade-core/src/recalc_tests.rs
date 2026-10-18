// The tests of `recalc`.
//
// Spliced by `include!` from `recalc.rs`: the same module, so the same private
// access and the same `super::` paths, without a single visibility line
// changed. This is the shape this repository already uses for 33 other
// modules.
//
// A `//!` is impossible here: a file spliced by `include!` is not a module,
// and an INNER doc comment has nothing to qualify.

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    fn tdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "tokenade-recalc-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn read_rows(p: &Path) -> Vec<serde_json::Value> {
        fs::read_to_string(p)
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }
    /// A plausible "v2 ran here" instant, before the overhead bound landed.
    const V2_RAN_AT: u64 = 1_783_591_882;

    fn row_by_op<'a>(rows: &'a [serde_json::Value], op: &str) -> Option<&'a serde_json::Value> {
        rows.iter().find(|r| r["op"].as_str() == Some(op))
    }

    #[test]
    fn inflated_legacy_ledger_is_rebounded() {
        let dir = tdir("policy");
        let ledger = [
            json!({"ts": 100, "source": "hook", "op": "webfetch_compact", "agent": "claude-code", "before": 50_000, "after": 3_000, "saved": 47_000}),
            json!({"ts": 101, "source": "hook", "op": "webfetch_binary_extracted", "agent": "claude-code", "before": 200_000, "after": 4_000, "saved": 196_000}),
            json!({"ts": 102, "source": "hook", "op": "read_image_dedup", "agent": "claude-code", "before": 999_999, "after": 0, "saved": 999_999}),
            json!({"ts": 103, "source": "wrap", "op": "html-to-markdown", "agent": "claude-code", "before": 28_000, "after": 7_000, "saved": 21_000}),
            json!({"ts": 104, "source": "wrap", "op": "html-to-markdown", "agent": "grok", "before": 28_000, "after": 7_000, "saved": 21_000}),
            json!({"ts": 105, "source": "wrap", "op": "git-status", "agent": "claude-code", "cmd": "git", "before": 2_000, "after": 300, "saved": 1_700}),
            json!({"ts": 106, "source": "cli", "op": "cd", "before": 500, "after": 0, "saved": 500}),
            json!({"ts": 107, "source": "cli", "op": "review-context", "before": 900, "after": 0, "saved": 900}),
        ]
        .map(|v| v.to_string())
        .join("\n");
        fs::write(dir.join("gain.jsonl"), ledger + "\n").unwrap();

        let s = run_in_dir(&dir).unwrap();
        assert!(s.ran);
        let rows = read_rows(&dir.join("gain.jsonl"));
        assert_eq!(rows.len(), 6, "cd/review-context deleted");
        assert_eq!(s.rows_deleted, 2);

        // SERVER-DEDUP INVARIANT: ts/op/before/after untouched on EVERY kept
        // row — only saved/realistic move.
        let w = row_by_op(&rows, "webfetch_compact").unwrap();
        assert_eq!(w["ts"], 100);
        assert_eq!(w["before"], 50_000, "dedup key: before preserved");
        assert_eq!(w["after"], 3_000, "dedup key: after preserved");
        assert_eq!(w["saved"], 0);
        assert_eq!(w["realistic"], 0);
        assert_eq!(w["recalc"], 1);

        let b = row_by_op(&rows, "webfetch_binary_extracted").unwrap();
        let tax = crate::coefficients::binary_turn_tax();
        assert_eq!(b["before"], 200_000, "dedup key: before preserved");
        assert_eq!(b["saved"].as_u64().unwrap(), tax);
        assert_eq!(b["realistic"].as_u64().unwrap(), tax);

        let i = row_by_op(&rows, "read_image_dedup").unwrap();
        assert_eq!(i["before"], 999_999, "dedup key: before preserved");
        assert_eq!(i["saved"].as_u64().unwrap(), IMAGE_VISION_TOKENS);
        assert_eq!(i["realistic"].as_u64().unwrap(), IMAGE_VISION_TOKENS);

        let ratio = crate::coefficients::web_native_clean_ratio();
        let h: Vec<_> = rows
            .iter()
            .filter(|r| r["op"] == "html-to-markdown")
            .collect();
        let claude = h.iter().find(|r| r["agent"] == "claude-code").unwrap();
        let expect_saved = ((28_000f64 * ratio).round() as u64).saturating_sub(7_000);
        assert_eq!(claude["before"], 28_000, "dedup key: before preserved");
        assert_eq!(claude["saved"].as_u64().unwrap(), expect_saved.min(21_000));
        let grok = h.iter().find(|r| r["agent"] == "grok").unwrap();
        assert_eq!(grok["saved"], 21_000, "non-claude web credit kept");
        assert!(grok.get("recalc").is_none());

        let git = row_by_op(&rows, "git-status").unwrap();
        assert_eq!(git["saved"], 1_700, "ordinary measured row untouched");
        assert!(git.get("recalc").is_none());
        assert!(
            git.get("realistic").is_none(),
            "untouched rows stay unstamped (derived live)"
        );
    }

    #[test]
    fn rollup_realistic_is_recomputed_with_new_caps_never_increased() {
        let dir = tdir("rollup");
        let rows = [
            // Read-channel bucket folded pre-fix with naive realistic.
            json!({"ts": 86_400, "source": "rollup", "op": "read_content_dedup", "label": "(rolled-up)", "agent": "(rolled-up)", "before": 400_000, "after": 10_000, "saved": 390_000, "realistic": 390_000, "rollup": true, "rollup_calls": 2}),
            // MCP bucket: previously uncapped (u64::MAX channel).
            json!({"ts": 86_400, "source": "rollup", "op": "mcp:everything", "label": "(rolled-up)", "agent": "(rolled-up)", "before": 500_000, "after": 5_000, "saved": 495_000, "realistic": 495_000, "rollup": true, "rollup_calls": 1}),
            // Already-small bucket: min() must keep the smaller old figure.
            json!({"ts": 86_400, "source": "rollup", "op": "read_small", "label": "(rolled-up)", "agent": "(rolled-up)", "before": 1_000, "after": 200, "saved": 800, "realistic": 700, "rollup": true, "rollup_calls": 1}),
        ]
        .map(|v| v.to_string())
        .join("\n");
        fs::write(dir.join("gain.rollup.jsonl"), rows + "\n").unwrap();

        run_in_dir(&dir).unwrap();
        let rows = read_rows(&dir.join("gain.rollup.jsonl"));
        let ratio = crate::coefficients::overflow_credit_ratio();

        let read = row_by_op(&rows, "read_content_dedup").unwrap();
        let cap = crate::coefficients::delivery_cap_tokens("", "", "read_content_dedup");
        let expect = crate::gain::realistic_saved(200_000, 5_000, cap, ratio) * 2;
        assert_eq!(read["realistic"].as_u64().unwrap(), expect.min(390_000));
        assert!(read["realistic"].as_u64().unwrap() < 390_000);

        let mcp = row_by_op(&rows, "mcp:everything").unwrap();
        let cap = crate::coefficients::delivery_cap_tokens("", "", "mcp:everything");
        let expect = crate::gain::realistic_saved(500_000, 5_000, cap, ratio);
        assert_eq!(mcp["realistic"].as_u64().unwrap(), expect.min(495_000));
        assert!(mcp["realistic"].as_u64().unwrap() < 495_000);

        let small = row_by_op(&rows, "read_small").unwrap();
        assert_eq!(small["realistic"], 700, "never increase a legacy figure");

        // Rollup buckets too: before/after/ts sums preserved verbatim.
        let read = row_by_op(&rows, "read_content_dedup").unwrap();
        assert_eq!(read["before"], 400_000);
        assert_eq!(read["after"], 10_000);
        assert_eq!(read["ts"], 86_400);
    }

    #[test]
    fn v2_recaps_inflated_strings_dump_on_already_migrated_machine() {
        // Simulate a machine where recalc-v1 already ran (v1.done present) but
        // left a 50 M `strings` dump on a non-truncating agent credited IN FULL
        // (u64::MAX-era). v2 (fresh marker) must re-fire once and re-cap it.
        let dir = tdir("v2recap");
        // Old v1 marker present — must NOT block the v2 run.
        fs::write(dir.join("recalc-v1.done"), "{\"v\":1}").unwrap();
        assert!(!dir.join(MARKER_FILE).exists());

        let ledger = [
            // The reported over-count: aider `strings`, 50 M before, credited full.
            json!({"ts": 300, "source": "wrap", "op": "strings", "agent": "aider", "before": 50_000_000u64, "after": 1_000, "saved": 49_999_000u64}),
            // gemini-cli `xxd` shell dump — same non-truncating channel.
            json!({"ts": 301, "source": "wrap", "op": "xxd", "agent": "gemini-cli", "before": 4_000_000u64, "after": 500, "saved": 3_999_500u64}),
            // A TRUNCATING claude-code shell row: must stay untouched (naive
            // saved kept, realistic derived live) — regression guard.
            json!({"ts": 302, "source": "wrap", "op": "git-status", "agent": "claude-code", "before": 2_000, "after": 300, "saved": 1_700}),
            // A gemini-cli READ (truncating channel for that agent) below cap:
            // realistic == naive, so no re-cap.
            json!({"ts": 303, "source": "hook", "op": "read", "agent": "gemini-cli", "before": 5_000, "after": 1_000, "saved": 4_000}),
        ]
        .map(|v| v.to_string())
        .join("\n");
        fs::write(dir.join("gain.jsonl"), ledger + "\n").unwrap();

        let s = run_in_dir(&dir).unwrap();
        assert!(s.ran, "v2 must run even though recalc-v1.done exists");
        assert!(dir.join(MARKER_FILE).exists(), "recalc-v2.done written");

        let rows = read_rows(&dir.join("gain.jsonl"));

        // aider strings: re-capped to the per-agent bash cap (overflow off).
        let st = row_by_op(&rows, "strings").unwrap();
        let cap = crate::coefficients::delivery_cap_tokens("aider", "wrap", "strings");
        assert_eq!(cap, 12_800);
        assert_eq!(st["saved"].as_u64().unwrap(), cap - 1_000); // 11_800
        assert_eq!(st["realistic"].as_u64().unwrap(), cap - 1_000);
        assert_eq!(st["before"], 50_000_000u64, "dedup key: before preserved");
        assert_eq!(st["after"], 1_000, "dedup key: after preserved");
        assert_eq!(st["recalc"], 1);

        // gemini-cli xxd is NO LONGER re-capped, and that is the point of the
        // v2 gate: the migration targets only the ex-`u64::MAX` rows of
        // NON-truncating channels. gemini-cli left that set on 2026-08-10 when
        // its shell truncation was measured, so its rows keep the ordinary
        // treatment. Asserted, not deleted: a row that stops being rewritten
        // is a behaviour change and deserves to be visible.
        let xx = row_by_op(&rows, "xxd").unwrap();
        assert!(
            !crate::coefficients::channel_nontruncating("gemini-cli", "wrap", "xxd"),
            "the gate that decides this row's fate"
        );
        assert_ne!(xx["saved"].as_u64().unwrap(), 12_800 - 500);

        // Truncating claude-code shell row untouched.
        let git = row_by_op(&rows, "git-status").unwrap();
        assert_eq!(git["saved"], 1_700);
        assert!(git.get("recalc").is_none());
        assert!(git.get("realistic").is_none());

        // Small gemini-cli read (truncating channel, below cap): untouched.
        let rd = row_by_op(&rows, "read").unwrap();
        assert_eq!(rd["saved"], 4_000);
        assert!(rd.get("recalc").is_none());

        // Idempotent: a second run is a no-op (marker present).
        assert!(!run_in_dir(&dir).unwrap().ran);
    }

    #[test]
    fn estimated_compounding_deleted_output_halved_batching_kept() {
        let dir = tdir("est");
        let rows = [
            json!({"ts": 200, "source": "hook", "op": "compounding", "class": "compounding", "estimated": true, "before": 1_000_000, "after": 0, "saved": 1_000_000}),
            json!({"ts": 201, "source": "hook", "op": "output_lean", "class": "output", "estimated": true, "before": 1_000, "after": 0, "saved": 1_000}),
            json!({"ts": 202, "source": "hook", "op": "batching", "class": "batching", "estimated": true, "before": 400, "after": 0, "saved": 400}),
            json!({"ts": 203, "source": "hook", "op": "scaffold", "class": "overhead", "estimated": true, "before": 900, "after": 0, "saved": 900}),
        ]
        .map(|v| v.to_string())
        .join("\n");
        fs::write(dir.join("gain_estimated.jsonl"), rows + "\n").unwrap();

        let s = run_in_dir(&dir).unwrap();
        let rows = read_rows(&dir.join("gain_estimated.jsonl"));
        assert_eq!(rows.len(), 3);
        assert!(row_by_op(&rows, "compounding").is_none());
        assert_eq!(s.rows_deleted, 1);

        let out = row_by_op(&rows, "output_lean").unwrap();
        assert_eq!(out["saved"], 500);
        assert_eq!(out["realistic"], 500);
        assert_eq!(out["before"], 1_000, "dedup key: before preserved");
        assert_eq!(out["after"], 0, "dedup key: after preserved");
        assert_eq!(out["recalc"], 1);

        assert_eq!(row_by_op(&rows, "batching").unwrap()["saved"], 400);
        assert_eq!(row_by_op(&rows, "scaffold").unwrap()["saved"], 900);
    }

    /// The charge side of the compounding horizon. Rows written before the
    /// bound landed carry the triangular old-rules figure and go the same way
    /// the credit side's did; rows written under the bound are correct and
    /// must survive byte-for-byte, as must every other overhead op whose
    /// formula never changed.
    #[test]
    fn estimated_unbounded_overhead_rebill_deleted_bounded_one_kept() {
        let dir = tdir("ovh");
        let rows = [
            json!({"ts": OVERHEAD_BOUND_TS - 1, "source": "hookwrap-userprompt", "op": "overhead_rebill", "class": "overhead", "io": "cache", "estimated": true, "before": 11_400_000, "after": 0, "saved": 11_400_000}),
            json!({"ts": OVERHEAD_BOUND_TS, "source": "hookwrap-userprompt", "op": "overhead_rebill", "class": "overhead", "io": "cache", "estimated": true, "before": 2_444, "after": 0, "saved": 2_444}),
            json!({"ts": 5, "source": "hook", "op": "injected_context", "class": "overhead", "estimated": true, "before": 700, "after": 0, "saved": 700}),
            json!({"ts": 6, "source": "hookwrap-sessionstart", "op": "scaffold_overhead", "class": "overhead", "estimated": true, "before": 900, "after": 0, "saved": 900}),
        ]
        .map(|v| v.to_string())
        .join("\n");
        fs::write(dir.join("gain_estimated.jsonl"), rows + "\n").unwrap();

        let s = run_in_dir(&dir).unwrap();
        assert_eq!(s.rows_deleted, 1, "only the pre-bound re-bill");
        let rows = read_rows(&dir.join("gain_estimated.jsonl"));
        assert_eq!(rows.len(), 3);

        // The bounded one is kept AND untouched — no `recalc` stamp, no
        // rewritten `saved`: a correct row must not be re-derived.
        let kept = rows
            .iter()
            .find(|r| r["op"] == "overhead_rebill")
            .expect("the row written under the bound survives");
        assert_eq!(kept["ts"], OVERHEAD_BOUND_TS);
        assert_eq!(kept["saved"], 2_444);
        assert!(kept.get("recalc").is_none());

        // Overhead ops that were never triangular are unaffected by their ts.
        assert_eq!(row_by_op(&rows, "injected_context").unwrap()["saved"], 700);
        assert_eq!(row_by_op(&rows, "scaffold_overhead").unwrap()["saved"], 900);
    }

    /// A correction nobody re-runs is not a correction. Every machine in the
    /// field carries `recalc-v2.done`, so the version bump IS the delivery
    /// mechanism — the same reason v2 could not reuse `recalc-v1.done`.
    #[test]
    fn an_already_v2_migrated_machine_still_gets_this_pass() {
        let dir = tdir("v2machine");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("recalc-v2.done"),
            json!({"v": 2, "ts": V2_RAN_AT}).to_string(),
        )
        .unwrap();
        let row = json!({"ts": OVERHEAD_BOUND_TS - 1, "source": "hookwrap-userprompt", "op": "overhead_rebill", "class": "overhead", "io": "cache", "estimated": true, "before": 9_000, "after": 0, "saved": 9_000});
        fs::write(dir.join("gain_estimated.jsonl"), row.to_string() + "\n").unwrap();

        let s = run_in_dir(&dir).unwrap();
        assert!(s.ran, "the v2 marker must not gate this pass");
        assert_eq!(s.rows_deleted, 1);
        assert!(read_rows(&dir.join("gain_estimated.jsonl")).is_empty());
    }

    /// v1 and v2 each ran once, before the rules they correct had produced a
    /// single row — so re-running their whole policy was free. A third pass
    /// does not inherit that: on a machine migrated in July it meets weeks of
    /// rows written under the CORRECT rules. Re-correcting those destroys real
    /// figures (measured on a live ledger: 8.1 M tokens of bounded compounding
    /// credit deleted by the v2 "legacy compounding" rule). The previous
    /// marker's own timestamp is the boundary.
    #[test]
    fn a_row_written_since_the_previous_recalc_is_not_re_corrected() {
        let dir = tdir("horizon");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("recalc-v2.done"),
            json!({"v": 2, "ts": V2_RAN_AT}).to_string(),
        )
        .unwrap();
        let est = [
            // Old-rules compounding: still deleted.
            json!({"ts": V2_RAN_AT - 100, "source": "hook", "op": "compounding", "class": "compounding", "estimated": true, "before": 900_000, "after": 0, "saved": 900_000}),
            // Written under the bound v2 shipped: MUST survive untouched.
            json!({"ts": V2_RAN_AT + 100, "source": "hook", "op": "compounding", "class": "compounding", "estimated": true, "before": 4_000, "after": 0, "saved": 4_000}),
            // Post-horizon, but before the overhead bound: the NEW rule owns
            // it, and its cutoff is its own — not the previous marker's.
            json!({"ts": OVERHEAD_BOUND_TS - 1, "source": "hookwrap-userprompt", "op": "overhead_rebill", "class": "overhead", "io": "cache", "estimated": true, "before": 500_000, "after": 0, "saved": 500_000}),
        ]
        .map(|v| v.to_string())
        .join("\n");
        fs::write(dir.join("gain_estimated.jsonl"), est + "\n").unwrap();
        let measured = [
            json!({"ts": V2_RAN_AT - 100, "source": "hook", "op": "webfetch_compact", "before": 9_000, "after": 100, "saved": 8_900}),
            json!({"ts": V2_RAN_AT + 100, "source": "hook", "op": "webfetch_compact", "before": 9_000, "after": 100, "saved": 8_900}),
        ]
        .map(|v| v.to_string())
        .join("\n");
        fs::write(dir.join("gain.jsonl"), measured + "\n").unwrap();

        assert!(run_in_dir(&dir).unwrap().ran);

        let est = read_rows(&dir.join("gain_estimated.jsonl"));
        assert_eq!(est.len(), 1, "old compounding and old re-bill both go");
        assert_eq!(est[0]["ts"], V2_RAN_AT + 100);
        assert_eq!(est[0]["saved"], 4_000);
        assert!(est[0].get("recalc").is_none());

        let m = read_rows(&dir.join("gain.jsonl"));
        assert_eq!(m.len(), 2);
        assert_eq!(m[0]["saved"], 0, "pre-horizon fetch: zeroed as in v2");
        assert_eq!(m[1]["saved"], 8_900, "post-horizon fetch: left alone");
        assert!(m[1].get("recalc").is_none());
    }

    /// A machine that never ran a recalc has no boundary: every row it holds
    /// predates the corrected rules, which is exactly what v1 assumed.
    #[test]
    fn no_previous_marker_means_every_row_is_old_rules() {
        let dir = tdir("nohorizon");
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(legacy_horizon(&dir), u64::MAX);
        // A marker with no usable timestamp is the same case, not a zero.
        fs::write(dir.join("recalc-v2.done"), "{}").unwrap();
        assert_eq!(legacy_horizon(&dir), u64::MAX);
    }

    #[test]
    fn idempotent_second_run_is_noop_and_backup_created() {
        let dir = tdir("idem");
        let orig = json!({"ts": 1, "source": "hook", "op": "webfetch_compact", "before": 9_000, "after": 100, "saved": 8_900})
            .to_string()
            + "\n";
        fs::write(dir.join("gain.jsonl"), &orig).unwrap();

        let s1 = run_in_dir(&dir).unwrap();
        assert!(s1.ran);
        assert_eq!(s1.rows_rewritten, 1);
        assert!(dir.join(MARKER_FILE).exists());

        // Backup holds the ORIGINAL bytes.
        let backup = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(BACKUP_DIR_PREFIX)
            })
            .expect("backup dir");
        assert_eq!(
            fs::read_to_string(backup.path().join("gain.jsonl")).unwrap(),
            orig
        );

        let after_first = fs::read_to_string(dir.join("gain.jsonl")).unwrap();
        let s2 = run_in_dir(&dir).unwrap();
        assert!(!s2.ran, "marker makes the second run a no-op");
        assert_eq!(
            fs::read_to_string(dir.join("gain.jsonl")).unwrap(),
            after_first
        );
    }

    #[test]
    fn fail_open_on_corrupt_lines_preserves_them_verbatim() {
        let dir = tdir("corrupt");
        let good = json!({"ts": 2, "source": "hook", "op": "webfetch_compact", "before": 5_000, "after": 50, "saved": 4_950}).to_string();
        fs::write(
            dir.join("gain.jsonl"),
            format!("{{not json at all\n{good}\n\u{1}binary-ish garbage\n"),
        )
        .unwrap();
        let s = run_in_dir(&dir).unwrap();
        assert!(s.ran);
        let body = fs::read_to_string(dir.join("gain.jsonl")).unwrap();
        assert!(body.contains("{not json at all"));
        assert!(body.contains("binary-ish garbage"));
        let rows: Vec<_> = body
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .collect();
        assert_eq!(rows[0]["saved"], 0, "valid row still recalculated");
    }

    #[test]
    fn reporter_offset_remapped_to_first_unsent_row() {
        let dir = tdir("offset");
        let l1 =
            json!({"ts": 10, "source": "cli", "op": "cd", "before": 500, "after": 0, "saved": 500})
                .to_string();
        let l2 = json!({"ts": 11, "source": "wrap", "op": "git-status", "before": 2_000, "after": 300, "saved": 1_700}).to_string();
        let l3 = json!({"ts": 12, "source": "hook", "op": "webfetch_compact", "before": 9_000, "after": 100, "saved": 8_900}).to_string();
        fs::write(dir.join("gain.jsonl"), format!("{l1}\n{l2}\n{l3}\n")).unwrap();
        // Reporter had uploaded line 1 only: offset = start of line 2.
        let old_off = l1.len() as u64 + 1;
        fs::write(
            dir.join("reporter-offset"),
            format!("{old_off}:9999:12345:777"),
        )
        .unwrap();

        run_in_dir(&dir).unwrap();
        let new_body = fs::read_to_string(dir.join("gain.jsonl")).unwrap();
        let off_raw = fs::read_to_string(dir.join("reporter-offset")).unwrap();
        let parts: Vec<&str> = off_raw.trim().split(':').collect();
        let new_off: u64 = parts[0].parse().unwrap();
        // Line 1 (`cd`) was deleted, so the first unsent row (old line 2) now
        // starts at byte 0 of the rewritten file.
        assert_eq!(new_off, 0);
        assert_eq!(parts[1].parse::<u64>().unwrap(), new_body.len() as u64);
        assert_eq!(parts[2], "12345", "last_ts preserved");
        assert_eq!(parts[3], "0", "head fingerprint disabled for one pass");
        // And the tail from the new offset starts with the (untouched,
        // byte-identical) first unsent row.
        assert!(new_body[new_off as usize..].starts_with(&l2));
    }

    #[test]
    fn missing_dir_and_missing_files_are_fine() {
        let dir = tdir("fresh").join("nested-does-not-exist");
        let s = run_in_dir(&dir).unwrap();
        assert!(s.ran);
        assert_eq!(s.files_touched, 0);
        assert!(dir.join(MARKER_FILE).exists());
    }
}
