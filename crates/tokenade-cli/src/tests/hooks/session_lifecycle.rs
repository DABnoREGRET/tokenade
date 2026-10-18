//! Tests for the `session_lifecycle` hook.

use super::*;

#[test]
fn extract_ref_hashes_finds_both_marker_shapes() {
    let s = "noise §ref:0123456789ab§ more text \
             fetch via expand_ref with hash=\"fedcba987654\" and \
             again §ref:0123456789ab§ (dup) plus hash=\"0123456789ab\" (dup).";
    let got = extract_ref_hashes(s);
    // Order-preserving, deduped across both shapes.
    assert_eq!(got, vec!["0123456789ab", "fedcba987654"]);
}

#[test]
fn extract_ref_hashes_finds_a_masked_blocks_recovery_pointer() {
    // A masked tool result leaves `tokenade expand-ref <hash>` and nothing
    // else — the bytes are gone from the transcript, so the stash is the only
    // copy. This shape must be protected from LRU eviction like any other.
    let s = "[tokenade:masked 4200 tokens on cold resume — recover: \
             tokenade expand-ref 0123456789ab]";
    assert_eq!(extract_ref_hashes(s), vec!["0123456789ab"]);
}

#[test]
fn extract_ref_hashes_rejects_malformed() {
    // Too short, too long, non-hex, unterminated.
    let s = "§ref:dead§ §ref:0123456789abcd§ hash=\"nothexnothex\" §ref:abc";
    assert!(extract_ref_hashes(s).is_empty());
}

#[test]
fn sanitize_session_id_agrees_with_every_inline_copy_of_the_same_recipe() {
    // The one spelling: ASCII alphanumerics, bounded at 40. It kept `-` and
    // `_` and allowed 80 until 2026-08-25, while 37 other sites spelled the
    // recipe inline — and the two disagreed on exactly the ids that matter,
    // because `TOKENADE_SESSION` arrives hyphen-free and a hook payload's
    // `session_id` is the full UUID.
    let inline = |id: &str| -> String {
        let c: String = id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(40)
            .collect();
        if c.is_empty() {
            "session".to_string()
        } else {
            c
        }
    };
    for id in [
        "67caa6a3-1a47-4a19-b18f-81af922cce79", // Claude Code payload
        "67caa6a31a474a19b18f81af922cce79",     // TOKENADE_SESSION
        "abc-123_DEF",
        "a/b\\c..d",
        "",
        "é".repeat(50).as_str(),
        "x".repeat(200).as_str(),
    ] {
        assert_eq!(
            sanitize_session_id(id),
            inline(id),
            "the two spellings of the session key disagree on {id:?} — that split \
             is what made `delivered` read 0 and over-credit compounding savings"
        );
    }
    assert_eq!(sanitize_session_id(""), "session");
    assert_eq!(sanitize_session_id(&"x".repeat(200)).len(), 40);
    // The two ids above name the SAME session and must reach the same file.
    assert_eq!(
        sanitize_session_id("67caa6a3-1a47-4a19-b18f-81af922cce79"),
        sanitize_session_id("67caa6a31a474a19b18f81af922cce79")
    );
}

/// A counter is read by SUMMING its lines, and found under the previous
/// filename when an in-flight session wrote it there.
#[test]
fn session_counter_sums_an_append_only_log_and_finds_a_legacy_name() {
    let dir = std::env::temp_dir().join(format!(
        "tokenade-sesscnt-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let canonical = sanitize_session_id(uuid);

    // Append-only shape: `trim().parse::<u64>()` fails on this outright, which
    // is how `delivered` came back 0 even under the right filename.
    std::fs::write(
        dir.join(format!("{canonical}-delivered")),
        "10
20
30
",
    )
    .unwrap();
    assert_eq!(session_counter(&dir, uuid, "delivered"), 60);

    // Single-value shape sums to itself.
    std::fs::write(
        dir.join(format!("{canonical}-turn")),
        "350
",
    )
    .unwrap();
    assert_eq!(session_counter(&dir, uuid, "turn"), 350);

    // Written under the OLD name only: still found, so an in-flight session
    // does not forget a reset the moment the naming is unified.
    let legacy = legacy_session_id(uuid).expect("a hyphenated id has a legacy name");
    assert_ne!(legacy, canonical);
    std::fs::write(
        dir.join(format!("{legacy}-ovh")),
        "7
8
",
    )
    .unwrap();
    assert_eq!(session_counter(&dir, uuid, "ovh"), 15);

    // Absent everywhere is 0, not a panic.
    assert_eq!(session_counter(&dir, uuid, "nope"), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The statusline is the only surface the host tells which model is running.
/// Park it, read it back, and keep the conservative default when nothing did.
#[test]
fn the_session_model_survives_from_the_statusline_to_the_horizon() {
    // `HOME` is process-wide; ScratchHome holds ENV_LOCK for its whole life,
    // which is this crate's settled way to isolate a path with no knob of its
    // own — and `~/.tokenade/sessions` is exactly that.
    let scratch = tokenade_core::platform::ScratchHome::new("sessmodel");
    let home = scratch.path().to_path_buf();

    let uuid = "bbbbbbbb-cccc-dddd-eeee-ffffffffffff";
    assert_eq!(
        session_model(uuid),
        "",
        "nothing recorded must read as unknown, not as a guess"
    );

    note_session_model(uuid, "claude-opus-5[1m]");
    assert_eq!(session_model(uuid), "claude-opus-5[1m]");

    // The statusline runs at a few Hz; a re-record of the same value is the
    // common case and must not churn the file.
    let p = home
        .join(".tokenade")
        .join("sessions")
        .join(format!("{}-model", sanitize_session_id(uuid)));
    let before = std::fs::metadata(&p).unwrap().modified().unwrap();
    note_session_model(uuid, "claude-opus-5[1m]");
    assert_eq!(std::fs::metadata(&p).unwrap().modified().unwrap(), before);

    // Empties are not a value.
    note_session_model(uuid, "");
    assert_eq!(session_model(uuid), "claude-opus-5[1m]");
    note_session_model("", "whatever");

    // The hyphenated and bare spellings of one session id reach one file.
    assert_eq!(
        session_model("bbbbbbbbccccddddeeeeffffffffffff"),
        "claude-opus-5[1m]"
    );
}
