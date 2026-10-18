//! `debug_log` must redact every string it writes, at every depth.
//!
//! The log is opt-in but not private: `tokenade report` collects it into a
//! support bundle by name, so a credential landing here is a credential the
//! user hands to someone else. The redactor used to name five fields
//! (`command`, `script`, `error`, `stderr`, `url`) and look only at the top
//! level, against 216 call sites emitting 241 distinct field names.
//!
//! Own binary: it sets `TOKENADE_DEBUG_LOG`, a process-global.

use std::fs;

/// `TOKENADE_DEBUG_LOG` is process-global, so the tests in this binary — which
/// run in parallel by default — would set and clear it under each other. One
/// probe came back with an EMPTY log for exactly that reason.
fn env_lock() -> &'static std::sync::Mutex<()> {
    static L: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    L.get_or_init(|| std::sync::Mutex::new(()))
}

fn probe(details: serde_json::Value) -> String {
    let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let p = std::env::temp_dir().join(format!(
        "tokenade-dbglog-{}-{}.log",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::remove_file(&p);
    std::env::set_var("TOKENADE_DEBUG_LOG", &p);
    tokenade_core::debug_log::log("ci-probe", "redaction", details);
    std::env::remove_var("TOKENADE_DEBUG_LOG");
    let s = fs::read_to_string(&p).unwrap_or_default();
    let _ = fs::remove_file(&p);
    s
}

/// Split so the needles are not themselves whole secrets in this source.
fn key() -> String {
    format!("{}{}", "AKIA", "IOSFODNN7EXAMPLE")
}
fn tok(tag: &str) -> String {
    format!("sk-ant-api03-{tag}0123456789abcdefghijklmnopqrstuvwxyz")
}

#[test]
fn no_field_name_and_no_depth_escapes_redaction() {
    let k = key();
    let line = probe(serde_json::json!({
        // The name that was on the old allowlist: the control. If this leaks,
        // the redactor is not running at all and the rest proves nothing.
        "command": format!("curl -H 'Authorization: Bearer {}'", tok("CMD")),
        // Names that were NOT on it. `label` is real: gain.rs logs a clamped
        // row's label, which is a full command line.
        "label": format!("aws s3 ls --profile {}", tok("LABEL")),
        "reason": format!("credentials rejected for {k}"),
        // A name nobody has used yet. The point of dropping the allowlist is
        // that tomorrow's call site is covered without being enumerated.
        "some_field_added_next_year": tok("FUTURE"),
        // Right name, wrong depth.
        "nested": {"command": format!("export TOKEN={}", tok("NESTED"))},
        // Inside an array, and inside an object inside an array.
        "list": [tok("ARRAY"), {"detail": tok("DEEP")}],
    }));

    assert!(
        line.contains("ci-probe"),
        "the probe event was not written at all: {line}"
    );
    for tag in ["CMD", "LABEL", "FUTURE", "NESTED", "ARRAY", "DEEP"] {
        assert!(
            !line.contains(&tok(tag)),
            "a secret in the `{tag}` position reached the log in the clear"
        );
    }
    assert!(
        !line.contains(&k),
        "an access key in a `reason` field reached the log in the clear"
    );
}

#[test]
fn the_argv_aware_pass_still_runs_for_a_command() {
    // The regex layer alone cannot catch `-p PWD`: it does not know which
    // argument is the value of which flag. Dropping the allowlist must not
    // drop that second pass.
    let line = probe(serde_json::json!({
        "command": "mysql -u root -p hunter2seekrit --host db",
        "nested": {"command": "sshpass -p hunter2seekrit ssh box"},
    }));
    assert!(
        !line.contains("hunter2seekrit"),
        "the argv-aware pass did not run: {line}"
    );
    // And it is still a command, not a wholesale erasure.
    assert!(
        line.contains("mysql"),
        "the command itself was lost: {line}"
    );
}

#[test]
fn numbers_and_keys_are_untouched() {
    // Redaction operates on string VALUES. A field name that looks like a
    // secret, and every number, must survive verbatim — a debug log whose
    // durations and sizes are masked is not a debug log.
    let line = probe(serde_json::json!({
        "elapsed_ms": 1234,
        "tokens_before": 98765,
        "ok": true,
        "api_key_present": false,
        "path": "/home/u/.tokenade/gain.jsonl",
    }));
    assert!(line.contains("1234"), "a number was altered: {line}");
    assert!(line.contains("98765"), "a number was altered: {line}");
    assert!(
        line.contains("api_key_present"),
        "a key name was altered: {line}"
    );
    assert!(
        line.contains("/home/u/.tokenade/gain.jsonl"),
        "an ordinary path was mangled: {line}"
    );
}

#[test]
fn no_row_exceeds_the_atomic_append_size() {
    // `details` is free-form and unbounded at 216 call sites. A record over
    // PIPE_BUF is not appended atomically, and a concurrent process is then
    // free to land inside it.
    let huge = "d".repeat(40_000);
    let line = probe(serde_json::json!({"payload": huge, "elapsed_ms": 7}));
    assert!(
        line.len() <= tokenade_core::debug_log::PIPE_BUF,
        "a {}-byte record cannot be appended atomically",
        line.len()
    );
    let v: serde_json::Value = serde_json::from_str(line.trim()).expect("still one JSON row");
    // The head that makes the row findable survives; only details go.
    assert_eq!(v["component"].as_str(), Some("ci-probe"));
    assert_eq!(v["event"].as_str(), Some("redaction"));
    assert!(v["pid"].as_u64().is_some());
    assert!(
        v["details"].as_str().unwrap_or("").contains("dropped"),
        "details vanished without saying so: {v}"
    );

    // And a record that fits is left entirely alone.
    let ok = probe(serde_json::json!({"payload": "small", "elapsed_ms": 7}));
    let v: serde_json::Value = serde_json::from_str(ok.trim()).unwrap();
    assert_eq!(v["details"]["payload"].as_str(), Some("small"));
}

#[test]
fn prose_is_not_run_through_the_argv_pass() {
    // The argv-aware pass tokenises on whitespace and rejoins with single
    // spaces, and reads the token after `-p` as a password. That is right for
    // a command line and wrong for everything else: it would collapse the
    // spacing of an error message and mask an ordinary word. Only a `command`
    // (or `cmd`) subtree gets it.
    let line = probe(serde_json::json!({
        "error": "failed to open  file: retry with -p later",
    }));
    assert!(
        line.contains("failed to open  file: retry with -p later"),
        "a prose field was reshaped by the argv pass: {line}"
    );
}
