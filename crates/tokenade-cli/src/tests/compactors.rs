//! Tests for `compactors`, split out of the flat `tests.rs`.

use super::*;

// #519 — lock-in for the read_stdin_or_file_with_tty_hint helper
// (#514/#515). With a valid file path, returns Ok(content). With a
// bogus path, returns Err(1) AFTER printing the file-error to
// stderr. Both branches pinned because every compactor command
// that uses the helper depends on them.
//
// The TTY-hint branch (Err(2) when stdin is a TTY) can't be unit-
// tested directly: cargo test stdin is *not* a TTY, so the gate
// never fires. The piped/file paths are what mattered to pin.
#[test]
fn read_stdin_helper_returns_ok_on_valid_file_and_err_on_missing() {
    with_tmp_cwd(|| {
        // valid file → Ok
        let p = std::env::current_dir().unwrap().join("input.txt");
        std::fs::write(&p, "hello\n").unwrap();
        let rest = args(&[p.to_str().unwrap()]);
        match read_stdin_or_file_with_tty_hint(&rest, "test", "hint") {
            Ok(s) => assert_eq!(s, "hello\n"),
            Err(c) => panic!("expected Ok, got Err({c})"),
        }
        // missing file → Err(1)
        let rest = args(&["/nonexistent/path/that/does/not/exist.txt"]);
        match read_stdin_or_file_with_tty_hint(&rest, "test", "hint") {
            Ok(_) => panic!("expected Err(1) for missing path"),
            Err(c) => assert_eq!(c, 1, "expected file-error exit code 1, got {c}"),
        }
    });
}

/// #983 — directory passed where a file/`-` is expected used to
/// emit a bare OS `Is a directory (os error 21)`. Now: helpful
/// hint mentioning the `-` stdin alias + a `find` enumeration
/// example. Every one of the 17+ compactor commands routes
/// through this helper, so a single test covers them all.
#[test]
fn read_stdin_helper_rejects_directory_with_actionable_hint() {
    with_tmp_cwd(|| {
        let dir = std::env::current_dir().unwrap().join("subdir");
        std::fs::create_dir_all(&dir).unwrap();
        let rest = args(&[dir.to_str().unwrap()]);
        match read_stdin_or_file_with_tty_hint(&rest, "test", "hint") {
            Ok(_) => panic!("expected Err(1) for directory path"),
            Err(c) => assert_eq!(c, 1, "expected exit 1 for dir, got {c}"),
        }
    });
}

// #614 — describe-tool's typo hint relies on Levenshtein distance.
// Lock the helper so a refactor doesn't quietly break the "did you
// mean" suggestion that turns `auto-compact` into `auto_compact`.
#[test]
fn levenshtein_handles_typical_cli_typos() {
    // Exact match.
    assert_eq!(super::levenshtein("auto_compact", "auto_compact"), 0);
    // One-char substitution — the common dash-vs-underscore typo.
    assert_eq!(super::levenshtein("auto-compact", "auto_compact"), 1);
    // Deletion.
    assert_eq!(super::levenshtein("autocompact", "auto_compact"), 1);
    // Empty-string corner case.
    assert_eq!(super::levenshtein("", "abc"), 3);
    assert_eq!(super::levenshtein("abc", ""), 3);
}

// A `--cmd` hint is a shell command LINE, not a program name. The naive
// `split_whitespace().next()` this replaced credited `cd` for cargo output
// (`cd /repo && cargo build`), printed `[tokenade:cd]` as provenance, and won
// the smallest-wins competition with it. Env prefixes and absolute paths
// missed their compactor entirely. Every case below is one of those three.
#[test]
fn hint_programs_routes_the_shapes_a_plugin_actually_sends() {
    use crate::compactors::hint_programs;

    // Plain.
    assert_eq!(hint_programs("git status"), vec!["git"]);
    // Absolute path — the scaffold promises `/usr/bin/git` behaves like `git`.
    assert_eq!(hint_programs("/usr/bin/git status"), vec!["git"]);
    // Env prefix.
    assert_eq!(hint_programs("FOO=1 cargo build"), vec!["cargo"]);
    // A chain offers every segment that runs a program; `cd` emits nothing,
    // so crediting it is always a mis-attribution.
    assert_eq!(hint_programs("cd /repo && cargo build"), vec!["cargo"]);
    assert_eq!(
        hint_programs("git status && cargo build"),
        vec!["git", "cargo"]
    );
    // Nothing to credit rather than something wrong.
    assert!(hint_programs("").is_empty());
    assert!(hint_programs("cd /repo").is_empty());
    assert!(hint_programs("FOO=1").is_empty());
}

// Unsplittable hints (`$(…)`, heredocs, control flow) must not lose their
// program: `split_chain_segments` returns None there, and the fallback treats
// the whole line as one segment.
#[test]
fn hint_programs_keeps_the_program_when_the_line_cannot_be_split() {
    use crate::compactors::hint_programs;
    assert_eq!(hint_programs("git log --since=$(date +%F)"), vec!["git"]);
    assert_eq!(
        hint_programs("for f in a b; do cargo build; done"),
        vec!["for"]
    );
}

