//! No ledger row may exceed `PIPE_BUF`.
//!
//! The ledger is appended to from every wrapped command, and its write lock is
//! process-LOCAL — its own comment says cross-process safety rests on the OS
//! making an append atomic, which is promised only below `PIPE_BUF` (4 KiB on
//! Linux and macOS). That comment also argued the point was moot because
//! "every record we emit is ≤200 bytes". Measured on a real ledger 2026-09-03:
//! 791 of 875 rows over 200 bytes, seven over PIPE_BUF, the largest 8 696 — a
//! `sha256sum` over many files, label alone 8 359. Two commands finishing
//! together interleave inside a row that long, and what they corrupt is the
//! accounting every published figure comes from.
//!
//! Own integration binary: it redirects `HOME`, which races anything in the
//! lib test binary that reads a store.

use std::fs;

const PIPE_BUF: usize = 4096;

#[test]
fn no_row_exceeds_the_atomic_append_size() {
    let _ledger = crate::ledger_lock();
    let tmp = std::env::temp_dir().join(format!(
        "tokenade-rowsize-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&tmp).unwrap();
    let prev_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", &tmp);
    let observed = tokenade_core::gain::current_ledger_path();
    if let Some(parent) = observed.parent() {
        let _ = fs::create_dir_all(parent);
    }
    // Dev builds book nothing; this test is about what the WRITE path emits.
    std::env::set_var("TOKENADE_FORCE_GAIN", "1");

    // The shape that produced the real 8 696-byte row: one command, a very
    // long file list. Plus a label that is pure multi-byte text, because the
    // cut has to land on a character boundary and not split a code point.
    let long_cmd = format!(
        "sha256sum {}",
        (0..400)
            .map(|i| format!("crates/tokenade-core/src/generated/module_{i:04}.rs"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    // THREE bytes per character, deliberately. `é` is two, so byte 512 is
    // already a boundary and a cut that ignores boundaries never splits one —
    // the fixture would be measuring nothing. `€` is three, 512 % 3 == 2, so
    // the naive cut lands mid-character and panics.
    let accented = "€".repeat(6000);
    // A secret STRADDLING the cut. Order matters and only here: cut first and
    // the pattern no longer matches, so the head of the key is stored in the
    // clear (measured: `AKIAIOSFODN`). Redact first and the whole key is one
    // `<redacted>` well before the cut is considered.
    //
    // The cap is private, so the key is repeated on a stride that is not a
    // divisor of any plausible cap: wherever the cut lands, it lands inside one
    // of these, and this test does not have to be edited when the cap moves.
    let straddling: String = std::iter::repeat_n("deploy --key AKIAIOSFODNN7EXAMPLE pad", 300)
        .collect::<Vec<_>>()
        .join(" ");
    let ascii_600 = "a".repeat(600);
    let wide_600 = "\u{20ac}".repeat(200);
    assert_eq!(ascii_600.len(), wide_600.len());
    for (op, label) in [
        ("rowsize_probe_long_cmd", long_cmd.as_str()),
        ("rowsize_probe_multibyte", accented.as_str()),
        ("rowsize_probe_straddle", straddling.as_str()),
        ("rowsize_probe_short", "git status"),
        // Same BYTE length, different character counts. The cap is a byte cap
        // because PIPE_BUF is a byte limit, so these two must be treated
        // identically — whatever the cap is set to.
        ("rowsize_probe_bytes_ascii", ascii_600.as_str()),
        ("rowsize_probe_bytes_wide", wide_600.as_str()),
    ] {
        tokenade_core::gain::record("ci-probe", op, label, tokenade_core::Savings::new(500, 100));
    }

    if let Some(prev) = prev_home {
        std::env::set_var("HOME", prev);
    } else {
        std::env::remove_var("HOME");
    }
    std::env::remove_var("TOKENADE_FORCE_GAIN");

    let content = fs::read_to_string(&observed).unwrap_or_default();
    let ours: Vec<&str> = content
        .lines()
        .filter(|l| l.contains("rowsize_probe_"))
        .collect();
    assert_eq!(ours.len(), 6, "the probe rows were not written:\n{content}");

    for l in &ours {
        assert!(
            l.len() < PIPE_BUF,
            "a row of {} bytes cannot be appended atomically (PIPE_BUF = {PIPE_BUF}): {}",
            l.len() + 1,
            &l[..120.min(l.len())]
        );
        // The row must still be a row: truncation happens inside the label,
        // never by cutting the JSON.
        let v: serde_json::Value =
            serde_json::from_str(l).unwrap_or_else(|e| panic!("row is not valid JSON ({e}): {l}"));
        assert!(v["op"].as_str().is_some(), "op lost: {l}");
        assert!(v["before"].as_u64().is_some() || v["tokens_before"].as_u64().is_some());
    }

    // A cut label says it was cut, and keeps the head — which is the half that
    // says WHICH command this was.
    let long_row: serde_json::Value = serde_json::from_str(
        ours.iter()
            .find(|l| l.contains("rowsize_probe_long_cmd"))
            .unwrap(),
    )
    .unwrap();
    let lbl = long_row["label"].as_str().expect("label present");
    assert!(
        lbl.starts_with("sha256sum "),
        "the head of the command was lost: {lbl}"
    );
    assert!(
        lbl.contains("bytes)"),
        "the label was cut without saying so: {lbl}"
    );

    // No fragment of the straddling key survives, at any length.
    let straddle_row: serde_json::Value = serde_json::from_str(
        ours.iter()
            .find(|l| l.contains("rowsize_probe_straddle"))
            .unwrap(),
    )
    .unwrap();
    let lbl = straddle_row["label"].as_str().expect("label present");
    assert!(
        !lbl.contains("AKIA"),
        "the head of a secret cut in two was stored in the clear: {}",
        &lbl[lbl.len().saturating_sub(80)..]
    );

    // Byte length decides, not character count.
    let cut_marker = |op: &str| -> bool {
        let row: serde_json::Value =
            serde_json::from_str(ours.iter().find(|l| l.contains(op)).unwrap()).unwrap();
        row["label"].as_str().unwrap().contains("bytes)")
    };
    assert_eq!(
        cut_marker("rowsize_probe_bytes_ascii"),
        cut_marker("rowsize_probe_bytes_wide"),
        "two labels of the same byte length were bounded differently, so the \
         cap is counting characters where PIPE_BUF counts bytes"
    );

    // A short label is left exactly alone.
    let short_row: serde_json::Value = serde_json::from_str(
        ours.iter()
            .find(|l| l.contains("rowsize_probe_short"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(short_row["label"].as_str(), Some("git status"));

    let _ = fs::remove_dir_all(&tmp);
}
