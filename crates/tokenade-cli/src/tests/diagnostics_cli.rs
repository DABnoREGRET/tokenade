//! Tests for `diagnostics_cli`, split out of the flat `tests.rs`.

use super::*;

#[test]
fn backup_file_dedups_and_caps_retention() {
    let base = std::env::temp_dir().join(format!("tok-backup-retention-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let count = |dir: &std::path::Path| {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("settings.json.tokenade-backup-")
            })
            .count()
    };

    // Dedup: identical content → only the first backup is written, the rest
    // are skipped (the periodic self-heal re-running is the common case).
    let d1 = base.join("dedup");
    std::fs::create_dir_all(&d1).unwrap();
    let f1 = d1.join("settings.json");
    std::fs::write(&f1, "same").unwrap();
    for _ in 0..10 {
        crate::diagnostics_cli::backup_file(&f1);
    }
    assert_eq!(
        count(&d1),
        1,
        "byte-identical content must not pile up backups"
    );

    // Retention: seed the historical pile (pre-dedup, distinct timestamps),
    // then one real content change. The old ts values (1000..) can't collide
    // with the current-second backup, so the prune caps the total exactly.
    let d2 = base.join("retention");
    std::fs::create_dir_all(&d2).unwrap();
    let f2 = d2.join("settings.json");
    std::fs::write(&f2, "orig").unwrap();
    for ts in 1000..1050u64 {
        std::fs::write(
            d2.join(format!("settings.json.tokenade-backup-{ts}")),
            "old",
        )
        .unwrap();
    }
    assert!(count(&d2) > MAX_BACKUPS_PER_FILE, "seeded a pile");
    std::fs::write(&f2, "changed").unwrap();
    crate::diagnostics_cli::backup_file(&f2);
    assert_eq!(
        count(&d2),
        MAX_BACKUPS_PER_FILE,
        "prunes down to the retention cap"
    );

    let _ = std::fs::remove_dir_all(&base);
}

/// #782 lock-in: `cmd_detect_competitors` must reject unknown flags
/// and unexpected positionals instead of silently running the scan.
#[test]
fn cmd_detect_competitors_rejects_unknown_flag() {
    let args = vec!["--bogus".to_string()];
    assert_eq!(super::cmd_detect_competitors(&args), 2);
}

#[test]
fn cmd_detect_competitors_rejects_positional() {
    let args = vec!["extra".to_string()];
    assert_eq!(super::cmd_detect_competitors(&args), 2);
}

/// #786 lock-in: `hooks status` and `hooks tail` must reject extra
/// arguments instead of silently ignoring them.
#[test]
fn cmd_hooks_status_rejects_extra_arg() {
    let args = vec!["status".to_string(), "--bogus".to_string()];
    assert_eq!(super::cmd_hooks(&args), 2);
    let args = vec!["status".to_string(), "extra".to_string()];
    assert_eq!(super::cmd_hooks(&args), 2);
}

#[test]
fn cmd_hooks_tail_rejects_extra_arg() {
    let args = vec!["tail".to_string(), "5".to_string(), "extra".to_string()];
    assert_eq!(super::cmd_hooks(&args), 2);
}