// The already-folded passthrough serves bytes that skipped every compactor, so it is the
// one path where nothing else can scrub them. Widening `already_folded` to
// recognise fold BANNERS put ordinary documents on that path for the first
// time — and an ordinary document can carry a secret. Measured 2026-08-08: a
// markdown file quoting `[tokenade:git] 82 line(s)` served a live
// `sk-ant-api03-…` key in cleartext.
#[test]
fn the_already_folded_passthrough_still_redacts() {
    use crate::compactors::passthrough_text;

    let secret = format!("sk-ant-api03-{}", "A".repeat(80));
    let src = format!("[tokenade:git] 82 line(s)\nAPI key: {secret}");
    let out = passthrough_text(&src);
    assert!(
        !out.contains(&secret),
        "passthrough served a live key: {out}"
    );
    assert!(out.ends_with('\n'), "passthrough must newline-terminate");
    // The banner itself is not a secret and must survive verbatim.
    assert!(out.contains("[tokenade:git] 82 line(s)"));
}

// Declaration ↔ asset. The signal is the only thing that lets a bridge tell
// "longer because we failed" from "longer because a secret was removed", and
// the two halves live in different languages, so nothing but a test keeps them
// spelled the same. Measured 2026-08-08: without the signal, three ~2 kB
// payloads carrying a key were all discarded by the bridges and the model got
// the raw key.
#[test]
fn plugin_bridges_look_for_the_redaction_signal() {
    use crate::compactors::REDACTION_SIGNAL;
    use crate::install_cli::{
        CLINE_PLUGIN_TS, HERMES_PLUGIN_INIT, OPENCLAW_PLUGIN_INDEX, OPENCODE_PLUGIN_TS,
        PI_EXTENSION_TS,
    };
    for (name, src) in [
        ("opencode", OPENCODE_PLUGIN_TS),
        ("pi", PI_EXTENSION_TS),
        ("openclaw", OPENCLAW_PLUGIN_INDEX),
        ("cline", CLINE_PLUGIN_TS),
        ("hermes", HERMES_PLUGIN_INIT),
    ] {
        assert!(
            src.contains(REDACTION_SIGNAL),
            "{name} bridge no longer looks for {REDACTION_SIGNAL:?} — a scrubbed result \
             that grew will be discarded and the secret served raw"
        );
    }
}

// Cline is the only bridge that memoizes — it re-sees the whole history every
// turn, so without a cache it would re-spawn tokenade for every old block. The
// key must therefore identify the BYTES. It used to sample them
// (`length:first96:last96`), which answers "do these look alike at the ends",
// not "are these the same". Demonstrated 2026-08-08: two 2 140-byte
// `kubectl get pods` snapshots differing in one middle STATUS field collide,
// so the model would read `Running` where the truth is `Failed` — silently
// wrong content, worse than not folding.
#[test]
fn the_cline_memo_keys_on_the_whole_payload_not_a_sample() {
    use crate::install_cli::CLINE_PLUGIN_TS;
    assert!(
        CLINE_PLUGIN_TS.contains("createHash(\"sha1\").update(text)"),
        "cline's memo key must hash the whole text"
    );
    assert!(
        !CLINE_PLUGIN_TS.contains("text.slice(-96)"),
        "cline's memo key went back to sampling the ends"
    );
}

// Every bridge spawns an ABSOLUTE path the installer injected. That path is
// correct exactly until tokenade moves — an upgrade into a different prefix, a
// reinstall, a uninstall. After that every spawn raises ENOENT, each bridge's
// blanket catch swallows it, and the bridge folds nothing for the rest of the
// install's life with no signal to the user. This pins the one-line PATH
// fallback that makes it self-heal, on all five, because four of them were
// written from the same template and cline already proved a template can be
// copied with a defence missing.
#[test]
fn every_bridge_survives_a_stale_injected_binary_path() {
    use crate::install_cli::{
        CLINE_PLUGIN_TS, HERMES_PLUGIN_INIT, OPENCLAW_PLUGIN_INDEX, OPENCODE_PLUGIN_TS,
        PI_EXTENSION_TS,
    };
    for (name, src, marker) in [
        ("opencode", OPENCODE_PLUGIN_TS, "ENOENT"),
        ("pi", PI_EXTENSION_TS, "ENOENT"),
        ("openclaw", OPENCLAW_PLUGIN_INDEX, "ENOENT"),
        ("cline", CLINE_PLUGIN_TS, "ENOENT"),
        ("hermes", HERMES_PLUGIN_INIT, "FileNotFoundError"),
    ] {
        assert!(
            src.contains(marker),
            "{name} bridge no longer handles a missing binary — it will fold nothing, silently"
        );
    }
    // And the placeholder fallback cline was missing: an uninjected asset must
    // spawn `tokenade` from PATH, never the literal `@@TOKENADE_BIN@@`.
    for (name, src) in [
        ("opencode", OPENCODE_PLUGIN_TS),
        ("pi", PI_EXTENSION_TS),
        ("openclaw", OPENCLAW_PLUGIN_INDEX),
        ("cline", CLINE_PLUGIN_TS),
    ] {
        assert!(
            src.contains("startsWith(\"@@\")"),
            "{name} bridge would spawn the literal placeholder when injection is skipped"
        );
    }
}
