//! Tests for the `read_staging` hook.

use super::*;

#[test]
fn s1_known_read_params_are_safe_to_rewrite() {
    // file_path / limit / offset are the only keys the rewrite preserves.
    let ti = serde_json::json!({"file_path": "/x", "limit": 20, "offset": 5});
    assert_eq!(unknown_read_param(&ti), None);
    let bare = serde_json::json!({"file_path": "/x"});
    assert_eq!(unknown_read_param(&bare), None);
}

#[test]
fn s1_unknown_param_blocks_rewrite() {
    // `pages` (PDF pagination) — and any future param — must force
    // passthrough so it is never silently stripped.
    let ti = serde_json::json!({"file_path": "/x", "pages": "1-12"});
    assert_eq!(unknown_read_param(&ti).as_deref(), Some("pages"));
    let future = serde_json::json!({"file_path": "/x", "limit": 20, "encoding": "utf16"});
    assert_eq!(unknown_read_param(&future).as_deref(), Some("encoding"));
}

#[test]
fn s3_small_native_output_keeps_injection() {
    // A real head slice (~20 short lines) → invariant held → inject.
    let resp = (0..20)
        .map(|i| format!("{i} line"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!native_read_breaks_invariant(&resp, 20));
}

#[test]
fn s3_large_native_output_skips_injection() {
    // updatedInput not applied / native render → far more than the head
    // we asked for → additive injection would inflate → skip.
    let many_lines = (0..400)
        .map(|i| format!("{i} y"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(native_read_breaks_invariant(&many_lines, 20));
    // Few lines but huge (e.g. a base64 image blob) → token cap catches it.
    let huge_blob = "x".repeat(40_000);
    assert!(native_read_breaks_invariant(&huge_blob, 20));
}

#[test]
fn s3_absent_response_fails_open() {
    // No tool_response to measure → inject as before (no regression).
    assert!(!native_read_breaks_invariant("", 20));
}

#[test]
fn read_pending_stage_roundtrip() {
    let _g = read_pending_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let d = tmp("read_pending");
    std::env::set_var("TOKENADE_READ_PENDING_DIR", &d);
    assert!(stage_read_pending(
        "sess-1",
        "/proj/file.rs",
        0,
        0,
        "compacted body"
    ));
    let key = read_pending_key("sess-1", "/proj/file.rs", 0, 0);
    let staged = d.join(format!("{key}.json"));
    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&staged).unwrap()).unwrap();
    assert_eq!(v["file_path"], "/proj/file.rs");
    assert_eq!(v["context"], "compacted body");
    assert!(v["ts"].as_u64().unwrap() > 0);
    // Distinct session ⇒ distinct key — concurrent agents reading the
    // same file must not steal each other's staged context.
    assert_ne!(key, read_pending_key("sess-2", "/proj/file.rs", 0, 0));
    // Deny-audit: distinct SLICE => distinct key — two parallel Reads of
    // the same file at different offsets must not steal each other's
    // staged context either.
    assert_ne!(key, read_pending_key("sess-1", "/proj/file.rs", 2000, 500));
    std::env::remove_var("TOKENADE_READ_PENDING_DIR");
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn hint_fresh_throttles_per_session_and_key() {
    let _g = read_pending_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let d = tmp("hint_fresh");
    std::env::set_var("TOKENADE_READ_PENDING_DIR", &d);
    // First time for (session, key) → fresh.
    assert!(hint_fresh_this_session("sess-A", "search-empty"));
    // Same pair again → throttled.
    assert!(!hint_fresh_this_session("sess-A", "search-empty"));
    assert!(!hint_fresh_this_session("sess-A", "search-empty"));
    // Different hint family in the same session → independent, fresh once.
    assert!(hint_fresh_this_session("sess-A", "read-code"));
    assert!(!hint_fresh_this_session("sess-A", "read-code"));
    // Different session, same key → independent, fresh once.
    assert!(hint_fresh_this_session("sess-B", "search-empty"));
    assert!(!hint_fresh_this_session("sess-B", "search-empty"));
    // Empty / weird session ids fall back to a stable bucket and still
    // throttle within themselves.
    assert!(hint_fresh_this_session("", "read-text"));
    assert!(!hint_fresh_this_session("!!!", "read-text"));
    std::env::remove_var("TOKENADE_READ_PENDING_DIR");
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn reset_session_hints_rearms_after_compaction() {
    let _g = read_pending_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let d = tmp("hint_reset");
    std::env::set_var("TOKENADE_READ_PENDING_DIR", &d);
    // Two hint families fire once, then throttle.
    assert!(hint_fresh_this_session("sess-A", "search-empty"));
    assert!(hint_fresh_this_session("sess-A", "read-code"));
    assert!(!hint_fresh_this_session("sess-A", "search-empty"));
    // A different session also has a live marker.
    assert!(hint_fresh_this_session("sess-B", "search-empty"));
    // Compaction for sess-A re-arms ONLY sess-A's hints.
    reset_session_hints("sess-A");
    assert!(
        hint_fresh_this_session("sess-A", "search-empty"),
        "sess-A hint must fire again after its compaction"
    );
    assert!(
        hint_fresh_this_session("sess-A", "read-code"),
        "every sess-A family re-arms, not just one"
    );
    // sess-B was untouched — note "sess-A" is a prefix of nothing here,
    // but the trailing '-' in the match prefix guards prefix-bleed.
    assert!(
        !hint_fresh_this_session("sess-B", "search-empty"),
        "another session's throttle must survive sess-A's reset"
    );
    std::env::remove_var("TOKENADE_READ_PENDING_DIR");
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn postread_additional_context_capped_below_spill() {
    // #1066 — small views pass through untouched.
    let small = "[tokenade:read-auto-compact] tiny view";
    assert_eq!(cap_postread_additional_context(small, "/p/f.log"), small);
    // Large views inject NO content (Paul 2026-06-17): replaced wholesale
    // by the steer, which is tiny — stays well under the spill threshold
    // and points at the cheap script/grep path instead of a re-readable
    // truncated digest.
    let big = "界".repeat(40_000); // multibyte, forces char-boundary handling
    let out = cap_postread_additional_context(&big, "/p/release.log");
    assert!(
        out.len() <= POSTREAD_INLINE_CAP_BYTES,
        "delivered must stay inline, got {} bytes",
        out.len()
    );
    assert!(out.contains("too large"));
    assert!(out.contains("tokenade exec"));
    assert!(out.contains("grep"));
    assert!(out.contains("/p/release.log"));
}

#[test]
fn large_compacted_read_view_steers_to_script() {
    // Paul's rule — a compacted view over READ_INLINE_TOKEN_CAP is replaced
    // by the steer (no digest content injected).
    let steer = read_too_large_steer("/p/release.log", 19_914);
    assert!(steer.contains("too large"));
    assert!(steer.contains("19914") || steer.contains("19_914") || steer.contains("~19914"));
    assert!(steer.contains("tokenade exec"));
    assert!(steer.contains("grep"));
    assert!(!steer.contains("read-auto-compact")); // carries no digest body
    assert!(tokenade_core::tokenizer::estimate(&steer) < READ_INLINE_TOKEN_CAP);
}

#[test]
fn search_steer_gating_and_env() {
    // Env-touching + pure-logic cases live in ONE test: the env var is
    // process-global, so splitting them races under parallel test runs.
    // Generic + inline view kept + many lines → steer.
    assert!(should_search_steer(false, false, 5303));
    // A specific compactor delivers its view directly (temp-redirect) → no steer.
    assert!(!should_search_steer(true, false, 5303));
    // Already swapped for the too-large steer → don't double-steer.
    assert!(!should_search_steer(false, true, 5303));
    // Small file delivered whole → no nag.
    assert!(!should_search_steer(false, false, 40));
    // Escape hatch / A/B gate disables it.
    std::env::set_var("TOKENADE_NO_READ_SEARCH_STEER", "1");
    assert!(!should_search_steer(false, false, 5303));
    std::env::remove_var("TOKENADE_NO_READ_SEARCH_STEER");
}

#[test]
fn search_steer_points_at_grep_and_exec() {
    let s = read_search_steer("/p/release.log", 5303);
    assert!(s.contains("5303"));
    assert!(s.contains("grep -n"));
    assert!(s.contains("tokenade exec"));
    assert!(s.contains("/p/release.log"));
    // A small footer, not a digest body.
    assert!(tokenade_core::tokenizer::estimate(&s) < 200);
}

#[test]
fn value_folding_kinds_get_original_file_steer() {
    use tokenade_core::auto::Kind;
    // Log family folds values away → needs the grep-the-original steer.
    assert!(kind_folds_values(&Kind::Logs));
    assert!(kind_folds_values(&Kind::AccessLog));
    assert!(kind_folds_values(&Kind::ContainerLogs));
    assert!(kind_folds_values(&Kind::CurlVerbose));
    // Lossless structured kinds carry every datum into the view → no steer.
    assert!(!kind_folds_values(&Kind::Json));
    assert!(!kind_folds_values(&Kind::Csv));
    assert!(!kind_folds_values(&Kind::Unknown));
}

#[test]
fn only_lossless_structured_kinds_get_read_view_substitution() {
    use tokenade_core::auto::Kind;
    std::env::remove_var("TOKENADE_READ_VIEW_ALL_KINDS");
    // Lossless structured serialization → substitute the compacted view at
    // Read time (the only regime that wins in the bench, e.g. pipeline-py).
    assert!(kind_lossless_read_view(&Kind::Json));
    assert!(kind_lossless_read_view(&Kind::Yaml));
    assert!(kind_lossless_read_view(&Kind::Csv));
    assert!(kind_lossless_read_view(&Kind::Tsv));
    assert!(kind_lossless_read_view(&Kind::Ndjson));
    assert!(kind_lossless_read_view(&Kind::TomlConfig));
    // Lossy kinds PASS THROUGH on the Read path — substituting them
    // regressed the bench (logs: log-needle-zh +60%, html: seo-audit +23%).
    assert!(!kind_lossless_read_view(&Kind::Logs));
    assert!(!kind_lossless_read_view(&Kind::AccessLog));
    assert!(!kind_lossless_read_view(&Kind::ContainerLogs));
    assert!(!kind_lossless_read_view(&Kind::CurlVerbose));
    assert!(!kind_lossless_read_view(&Kind::Html));
    assert!(!kind_lossless_read_view(&Kind::Markdown));
    assert!(!kind_lossless_read_view(&Kind::Unknown));
    // A/B escape restores pre-0.6.2 "substitute any kind" behaviour.
    std::env::set_var("TOKENADE_READ_VIEW_ALL_KINDS", "1");
    assert!(kind_lossless_read_view(&Kind::Html));
    assert!(kind_lossless_read_view(&Kind::Logs));
    std::env::remove_var("TOKENADE_READ_VIEW_ALL_KINDS");
}

#[test]
fn read_flow_env_overrides_marker() {
    std::env::set_var("TOKENADE_READ_FLOW", "deny");
    assert!(read_flow_wants_legacy_deny());
    std::env::set_var("TOKENADE_READ_FLOW", "allow");
    assert!(!read_flow_wants_legacy_deny());
    std::env::remove_var("TOKENADE_READ_FLOW");
}
