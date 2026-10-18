//! Tests for the `grep_pretool` hook.

use super::*;

#[test]
fn build_rg_argv_reconstructs_supported_params_and_is_order_stable() {
    // The pattern + path land after `--`; flags map to their rg long forms.
    let ti = serde_json::json!({
        "pattern": "handleSubmit",
        "path": "src",
        "glob": "*.ts",
        "-n": true,
        "-i": true,
        "-C": 2,
    });
    let argv = build_rg_argv(&ti).expect("supported");
    // Pattern + path are the trailing operands after the `--` sentinel.
    let dd = argv.iter().position(|a| a == "--").expect("has --");
    assert_eq!(
        &argv[dd + 1..],
        &["handleSubmit".to_string(), "src".to_string()]
    );
    assert!(
        argv.contains(&"--line-number".to_string()),
        "-n → --line-number"
    );
    assert!(
        argv.contains(&"--ignore-case".to_string()),
        "-i → --ignore-case"
    );
    assert!(
        argv.windows(2).any(|w| w == ["--glob", "*.ts"]),
        "glob mapped"
    );
    assert!(
        argv.windows(2).any(|w| w == ["--context", "2"]),
        "-C mapped"
    );
    // A bare pattern (no extras) still reconstructs.
    let bare = build_rg_argv(&serde_json::json!({"pattern": "foo"})).unwrap();
    assert_eq!(bare.last().unwrap(), "foo");
    // Empty / missing pattern → None (caller passes through).
    assert!(build_rg_argv(&serde_json::json!({"pattern": ""})).is_none());
    assert!(build_rg_argv(&serde_json::json!({"path": "x"})).is_none());
}

#[test]
fn grep_pending_sig_is_stable_and_param_sensitive() {
    let a = serde_json::json!({"pattern": "x", "path": "src", "glob": "*.rs"});
    let b = serde_json::json!({"pattern": "x", "path": "src", "glob": "*.rs"});
    let c = serde_json::json!({"pattern": "x", "path": "lib", "glob": "*.rs"});
    assert_eq!(
        grep_pending_sig(&a),
        grep_pending_sig(&b),
        "same params → same key"
    );
    assert_ne!(
        grep_pending_sig(&a),
        grep_pending_sig(&c),
        "different path → different key"
    );
}
