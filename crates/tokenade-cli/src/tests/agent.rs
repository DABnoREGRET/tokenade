//! Tests for `agent`, split out of the flat `tests.rs`.

use super::*;

#[test]
fn format_stale_mcp_nudge_truncates_past_3_entries() {
    // 5 stale → list first 3 + "... 2 more" line.
    let stale: Vec<(String, String)> = (1..=5)
        .map(|n| (n.to_string(), format!("/u/tokenade-{n}")))
        .collect();
    let s = super::format_stale_mcp_nudge(&stale, None);
    assert!(s.contains("5 tokenade MCP process(es)"), "got: {s}");
    assert!(s.contains("pid 1 → /u/tokenade-1"), "got: {s}");
    assert!(s.contains("pid 3 → /u/tokenade-3"), "got: {s}");
    assert!(!s.contains("pid 4 → "), "PID 4 must be truncated: {s}");
    assert!(s.contains("... 2 more"), "got: {s}");
}

#[test]
fn batch_nudge_fires_once_at_threshold() {
    let _g = read_pending_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    // #1072 — reactive batching nudge: silent for the first reads, fires
    // exactly once on the Nth (3rd), then stays silent.
    let d = tmp("batch_nudge");
    // The fire/ok/ignored telemetry inside batching_family_nudge writes to
    // the estimated ledger — redirect it away from the real ~/.tokenade
    // (OnceLock: first setter wins process-wide, harmless for other tests).
    tokenade_core::gain::set_root(std::path::PathBuf::from(&d));
    std::env::set_var("TOKENADE_READ_PENDING_DIR", &d);
    std::env::remove_var("TOKENADE_NO_BATCH_NUDGE");
    std::env::remove_var("TOKENADE_HOOK_DISABLED");
    // unique session key so a concurrent read_pending test can't collide
    let ev = serde_json::json!({"session_id": "sess-batch-nudge-test", "tool_name": "Read"});
    // Sequential single-read TURNS: space the recorded timestamps out (the
    // <2 s window now classifies same-instant reads as parallel/batched and
    // RESETS the streak — see the aging helper below).
    let age_last = || {
        let f = std::path::Path::new(&d).join("batch-streak-read-sessbatchnudgetest");
        if let Ok(v) = fs::read_to_string(&f) {
            if let Some((n, _)) = v.trim().split_once(':') {
                let _ = fs::write(&f, format!("{n}:1"));
            }
        }
    };
    let fire: Vec<bool> = (0..5)
        .map(|_| {
            let r = batching_read_nudge(&ev).is_some();
            age_last();
            r
        })
        .collect();
    assert_eq!(
        fire,
        vec![false, false, true, false, false],
        "nudge must fire exactly once, on the 3rd spaced-out Read"
    );
    // PARALLEL reads (same instant, no aging) never trip the nudge: the
    // agent is already batching.
    let evp = serde_json::json!({"session_id": "sess-batch-par", "tool_name": "Read"});
    let par: Vec<bool> = (0..6)
        .map(|_| batching_read_nudge(&evp).is_some())
        .collect();
    assert!(
        par.iter().all(|f| !f),
        "parallel reads must not fire the nudge"
    );
    let msg = {
        let d2 = tmp("batch_nudge2");
        std::env::set_var("TOKENADE_READ_PENDING_DIR", &d2);
        let ev2 = serde_json::json!({"session_id": "s2", "tool_name": "Read"});
        let age2 = || {
            let f = std::path::Path::new(&d2).join("batch-streak-read-s2");
            if let Ok(v) = fs::read_to_string(&f) {
                if let Some((n, _)) = v.trim().split_once(':') {
                    let _ = fs::write(&f, format!("{n}:1"));
                }
            }
        };
        batching_read_nudge(&ev2);
        age2();
        batching_read_nudge(&ev2);
        age2();
        let m = batching_read_nudge(&ev2).unwrap();
        let _ = fs::remove_dir_all(&d2);
        m
    };
    assert!(msg.contains("parallel"));
    // disable knob silences it
    std::env::set_var("TOKENADE_NO_BATCH_NUDGE", "1");
    let ev3 = serde_json::json!({"session_id": "s3", "tool_name": "Read"});
    for _ in 0..5 {
        assert!(batching_read_nudge(&ev3).is_none());
    }
    std::env::remove_var("TOKENADE_NO_BATCH_NUDGE");
    std::env::remove_var("TOKENADE_READ_PENDING_DIR");
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn hook_install_creates_backup_when_settings_exists() {
    let d = tmp("hook_backup");
    let settings_path = d.join(".claude").join("settings.json");
    fs::create_dir_all(settings_path.parent().unwrap()).unwrap();
    let original = serde_json::json!({"my_custom_key": "preserve_me"});
    fs::write(&settings_path, original.to_string()).unwrap();

    let _ = install_claude_code_hook(&d, "/x/tokenade", None, false, None, false);

    // A `.json.tokenade-backup-<ts>` sibling must exist with the original content.
    let claude_dir = d.join(".claude");
    let entries: Vec<_> = fs::read_dir(&claude_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("settings.json.tokenade-backup-")
        })
        .collect();
    assert_eq!(entries.len(), 1, "expected one backup, got {:?}", entries);
    let backup_content = fs::read_to_string(entries[0].path()).unwrap();
    assert!(backup_content.contains("my_custom_key"));
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn hook_install_preserves_existing_hooks() {
    // User already has an rtk/codegraph hook — STS must merge alongside,
    // never overwrite.
    let d = tmp("hook_preserve");
    let pre = d.join(".claude").join("settings.json");
    fs::create_dir_all(pre.parent().unwrap()).unwrap();
    let existing = serde_json::json!({
        "hooks": {
            "PostToolUse": [{
                "matcher": "Bash",
                "hooks": [{"type":"command","command":"rtk-prefilter","_tag":"rtk"}]
            }]
        }
    });
    fs::write(&pre, serde_json::to_string_pretty(&existing).unwrap()).unwrap();

    let _ = install_claude_code_hook(&d, "/x/tokenade", None, false, None, false);
    let s = fs::read_to_string(&pre).unwrap();
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    let arr = v["hooks"]["PostToolUse"].as_array().unwrap();
    assert_eq!(arr.len(), 2, "should now have rtk + tokenade entries");
    let tags: Vec<&str> = arr
        .iter()
        .filter_map(|g| g["hooks"][0]["_tag"].as_str())
        .collect();
    assert!(tags.contains(&"rtk"));
    assert!(tags.contains(&"tokenade"));
    let _ = fs::remove_dir_all(&d);
}

// #616 — `tokenade gain --json` must produce a structured shape
// consumers can parse: top-level totals + `by_op`/`by_source`
// arrays. Lock the shape so a refactor doesn't quietly break CI
// dashboards that grep these fields.
/// `cmd_gain` must REFUSE both forms, not only the shared helper.
///
/// The helper tested alone lets "the call site no longer routes" survive: that
/// is mutation M3, and the same lesson as before — a mechanism proven on its
/// helper is not proven on its consumer. The refusal comes before any read of
/// the ledger, so this test does not touch the disk.
#[test]
fn gain_refuses_two_shapes_before_it_reads_anything() {
    let both = vec!["--history".to_string(), "--json".to_string()];
    assert_eq!(crate::dashboard_cli::cmd_gain(&both), 2);
    // Anti-vacuity: a single form is not refused by this path.
    assert_ne!(
        crate::dashboard_cli::cmd_gain(&["--json".to_string()]),
        2,
        "`--json` alone must keep answering"
    );
}

#[test]
fn gain_json_shape_is_stable() {
    // Build the same JSON value the renderer assembles, so the
    // assertion runs hermetically without touching disk.
    let per_op = [("auto:json".to_string(), (1000u64, 200u64, 5u64))];
    let per_source = [("cli".to_string(), (1000u64, 200u64, 5u64))];
    let per_op_arr: Vec<serde_json::Value> = per_op
        .iter()
        .map(|(op, (b, a, n))| {
            serde_json::json!({
                "op": op,
                "before": b,
                "after": a,
                "count": n,
                "saved": b.saturating_sub(*a),
                "percent": super::Savings::new(*b as usize, *a as usize).percent(),
            })
        })
        .collect();
    let per_source_arr: Vec<serde_json::Value> = per_source
        .iter()
        .map(|(src, (b, a, n))| {
            serde_json::json!({
                "source": src,
                "before": b,
                "after": a,
                "count": n,
                "saved": b.saturating_sub(*a),
                "percent": super::Savings::new(*b as usize, *a as usize).percent(),
            })
        })
        .collect();
    let root = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "operations": 5u64,
        "before": 1000u64,
        "after": 200u64,
        "saved": 800u64,
        "percent": super::Savings::new(1000, 200).percent(),
        "by_op": per_op_arr,
        "by_source": per_source_arr,
    });
    let s = serde_json::to_string(&root).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert_eq!(parsed["operations"], serde_json::json!(5));
    assert_eq!(parsed["saved"], serde_json::json!(800));
    // Approximate 80% saved (1000 → 200).
    let pct = parsed["percent"].as_f64().unwrap();
    assert!(
        (pct - 80.0).abs() < 0.01,
        "percent should be ~80.0, got {pct}"
    );
    assert_eq!(parsed["by_op"][0]["op"], serde_json::json!("auto:json"));
    assert_eq!(parsed["by_op"][0]["count"], serde_json::json!(5));
    assert_eq!(parsed["by_source"][0]["source"], serde_json::json!("cli"));
}
